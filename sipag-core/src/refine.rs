//! Dispatch Refinement Engine — headless batch.
//!
//! # ⛔ Deprecated 2026-05-17
//!
//! Companion to [`crate::feature`] — the batch processor that turns raw
//! features into actionable tickets via a `claude` subprocess. Deprecated
//! together with `feature` per the work-model reframe: Experimentation
//! replaces the kanban refinement pipeline. See
//! [`crate::feature`]'s top-of-file doc, `docs/modules.md` §3, and project
//! memory `project-sipag-work-model-experimentation` for the rationale.
//!
//! Source preserved per `feedback-deprecate-with-rationale` memory. All
//! wiring has been stripped (CLI `sipag refine` subcommand removed).
//!
//! The Node-port (`katulong/lib/dispatch-refine.js`) translation work
//! captured here may have **reusable** pieces for Experimentation's `act`
//! sub-module if the agent loop ever needs to spawn `claude` directly
//! with `--output-format stream-json` and pump `tool_use` events. Lift
//! deliberately, don't rehabilitate this module in place.
//!
//! ---
//!
//! ## Original docs (preserved for archaeology)
//!
//! Runs a single `claude` subprocess with `--output-format stream-json` to
//! refine one or more raw feature ideas into actionable tickets. Progress
//! bullets are derived from `tool_use` events in the stream and appended to
//! each grouped feature via `Feature::add_log`. The caller supplies an
//! optional `on_progress` callback that fires for each translated bullet
//! (deduped across the whole batch) so TUIs / CLIs can show live activity.
//!
//! This is a port of `katulong/lib/dispatch-refine.js`. The Node version
//! used a `child_process.spawn` + stream-json contract that we faithfully
//! replicate here with `std::process::Command` + `BufReader::lines()`. No
//! tokio — the rest of sipag-core is intentionally sync, and the existing
//! `sipag sub` command already uses this pattern.
//!
//! Error handling is sanitized at the wire boundary: `RefinerError` carries
//! a short `public` string safe to display and a longer `detail` string for
//! logs and tests. Callers should print `public` to the user and log
//! `detail` — never the reverse. This mirrors the `refine-failed` SSE event
//! in the JS which also emits a fixed "Refinement failed — check server
//! logs" string.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value as JsonValue;

use crate::feature::Feature;

/// Boxed progress callback. Aliased so the long trait-object type doesn't
/// clutter function signatures.
type ProgressCb = Box<dyn FnMut(&str) + Send>;

// ── Public surface ───────────────────────────────────────────────────────────

/// One refined ticket as returned by claude. Mirrors the JSON shape the
/// prompt asks for.
#[derive(Debug, Clone)]
pub struct RefinedTicket {
    pub title: String,
    pub spec: String,
    pub project: Option<String>,
    pub source_ids: Vec<String>,
    /// `"refined"` or `"needs-info"`.
    pub status: String,
    pub subtasks: Vec<Subtask>,
    pub estimated_agents: u32,
    pub needs_info_reason: Option<String>,
}

/// A single subtask under a refined ticket.
#[derive(Debug, Clone)]
pub struct Subtask {
    pub id: String,
    pub description: String,
    pub worktree: bool,
}

/// Runtime knobs for `Refiner::refine_batch`. Most callers only need
/// `on_progress`; tests override `claude_bin` to inject a fake subprocess.
#[derive(Default)]
pub struct RefineOptions {
    /// Fired once per unique translated progress bullet (deduped across the
    /// whole batch). Panics in the callback are swallowed so a buggy listener
    /// can't tear down the refine.
    pub on_progress: Option<ProgressCb>,
    /// Override the `claude` binary path. Defaults to the string `"claude"`
    /// (resolved on the PATH at spawn time). Tests point this at a fake
    /// shell script.
    pub claude_bin: Option<PathBuf>,
    /// Extra environment variables to pass to the subprocess. Useful for
    /// tests that need the fake script to look something up.
    pub extra_env: Vec<(String, String)>,
}

/// Error type returned from `Refiner::refine_batch`. The `public` field is
/// a short sanitized message safe to show users; `detail` is the full story
/// and should only go to logs and tests. See the module doc for rationale.
#[derive(Debug, Clone)]
pub struct RefinerError {
    pub public: String,
    pub detail: String,
}

impl RefinerError {
    fn new(public: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            public: public.into(),
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for RefinerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.public)
    }
}

impl std::error::Error for RefinerError {}

/// The refinement engine. Stateless; one instance per call is fine.
#[derive(Debug, Default)]
pub struct Refiner;

impl Refiner {
    pub fn new() -> Self {
        Self
    }

