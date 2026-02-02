use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::pin::Pin;
use tracing::debug;

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub enum LLMResponse {
    Text(String),
    ToolCalls(Vec<ToolCall>),
}

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub delta: String,
    pub done: bool,
}

pub type StreamResult = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

#[async_trait]
pub trait LLMProvider: Send + Sync {
    async fn chat(&self, messages: &[Message], tools: Option<&[ToolSchema]>)
        -> Result<LLMResponse>;

    async fn summarize(&self, text: &str) -> Result<String>;

    /// Stream chat response (default: falls back to non-streaming)
    async fn chat_stream(
        &self,
        messages: &[Message],
        _tools: Option<&[ToolSchema]>,
    ) -> Result<StreamResult> {
        // Default implementation: single chunk with full response
        let resp = self.chat(messages, None).await?;
        let text = match resp {
            LLMResponse::Text(t) => t,
            LLMResponse::ToolCalls(_) => {
                return Err(anyhow::anyhow!("Tool calls not supported in streaming"))
            }
        };
        Ok(Box::pin(futures::stream::once(async move {
            Ok(StreamChunk {
                delta: text,
                done: true,
            })
        })))
    }
}

pub fn create_provider(model: &str, config: &Config) -> Result<Box<dyn LLMProvider>> {
    // Determine provider from model name
    if model.starts_with("gpt-") || model.starts_with("o1") {
        let openai_config = config
            .providers
            .openai
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OpenAI provider not configured"))?;

        Ok(Box::new(OpenAIProvider::new(
            &openai_config.api_key,
            &openai_config.base_url,
            model,
        )?))
    } else if model.starts_with("claude-") {
        let anthropic_config = config
            .providers
            .anthropic
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Anthropic provider not configured"))?;

        Ok(Box::new(AnthropicProvider::new(
            &anthropic_config.api_key,
            &anthropic_config.base_url,
            model,
        )?))
    } else if let Some(ollama_config) = &config.providers.ollama {
        Ok(Box::new(OllamaProvider::new(
            &ollama_config.endpoint,
            model,
        )?))
    } else {
        anyhow::bail!("Unknown model or provider not configured: {}", model)
    }
}

// OpenAI Provider
pub struct OpenAIProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAIProvider {
    pub fn new(api_key: &str, base_url: &str, model: &str) -> Result<Self> {
        Ok(Self {
            client: Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
        })
    }

    fn format_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect()
    }

    fn format_messages(&self, messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let mut msg = json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "tool",
                    },
                    "content": m.content
                });

                if let Some(ref tool_calls) = m.tool_calls {
                    msg["tool_calls"] = json!(tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments
                                }
                            })
                        })
                        .collect::<Vec<_>>());
                }

                if let Some(ref tool_call_id) = m.tool_call_id {
                    msg["tool_call_id"] = json!(tool_call_id);
                }

                msg
            })
            .collect()
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<LLMResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": self.format_messages(messages)
        });

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(self.format_tools(tools));
            }
        }

        debug!("OpenAI request: {}", serde_json::to_string_pretty(&body)?);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let response_body: Value = response.json().await?;
        debug!(
            "OpenAI response: {}",
            serde_json::to_string_pretty(&response_body)?
        );

        // Check for errors
        if let Some(error) = response_body.get("error") {
            anyhow::bail!("OpenAI API error: {}", error);
        }

        let choice = response_body["choices"]
            .get(0)
            .ok_or_else(|| anyhow::anyhow!("No choices in response"))?;

        let message = &choice["message"];

        // Check for tool calls
        if let Some(tool_calls) = message.get("tool_calls") {
            if let Some(calls) = tool_calls.as_array() {
                let parsed_calls: Vec<ToolCall> = calls
                    .iter()
                    .map(|tc| ToolCall {
                        id: tc["id"].as_str().unwrap_or("").to_string(),
                        name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or("{}")
                            .to_string(),
                    })
                    .collect();

                if !parsed_calls.is_empty() {
                    return Ok(LLMResponse::ToolCalls(parsed_calls));
                }
            }
        }

        let content = message["content"].as_str().unwrap_or("").to_string();

        Ok(LLMResponse::Text(content))
    }

    async fn summarize(&self, text: &str) -> Result<String> {
        let messages = vec![Message {
            role: Role::User,
            content: format!(
                "Summarize the following conversation concisely, preserving key information and context:\n\n{}",
                text
            ),
            tool_calls: None,
            tool_call_id: None,
        }];

        match self.chat(&messages, None).await? {
            LLMResponse::Text(summary) => Ok(summary),
            _ => anyhow::bail!("Unexpected response type"),
        }
    }
}

