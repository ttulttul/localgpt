use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use uuid::Uuid;

use super::providers::{LLMProvider, Message, Role};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    id: String,
    created_at: DateTime<Utc>,
    messages: Vec<Message>,
    system_context: Option<String>,
    token_count: usize,
    compaction_count: u32,
}

#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub id: String,
    pub message_count: usize,
    pub token_count: usize,
    pub compaction_count: u32,
}

impl Session {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            messages: Vec::new(),
            system_context: None,
            token_count: 0,
            compaction_count: 0,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn token_count(&self) -> usize {
        self.token_count
    }

    pub fn set_system_context(&mut self, context: String) {
        self.system_context = Some(context);
        self.recalculate_tokens();
    }

    pub fn add_message(&mut self, message: Message) {
        let tokens = estimate_tokens(&message.content);
        self.token_count += tokens;
        self.messages.push(message);
    }

    pub fn messages_for_llm(&self) -> Vec<Message> {
        let mut messages = Vec::new();

        // Add system context if present
        if let Some(ref context) = self.system_context {
            messages.push(Message {
                role: Role::System,
                content: context.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Add conversation messages
        messages.extend(self.messages.clone());

        messages
    }

    pub async fn compact(&mut self, provider: &dyn LLMProvider) -> Result<()> {
        if self.messages.len() < 4 {
            return Ok(()); // Nothing to compact
        }

        // Keep the most recent messages
        let keep_count = 4;
        let to_summarize = &self.messages[..self.messages.len() - keep_count];

        // Build text to summarize
        let text: String = to_summarize
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        // Get summary from LLM
        let summary = provider.summarize(&text).await?;

        // Create new message list with summary
        let mut new_messages = vec![Message {
            role: Role::System,
            content: format!("Previous conversation summary:\n\n{}", summary),
            tool_calls: None,
            tool_call_id: None,
        }];

        // Add recent messages
        new_messages.extend(self.messages[self.messages.len() - keep_count..].to_vec());

        self.messages = new_messages;
        self.compaction_count += 1;
        self.recalculate_tokens();

        Ok(())
    }

    fn recalculate_tokens(&mut self) {
        self.token_count = 0;

        if let Some(ref context) = self.system_context {
            self.token_count += estimate_tokens(context);
        }

        for message in &self.messages {
            self.token_count += estimate_tokens(&message.content);
        }
    }

    pub fn save(&self) -> Result<PathBuf> {
        let dir = get_sessions_dir()?;
        fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{}.jsonl", self.id));
        let mut file = File::create(&path)?;

        // Write header
        let header = serde_json::json!({
            "type": "session_header",
            "id": self.id,
            "created_at": self.created_at,
            "compaction_count": self.compaction_count
        });
        writeln!(file, "{}", serde_json::to_string(&header)?)?;

        // Write system context if present
        if let Some(ref context) = self.system_context {
            let context_entry = serde_json::json!({
                "type": "system_context",
                "content": context
            });
            writeln!(file, "{}", serde_json::to_string(&context_entry)?)?;
        }

        // Write messages
        for message in &self.messages {
            let entry = serde_json::json!({
                "type": "message",
                "role": message.role,
                "content": message.content,
                "tool_calls": message.tool_calls,
                "tool_call_id": message.tool_call_id
            });
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        }

        Ok(path)
    }

    pub fn load(session_id: &str) -> Result<Self> {
        let dir = get_sessions_dir()?;
        let path = dir.join(format!("{}.jsonl", session_id));

        if !path.exists() {
            anyhow::bail!("Session not found: {}", session_id);
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);

        let mut session = Session {
            id: session_id.to_string(),
            created_at: Utc::now(),
            messages: Vec::new(),
            system_context: None,
            token_count: 0,
            compaction_count: 0,
        };

        for line in reader.lines() {
            let line = line?;
            let entry: serde_json::Value = serde_json::from_str(&line)?;

            match entry["type"].as_str() {
                Some("session_header") => {
                    if let Ok(created_at) = serde_json::from_value(entry["created_at"].clone()) {
                        session.created_at = created_at;
                    }
                    if let Some(count) = entry["compaction_count"].as_u64() {
                        session.compaction_count = count as u32;
                    }
                }
                Some("system_context") => {
                    if let Some(content) = entry["content"].as_str() {
                        session.system_context = Some(content.to_string());
                    }
                }
                Some("message") => {
                    let message = Message {
                        role: serde_json::from_value(entry["role"].clone())?,
                        content: entry["content"].as_str().unwrap_or("").to_string(),
                        tool_calls: serde_json::from_value(entry["tool_calls"].clone()).ok(),
                        tool_call_id: entry["tool_call_id"].as_str().map(|s| s.to_string()),
                    };
                    session.messages.push(message);
                }
                _ => {}
            }
        }

        session.recalculate_tokens();
        Ok(session)
    }

    pub fn status(&self) -> SessionStatus {
        SessionStatus {
            id: self.id.clone(),
            message_count: self.messages.len(),
            token_count: self.token_count,
            compaction_count: self.compaction_count,
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Default agent ID (matches OpenClaw's default)
pub const DEFAULT_AGENT_ID: &str = "main";

fn get_sessions_dir() -> Result<PathBuf> {
    get_sessions_dir_for_agent(DEFAULT_AGENT_ID)
}

/// Get sessions directory for a specific agent
/// Path: ~/.localgpt/agents/<agentId>/sessions/
pub fn get_sessions_dir_for_agent(agent_id: &str) -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    Ok(base
        .home_dir()
        .join(".localgpt")
        .join("agents")
        .join(agent_id)
        .join("sessions"))
}

/// Get the state directory root
/// Path: ~/.localgpt/
pub fn get_state_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    Ok(base.home_dir().join(".localgpt"))
}

/// Rough token estimation (4 chars per token on average)
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Session info for listing
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub message_count: usize,
    pub file_size: u64,
}

/// List available sessions
pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    list_sessions_for_agent(DEFAULT_AGENT_ID)
}

/// List sessions for a specific agent
pub fn list_sessions_for_agent(agent_id: &str) -> Result<Vec<SessionInfo>> {
    let sessions_dir = get_sessions_dir_for_agent(agent_id)?;

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Skip sessions.json and non-.jsonl files
        if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
            continue;
        }

        let filename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Skip if not a valid UUID-like name
        if filename.len() < 32 {
            continue;
        }

        let metadata = fs::metadata(&path)?;
        let file_size = metadata.len();

        // Read first line to get session header
        if let Ok(file) = File::open(&path) {
            let reader = BufReader::new(file);
            if let Some(Ok(first_line)) = reader.lines().next() {
                if let Ok(header) = serde_json::from_str::<serde_json::Value>(&first_line) {
                    if header["type"].as_str() == Some("session_header") {
                        let created_at = header["created_at"]
                            .as_str()
                            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(Utc::now);

                        // Count messages (approximate by counting lines - 2 for header and context)
                        let message_count = fs::read_to_string(&path)
                            .map(|s| s.lines().count().saturating_sub(2))
                            .unwrap_or(0);

                        sessions.push(SessionInfo {
                            id: filename.to_string(),
                            created_at,
                            message_count,
                            file_size,
                        });
                    }
                }
            }
        }
    }

    // Sort by created_at (newest first)
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(sessions)
}

/// Get the most recent session ID
pub fn get_last_session_id() -> Result<Option<String>> {
    let sessions = list_sessions()?;
    Ok(sessions.first().map(|s| s.id.clone()))
}

/// Auto-save session periodically (call after each message)
impl Session {
    pub fn auto_save(&self) -> Result<()> {
        // Only save if we have messages
        if self.messages.is_empty() {
            return Ok(());
        }
        self.save()?;
        Ok(())
    }
}
