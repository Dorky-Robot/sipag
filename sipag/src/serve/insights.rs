//! Knowledge-base insights for ambient drafting.
//!
//! Sipag enriches the drafting surface (objective / KR / task forms)
//! with insights pulled from the project's indexed history. While the
//! user types, the form shows ~3 relevant decisions / patterns / scars
//! / learnings that the codebase already remembers. The intent: turn
//! drafting into a hallway-collision moment between current intent and
//! prior knowledge.
//!
//! Implementation detail: shells out to the `diwa` CLI. We deliberately
//! avoid reading diwa's sqlite directly so we don't break its
//! encapsulation — if diwa changes its schema, sipag still works as
//! long as the CLI surface is stable.
//!
//! Two endpoints:
//!   GET /api/insights/repos                          → known-repo list
//!   GET /api/insights/search?q=…&repo=…&n=…          → ranked insights
//!
//! Both are auth-gated (the gate sits above the router).

use crate::serve::error::ApiError;
use crate::serve::state::AppState;
use axum::{
    extract::Query,
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;

const DIWA_BIN: &str = "diwa";
const DEFAULT_SEARCH_N: usize = 3;
const MAX_SEARCH_N: usize = 25;
const MAX_QUERY_LEN: usize = 256;
const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/insights/repos", get(list_repos))
        .route("/api/insights/search", get(search_insights))
}

// ---------- /api/insights/repos ----------

#[derive(Debug, Serialize, Clone)]
pub struct KnownRepo {
    /// Canonical name (e.g. `Dorky-Robot/sipag`). Stable across machines.
    pub name: String,
    pub path: String,
    pub insights_count: u32,
}

async fn list_repos() -> Result<Json<Vec<KnownRepo>>, ApiError> {
    let out = run_diwa(&["ls"]).await?;
    Ok(Json(parse_diwa_ls(&out)))
}

/// Parse `diwa ls` output. Format per line (after ANSI stripping):
/// `  <name>  <count> insights  <path>`
fn parse_diwa_ls(stdout: &str) -> Vec<KnownRepo> {
    stdout
        .lines()
        .filter_map(|line| {
            let stripped = strip_ansi(line);
            let trimmed = stripped.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Split on runs of 2+ spaces — the diwa ls format uses
            // two-space gaps between fields.
            let parts: Vec<&str> = trimmed.split("  ").filter(|s| !s.is_empty()).collect();
            if parts.len() < 3 {
                return None;
            }
            let name = parts[0].trim().to_string();
            let count: u32 = parts[1]
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())?;
            let path = parts[2].trim().to_string();
            if name.is_empty() || path.is_empty() {
                return None;
            }
            Some(KnownRepo {
                name,
                path,
                insights_count: count,
            })
        })
        .collect()
}

/// Strip ANSI escape sequences (CSI: ESC `[` digits/`;` `m`). Hand-rolled
/// so we don't pull in a regex crate just for this.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // Skip until the terminating letter.
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // skip the terminating letter
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------- /api/insights/search ----------

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    repo: Option<String>,
    n: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Insight {
    pub id: u64,
    pub commit_sha: String,
    pub commit_date: String,
    /// `decision` | `pattern` | `scar` | `learning` | etc.
    pub category: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub pr_number: Option<u64>,
    /// Carries the repo so callers know which one matched in cross-repo
    /// searches. Filled in by sipag (diwa's JSON doesn't include it).
    #[serde(default)]
    pub repo: String,
}

