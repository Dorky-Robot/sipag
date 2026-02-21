//! Token usage tracking per worker run.
//!
//! Records are stored in `~/.sipag/usage.log` as newline-delimited JSON (NDJSON).
//! One record per worker run (issue or PR iteration).

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// A single worker run usage record.
///
/// Stored as a NDJSON line in `~/.sipag/usage.log`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    /// ISO-8601 timestamp when the worker finished.
    pub ts: String,
    /// Repository in `"owner/repo"` format.
    pub repo: String,
    /// GitHub issue number, or `None` for PR iterations.
    pub issue: Option<u64>,
    /// Task ID (slug-based identifier).
    pub task_id: String,
    /// Input tokens consumed, or `None` if not captured.
    pub input_tokens: Option<u64>,
    /// Output tokens generated, or `None` if not captured.
    pub output_tokens: Option<u64>,
    /// Cache read tokens, or `None` if not captured.
    pub cache_read_tokens: Option<u64>,
    /// Actual cost in USD reported by Claude, or `None` if not captured.
    pub cost_usd: Option<f64>,
    /// Wall-clock duration in seconds.
    pub duration_s: u64,
    /// `"success"` or `"failure"`.
    pub result: String,
}

impl UsageRecord {
    /// Serialize to a NDJSON line.
    pub fn to_ndjson(&self) -> String {
        let issue = opt_u64_json(self.issue);
        let input = opt_u64_json(self.input_tokens);
        let output = opt_u64_json(self.output_tokens);
        let cache = opt_u64_json(self.cache_read_tokens);
        let cost = match self.cost_usd {
            Some(c) => format!("{c:.6}"),
            None => "null".to_string(),
        };
        format!(
            r#"{{"ts":"{ts}","repo":"{repo}","issue":{issue},"task_id":"{task_id}","input_tokens":{input},"output_tokens":{output},"cache_read_tokens":{cache},"cost_usd":{cost},"duration_s":{dur},"result":"{result}"}}"#,
            ts = self.ts,
            repo = json_escape(&self.repo),
            task_id = json_escape(&self.task_id),
            dur = self.duration_s,
            result = json_escape(&self.result),
        )
    }

    /// Parse from a NDJSON line. Returns `None` if the line cannot be parsed.
    pub fn from_ndjson(line: &str) -> Option<Self> {
        let ts = extract_str(line, "ts")?;
        let repo = extract_str(line, "repo")?;
        let issue = extract_nullable_u64(line, "issue");
        let task_id = extract_str(line, "task_id").unwrap_or_default();
        let input_tokens = extract_nullable_u64(line, "input_tokens");
        let output_tokens = extract_nullable_u64(line, "output_tokens");
        let cache_read_tokens = extract_nullable_u64(line, "cache_read_tokens");
        let cost_usd = extract_nullable_f64(line, "cost_usd");
        let duration_s = extract_nullable_u64(line, "duration_s").unwrap_or(0);
        let result = extract_str(line, "result").unwrap_or_else(|| "unknown".to_string());

        Some(Self {
            ts,
            repo,
            issue,
            task_id,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cost_usd,
            duration_s,
            result,
        })
    }

    /// Effective cost in USD: actual if known, otherwise estimated from token counts
    /// using Claude Sonnet 3.5 pricing ($3/M input tokens, $15/M output tokens).
    pub fn effective_cost_usd(&self) -> f64 {
        if let Some(cost) = self.cost_usd {
            return cost;
        }
        let input = self.input_tokens.unwrap_or(0) as f64 * 3.0 / 1_000_000.0;
        let output = self.output_tokens.unwrap_or(0) as f64 * 15.0 / 1_000_000.0;
        input + output
    }
}

