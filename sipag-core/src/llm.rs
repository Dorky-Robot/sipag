//! Ollama HTTP client.
//!
//! Local-first LLM access for sipag's autonomous workers. Talks to a
//! running ollama daemon (default `http://localhost:11434`) via its
//! `/api/chat` endpoint. Workers gather context, build chat-style
//! `messages`, and call `chat(...)`.
//!
//! No streaming yet (v2). Workers care about the final synthesized text
//! more than token-by-token playback.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default ollama daemon address. Override with `OLLAMA_HOST`.
pub const DEFAULT_HOST: &str = "http://localhost:11434";
/// Default model. Override with `OLLAMA_MODEL`. Picked to be a sane
/// "available on most installs" default; nothing special about 8B
/// other than balance of latency vs. quality on a workstation.
pub const DEFAULT_MODEL: &str = "llama3.1:8b";

/// One message in a chat-style conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// `"system"`, `"user"`, or `"assistant"`.
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// Knobs for `chat`.
#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub model: String,
    pub temperature: f32,
    /// Max tokens to predict. `None` lets ollama decide.
    pub num_predict: Option<u32>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            model: env_model(),
            temperature: 0.7,
            num_predict: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("ollama transport error: {0}")]
    Transport(String),
    #[error("ollama returned HTTP {0}: {1}")]
    Http(u16, String),
    #[error("ollama response shape unexpected: {0}")]
    BadResponse(String),
}

/// Resolve the ollama base URL from env, falling back to the default.
pub fn env_host() -> String {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string())
}

/// Resolve the model name from env, falling back to the default.
pub fn env_model() -> String {
    std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    options: ChatRequestOptions,
}

#[derive(Serialize)]
struct ChatRequestOptions {
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    message: Option<ResponseMessage>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
}

/// Send a chat request to ollama and return the assistant's text.
///
/// Uses the supplied `reqwest::Client` and `host` (e.g.
/// `http://localhost:11434`). The `host` is whatever `OLLAMA_HOST`
/// resolves to in the caller; we don't read env here so injection is
/// cheap in tests.
pub async fn chat(
    http: &reqwest::Client,
    host: &str,
    messages: Vec<ChatMessage>,
    opts: ChatOptions,
) -> Result<String, LlmError> {
    let url = format!("{}/api/chat", host.trim_end_matches('/'));
    let req = ChatRequest {
        model: &opts.model,
        messages: &messages,
        stream: false,
        options: ChatRequestOptions {
            temperature: opts.temperature,
            num_predict: opts.num_predict,
        },
    };

    let resp = http
        .post(&url)
        .json(&req)
        .send()
        .await
        .map_err(|e| LlmError::Transport(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(LlmError::Http(status.as_u16(), body));
    }
    let body: ChatResponse = resp
        .json()
        .await
        .map_err(|e| LlmError::BadResponse(e.to_string()))?;
    if let Some(err) = body.error {
        return Err(LlmError::BadResponse(err));
    }
    let msg = body
        .message
        .ok_or_else(|| LlmError::BadResponse("missing message".into()))?;
    Ok(msg.content)
}

/// Convenience: read host/model from env and call `chat`.
pub async fn chat_with_env(
    http: &reqwest::Client,
    messages: Vec<ChatMessage>,
) -> Result<String, LlmError> {
    chat(http, &env_host(), messages, ChatOptions::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_message_helpers() {
        let s = ChatMessage::system("be concise");
        assert_eq!(s.role, "system");
        assert_eq!(s.content, "be concise");
        let u = ChatMessage::user("hello");
        assert_eq!(u.role, "user");
        let a = ChatMessage::assistant("hi");
        assert_eq!(a.role, "assistant");
    }

    #[test]
    fn parses_chat_response_shape() {
        // The JSON shape we care about — non-streaming /api/chat reply.
        let raw = r#"{"model":"llama3.1:8b","created_at":"2026-04-26T00:00:00Z","message":{"role":"assistant","content":"hello there"},"done":true}"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let msg = parsed.message.expect("message present");
        assert_eq!(msg.content, "hello there");
    }

    #[test]
    fn parses_error_response() {
        let raw = r#"{"error":"model not found"}"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.message.is_none());
        assert_eq!(parsed.error.as_deref(), Some("model not found"));
    }

    #[test]
    fn defaults_use_env_when_set() {
        // We can only assert these don't panic in CI where env may be
        // unset — the explicit env-mutation tests are tricky with shared
        // process state. Just make sure the helpers return something.
        let host = env_host();
        assert!(host.starts_with("http"));
        let model = env_model();
        assert!(!model.is_empty());
    }
}