    /// Refine a batch of raw features via a single headless `claude`
    /// subprocess. On success, returns the newly created refined features
    /// (one per ticket from claude). On failure, all input features are
    /// reverted to `raw` (their `grouped_into` cleared) so the caller can
    /// retry.
    pub fn refine_batch(
        &self,
        sipag_dir: &Path,
        project: &str,
        feature_ids: &[String],
        opts: &mut RefineOptions,
    ) -> Result<Vec<Feature>, RefinerError> {
        // 1. Load input features. Missing ids are silently skipped, matching
        //    the JS `.filter(Boolean)` behavior.
        let mut features: Vec<Feature> = Vec::with_capacity(feature_ids.len());
        for id in feature_ids {
            match Feature::get(sipag_dir, project, id) {
                Ok(Some(f)) => features.push(f),
                Ok(None) => {}
                Err(e) => {
                    return Err(RefinerError::new(
                        "Refinement failed",
                        format!("failed to load feature {id}: {e}"),
                    ));
                }
            }
        }
        if features.is_empty() {
            return Err(RefinerError::new(
                "no valid features",
                "No valid features to refine",
            ));
        }

        let session_tag = format!(
            "batch-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .chars()
                .take(8)
                .collect::<String>()
        );

        let feature_refs: Vec<&Feature> = features.iter().collect();
        let prompt = build_batch_prompt(&feature_refs);

        // 2. Spawn claude with the stream-json arg set. Capture stdout/stderr
        //    separately so we can surface stderr in the error detail but
        //    never confuse it with the result text.
        let bin: PathBuf = opts
            .claude_bin
            .clone()
            .unwrap_or_else(|| PathBuf::from("claude"));

        let mut cmd = Command::new(&bin);
        cmd.arg("-p")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--dangerously-skip-permissions")
            .arg(&prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &opts.extra_env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                revert_all_to_raw(sipag_dir, project, &features);
                return Err(RefinerError::new(
                    "Refinement failed",
                    format!("failed to spawn {}: {e}", bin.display()),
                ));
            }
        };

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RefinerError::new("Refinement failed", "claude stdout pipe missing"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RefinerError::new("Refinement failed", "claude stderr pipe missing"))?;

        // 3. Stream-parse stdout, translating tool_use events into bullets.
        let mut last_result_text = String::new();
        let mut last_bullet: HashMap<String, String> = HashMap::new();
        let mut last_broadcast: Option<String> = None;

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<JsonValue>(line) else {
                // Not valid JSON — ignore partial lines, matching the JS.
                continue;
            };
            process_event(
                &event,
                &mut last_result_text,
                &mut last_bullet,
                &mut last_broadcast,
                sipag_dir,
                project,
                &features,
                &mut opts.on_progress,
            );
        }

        // 4. Drain stderr (up to 4 KiB to cap memory) and wait for exit.
        let mut stderr_buf = Vec::with_capacity(512);
        let _ = stderr.take(4096).read_to_end(&mut stderr_buf);
        let status = match child.wait() {
            Ok(s) => s,
            Err(e) => {
                revert_all_to_raw(sipag_dir, project, &features);
                return Err(RefinerError::new(
                    "Refinement failed",
                    format!("failed to wait on claude: {e}"),
                ));
            }
        };

        let exit_code = status.code().unwrap_or(-1);
        let stderr_tail = String::from_utf8_lossy(&stderr_buf)
            .chars()
            .take(500)
            .collect::<String>();

        if !status.success() && last_result_text.trim().is_empty() {
            revert_all_to_raw(sipag_dir, project, &features);
            return Err(RefinerError::new(
                "Refinement failed",
                format!("claude exited with code {exit_code}: {stderr_tail}"),
            ));
        }

        // 5. Parse the final result text.
        let tickets = match parse_result(&last_result_text) {
            Ok(t) if !t.is_empty() => t,
            Ok(_) => {
                revert_all_to_raw(sipag_dir, project, &features);
                return Err(RefinerError::new(
                    "Refinement failed",
                    "failed to parse result: expected non-empty JSON array".to_string(),
                ));
            }
            Err(e) => {
                revert_all_to_raw(sipag_dir, project, &features);
                return Err(RefinerError::new(
                    "Refinement failed",
                    format!("failed to parse result: {e}"),
                ));
            }
        };

        // 6. Create refined features from tickets.
        let mut created: Vec<Feature> = Vec::with_capacity(tickets.len());
        for ticket in &tickets {
            let new_feature = match Feature::add(sipag_dir, project, &ticket.spec, None) {
                Ok(f) => f,
                Err(e) => {
                    revert_all_to_raw(sipag_dir, project, &features);
                    return Err(RefinerError::new(
                        "Refinement failed",
                        format!("failed to create refined feature: {e}"),
                    ));
                }
            };

            let status = if ticket.status == "needs-info" {
                "needs-info".to_string()
            } else {
                "refined".to_string()
            };
            let title = ticket.title.clone();
            let spec = ticket.spec.clone();
            let project_field = ticket.project.clone();
            let source_ids = ticket.source_ids.clone();
            let refined_payload = ticket_to_refined_json(ticket);

            let updated = Feature::update(sipag_dir, project, &new_feature.id, |f| {
                f.status = status;
                f.project = project_field;
                f.source_ids = Some(source_ids);
                f.body = format!("{title}\n\n{spec}");
                f.refined = Some(refined_payload);
            });
            match updated {
                Ok(Some(f)) => created.push(f),
                Ok(None) => {
                    revert_all_to_raw(sipag_dir, project, &features);
                    return Err(RefinerError::new(
                        "Refinement failed",
                        format!("refined feature {} disappeared mid-write", new_feature.id),
                    ));
                }
                Err(e) => {
                    revert_all_to_raw(sipag_dir, project, &features);
                    return Err(RefinerError::new(
                        "Refinement failed",
                        format!("failed to update refined feature: {e}"),
                    ));
                }
            }
        }

        // 7. Mark source features as `grouped`. Skip any that are already
        //    grouped — the caller may have pre-marked them with its own
        //    session tag (the JS comment about not clobbering pre-set tags).
        for f in &features {
            let latest = match Feature::get(sipag_dir, project, &f.id) {
                Ok(Some(l)) => l,
                _ => continue,
            };
            if latest.status == "grouped" {
                continue;
            }
            let tag = session_tag.clone();
            if let Err(e) = Feature::update(sipag_dir, project, &f.id, |src| {
                src.status = "grouped".to_string();
                src.grouped_into = Some(tag);
            }) {
                // Non-fatal: log and continue. Reverting here would undo
                // already-created refined features and is surprising.
                log::warn!("failed to mark feature {} grouped: {e}", f.id);
            }
        }

        Ok(created)
    }
}

