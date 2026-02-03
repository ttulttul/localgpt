use anyhow::Result;
use clap::Args;
use futures::StreamExt;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, Write};

use localgpt::agent::{get_last_session_id, list_sessions, Agent, AgentConfig};
use localgpt::config::Config;
use localgpt::memory::MemoryManager;

#[derive(Args)]
pub struct ChatArgs {
    /// Model to use (overrides config)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Session ID to resume
    #[arg(short, long)]
    pub session: Option<String>,

    /// Resume the most recent session
    #[arg(long)]
    pub resume: bool,
}

pub async fn run(args: ChatArgs) -> Result<()> {
    let config = Config::load()?;
    let memory = MemoryManager::new(&config.memory)?;

    let agent_config = AgentConfig {
        model: args.model.unwrap_or(config.agent.default_model.clone()),
        context_window: config.agent.context_window,
        reserve_tokens: config.agent.reserve_tokens,
    };

    let mut agent = Agent::new(agent_config, &config, memory).await?;

    // Determine session to use
    let session_id = if let Some(id) = args.session {
        Some(id)
    } else if args.resume {
        get_last_session_id()?
    } else {
        None
    };

    // Resume or create session
    if let Some(session_id) = session_id {
        match agent.resume_session(&session_id).await {
            Ok(()) => {
                let status = agent.session_status();
                println!(
                    "Resumed session {} ({} messages)\n",
                    &session_id[..8],
                    status.message_count
                );
            }
            Err(e) => {
                eprintln!("Could not resume session: {}. Starting new session.\n", e);
                agent.new_session().await?;
            }
        }
    } else {
        agent.new_session().await?;
    }

    println!(
        "LocalGPT v{} | Model: {} | Memory: {} chunks indexed\n",
        env!("CARGO_PKG_VERSION"),
        agent.model(),
        agent.memory_chunk_count()
    );
    println!("Type /help for commands, /quit to exit\n");

    let mut rl = DefaultEditor::new()?;
    let mut stdout = io::stdout();

    loop {
        let readline = rl.readline("You: ");

        let input = match readline {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                break; // Ctrl+D
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        };

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Add to history
        let _ = rl.add_history_entry(input);

        // Handle commands
        if input.starts_with('/') {
            match handle_command(input, &mut agent).await {
                CommandResult::Continue => continue,
                CommandResult::Quit => break,
                CommandResult::Error(e) => {
                    eprintln!("Error: {}", e);
                    continue;
                }
            }
        }

        // Send message to agent with streaming
        print!("\nLocalGPT: ");
        stdout.flush()?;

        match agent.chat_stream(input).await {
            Ok(mut stream) => {
                let mut full_response = String::new();

                while let Some(result) = stream.next().await {
                    match result {
                        Ok(chunk) => {
                            print!("{}", chunk.delta);
                            stdout.flush()?;
                            full_response.push_str(&chunk.delta);
                        }
                        Err(e) => {
                            eprintln!("\nStream error: {}", e);
                            break;
                        }
                    }
                }

                // Add the response to session history and auto-save
                agent.finish_chat_stream(&full_response);
                if let Err(e) = agent.auto_save_session() {
                    eprintln!("Warning: Failed to auto-save session: {}", e);
                }
                println!("\n");
            }
            Err(e) => {
                eprintln!("Error: {}\n", e);
            }
        }
    }

    println!("Goodbye!");
    Ok(())
}

enum CommandResult {
    Continue,
    Quit,
    Error(String),
}