// Anthropic Provider
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, base_url: &str, model: &str) -> Result<Self> {
        Ok(Self {
            client: Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
        })
    }

    fn format_tools(&self, tools: &[ToolSchema]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters
                })
            })
            .collect()
    }

    fn format_messages(&self, messages: &[Message]) -> (Option<String>, Vec<Value>) {
        let mut system_prompt = None;
        let mut formatted = Vec::new();

        for m in messages {
            match m.role {
                Role::System => {
                    system_prompt = Some(m.content.clone());
                }
                Role::User => {
                    formatted.push(json!({
                        "role": "user",
                        "content": m.content
                    }));
                }
                Role::Assistant => {
                    if let Some(ref tool_calls) = m.tool_calls {
                        let tool_use: Vec<Value> = tool_calls.iter().map(|tc| {
                            json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": serde_json::from_str::<Value>(&tc.arguments).unwrap_or(json!({}))
                            })
                        }).collect();
                        formatted.push(json!({
                            "role": "assistant",
                            "content": tool_use
                        }));
                    } else {
                        formatted.push(json!({
                            "role": "assistant",
                            "content": m.content
                        }));
                    }
                }
                Role::Tool => {
                    if let Some(ref tool_call_id) = m.tool_call_id {
                        formatted.push(json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": tool_call_id,
                                "content": m.content
                            }]
                        }));
                    }
                }
            }
        }

        (system_prompt, formatted)
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<LLMResponse> {
        let (system_prompt, formatted_messages) = self.format_messages(messages);

        let mut body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": formatted_messages
        });

        if let Some(system) = system_prompt {
            body["system"] = json!(system);
        }

        if let Some(tools) = tools {
            if !tools.is_empty() {
                body["tools"] = json!(self.format_tools(tools));
            }
        }

        debug!(
            "Anthropic request: {}",
            serde_json::to_string_pretty(&body)?
        );

        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let response_body: Value = response.json().await?;
        debug!(
            "Anthropic response: {}",
            serde_json::to_string_pretty(&response_body)?
        );

        // Check for errors
        if let Some(error) = response_body.get("error") {
            anyhow::bail!("Anthropic API error: {}", error);
        }

        let content = response_body["content"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No content in response"))?;

        // Check for tool use
        let tool_calls: Vec<ToolCall> = content
            .iter()
            .filter(|c| c["type"] == "tool_use")
            .map(|c| ToolCall {
                id: c["id"].as_str().unwrap_or("").to_string(),
                name: c["name"].as_str().unwrap_or("").to_string(),
                arguments: serde_json::to_string(&c["input"]).unwrap_or("{}".to_string()),
            })
            .collect();

        if !tool_calls.is_empty() {
            return Ok(LLMResponse::ToolCalls(tool_calls));
        }

        // Get text content
        let text = content
            .iter()
            .filter(|c| c["type"] == "text")
            .map(|c| c["text"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("");

        Ok(LLMResponse::Text(text))
    }

    async fn summarize(&self, text: &str) -> Result<String> {
        let messages = vec![Message {
            role: Role::User,
            content: format!(
                "Summarize the following conversation concisely, preserving key information and context:\n\n{}",
                text
            ),
            tool_calls: None,
            tool_call_id: None,
        }];

        match self.chat(&messages, None).await? {
            LLMResponse::Text(summary) => Ok(summary),
            _ => anyhow::bail!("Unexpected response type"),
        }
    }
}

// Ollama Provider (for local models)
pub struct OllamaProvider {
    client: Client,
    endpoint: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(endpoint: &str, model: &str) -> Result<Self> {
        Ok(Self {
            client: Client::new(),
            endpoint: endpoint.to_string(),
            model: model.to_string(),
        })
    }
}

#[async_trait]
impl LLMProvider for OllamaProvider {
    async fn chat(
        &self,
        messages: &[Message],
        _tools: Option<&[ToolSchema]>,
    ) -> Result<LLMResponse> {
        // Note: Ollama tool support is limited, so we format as plain chat
        let formatted_messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "user", // Treat tool results as user messages
                    },
                    "content": m.content
                })
            })
            .collect();

        let body = json!({
            "model": self.model,
            "messages": formatted_messages,
            "stream": false
        });

        debug!("Ollama request: {}", serde_json::to_string_pretty(&body)?);

        let response = self
            .client
            .post(format!("{}/api/chat", self.endpoint))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let response_body: Value = response.json().await?;
        debug!(
            "Ollama response: {}",
            serde_json::to_string_pretty(&response_body)?
        );

        let content = response_body["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        Ok(LLMResponse::Text(content))
    }

    async fn summarize(&self, text: &str) -> Result<String> {
        let messages = vec![Message {
            role: Role::User,
            content: format!(
                "Summarize the following conversation concisely, preserving key information and context:\n\n{}",
                text
            ),
            tool_calls: None,
            tool_call_id: None,
        }];

        match self.chat(&messages, None).await? {
            LLMResponse::Text(summary) => Ok(summary),
            _ => anyhow::bail!("Unexpected response type"),
        }
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _tools: Option<&[ToolSchema]>,
    ) -> Result<StreamResult> {
        let formatted_messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "user",
                    },
                    "content": m.content
                })
            })
            .collect();

        let body = json!({
            "model": self.model,
            "messages": formatted_messages,
            "stream": true
        });

        debug!(
            "Ollama streaming request: {}",
            serde_json::to_string_pretty(&body)?
        );

        let response = self
            .client
            .post(format!("{}/api/chat", self.endpoint))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        // Ollama streams newline-delimited JSON
        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        // Process complete lines
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].to_string();
                            buffer = buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Ok(json) = serde_json::from_str::<Value>(&line) {
                                let content = json["message"]["content"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                let done = json["done"].as_bool().unwrap_or(false);

                                yield Ok(StreamChunk {
                                    delta: content,
                                    done,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(anyhow::anyhow!("Stream error: {}", e));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}