// ── Event processing ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn process_event(
    event: &JsonValue,
    last_result_text: &mut String,
    last_bullet: &mut HashMap<String, String>,
    last_broadcast: &mut Option<String>,
    sipag_dir: &Path,
    project: &str,
    features: &[Feature],
    on_progress: &mut Option<ProgressCb>,
) {
    let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ty {
        "result" => {
            if let Some(s) = event.get("result").and_then(|v| v.as_str()) {
                *last_result_text = s.to_string();
            } else if let Some(s) = event.get("text").and_then(|v| v.as_str()) {
                *last_result_text = s.to_string();
            }
        }
        "assistant" => {
            // Assistant messages carry both text (final response) and
            // tool_use blocks (progress signals) inside
            // event.message.content[]. The top-level stream-json event type
            // is "assistant" — tool_use never appears as a top-level event
            // in practice, it's always nested in a block. We handle both
            // defensively (see the top-level "tool_use" branch below).
            let Some(message) = event.get("message") else {
                return;
            };
            let Some(content) = message.get("content") else {
                return;
            };
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if btype == "text" {
                        if let Some(s) = block.get("text").and_then(|v| v.as_str()) {
                            *last_result_text = s.to_string();
                        }
                    } else if btype == "tool_use" {
                        if let Some(bullet) = tool_use_bullet_value(block) {
                            add_bullet_to_all(
                                &bullet,
                                last_bullet,
                                last_broadcast,
                                sipag_dir,
                                project,
                                features,
                                on_progress,
                            );
                        }
                    }
                }
            } else if let Some(s) = content.as_str() {
                *last_result_text = s.to_string();
            }
        }
        "tool_use" => {
            // Defensive: a future stream-json version might emit tool_use at
            // the top level. Harmless today. See the JS comment at lines
            // 198-202 of dispatch-refine.js.
            if let Some(bullet) = tool_use_bullet_value(event) {
                add_bullet_to_all(
                    &bullet,
                    last_bullet,
                    last_broadcast,
                    sipag_dir,
                    project,
                    features,
                    on_progress,
                );
            }
        }
        _ => {}
    }
}

/// Append a bullet to every source feature (deduped per feature) and fire the
/// batch-level `on_progress` callback (deduped across the batch). Panics in
/// the callback are swallowed to match the JS `try { ... } catch { log }`.
fn add_bullet_to_all(
    text: &str,
    last_bullet: &mut HashMap<String, String>,
    last_broadcast: &mut Option<String>,
    sipag_dir: &Path,
    project: &str,
    features: &[Feature],
    on_progress: &mut Option<ProgressCb>,
) {
    for f in features {
        if last_bullet.get(&f.id).map(String::as_str) == Some(text) {
            continue;
        }
        last_bullet.insert(f.id.clone(), text.to_string());
        if let Err(e) = Feature::add_log(sipag_dir, project, &f.id, text) {
            log::warn!("failed to add log to feature {}: {e}", f.id);
        }
    }

    if last_broadcast.as_deref() == Some(text) {
        return;
    }
    *last_broadcast = Some(text.to_string());

    if let Some(cb) = on_progress.as_mut() {
        // Swallow panics from the listener — a bad callback must not kill
        // the refine. This mirrors the JS `try { opts.onProgress(text); }
        // catch (err) { log.warn(...) }`.
        //
        // `catch_unwind` requires `UnwindSafe`; `FnMut` is not, so we wrap
        // the closure in `AssertUnwindSafe`. This is sound here because the
        // only mutable state touched by the callback is its own captures,
        // which the caller has opted to expose via `Box<dyn FnMut + Send>`.
        let text_owned = text.to_string();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cb(&text_owned);
        }));
        if res.is_err() {
            log::warn!("on_progress listener panicked");
        }
    }
}

