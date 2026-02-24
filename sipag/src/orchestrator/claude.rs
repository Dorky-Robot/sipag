use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use sipag_core::config::Credentials;
use std::process::Command;

/// Result of a non-interactive Claude invocation.
#[derive(Debug)]
pub struct ClaudeResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Parameters for a single Claude invocation.
pub struct ClaudeInvocation {
    pub prompt: String,
    pub working_dir: Option<String>,
    pub allowed_tools: Vec<String>,
}

/// Invoke `claude -p` with the given prompt, capturing output.
pub fn invoke_claude(invocation: &ClaudeInvocation, creds: &Credentials) -> Result<ClaudeResult> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p").arg(&invocation.prompt);

    if !invocation.allowed_tools.is_empty() {
        cmd.arg("--allowedTools")
            .arg(invocation.allowed_tools.join(","));
    }

    if let Some(ref dir) = invocation.working_dir {
        cmd.current_dir(dir);
    }

    if let Some(ref token) = creds.oauth_token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }
    if let Some(ref key) = creds.api_key {
        cmd.env("ANTHROPIC_API_KEY", key);
    }

    let output = cmd.output().context("Failed to run claude")?;

    Ok(ClaudeResult {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Invoke multiple Claude prompts in parallel using scoped threads.
pub fn invoke_claude_parallel(
    invocations: &[ClaudeInvocation],
    creds: &Credentials,
) -> Vec<Result<ClaudeResult>> {
    std::thread::scope(|s| {
        let handles: Vec<_> = invocations
            .iter()
            .map(|inv| s.spawn(|| invoke_claude(inv, creds)))
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().expect("Claude invocation thread panicked"))
            .collect()
    })
}

/// Extract JSON of type T from Claude's output.
///
/// Looks for JSON in fenced code blocks (```json ... ```) first,
/// then tries parsing the entire output as JSON.
pub fn extract_json<T: DeserializeOwned>(output: &str) -> Result<T> {
    if let Some(json_str) = extract_fenced_json(output) {
        return serde_json::from_str(&json_str)
            .context("Failed to parse JSON from fenced code block");
    }

    serde_json::from_str(output).context("Failed to parse JSON from Claude output")
}

/// Extract the first ```json ... ``` fenced block from text.
fn extract_fenced_json(text: &str) -> Option<String> {
    let mut in_block = false;
    let mut json_lines = Vec::new();

    for line in text.lines() {
        if !in_block {
            let trimmed = line.trim();
            if trimmed == "```json" || trimmed == "```JSON" {
                in_block = true;
                json_lines.clear();
                continue;
            }
        } else if line.trim() == "```" {
            return Some(json_lines.join("\n"));
        } else {
            json_lines.push(line.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_fenced_json_basic() {
        let input = "Some text\n```json\n{\"key\": \"value\"}\n```\nMore text";
        let result = extract_fenced_json(input).unwrap();
        assert_eq!(result, "{\"key\": \"value\"}");
    }

    #[test]
    fn extract_fenced_json_multiline() {
        let input = "```json\n{\n  \"a\": 1,\n  \"b\": 2\n}\n```";
        let result = extract_fenced_json(input).unwrap();
        assert!(result.contains("\"a\": 1"));
    }

    #[test]
    fn extract_fenced_json_none_when_missing() {
        let input = "No JSON here";
        assert!(extract_fenced_json(input).is_none());
    }

    #[test]
    fn extract_json_from_fenced_block() {
        let input = "Here's the result:\n```json\n{\"name\": \"test\", \"count\": 42}\n```";
        let result: serde_json::Value = extract_json(input).unwrap();
        assert_eq!(result["name"], "test");
        assert_eq!(result["count"], 42);
    }

    #[test]
    fn extract_json_from_raw_output() {
        let input = "{\"name\": \"test\"}";
        let result: serde_json::Value = extract_json(input).unwrap();
        assert_eq!(result["name"], "test");
    }

    #[test]
    fn extract_fenced_json_ignores_non_json_blocks() {
        let input = "```rust\nfn main() {}\n```\n```json\n{\"x\": 1}\n```";
        let result = extract_fenced_json(input).unwrap();
        assert_eq!(result, "{\"x\": 1}");
    }

    #[test]
    fn extract_fenced_json_first_block_wins() {
        let input = "```json\n{\"first\": true}\n```\n```json\n{\"second\": true}\n```";
        let result = extract_fenced_json(input).unwrap();
        assert!(result.contains("first"));
    }

    #[test]
    fn extract_fenced_json_empty_block() {
        let input = "```json\n```";
        let result = extract_fenced_json(input).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn extract_fenced_json_unclosed_block() {
        let input = "```json\n{\"key\": \"value\"}";
        assert!(extract_fenced_json(input).is_none());
    }

    #[test]
    fn extract_fenced_json_uppercase() {
        let input = "```JSON\n{\"upper\": true}\n```";
        let result = extract_fenced_json(input).unwrap();
        assert!(result.contains("upper"));
    }

    #[test]
    fn extract_fenced_json_with_surrounding_text() {
        let input = "I found the following:\n\nHere is the analysis:\n\n```json\n[1, 2, 3]\n```\n\nThat's all.";
        let result = extract_fenced_json(input).unwrap();
        assert_eq!(result, "[1, 2, 3]");
    }

    #[test]
    fn extract_json_invalid_json_returns_error() {
        let input = "not json at all";
        let result: Result<serde_json::Value> = extract_json(input);
        assert!(result.is_err());
    }

    #[test]
    fn extract_json_fenced_block_invalid_json_returns_error() {
        let input = "```json\nnot valid json\n```";
        let result: Result<serde_json::Value> = extract_json(input);
        assert!(result.is_err());
    }

    #[test]
    fn extract_json_into_typed_struct() {
        #[derive(serde::Deserialize, Debug)]
        struct TestResult {
            name: String,
            count: u32,
        }

        let input = "```json\n{\"name\": \"test\", \"count\": 42}\n```";
        let result: TestResult = extract_json(input).unwrap();
        assert_eq!(result.name, "test");
        assert_eq!(result.count, 42);
    }

    #[test]
    fn extract_json_array() {
        let input = "```json\n[{\"id\": 1}, {\"id\": 2}]\n```";
        let result: Vec<serde_json::Value> = extract_json(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["id"], 1);
    }

    #[test]
    fn extract_fenced_json_preserves_whitespace_in_values() {
        let input = "```json\n{\"message\": \"hello world\"}\n```";
        let result = extract_fenced_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["message"], "hello world");
    }

    #[test]
    fn extract_fenced_json_nested_objects() {
        let input = "```json\n{\"outer\": {\"inner\": [1, 2, 3]}}\n```";
        let result = extract_fenced_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["outer"]["inner"][0], 1);
    }
}