async fn search_insights(
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<Insight>>, ApiError> {
    let query = q.q.unwrap_or_default();
    let n = q.n.unwrap_or(DEFAULT_SEARCH_N);
    Ok(Json(search(&query, q.repo.as_deref(), n).await?))
}

/// Direct callable for non-HTTP consumers (the HTMX spike, future
/// templated handlers). Same shape as `GET /api/insights/search`.
pub async fn search(
    query: &str,
    repo: Option<&str>,
    n: usize,
) -> Result<Vec<Insight>, ApiError> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query.len() > MAX_QUERY_LEN {
        return Err(ApiError::BadRequest("query too long"));
    }
    let n = n.min(MAX_SEARCH_N).max(1);

    let repos: Vec<String> = match repo {
        Some(r) if !r.trim().is_empty() => vec![r.trim().to_string()],
        _ => {
            // No repo specified — search across every indexed repo.
            let out = run_diwa(&["ls"]).await.unwrap_or_default();
            parse_diwa_ls(&out).into_iter().map(|r| r.name).collect()
        }
    };

    let safe_query = sanitize_query(query);
    if safe_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut all: Vec<Insight> = Vec::new();
    // Sequential, not parallel — diwa shells out to its own indexer
    // and parallel processes are needless complexity for a 24-repo
    // worst case. If this gets slow, revisit.
    for repo in repos {
        let n_str = n.to_string();
        let args = ["search", &repo, &safe_query, "--json", "-n", &n_str];
        let Ok(out) = run_diwa(&args).await else {
            continue;
        };
        let Ok(rows): Result<Vec<Insight>, _> = serde_json::from_str(&out) else {
            continue;
        };
        for mut row in rows {
            if row.repo.is_empty() {
                row.repo = repo.clone();
            }
            all.push(row);
        }
    }

    // Sort by commit_date desc — most-recent-first matches the
    // "what's been figured out lately" framing better than rank-by-FTS.
    all.sort_by(|a, b| b.commit_date.cmp(&a.commit_date));
    all.truncate(n);

    Ok(all)
}

/// Conservative query sanitizer. diwa's `search` uses sqlite FTS5,
/// which interprets `"`, `*`, `(`, `)`, `:` and a few others. We strip
/// them rather than try to escape — this is a draft-time autocomplete,
/// not a power-user search bar, so trading a bit of expressivity for
/// "never crashes the indexer" is the right tradeoff.
fn sanitize_query(q: &str) -> String {
    q.chars()
        .map(|c| match c {
            '"' | '\'' | '(' | ')' | ':' | '*' | '^' | ';' | '\\' | '`' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------- shell-out helpers ----------

async fn run_diwa(args: &[&str]) -> Result<String, ApiError> {
    let mut cmd = Command::new(DIWA_BIN);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let exec = tokio::time::timeout(SEARCH_TIMEOUT, cmd.output()).await;
    let output = match exec {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, args = ?args, "diwa exec failed");
            return Err(ApiError::Internal(anyhow::anyhow!(
                "diwa exec failed: {e}"
            )));
        }
        Err(_) => {
            tracing::warn!(args = ?args, "diwa exec timed out");
            return Err(ApiError::Internal(anyhow::anyhow!("diwa timed out")));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            args = ?args,
            status = ?output.status,
            stderr = %stderr,
            "diwa returned non-zero"
        );
        return Err(ApiError::Internal(anyhow::anyhow!(
            "diwa returned non-zero: {}",
            stderr.lines().next().unwrap_or("unknown error")
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diwa_ls_handles_ansi_stripped_lines() {
        let raw = "  Dorky-Robot/sipag  \x1b[90m222 insights  ~/Projects/dorky_robot/sipag\x1b[0m\n  Dorky-Robot/alon  \x1b[90m39 insights  ~/Projects/dorky_robot/alon\x1b[0m\n";
        let parsed = parse_diwa_ls(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "Dorky-Robot/sipag");
        assert_eq!(parsed[0].insights_count, 222);
        assert_eq!(parsed[0].path, "~/Projects/dorky_robot/sipag");
        assert_eq!(parsed[1].name, "Dorky-Robot/alon");
        assert_eq!(parsed[1].insights_count, 39);
    }

    #[test]
    fn parse_diwa_ls_skips_summary_lines() {
        let raw = "  Dorky-Robot/sipag  222 insights  ~/Projects/dorky_robot/sipag\n\n24 repos indexed.\n";
        let parsed = parse_diwa_ls(raw);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parse_diwa_ls_empty_input() {
        assert!(parse_diwa_ls("").is_empty());
        assert!(parse_diwa_ls("\n\n").is_empty());
    }

    #[test]
    fn strip_ansi_removes_color_sequences() {
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b[90mgrey\x1b[0m"), "grey");
        assert_eq!(
            strip_ansi("a\x1b[1;32mb\x1b[0mc"),
            "abc"
        );
    }

    #[test]
    fn sanitize_query_strips_fts_metacharacters() {
        assert_eq!(sanitize_query("auth refactor"), "auth refactor");
        assert_eq!(
            sanitize_query("auth \"refactor\" (#3)"),
            "auth refactor #3"
        );
        assert_eq!(sanitize_query("*"), "");
        assert_eq!(sanitize_query(":"), "");
        assert_eq!(sanitize_query("  hello  world  "), "hello world");
    }
}