/// Revert all source features to `raw` and clear their `grouped_into`. Called
/// on any failure path after features may have been pre-marked grouped.
fn revert_all_to_raw(sipag_dir: &Path, project: &str, features: &[Feature]) {
    for f in features {
        if let Err(e) = Feature::update(sipag_dir, project, &f.id, |src| {
            src.status = "raw".to_string();
            src.grouped_into = None;
        }) {
            log::warn!("failed to revert feature {} to raw: {e}", f.id);
        }
    }
}

// ── Pure helpers (exposed for testing) ───────────────────────────────────────

/// Translate a stream-json tool_use block into a short human-readable bullet,
/// or `None` to skip the event. Handles both input shapes:
///
///   * `{ "tool": "Bash", "tool_input": { ... } }` — the `type: "tool_use"`
///     top-level shape
///   * `{ "name": "Bash", "input": { ... } }` — the shape nested inside
///     `event.message.content[]`
pub fn tool_use_bullet(tool: &str, input: &JsonValue) -> Option<String> {
    if tool == "Bash" {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if cmd.contains("diwa search") {
            return Some("Searching diwa history".to_string());
        }
        if cmd.contains("diwa ls") {
            return Some("Listing projects".to_string());
        }
        if cmd.contains("npm test") || cmd.contains("node test") {
            return Some("Running tests".to_string());
        }
        // skip noisy git ops and everything else
        return None;
    }
    if tool == "Read" {
        let fp = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(stripped) = fp.strip_suffix("CLAUDE.md") {
            // The project is the directory that contains CLAUDE.md.
            let trimmed = stripped.trim_end_matches('/');
            let project = trimmed
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("project");
            return Some(format!("Reading {project} CLAUDE.md"));
        }
        let base = fp
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("file");
        return Some(format!("Reading {base}"));
    }
    if tool == "Grep" || tool == "Glob" {
        return Some("Searching codebase".to_string());
    }
    None
}

/// Pull the tool name and input object out of a tool_use block and delegate
/// to `tool_use_bullet`. Accepts either `{tool, tool_input}` or
/// `{name, input}`.
fn tool_use_bullet_value(block: &JsonValue) -> Option<String> {
    let tool = block
        .get("tool")
        .and_then(|v| v.as_str())
        .or_else(|| block.get("name").and_then(|v| v.as_str()))
        .unwrap_or("");
    let default_input = JsonValue::Object(serde_json::Map::new());
    let input = block
        .get("tool_input")
        .or_else(|| block.get("input"))
        .unwrap_or(&default_input);
    tool_use_bullet(tool, input)
}

/// Build the triage+refine prompt for a numbered list of raw ideas. The text
/// is carried verbatim from the JS reference (the wording is load-bearing —
/// the diwa and CLAUDE.md instructions condition the model's behavior).
pub fn build_batch_prompt(features: &[&Feature]) -> String {
    let mut numbered = String::new();
    for (i, f) in features.iter().enumerate() {
        if i > 0 {
            numbered.push('\n');
        }
        numbered.push_str(&format!("{}. [{}] {}", i + 1, f.id, f.body));
    }

    format!(
        r#"You are a feature triage and refinement engine. You will be given a numbered list of raw feature ideas, each tagged with a [f-xxx] ID.

Your job:
1. Run `diwa ls` to discover available projects.
2. For every project that might be affected by any idea, read its CLAUDE.md and run `diwa search <project> "<idea>"` for context.
3. Triage the ideas:
   - Consolidate duplicates (reference all source IDs).
   - Split cross-project ideas into separate tickets.
   - Flag vague ones as "needs-info".
4. Emit a JSON array as your FINAL response — nothing else. Each element:
   {{
     "title": "short imperative title (under 60 chars)",
     "spec": "detailed specification of what to build",
     "project": "target project name from diwa ls",
     "sourceIds": ["f-xxx", ...],
     "status": "refined" or "needs-info",
     "subtasks": [{{ "id": "st-1", "description": "...", "worktree": true }}],
     "estimatedAgents": <number>,
     "needsInfoReason": "only if status is needs-info"
   }}

Every source ID from the input MUST appear in exactly one ticket's sourceIds.

Raw ideas:
{numbered}

IMPORTANT: Your final response must be ONLY the JSON array. No markdown fences, no explanation, just the array."#
    )
}

