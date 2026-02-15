//! CLI subcommand for LocalGPT Gen — 3D scene generation mode.

use anyhow::Result;
use clap::Args;

use crate::gen3d;

#[derive(Args)]
pub struct GenArgs {
    /// Initial prompt to send (optional — starts interactive if omitted)
    pub prompt: Option<String>,
}

/// Launch LocalGPT Gen: Bevy window on main thread, agent on background tokio runtime.
pub fn run(args: GenArgs, agent_id: &str) -> Result<()> {
    // Create the channel pair
    let (bridge, channels) = gen3d::create_gen_channels();

    // Clone values for the background thread
    let agent_id = agent_id.to_string();
    let initial_prompt = args.prompt;
    let bridge_for_agent = bridge.clone();

    // Spawn tokio runtime + agent loop on a background thread
    // (Bevy must own the main thread for windowing/GPU on macOS)
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for gen agent");

        rt.block_on(async move {
            if let Err(e) = run_agent_loop(bridge_for_agent, &agent_id, initial_prompt).await {
                tracing::error!("Gen agent loop error: {}", e);
            }
        });
    });

    // Run Bevy on the main thread
    run_bevy_app(channels)
}

/// Set up and run the Bevy application on the main thread.
fn run_bevy_app(channels: gen3d::GenChannels) -> Result<()> {
    use bevy::prelude::*;

    let mut app = App::new();

    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "LocalGPT Gen".into(),
                    resolution: bevy::window::WindowResolution::new(1280.0, 720.0),
                    ..default()
                }),
                ..default()
            })
            .disable::<bevy::log::LogPlugin>(),
    );

    gen3d::plugin::setup_gen_app(&mut app, channels);

    app.run();

    Ok(())
}

/// Run the interactive agent loop with Gen tools available.
async fn run_agent_loop(
    bridge: std::sync::Arc<gen3d::GenBridge>,
    agent_id: &str,
    initial_prompt: Option<String>,
) -> Result<()> {
    use crate::agent::Agent;
    use crate::config::Config;
    use crate::memory::MemoryManager;
    use std::io::{self, Write};
    use std::sync::Arc;

    // Load config
    let config = Config::load()?;

    // Set up memory
    let memory = MemoryManager::new_with_agent(&config.memory, agent_id)?;
    let memory = Arc::new(memory);

    // Create default tools + gen tools
    let mut tools = crate::agent::tools::create_default_tools(&config, Some(memory.clone()))?;
    tools.extend(gen3d::tools::create_gen_tools(bridge));

    // Create agent with combined tools
    let mut agent = Agent::new_with_tools(config.clone(), agent_id, memory, tools)?;
    agent.new_session().await?;

    // If initial prompt given, send it
    if let Some(prompt) = initial_prompt {
        println!("\n> {}", prompt);
        let response = agent.chat(&prompt).await?;
        println!("\n{}\n", response);
    }

    // Interactive loop
    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            break; // EOF
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" || input == "/exit" || input == "/q" {
            break;
        }

        let response = agent.chat(input).await?;
        println!("\n{}\n", response);
    }

    Ok(())
}