/// Parse token usage from the text content of a Claude log file.
///
/// Handles common Claude Code output formats (plain text and JSON-like).
/// Returns `(input_tokens, output_tokens, cache_read_tokens, cost_usd)`.
/// Fields that cannot be parsed are `None`.
pub fn parse_token_usage(content: &str) -> (Option<u64>, Option<u64>, Option<u64>, Option<f64>) {
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut cache_read_tokens: Option<u64> = None;
    let mut cost_usd: Option<f64> = None;

    for line in content.lines() {
        let lower = line.to_lowercase();

        // Cache read tokens — must be checked before generic "input" to avoid
        // matching "cache_read_input_tokens" for the input_tokens field.
        if cache_read_tokens.is_none()
            && lower.contains("cache")
            && lower.contains("read")
        {
            // Try JSON: "cache_read_input_tokens":N
            if let Some(n) = extract_count_after(&lower, "\"cache_read_input_tokens\":") {
                cache_read_tokens = Some(n);
            } else if let Some(n) = extract_count_after(&lower, "cache_read_tokens:") {
                cache_read_tokens = Some(n);
            } else if let Some(n) = extract_count_after(&lower, "cache read tokens:") {
                cache_read_tokens = Some(n);
            }
        }

        // Input tokens — skip lines that are about cache to avoid false matches.
        if input_tokens.is_none() && lower.contains("input") && !lower.contains("cache") {
            // JSON: "input_tokens":N
            if let Some(n) = extract_count_after(&lower, "\"input_tokens\":") {
                input_tokens = Some(n);
            // Text: "input_tokens: N" or "input tokens: N"
            } else if let Some(n) = extract_count_after(&lower, "input_tokens:") {
                input_tokens = Some(n);
            } else if let Some(n) = extract_count_after(&lower, "input tokens:") {
                input_tokens = Some(n);
            // Brief: "input: N tokens" (number directly after "input:")
            } else if lower.contains("token") {
                if let Some(n) = extract_count_after(&lower, "input:") {
                    input_tokens = Some(n);
                }
            }
        }

        // Output tokens — same approach.
        if output_tokens.is_none() && lower.contains("output") && !lower.contains("cache") {
            if let Some(n) = extract_count_after(&lower, "\"output_tokens\":") {
                output_tokens = Some(n);
            } else if let Some(n) = extract_count_after(&lower, "output_tokens:") {
                output_tokens = Some(n);
            } else if let Some(n) = extract_count_after(&lower, "output tokens:") {
                output_tokens = Some(n);
            } else if lower.contains("token") {
                if let Some(n) = extract_count_after(&lower, "output:") {
                    output_tokens = Some(n);
                }
            }
        }

        // Cost — JSON "cost_usd" field or text "$N.NN" with "cost" on the same line.
        if cost_usd.is_none() {
            if let Some(c) = extract_count_after_float(&lower, "\"cost_usd\":") {
                cost_usd = Some(c);
            } else if let Some(c) = extract_count_after_float(&lower, "\"total_cost_usd\":") {
                cost_usd = Some(c);
            } else if lower.contains("cost") {
                if let Some(c) = extract_dollar_amount(&lower) {
                    cost_usd = Some(c);
                }
            }
        }
    }

    (input_tokens, output_tokens, cache_read_tokens, cost_usd)
}

/// Find `keyword` in `line`, then extract the first integer (with optional k/M suffix)
/// that follows it.
fn extract_count_after(line: &str, keyword: &str) -> Option<u64> {
    let pos = line.find(keyword)? + keyword.len();
    let rest = &line[pos..];

    // Skip non-digit characters
    let digit_start = rest.find(|c: char| c.is_ascii_digit())?;
    let rest = &rest[digit_start..];

    // Collect digits and commas (e.g. "85,000")
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != ',')
        .unwrap_or(rest.len());
    let num_str: String = rest[..end].chars().filter(|c| c.is_ascii_digit()).collect();
    if num_str.is_empty() {
        return None;
    }
    let base: u64 = num_str.parse().ok()?;

    // Optional k/K or m/M multiplier suffix
    let multiplier = match rest[end..].chars().next() {
        Some('k') | Some('K') => 1_000u64,
        Some('m') | Some('M') => 1_000_000u64,
        _ => 1u64,
    };

    Some(base * multiplier)
}