/// Parse the final result text from claude. Strips ```json fences, skips any
/// leading prose before the first `[`, validates the JSON is an array, and
/// checks every element has the required `title`, `spec`, `sourceIds` fields.
pub fn parse_result(text: &str) -> Result<Vec<RefinedTicket>, String> {
    let mut json = text.trim().to_string();

    // Strip ```json ... ``` or ``` ... ``` fences.
    if let Some(inner) = strip_fences(&json) {
        json = inner;
    }

    // Sometimes the output has leading text before the array.
    if let Some(start) = json.find('[') {
        if start > 0 {
            json = json[start..].to_string();
        }
    }

    let value: JsonValue = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let arr = value
        .as_array()
        .ok_or_else(|| "expected JSON array".to_string())?;

    let mut tickets = Vec::with_capacity(arr.len());
    for item in arr {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Invalid ticket structure: missing title, spec, or sourceIds".to_string()
            })?
            .to_string();
        let spec = item
            .get("spec")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Invalid ticket structure: missing title, spec, or sourceIds".to_string()
            })?
            .to_string();
        let source_ids_val = item.get("sourceIds").ok_or_else(|| {
            "Invalid ticket structure: missing title, spec, or sourceIds".to_string()
        })?;
        let source_ids_arr = source_ids_val.as_array().ok_or_else(|| {
            "Invalid ticket structure: missing title, spec, or sourceIds".to_string()
        })?;
        let source_ids: Vec<String> = source_ids_arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        let project = item
            .get("project")
            .and_then(|v| v.as_str())
            .map(String::from);
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("refined")
            .to_string();
        let estimated_agents = item
            .get("estimatedAgents")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;
        let needs_info_reason = item
            .get("needsInfoReason")
            .and_then(|v| v.as_str())
            .map(String::from);

        let subtasks: Vec<Subtask> = item
            .get("subtasks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|st| Subtask {
                        id: st
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        description: st
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        worktree: st
                            .get("worktree")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();

        tickets.push(RefinedTicket {
            title,
            spec,
            project,
            source_ids,
            status,
            subtasks,
            estimated_agents,
            needs_info_reason,
        });
    }

    Ok(tickets)
}

/// If `text` is wrapped in a ```json ... ``` (or plain ```) fence, return the
/// inner content. Otherwise return None.
fn strip_fences(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_open = &text[start + 3..];
    // Skip an optional "json" language tag and any whitespace.
    let after_lang = after_open.trim_start_matches("json").trim_start();
    let close = after_lang.find("```")?;
    Some(after_lang[..close].trim().to_string())
}