async fn handle_command(input: &str, agent: &mut Agent) -> CommandResult {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts[0];

    match cmd {
        "/quit" | "/exit" | "/q" => CommandResult::Quit,

        "/help" | "/h" | "/?" => {
            println!("\nCommands:");
            println!("  /help, /h, /?     - Show this help");
            println!("  /quit, /exit, /q  - Exit chat");
            println!("  /new              - Start a fresh session (reloads memory context)");
            println!("  /sessions         - List available sessions");
            println!("  /resume <id>      - Resume a specific session");
            println!("  /compact          - Compact session history");
            println!("  /clear            - Clear session history (keeps context)");
            println!("  /memory <query>   - Search memory");
            println!("  /save             - Save current session");
            println!("  /status           - Show session status");
            println!();
            CommandResult::Continue
        }

        "/sessions" => {
            match list_sessions() {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        println!("\nNo saved sessions found.\n");
                    } else {
                        println!("\nAvailable sessions:");
                        for (i, session) in sessions.iter().take(10).enumerate() {
                            println!(
                                "  {}. {} ({} messages, {})",
                                i + 1,
                                &session.id[..8],
                                session.message_count,
                                session.created_at.format("%Y-%m-%d %H:%M")
                            );
                        }
                        if sessions.len() > 10 {
                            println!("  ... and {} more", sessions.len() - 10);
                        }
                        println!("\nUse /resume <id> to resume a session.\n");
                    }
                    CommandResult::Continue
                }
                Err(e) => CommandResult::Error(format!("Failed to list sessions: {}", e)),
            }
        }

        "/resume" => {
            if parts.len() < 2 {
                return CommandResult::Error("Usage: /resume <session-id>".into());
            }
            let session_id = parts[1];

            // Find session by prefix match
            match list_sessions() {
                Ok(sessions) => {
                    let matching: Vec<_> = sessions
                        .iter()
                        .filter(|s| s.id.starts_with(session_id))
                        .collect();

                    match matching.len() {
                        0 => CommandResult::Error(format!("No session found matching '{}'", session_id)),
                        1 => {
                            let full_id = matching[0].id.clone();
                            match futures::executor::block_on(agent.resume_session(&full_id)) {
                                Ok(()) => {
                                    let status = agent.session_status();
                                    println!(
                                        "\nResumed session {} ({} messages)\n",
                                        &full_id[..8],
                                        status.message_count
                                    );
                                    CommandResult::Continue
                                }
                                Err(e) => CommandResult::Error(format!("Failed to resume: {}", e)),
                            }
                        }
                        _ => CommandResult::Error(format!(
                            "Multiple sessions match '{}'. Please be more specific.",
                            session_id
                        )),
                    }
                }
                Err(e) => CommandResult::Error(format!("Failed to list sessions: {}", e)),
            }
        }

        "/compact" => match agent.compact_session().await {
            Ok((before, after)) => {
                println!("\nSession compacted. Token count: {} → {}\n", before, after);
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to compact: {}", e)),
        },

        "/clear" => {
            agent.clear_session();
            println!("\nSession cleared.\n");
            CommandResult::Continue
        }

        "/new" => match agent.new_session().await {
            Ok(()) => {
                println!("\nNew session started. Memory context reloaded.\n");
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to create new session: {}", e)),
        }

        "/memory" => {
            if parts.len() < 2 {
                return CommandResult::Error("Usage: /memory <query>".into());
            }
            let query = parts[1..].join(" ");
            match agent.search_memory(&query).await {
                Ok(results) => {
                    println!("\nMemory search results for '{}':", query);
                    for (i, result) in results.iter().enumerate() {
                        println!(
                            "{}. [{}:{}] {}",
                            i + 1,
                            result.file,
                            result.line_start,
                            result.content.chars().take(100).collect::<String>()
                        );
                    }
                    println!();
                    CommandResult::Continue
                }
                Err(e) => CommandResult::Error(format!("Memory search failed: {}", e)),
            }
        }

        "/save" => match agent.save_session().await {
            Ok(path) => {
                println!("\nSession saved to: {}\n", path.display());
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to save session: {}", e)),
        },

        "/status" => {
            let status = agent.session_status();
            println!("\nSession Status:");
            println!("  ID: {}", status.id);
            println!("  Messages: {}", status.message_count);
            println!("  Tokens: ~{}", status.token_count);
            println!("  Compactions: {}", status.compaction_count);
            println!();
            CommandResult::Continue
        }

        _ => CommandResult::Error(format!(
            "Unknown command: {}. Type /help for commands.",
            cmd
        )),
    }
}