/// Like `extract_count_after` but returns an `f64` (for cost fields).
fn extract_count_after_float(line: &str, keyword: &str) -> Option<f64> {
    let pos = line.find(keyword)? + keyword.len();
    let rest = &line[pos..];
    if rest.starts_with("null") {
        return None;
    }
    let digit_start = rest.find(|c: char| c.is_ascii_digit() || c == '.')?;
    let rest = &rest[digit_start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Extract the first `$N.NN` amount from a line.
fn extract_dollar_amount(line: &str) -> Option<f64> {
    let pos = line.find('$')?;
    let rest = &line[pos + 1..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse().ok()
}

/// Append a usage record to `<sipag_dir>/usage.log`.
pub fn append_usage(sipag_dir: &Path, record: &UsageRecord) -> Result<()> {
    let log_path = sipag_dir.join("usage.log");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open {}", log_path.display()))?;
    writeln!(file, "{}", record.to_ndjson())?;
    Ok(())
}

/// Load all usage records from `<sipag_dir>/usage.log`.
pub fn load_usage(sipag_dir: &Path) -> Result<Vec<UsageRecord>> {
    let log_path = sipag_dir.join("usage.log");
    if !log_path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(&log_path)
        .with_context(|| format!("Failed to open {}", log_path.display()))?;
    let reader = BufReader::new(file);
    let records = reader
        .lines()
        .filter_map(|line| line.ok())
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| UsageRecord::from_ndjson(&line))
        .collect();
    Ok(records)
}

// ─── NDJSON helpers ───────────────────────────────────────────────────────────

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn opt_u64_json(n: Option<u64>) -> String {
    match n {
        Some(v) => v.to_string(),
        None => "null".to_string(),
    }
}

/// Extract a JSON string field: `"key":"value"`.
fn extract_str(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", key);
    let start = s.find(&needle)? + needle.len();
    let bytes = s[start..].as_bytes();
    let mut result = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'"' => result.push('"'),
                b'\\' => result.push('\\'),
                b'n' => result.push('\n'),
                b'r' => result.push('\r'),
                b't' => result.push('\t'),
                c => {
                    result.push('\\');
                    result.push(c as char);
                }
            }
            i += 2;
        } else if bytes[i] == b'"' {
            break;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    Some(result)
}

/// Extract a nullable `u64` field: `"key":123` or `"key":null`.
fn extract_nullable_u64(s: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    if rest.starts_with("null") {
        return None;
    }
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse().ok()
}

/// Extract a nullable `f64` field: `"key":1.23` or `"key":null`.
fn extract_nullable_f64(s: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{}\":", key);
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    if rest.starts_with("null") {
        return None;
    }
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_ndjson_full() {
        let r = UsageRecord {
            ts: "2026-02-20T18:30:00Z".to_string(),
            repo: "org/repo".to_string(),
            issue: Some(42),
            task_id: "20260220-fix-auth".to_string(),
            input_tokens: Some(85000),
            output_tokens: Some(12000),
            cache_read_tokens: Some(0),
            cost_usd: Some(0.14),
            duration_s: 420,
            result: "success".to_string(),
        };
        let json = r.to_ndjson();
        assert!(json.contains(r#""ts":"2026-02-20T18:30:00Z""#));
        assert!(json.contains(r#""repo":"org/repo""#));
        assert!(json.contains(r#""issue":42"#));
        assert!(json.contains(r#""input_tokens":85000"#));
        assert!(json.contains(r#""output_tokens":12000"#));
        assert!(json.contains(r#""duration_s":420"#));
        assert!(json.contains(r#""result":"success""#));
    }

    #[test]
    fn test_to_ndjson_nulls() {
        let r = UsageRecord {
            ts: "2026-02-20T18:30:00Z".to_string(),
            repo: "org/repo".to_string(),
            issue: None,
            task_id: "pr-42-iter".to_string(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cost_usd: None,
            duration_s: 60,
            result: "success".to_string(),
        };
        let json = r.to_ndjson();
        assert!(json.contains(r#""issue":null"#));
        assert!(json.contains(r#""input_tokens":null"#));
        assert!(json.contains(r#""cost_usd":null"#));
    }

    #[test]
    fn test_roundtrip() {
        let r = UsageRecord {
            ts: "2026-02-20T18:30:00Z".to_string(),
            repo: "org/repo".to_string(),
            issue: Some(42),
            task_id: "20260220-fix-auth".to_string(),
            input_tokens: Some(85000),
            output_tokens: Some(12000),
            cache_read_tokens: Some(500),
            cost_usd: Some(0.14),
            duration_s: 420,
            result: "success".to_string(),
        };
        let json = r.to_ndjson();
        let parsed = UsageRecord::from_ndjson(&json).expect("parse failed");
        assert_eq!(parsed.ts, r.ts);
        assert_eq!(parsed.repo, r.repo);
        assert_eq!(parsed.issue, r.issue);
        assert_eq!(parsed.task_id, r.task_id);
        assert_eq!(parsed.input_tokens, r.input_tokens);
        assert_eq!(parsed.output_tokens, r.output_tokens);
        assert_eq!(parsed.cache_read_tokens, r.cache_read_tokens);
        assert_eq!(parsed.duration_s, r.duration_s);
        assert_eq!(parsed.result, r.result);
    }

    #[test]
    fn test_roundtrip_nulls() {
        let r = UsageRecord {
            ts: "2026-02-20T18:30:00Z".to_string(),
            repo: "org/repo".to_string(),
            issue: None,
            task_id: "pr-42-iter".to_string(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cost_usd: None,
            duration_s: 60,
            result: "failure".to_string(),
        };
        let json = r.to_ndjson();
        let parsed = UsageRecord::from_ndjson(&json).expect("parse failed");
        assert_eq!(parsed.issue, None);
        assert_eq!(parsed.input_tokens, None);
        assert_eq!(parsed.cost_usd, None);
    }

    #[test]
    fn test_from_ndjson_invalid() {
        assert!(UsageRecord::from_ndjson("not json").is_none());
        assert!(UsageRecord::from_ndjson("").is_none());
        // Missing required fields
        assert!(UsageRecord::from_ndjson(r#"{"repo":"org/repo"}"#).is_none());
    }

    #[test]
    fn test_parse_token_usage_json_format() {
        // JSON-like format (e.g., from claude --output-format json)
        let content = r#"{"input_tokens":85000,"output_tokens":12000,"cache_read_input_tokens":500,"cost_usd":0.14}"#;
        let (input, output, cache, cost) = parse_token_usage(content);
        assert_eq!(input, Some(85000));
        assert_eq!(output, Some(12000));
        assert_eq!(cache, Some(500));
        assert!(cost.is_some());
        assert!((cost.unwrap() - 0.14).abs() < 0.001);
    }

    #[test]
    fn test_parse_token_usage_text_format() {
        let content = "Input tokens: 85,000\nOutput tokens: 12,000\nTotal cost: $0.14\n";
        let (input, output, _cache, cost) = parse_token_usage(content);
        assert_eq!(input, Some(85000));
        assert_eq!(output, Some(12000));
        assert!(cost.is_some());
        assert!((cost.unwrap() - 0.14).abs() < 0.001);
    }

    #[test]
    fn test_parse_token_usage_k_suffix() {
        let content = "Input: 85k tokens\nOutput: 12k tokens\nCost: $0.14\n";
        let (input, output, _cache, cost) = parse_token_usage(content);
        assert_eq!(input, Some(85_000));
        assert_eq!(output, Some(12_000));
        assert!(cost.is_some());
    }

    #[test]
    fn test_parse_token_usage_empty() {
        let (input, output, cache, cost) = parse_token_usage("");
        assert_eq!(input, None);
        assert_eq!(output, None);
        assert_eq!(cache, None);
        assert_eq!(cost, None);
    }

    #[test]
    fn test_effective_cost_actual() {
        let r = UsageRecord {
            ts: String::new(),
            repo: String::new(),
            issue: None,
            task_id: String::new(),
            input_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
            cache_read_tokens: None,
            cost_usd: Some(5.0),
            duration_s: 0,
            result: String::new(),
        };
        assert!((r.effective_cost_usd() - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_effective_cost_estimated() {
        let r = UsageRecord {
            ts: String::new(),
            repo: String::new(),
            issue: None,
            task_id: String::new(),
            input_tokens: Some(1_000_000), // $3
            output_tokens: Some(1_000_000), // $15
            cache_read_tokens: None,
            cost_usd: None,
            duration_s: 0,
            result: String::new(),
        };
        // $3 + $15 = $18
        assert!((r.effective_cost_usd() - 18.0).abs() < 0.001);
    }

    #[test]
    fn test_append_and_load_usage() {
        let dir = tempfile::TempDir::new().unwrap();
        let record = UsageRecord {
            ts: "2026-02-20T18:30:00Z".to_string(),
            repo: "org/repo".to_string(),
            issue: Some(42),
            task_id: "20260220-fix-auth".to_string(),
            input_tokens: Some(85000),
            output_tokens: Some(12000),
            cache_read_tokens: None,
            cost_usd: Some(0.14),
            duration_s: 420,
            result: "success".to_string(),
        };
        append_usage(dir.path(), &record).unwrap();
        let records = load_usage(dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].repo, "org/repo");
        assert_eq!(records[0].issue, Some(42));
    }

    #[test]
    fn test_load_usage_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let records = load_usage(dir.path()).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_json_escape() {
        assert_eq!(json_escape("org/repo"), "org/repo");
        assert_eq!(json_escape("has\"quote"), "has\\\"quote");
        assert_eq!(json_escape("has\\back"), "has\\\\back");
    }
}