fn ticket_to_refined_json(ticket: &RefinedTicket) -> JsonValue {
    let subtasks: Vec<JsonValue> = ticket
        .subtasks
        .iter()
        .map(|st| {
            serde_json::json!({
                "id": st.id,
                "description": st.description,
                "worktree": st.worktree,
            })
        })
        .collect();
    serde_json::json!({
        "title": ticket.title,
        "spec": ticket.spec,
        "subtasks": subtasks,
        "estimatedAgents": ticket.estimated_agents,
        "needsInfoReason": ticket.needs_info_reason,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::{NamedTempFile, TempDir};

    // ── Pure function tests ──────────────────────────────────────────────────

    fn bash(command: &str) -> JsonValue {
        serde_json::json!({ "command": command })
    }

    #[test]
    fn tool_use_bullet_bash_diwa_ls() {
        let res = tool_use_bullet("Bash", &bash("diwa ls"));
        assert_eq!(res.as_deref(), Some("Listing projects"));
    }

    #[test]
    fn tool_use_bullet_bash_diwa_search() {
        let res = tool_use_bullet("Bash", &bash("diwa search katulong \"vim\""));
        assert_eq!(res.as_deref(), Some("Searching diwa history"));
    }

    #[test]
    fn tool_use_bullet_read_claude_md() {
        let input = serde_json::json!({ "file_path": "/work/katulong/CLAUDE.md" });
        let res = tool_use_bullet("Read", &input);
        assert_eq!(res.as_deref(), Some("Reading katulong CLAUDE.md"));
    }

    #[test]
    fn tool_use_bullet_read_generic() {
        let input = serde_json::json!({ "file_path": "/foo/bar.js" });
        let res = tool_use_bullet("Read", &input);
        assert_eq!(res.as_deref(), Some("Reading bar.js"));
    }

    #[test]
    fn tool_use_bullet_grep_and_glob() {
        let empty = JsonValue::Object(Default::default());
        assert_eq!(
            tool_use_bullet("Grep", &empty).as_deref(),
            Some("Searching codebase")
        );
        assert_eq!(
            tool_use_bullet("Glob", &empty).as_deref(),
            Some("Searching codebase")
        );
    }

    #[test]
    fn tool_use_bullet_unknown_tool_returns_none() {
        let empty = JsonValue::Object(Default::default());
        assert!(tool_use_bullet("Unknown", &empty).is_none());
    }

    #[test]
    fn tool_use_bullet_generic_bash_returns_none() {
        assert!(tool_use_bullet("Bash", &bash("ls -la")).is_none());
        assert!(tool_use_bullet("Bash", &bash("git status")).is_none());
    }

    #[test]
    fn tool_use_bullet_both_input_shapes() {
        // Shape 1: top-level {tool, tool_input}
        let block1 = serde_json::json!({
            "type": "tool_use",
            "tool": "Bash",
            "tool_input": { "command": "diwa ls" }
        });
        assert_eq!(
            tool_use_bullet_value(&block1).as_deref(),
            Some("Listing projects")
        );

        // Shape 2: content-block {name, input}
        let block2 = serde_json::json!({
            "type": "tool_use",
            "name": "Read",
            "input": { "file_path": "/work/yelo/CLAUDE.md" }
        });
        assert_eq!(
            tool_use_bullet_value(&block2).as_deref(),
            Some("Reading yelo CLAUDE.md")
        );
    }

    #[test]
    fn build_batch_prompt_includes_all_feature_ids() {
        // Build two features manually so we don't need disk.
        let f1 = Feature {
            id: "f-aaa".to_string(),
            status: "raw".to_string(),
            projects: None,
            project: None,
            created: String::new(),
            updated: String::new(),
            body: "add vim bindings".to_string(),
            grouped_into: None,
            source_ids: None,
            refined: None,
        };
        let f2 = Feature {
            id: "f-bbb".to_string(),
            status: "raw".to_string(),
            projects: None,
            project: None,
            created: String::new(),
            updated: String::new(),
            body: "dark mode".to_string(),
            grouped_into: None,
            source_ids: None,
            refined: None,
        };
        let prompt = build_batch_prompt(&[&f1, &f2]);
        assert!(prompt.contains("[f-aaa]"));
        assert!(prompt.contains("[f-bbb]"));
        assert!(prompt.contains("1. [f-aaa]"));
        assert!(prompt.contains("2. [f-bbb]"));
    }

    #[test]
    fn build_batch_prompt_includes_diwa_instructions() {
        let f = Feature {
            id: "f-x".to_string(),
            status: "raw".to_string(),
            projects: None,
            project: None,
            created: String::new(),
            updated: String::new(),
            body: "test".to_string(),
            grouped_into: None,
            source_ids: None,
            refined: None,
        };
        let prompt = build_batch_prompt(&[&f]);
        assert!(prompt.contains("diwa ls"));
        assert!(prompt.contains("diwa search"));
    }

    #[test]
    fn parse_result_raw_array() {
        let input = r#"[{"title":"T","spec":"S","sourceIds":["f-1"]}]"#;
        let tickets = parse_result(input).unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].title, "T");
        assert_eq!(tickets[0].spec, "S");
        assert_eq!(tickets[0].source_ids, vec!["f-1".to_string()]);
    }

    #[test]
    fn parse_result_strips_markdown_fences() {
        let input = "```json\n[{\"title\":\"T\",\"spec\":\"S\",\"sourceIds\":[\"f-1\"]}]\n```";
        let tickets = parse_result(input).unwrap();
        assert_eq!(tickets[0].title, "T");
    }

    #[test]
    fn parse_result_strips_plain_fences() {
        let input = "```\n[{\"title\":\"T\",\"spec\":\"S\",\"sourceIds\":[\"f-1\"]}]\n```";
        let tickets = parse_result(input).unwrap();
        assert_eq!(tickets[0].title, "T");
    }

    #[test]
    fn parse_result_handles_leading_text() {
        let input =
            "Here is the result:\n[{\"title\":\"T\",\"spec\":\"S\",\"sourceIds\":[\"f-1\"]}]";
        let tickets = parse_result(input).unwrap();
        assert_eq!(tickets[0].title, "T");
    }

    #[test]
    fn parse_result_errors_on_invalid_json() {
        assert!(parse_result("not json at all").is_err());
    }

    #[test]
    fn parse_result_errors_on_missing_fields() {
        let input = r#"[{"title":"T"}]"#;
        assert!(parse_result(input).is_err());
    }

    // ── Integration tests with a fake claude script ──────────────────────────

    /// Write a shell script that emits `events` as one JSON object per line on
    /// stdout, then exits with `exit_code`. The returned handle must live
    /// longer than any `Refiner::refine_batch` call that uses it (the temp
    /// file is deleted on drop).
    ///
    /// We use a separate payload file (written alongside the script) instead
    /// of a heredoc to avoid shell quoting pitfalls — the stream-json fixtures
    /// contain arbitrary strings, and a heredoc with a fixed sentinel could
    /// collide with payload bytes.
    fn fake_claude_script(events: &[JsonValue], exit_code: i32) -> (NamedTempFile, NamedTempFile) {
        let mut payload = NamedTempFile::new().unwrap();
        for event in events {
            writeln!(payload, "{}", serde_json::to_string(event).unwrap()).unwrap();
        }
        payload.flush().unwrap();

        let mut script = NamedTempFile::new().unwrap();
        writeln!(
            script,
            "#!/bin/sh\ncat '{}'\nexit {}",
            payload.path().display(),
            exit_code
        )
        .unwrap();
        script.flush().unwrap();
        let mut perms = std::fs::metadata(script.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(script.path(), perms).unwrap();
        (script, payload)
    }

    fn assistant_tool_use(name: &str, input: JsonValue) -> JsonValue {
        serde_json::json!({
            "type": "assistant",
            "message": { "content": [{ "type": "tool_use", "name": name, "input": input }] }
        })
    }

    fn stream_events(tickets_json: &JsonValue) -> Vec<JsonValue> {
        vec![
            assistant_tool_use("Bash", serde_json::json!({ "command": "diwa ls" })),
            assistant_tool_use(
                "Bash",
                serde_json::json!({ "command": "diwa search katulong \"x\"" }),
            ),
            assistant_tool_use(
                "Read",
                serde_json::json!({ "file_path": "/work/katulong/CLAUDE.md" }),
            ),
            assistant_tool_use("Grep", serde_json::json!({ "pattern": "foo" })),
            serde_json::json!({ "type": "result", "result": tickets_json.to_string() }),
        ]
    }

    const PROJECT: &str = "demo";

    #[test]
    fn refine_batch_consolidates_and_marks_sources_grouped() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "add vim keybindings", None).unwrap();
        let f2 = Feature::add(dir.path(), PROJECT, "dark mode", None).unwrap();

        let tickets = serde_json::json!([
            {
                "title": "Key bindings",
                "spec": "vim-style bindings",
                "project": "katulong",
                "sourceIds": [f1.id],
                "status": "refined",
                "subtasks": [{"id": "st-1", "description": "parser", "worktree": true}],
                "estimatedAgents": 1
            },
            {
                "title": "Dark mode",
                "spec": "theme toggle",
                "project": "yelo",
                "sourceIds": [f2.id],
                "status": "refined",
                "subtasks": [],
                "estimatedAgents": 1
            }
        ]);

        let (script, _payload) = fake_claude_script(&stream_events(&tickets), 0);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            ..Default::default()
        };

        let created = Refiner::new()
            .refine_batch(
                dir.path(),
                PROJECT,
                &[f1.id.clone(), f2.id.clone()],
                &mut opts,
            )
            .expect("refine_batch should succeed");

        assert_eq!(created.len(), 2);

        let updated1 = Feature::get(dir.path(), PROJECT, &f1.id).unwrap().unwrap();
        let updated2 = Feature::get(dir.path(), PROJECT, &f2.id).unwrap().unwrap();
        assert_eq!(updated1.status, "grouped");
        assert_eq!(updated2.status, "grouped");
        assert!(updated1.grouped_into.is_some());
        assert_eq!(updated1.grouped_into, updated2.grouped_into);

        let for_katulong = created
            .iter()
            .find(|f| f.project.as_deref() == Some("katulong"))
            .unwrap();
        let for_yelo = created
            .iter()
            .find(|f| f.project.as_deref() == Some("yelo"))
            .unwrap();
        assert_eq!(
            for_katulong.source_ids.as_deref(),
            Some(&[f1.id.clone()][..])
        );
        assert_eq!(for_yelo.source_ids.as_deref(), Some(&[f2.id.clone()][..]));

        // Refined features carry the "title\n\nspec" body and status refined.
        assert_eq!(for_katulong.status, "refined");
        assert!(for_katulong.body.starts_with("Key bindings\n\n"));
    }

    #[test]
    fn refine_batch_appends_progress_bullets() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "feature one", None).unwrap();

        let tickets = serde_json::json!([
            {
                "title": "T",
                "spec": "S",
                "project": "p",
                "sourceIds": [f1.id],
                "status": "refined",
                "subtasks": [],
                "estimatedAgents": 1
            }
        ]);

        let (script, _payload) = fake_claude_script(&stream_events(&tickets), 0);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            ..Default::default()
        };

        Refiner::new()
            .refine_batch(dir.path(), PROJECT, std::slice::from_ref(&f1.id), &mut opts)
            .unwrap();

        // After refinement the source feature is `grouped` — the log lines
        // were appended to its body before it was marked grouped.
        let updated = Feature::get(dir.path(), PROJECT, &f1.id).unwrap().unwrap();
        assert!(
            updated.body.contains("Listing projects"),
            "body: {}",
            updated.body
        );
        assert!(updated.body.contains("Searching diwa history"));
        assert!(updated.body.contains("Reading katulong CLAUDE.md"));
        assert!(updated.body.contains("Searching codebase"));
    }

    #[test]
    fn refine_batch_deduplicates_consecutive_identical_bullets() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "test dedupe", None).unwrap();

        let tickets = serde_json::json!([
            {
                "title": "T",
                "spec": "S",
                "project": "p",
                "sourceIds": [f1.id],
                "status": "refined",
                "subtasks": [],
                "estimatedAgents": 1
            }
        ]);

        let events = vec![
            assistant_tool_use("Grep", serde_json::json!({})),
            assistant_tool_use("Grep", serde_json::json!({})),
            assistant_tool_use("Grep", serde_json::json!({})),
            serde_json::json!({ "type": "result", "result": tickets.to_string() }),
        ];

        let (script, _payload) = fake_claude_script(&events, 0);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            ..Default::default()
        };

        Refiner::new()
            .refine_batch(dir.path(), PROJECT, std::slice::from_ref(&f1.id), &mut opts)
            .unwrap();

        let updated = Feature::get(dir.path(), PROJECT, &f1.id).unwrap().unwrap();
        let count = updated.body.matches("Searching codebase").count();
        assert_eq!(
            count, 1,
            "expected exactly 1 dedupe bullet, got body:\n{}",
            updated.body
        );
    }

    #[test]
    fn refine_batch_invokes_on_progress_deduped_batch_wide() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "one", None).unwrap();
        let f2 = Feature::add(dir.path(), PROJECT, "two", None).unwrap();

        let tickets = serde_json::json!([
            {
                "title": "T1",
                "spec": "S1",
                "project": "p",
                "sourceIds": [f1.id],
                "status": "refined",
                "subtasks": [],
                "estimatedAgents": 1
            },
            {
                "title": "T2",
                "spec": "S2",
                "project": "p",
                "sourceIds": [f2.id],
                "status": "refined",
                "subtasks": [],
                "estimatedAgents": 1
            }
        ]);

        let events = vec![
            assistant_tool_use("Bash", serde_json::json!({ "command": "diwa ls" })),
            assistant_tool_use("Bash", serde_json::json!({ "command": "diwa ls" })), // dupe
            assistant_tool_use(
                "Bash",
                serde_json::json!({ "command": "diwa search katulong \"x\"" }),
            ),
            assistant_tool_use(
                "Read",
                serde_json::json!({ "file_path": "/work/katulong/CLAUDE.md" }),
            ),
            assistant_tool_use("Grep", serde_json::json!({ "pattern": "foo" })),
            serde_json::json!({ "type": "result", "result": tickets.to_string() }),
        ];

        let (script, _payload) = fake_claude_script(&events, 0);
        // Collect received bullets via a channel — the callback is FnMut +
        // Send, so a Mutex is the simplest thread-safe accumulator.
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_clone = std::sync::Arc::clone(&received);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            on_progress: Some(Box::new(move |b| {
                received_clone.lock().unwrap().push(b.to_string());
            })),
            ..Default::default()
        };

        Refiner::new()
            .refine_batch(
                dir.path(),
                PROJECT,
                &[f1.id.clone(), f2.id.clone()],
                &mut opts,
            )
            .unwrap();

        let got = received.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![
                "Listing projects".to_string(),
                "Searching diwa history".to_string(),
                "Reading katulong CLAUDE.md".to_string(),
                "Searching codebase".to_string(),
            ]
        );
    }

    #[test]
    fn refine_batch_reverts_features_to_raw_on_nonzero_exit() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "will fail", None).unwrap();
        let f2 = Feature::add(dir.path(), PROJECT, "also fails", None).unwrap();

        // Empty stream + non-zero exit: no result text, no tool_use events.
        let (script, _payload) = fake_claude_script(&[], 1);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            ..Default::default()
        };

        let err = Refiner::new()
            .refine_batch(
                dir.path(),
                PROJECT,
                &[f1.id.clone(), f2.id.clone()],
                &mut opts,
            )
            .expect_err("should fail");
        assert_eq!(err.public, "Refinement failed");
        assert!(
            err.detail.contains("claude exited with code 1"),
            "detail: {}",
            err.detail
        );

        assert_eq!(
            Feature::get(dir.path(), PROJECT, &f1.id)
                .unwrap()
                .unwrap()
                .status,
            "raw"
        );
        assert_eq!(
            Feature::get(dir.path(), PROJECT, &f2.id)
                .unwrap()
                .unwrap()
                .status,
            "raw"
        );
    }

    #[test]
    fn refine_batch_reverts_on_invalid_json_result() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "bad json", None).unwrap();

        let events = vec![serde_json::json!({
            "type": "result",
            "result": "this is not valid json [[["
        })];
        let (script, _payload) = fake_claude_script(&events, 0);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            ..Default::default()
        };

        let err = Refiner::new()
            .refine_batch(dir.path(), PROJECT, std::slice::from_ref(&f1.id), &mut opts)
            .expect_err("should fail");
        assert_eq!(err.public, "Refinement failed");
        assert!(
            err.detail.contains("failed to parse result"),
            "detail: {}",
            err.detail
        );

        assert_eq!(
            Feature::get(dir.path(), PROJECT, &f1.id)
                .unwrap()
                .unwrap()
                .status,
            "raw"
        );
    }

    #[test]
    fn refine_batch_errors_when_no_valid_features() {
        let dir = TempDir::new().unwrap();
        let mut opts = RefineOptions::default();
        let err = Refiner::new()
            .refine_batch(
                dir.path(),
                PROJECT,
                &["f-nonexistent".to_string()],
                &mut opts,
            )
            .expect_err("should fail");
        assert_eq!(err.public, "no valid features");
        assert!(err.detail.contains("No valid features"));
    }

    #[test]
    fn refine_batch_handles_needs_info_status() {
        let dir = TempDir::new().unwrap();
        let f1 = Feature::add(dir.path(), PROJECT, "vague idea", None).unwrap();

        let tickets = serde_json::json!([
            {
                "title": "Unclear",
                "spec": "need more info",
                "project": "katulong",
                "sourceIds": [f1.id],
                "status": "needs-info",
                "needsInfoReason": "too vague",
                "subtasks": [],
                "estimatedAgents": 0
            }
        ]);
        let (script, _payload) = fake_claude_script(&stream_events(&tickets), 0);
        let mut opts = RefineOptions {
            claude_bin: Some(script.path().to_path_buf()),
            ..Default::default()
        };

        let created = Refiner::new()
            .refine_batch(dir.path(), PROJECT, std::slice::from_ref(&f1.id), &mut opts)
            .unwrap();

        assert_eq!(created.len(), 1);
        assert_eq!(created[0].status, "needs-info");
    }
}
