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
    /// Bearer token for the upstream. `None` means no `Authorization`
    /// header. Used when `OLLAMA_HOST` points at a katulong-style
    /// authenticated bridge instead of a raw ollama daemon.
    pub auth_bearer: Option<String>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            model: env_model(),
            temperature: 0.7,
            num_predict: None,
            auth_bearer: env_auth(),
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

/// Resolve the bearer token for an authenticated bridge from env.
///
/// `OLLAMA_AUTH` holds either the token itself or `@<path>` to point at
/// a file (curl convention). Whitespace is trimmed. Returns `None` when
/// unset or empty so callers can simply skip the `Authorization` header.
pub fn env_auth() -> Option<String> {
    let raw = std::env::var("OLLAMA_AUTH").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(path) = trimmed.strip_prefix('@') {
        let expanded = expand_tilde(path);
        let contents = std::fs::read_to_string(&expanded).ok()?;
        let token = contents.trim().to_string();
        if token.is_empty() {
            return None;
        }
        return Some(token);
    }
    Some(trimmed.to_string())
}

fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    temperature: f32,
    #[serde(rename = "max_tokens", skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct SseChunk {
    #[serde(default)]
    choices: Vec<SseChoice>,
    #[serde(default)]
    error: Option<SseError>,
}

#[derive(Deserialize)]
struct SseChoice {
    #[serde(default)]
    delta: SseDelta,
}

#[derive(Deserialize, Default)]
struct SseDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct SseError {
    #[serde(default)]
    message: String,
}

/// Send a chat request and return the assistant's text.
///
/// We talk to the OpenAI-compatible endpoint (`/v1/chat/completions`)
/// rather than ollama's native `/api/chat`. Both ollama and any future
/// drop-in replacement (Anthropic, Grok, hosted inference) speak this
/// shape, and — crucially — the response is `Content-Type:
/// text/event-stream`, which Cloudflare and other CDNs recognize as
/// "do not buffer." Ollama's NDJSON path triggers the 100s no-first-
/// byte timeout because the edge holds the response.
pub async fn chat(
    http: &reqwest::Client,
    host: &str,
    messages: Vec<ChatMessage>,
    opts: ChatOptions,
) -> Result<String, LlmError> {
    use futures_util::StreamExt;

    let url = format!("{}/v1/chat/completions", host.trim_end_matches('/'));
    let req = ChatRequest {
        model: &opts.model,
        messages: &messages,
        stream: true,
        temperature: opts.temperature,
        max_tokens: opts.num_predict,
    };

    let mut builder = http.post(&url).json(&req);
    if let Some(token) = opts.auth_bearer.as_deref() {
        builder = builder.bearer_auth(token);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| LlmError::Transport(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(LlmError::Http(status.as_u16(), body));
    }

    // Parse the SSE stream. Frames are `data: <json>\n\n`, terminated
    // by `data: [DONE]`. Lines starting with `:` are keepalive comments
    // (ignored). Buffer partial lines across chunks so we don't crash
    // on a frame split mid-event.
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut out = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| LlmError::Transport(e.to_string()))?;
        buf.push_str(std::str::from_utf8(&bytes).map_err(|e| LlmError::BadResponse(e.to_string()))?);
        while let Some(idx) = buf.find('\n') {
            let line = buf[..idx].to_string();
            buf.drain(..=idx);
            let trimmed = line.trim_end_matches('\r');
            if trimmed.is_empty() || trimmed.starts_with(':') {
                continue;
            }
            let Some(payload) = trimmed.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                buf.clear();
                break;
            }
            let frame: SseChunk = serde_json::from_str(payload)
                .map_err(|e| LlmError::BadResponse(format!("bad frame: {e}")))?;
            if let Some(err) = frame.error {
                return Err(LlmError::BadResponse(err.message));
            }
            for choice in frame.choices {
                if let Some(content) = choice.delta.content {
                    out.push_str(&content);
                }
            }
        }
    }
    if out.is_empty() {
        return Err(LlmError::BadResponse("no content in stream".into()));
    }
    Ok(out)
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
    fn parses_sse_chunk_shape() {
        // One frame from /v1/chat/completions with stream=true.
        let raw = r#"{"id":"x","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#;
        let parsed: SseChunk = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.choices.len(), 1);
        assert_eq!(parsed.choices[0].delta.content.as_deref(), Some("hello"));
    }

    #[test]
    fn parses_sse_error_chunk() {
        let raw = r#"{"error":{"message":"model not found","type":"not_found"}}"#;
        let parsed: SseChunk = serde_json::from_str(raw).unwrap();
        assert!(parsed.choices.is_empty());
        assert_eq!(parsed.error.unwrap().message, "model not found");
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
