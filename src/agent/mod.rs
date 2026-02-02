mod providers;
mod session;
mod tools;

pub use providers::{
    LLMProvider, LLMResponse, Message, Role, StreamChunk, StreamResult, ToolCall, ToolSchema,
};
pub use session::{Session, SessionStatus};
pub use tools::{Tool, ToolResult};

use anyhow::Result;
use std::path::PathBuf;
use tracing::{debug, info};

use crate::config::Config;
use crate::memory::{MemoryChunk, MemoryManager};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub context_window: usize,
    pub reserve_tokens: usize,
}

pub struct Agent {
    config: AgentConfig,
    provider: Box<dyn LLMProvider>,
    session: Session,
    memory: MemoryManager,
    tools: Vec<Box<dyn Tool>>,
}

impl Agent {
    pub async fn new(
        config: AgentConfig,
        app_config: &Config,
        memory: MemoryManager,
    ) -> Result<Self> {
        let provider = providers::create_provider(&config.model, app_config)?;
        let tools = tools::create_default_tools(app_config)?;

        Ok(Self {
            config,
            provider,
            session: Session::new(),
            memory,
            tools,
        })
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub fn memory_chunk_count(&self) -> usize {
        self.memory.chunk_count().unwrap_or(0)
    }

    pub async fn new_session(&mut self) -> Result<()> {
        self.session = Session::new();

        // Load memory context
        let memory_context = self.build_memory_context().await?;
        if !memory_context.is_empty() {
            self.session.set_system_context(memory_context);
        }

        info!("Created new session: {}", self.session.id());
        Ok(())
    }

    pub async fn resume_session(&mut self, session_id: &str) -> Result<()> {
        self.session = Session::load(session_id)?;
        info!("Resumed session: {}", session_id);
        Ok(())
    }

    pub async fn chat(&mut self, message: &str) -> Result<String> {
        // Add user message
        self.session.add_message(Message {
            role: Role::User,
            content: message.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        // Check if we need to compact
        if self.should_compact() {
            self.compact_session().await?;
        }

        // Build messages for LLM
        let messages = self.session.messages_for_llm();

        // Get available tools
        let tool_schemas: Vec<ToolSchema> = self.tools.iter().map(|t| t.schema()).collect();

        // Invoke LLM
        let response = self
            .provider
            .chat(&messages, Some(tool_schemas.as_slice()))
            .await?;

        // Handle tool calls if any
        let final_response = self.handle_response(response).await?;

        // Add assistant response
        self.session.add_message(Message {
            role: Role::Assistant,
            content: final_response.clone(),
            tool_calls: None,
            tool_call_id: None,
        });

        Ok(final_response)
    }

    async fn handle_response(&mut self, response: LLMResponse) -> Result<String> {
        match response {
            LLMResponse::Text(text) => Ok(text),
            LLMResponse::ToolCalls(calls) => {
                // Execute tool calls
                let mut results = Vec::new();

                for call in &calls {
                    debug!(
                        "Executing tool: {} with args: {}",
                        call.name, call.arguments
                    );

                    let result = self.execute_tool(call).await;
                    results.push(ToolResult {
                        call_id: call.id.clone(),
                        output: result.unwrap_or_else(|e| format!("Error: {}", e)),
                    });
                }

                // Add tool call message
                self.session.add_message(Message {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_calls: Some(calls),
                    tool_call_id: None,
                });

                // Add tool results
                for result in &results {
                    self.session.add_message(Message {
                        role: Role::Tool,
                        content: result.output.clone(),
                        tool_calls: None,
                        tool_call_id: Some(result.call_id.clone()),
                    });
                }

                // Continue conversation with tool results
                let messages = self.session.messages_for_llm();
                let tool_schemas: Vec<ToolSchema> = self.tools.iter().map(|t| t.schema()).collect();
                let next_response = self
                    .provider
                    .chat(&messages, Some(tool_schemas.as_slice()))
                    .await?;

                // Recursively handle (in case of more tool calls)
                Box::pin(self.handle_response(next_response)).await
            }
        }
    }

    async fn execute_tool(&self, call: &ToolCall) -> Result<String> {
        for tool in &self.tools {
            if tool.name() == call.name {
                return tool.execute(&call.arguments).await;
            }
        }
        anyhow::bail!("Unknown tool: {}", call.name)
    }

    async fn build_memory_context(&self) -> Result<String> {
        let mut context = String::new();

        // Load MEMORY.md if it exists
        if let Ok(memory_content) = self.memory.read_memory_file() {
            if !memory_content.is_empty() {
                context.push_str("# Long-term Memory (MEMORY.md)\n\n");
                context.push_str(&memory_content);
                context.push_str("\n\n");
            }
        }

        // Load today's and yesterday's daily logs
        if let Ok(recent_logs) = self.memory.read_recent_daily_logs(2) {
            if !recent_logs.is_empty() {
                context.push_str("# Recent Daily Logs\n\n");
                context.push_str(&recent_logs);
                context.push_str("\n\n");
            }
        }

        // Load HEARTBEAT.md if it exists
        if let Ok(heartbeat) = self.memory.read_heartbeat_file() {
            if !heartbeat.is_empty() {
                context.push_str("# Pending Tasks (HEARTBEAT.md)\n\n");
                context.push_str(&heartbeat);
                context.push_str("\n");
            }
        }

        Ok(context)
    }

    fn should_compact(&self) -> bool {
        self.session.token_count() > (self.config.context_window - self.config.reserve_tokens)
    }

    pub async fn compact_session(&mut self) -> Result<(usize, usize)> {
        let before = self.session.token_count();

        // Trigger memory flush before compacting
        self.memory_flush().await?;

        // Compact the session
        self.session.compact(&*self.provider).await?;

        let after = self.session.token_count();
        info!("Session compacted: {} -> {} tokens", before, after);

        Ok((before, after))
    }

    async fn memory_flush(&mut self) -> Result<()> {
        let flush_prompt = r#"Pre-compaction memory flush.
Store durable memories now (use memory/YYYY-MM-DD.md).
If nothing to store, reply: NO_REPLY"#;

        let messages = vec![Message {
            role: Role::System,
            content: flush_prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = self.provider.chat(&messages, None).await?;

        if let LLMResponse::Text(text) = response {
            if text.trim() != "NO_REPLY" {
                debug!("Memory flush response: {}", text);
            }
        }

        Ok(())
    }

    pub fn clear_session(&mut self) {
        self.session = Session::new();
    }

    pub async fn search_memory(&self, query: &str) -> Result<Vec<MemoryChunk>> {
        self.memory.search(query, 10)
    }

    pub async fn save_session(&self) -> Result<PathBuf> {
        self.session.save()
    }

    pub fn session_status(&self) -> SessionStatus {
        self.session.status()
    }

    /// Get a reference to the LLM provider for streaming
    pub fn provider(&self) -> &dyn LLMProvider {
        &*self.provider
    }

    /// Get messages for the LLM (for streaming)
    pub fn session_messages(&self) -> Vec<Message> {
        self.session.messages_for_llm()
    }

    /// Add a user message to the session
    pub fn add_user_message(&mut self, content: &str) {
        self.session.add_message(Message {
            role: Role::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    /// Add an assistant message to the session
    pub fn add_assistant_message(&mut self, content: &str) {
        self.session.add_message(Message {
            role: Role::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
    }
}
