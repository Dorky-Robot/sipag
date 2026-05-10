//! HTTP client for katulong's session API.
//!
//! Uses `curl` via `std::process::Command` for HTTP requests, consistent
//! with how sipag already calls `docker` and `gh`.
//!
//! Session I/O routes are id-keyed (`/sessions/by-id/{id}/...`) — names
//! can be renamed, ids can't, so an in-flight request never gets
//! invalidated by a rename. Callers create-or-find a session by name
//! once via [`KatulongClient::create_session`], capture the returned
//! [`Session::id`], then pass that id to subsequent operations.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// A katulong session. `id` is the stable, immutable handle used for
/// all I/O calls; `name` is the friendly identifier (e.g. `katulong--dev`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
}

impl Session {
    /// Defense-in-depth check that the server-supplied `id` is safe
    /// to interpolate into a URL path segment. Call after
    /// deserializing a `Session` from a katulong response, before
    /// the id flows into [`exec_url`] / [`status_url`] / [`kill_url`]
    /// / [`output_lines_url`].
    ///
    /// Without this, a compromised or misbehaving katulong returning
    /// a crafted id (`../admin`, `foo?inject=1`, `s_valid/../../other`)
    /// would steer sipag's subsequent requests at unintended endpoints
    /// on the same host. Blast radius is bounded to that katulong
    /// instance, but cheap to defend against.
    pub fn validate_id(&self) -> Result<()> {
        if !is_valid_session_id(&self.id) {
            anyhow::bail!("invalid session id format from katulong: {:?}", self.id);
        }
        Ok(())
    }
}

/// Check whether a session id matches katulong's nanoid-shaped
/// format: ASCII alphanumeric + `_` / `-`, 1-64 chars. Rejects
/// path-traversal, query-string, and shell-injection metacharacters
/// before they reach a URL builder. See [`Session::validate_id`].
pub fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Status returned by `GET /sessions/by-id/{id}/status`. Only the fields
/// sipag currently consumes are mapped; katulong returns more (pane,
/// agent, childCount) and serde silently drops them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionStatus {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub alive: bool,
    #[serde(default, rename = "hasChildProcesses")]
    pub has_child_processes: bool,
}

/// Remote connection config from `~/.katulong/remote.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
}

impl RemoteConfig {
    /// Load from `~/.katulong/remote.json`.
    pub fn load() -> Result<Self> {
        let home = std::env::var("HOME").context("HOME not set")?;
        let path = std::path::PathBuf::from(home)
            .join(".katulong")
            .join("remote.json");
        Self::load_from(&path)
    }

    /// Load from a specific path. Rejects empty `url` / `apiKey` so a
    /// misconfigured file fails fast with a clear message instead of
    /// surfacing as a confusing 401 / DNS error at request time.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in {}", path.display()))?;
        if config.url.is_empty() {
            anyhow::bail!("'url' is empty in {}", path.display());
        }
        if config.api_key.is_empty() {
            anyhow::bail!("'apiKey' is empty in {}", path.display());
        }
        Ok(config)
    }

    /// SSE subscribe URL for a katulong pub/sub topic. `/` in the
    /// topic is `%2F`-encoded so the topic stays one path segment;
    /// katulong's broker uses slashes inside topic names (e.g.
    /// `crew/<project>/<role>/...`).
    pub fn sub_url(&self, topic: &str, from_seq: u64) -> String {
        let encoded_topic = topic.replace('/', "%2F");
        format!(
            "{}/sub/{encoded_topic}?fromSeq={from_seq}",
            self.url.trim_end_matches('/')
        )
    }
}

/// HTTP client for the katulong session API.
pub struct KatulongClient {
    url: String,
    api_key: String,
}

impl KatulongClient {
    /// Load connection details from `~/.katulong/remote.json`.
    pub fn from_remote_json() -> Result<Self> {
        let config = RemoteConfig::load()?;
        Ok(Self::new(config.url, config.api_key))
    }

    /// Create a client from explicit URL and API key.
    pub fn new(url: String, api_key: String) -> Self {
        // Normalize: strip trailing slash from URL.
        let url = url.trim_end_matches('/').to_string();
        Self { url, api_key }
    }

    /// `POST /sessions` — create a session, or return the existing one
    /// with the same name. The katulong server returns 201 with
    /// `{name, id}` on create and 409 with `{error}` on conflict; on
    /// conflict this method falls back to `list_sessions` to recover
    /// the existing id, so the call is idempotent.
    pub fn create_session(&self, name: &str) -> Result<Session> {
        let body = serde_json::json!({ "name": name });
        let url = sessions_url(&self.url);
        let resp = curl_post(&url, &self.api_key, &body.to_string())?;

        let session: Session = match resp.status {
            200 | 201 => serde_json::from_str(&resp.body)
                .with_context(|| format!("invalid create response for '{name}': {}", resp.body))?,
            409 => self
                .list_sessions()?
                .into_iter()
                .find(|s| s.name == name)
                .with_context(|| {
                    format!("session '{name}' returned 409 but list lookup missed it")
                })?,
            code => anyhow::bail!(
                "create session '{name}' returned HTTP {code}: {}",
                resp.body.trim()
            ),
        };
        session.validate_id()?;
        Ok(session)
    }

    /// `GET /sessions` — list all sessions.
    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let url = sessions_url(&self.url);
        let resp = curl_get(&url, &self.api_key)?;
        if resp.status != 200 {
            anyhow::bail!(
                "list sessions returned HTTP {}: {}",
                resp.status,
                resp.body.trim()
            );
        }
        serde_json::from_str(&resp.body)
            .with_context(|| format!("invalid JSON from sessions list: {}", resp.body))
    }

    /// `POST /sessions/by-id/{id}/exec` — send a command. The katulong
    /// server appends `\r` to the input, so this is for line-oriented
    /// commands. Caller must pass the session's stable `id`, not its
    /// friendly name (see [`Session`]).
    pub fn exec_session(&self, id: &str, input: &str) -> Result<()> {
        let body = serde_json::json!({ "input": input });
        let url = exec_url(&self.url, id);
        let resp = curl_post(&url, &self.api_key, &body.to_string())?;
        if !is_success(resp.status) {
            anyhow::bail!(
                "exec in session '{id}' returned HTTP {}: {}",
                resp.status,
                resp.body.trim()
            );
        }
        Ok(())
    }

    /// `GET /sessions/by-id/{id}/status`.
    pub fn session_status(&self, id: &str) -> Result<SessionStatus> {
        let url = status_url(&self.url, id);
        let resp = curl_get(&url, &self.api_key)?;
        if resp.status != 200 {
            anyhow::bail!(
                "status for '{id}' returned HTTP {}: {}",
                resp.status,
                resp.body.trim()
            );
        }
        serde_json::from_str(&resp.body)
            .with_context(|| format!("invalid status JSON for '{id}': {}", resp.body))
    }

    /// `DELETE /sessions/by-id/{id}` — kill a session.
    pub fn kill_session(&self, id: &str) -> Result<()> {
        let url = kill_url(&self.url, id);
        let resp = curl_delete(&url, &self.api_key)?;
        if !is_success(resp.status) {
            anyhow::bail!(
                "kill session '{id}' returned HTTP {}: {}",
                resp.status,
                resp.body.trim()
            );
        }
        Ok(())
    }

    /// Return the base URL (for display/diagnostics).
    pub fn url(&self) -> &str {
        &self.url
    }
}

// ── HTTP plumbing ───────────────────────────────────────────────────────────

/// Parsed response from a curl invocation.
struct HttpResponse {
    status: u16,
    body: String,
}

fn is_success(code: u16) -> bool {
    (200..300).contains(&code)
}

/// Run curl and parse `<body>\n<status>` (the trailing status code is
/// emitted via `-w "\n%{http_code}"`). Returns an error only if curl
/// itself fails to run; HTTP-level errors come back as a non-2xx
/// `status` field for the caller to interpret.
fn run_curl(args: &[&str]) -> Result<HttpResponse> {
    let output = Command::new("curl")
        .args(args)
        .output()
        .context("failed to run curl")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("curl failed: {}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let (body, status_str) = stdout
        .rsplit_once('\n')
        .context("malformed curl output: missing trailing status code")?;
    let status: u16 = status_str
        .trim()
        .parse()
        .with_context(|| format!("invalid status code from curl: '{status_str}'"))?;
    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}

fn curl_post(url: &str, api_key: &str, body: &str) -> Result<HttpResponse> {
    let auth = format!("Authorization: Bearer {api_key}");
    run_curl(&[
        "-s",
        "-w",
        "\n%{http_code}",
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "-H",
        &auth,
        "-d",
        body,
        url,
    ])
}

fn curl_get(url: &str, api_key: &str) -> Result<HttpResponse> {
    let auth = format!("Authorization: Bearer {api_key}");
    run_curl(&["-s", "-w", "\n%{http_code}", "-H", &auth, url])
}

fn curl_delete(url: &str, api_key: &str) -> Result<HttpResponse> {
    let auth = format!("Authorization: Bearer {api_key}");
    run_curl(&[
        "-s",
        "-w",
        "\n%{http_code}",
        "-X",
        "DELETE",
        "-H",
        &auth,
        url,
    ])
}

// ── URL builders ────────────────────────────────────────────────────────────
//
// Pure URL construction. Both `KatulongClient` (curl-based, sync) and
// the async helpers in `sipag/src/serve/` use these so wire-format
// knowledge stays in one place. Callers pass a base URL with no
// trailing slash (`KatulongClient::new` and `Host::base_url` both
// already strip it).

/// `POST /sessions` to create, or `GET /sessions` to list.
pub fn sessions_url(base: &str) -> String {
    format!("{base}/sessions")
}

/// `POST /sessions/by-id/{id}/exec` — line-oriented input (server
/// appends `\r`).
pub fn exec_url(base: &str, session_id: &str) -> String {
    format!("{base}/sessions/by-id/{session_id}/exec")
}

/// `GET /sessions/by-id/{id}/status`.
pub fn status_url(base: &str, session_id: &str) -> String {
    format!("{base}/sessions/by-id/{session_id}/status")
}

/// `GET /sessions/by-id/{id}/output?lines=N` — pull the last N lines
/// of the visible pane (capture-pane equivalent). Distinct from the
/// `?fromSeq=` cursor-based mode and the `?screen=true` snapshot mode.
pub fn output_lines_url(base: &str, session_id: &str, lines: u32) -> String {
    format!("{base}/sessions/by-id/{session_id}/output?lines={lines}")
}

/// `DELETE /sessions/by-id/{id}`.
pub fn kill_url(base: &str, session_id: &str) -> String {
    format!("{base}/sessions/by-id/{session_id}")
}

/// `GET /api/claude-transcript/{uuid}?limit=N` — fetch a Claude
/// session's transcript JSONL up to `limit` recent entries.
pub fn claude_transcript_url(base: &str, uuid: &str, limit: u32) -> String {
    format!("{base}/api/claude-transcript/{uuid}?limit={limit}")
}

/// `POST /api/claude/respond/{uuid}` — type a message into a running
/// Claude TUI as if from the user's keyboard.
pub fn claude_respond_url(base: &str, uuid: &str) -> String {
    format!("{base}/api/claude/respond/{uuid}")
}

// ── Dispatch logic ──────────────────────────────────────────────────────────

/// Session naming convention: `{project}--{role}`.
pub fn session_name(project: &str, role: &str) -> String {
    format!("{project}--{role}")
}

/// Generate the worktree path for a task within a project container.
pub fn worktree_path(project: &str, task_id: u64) -> String {
    format!("/work/{project}/.worktrees/task-{task_id}")
}

/// Generate the worktree branch name for a task.
pub fn worktree_branch(task_id: u64) -> String {
    format!("fix/task-{task_id}")
}

/// Generate the git worktree add command.
pub fn worktree_command(project: &str, task_id: u64) -> String {
    let branch = worktree_branch(task_id);
    format!("cd /work/{project} && git worktree add .worktrees/task-{task_id} -b {branch}")
}

/// Generate the agent launch command for a task. The result is sent
/// over `exec_session` to the katulong PTY where it is interpreted by
/// the user's shell, so the title is single-quote-escaped to prevent
/// task names like `it's broken` (typo) or `'; rm -rf $HOME; '`
/// (malicious) from breaking out of the prompt argument.
pub fn agent_command(
    project: &str,
    task_id: u64,
    title: &str,
    role_command: &str,
    use_worktree: bool,
) -> String {
    let work_dir = if use_worktree {
        worktree_path(project, task_id)
    } else {
        format!("/work/{project}")
    };
    let safe_title = sh_single_quote_escape(title);
    format!("cd {work_dir} && {role_command} -p 'Work on task #{task_id}: {safe_title}'")
}

/// Escape a string so it is safe to embed inside `'...'` in a POSIX
/// shell command. The standard idiom: close the quote, emit an
/// escaped `\'`, reopen the quote. Caller must wrap the result in `'`.
fn sh_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_format() {
        assert_eq!(session_name("katulong", "dev"), "katulong--dev");
        assert_eq!(session_name("kubo", "test"), "kubo--test");
    }

    #[test]
    fn worktree_path_format() {
        assert_eq!(
            worktree_path("katulong", 42),
            "/work/katulong/.worktrees/task-42"
        );
    }

    #[test]
    fn worktree_branch_format() {
        assert_eq!(worktree_branch(42), "fix/task-42");
    }

    #[test]
    fn worktree_command_format() {
        let cmd = worktree_command("katulong", 42);
        assert!(cmd.contains("cd /work/katulong"));
        assert!(cmd.contains("git worktree add .worktrees/task-42 -b fix/task-42"));
    }

    #[test]
    fn agent_command_with_worktree() {
        let cmd = agent_command("katulong", 42, "Fix auth bug", "yolo", true);
        assert!(cmd.contains("cd /work/katulong/.worktrees/task-42"));
        assert!(cmd.contains("yolo -p 'Work on task #42: Fix auth bug'"));
    }

    #[test]
    fn agent_command_without_worktree() {
        let cmd = agent_command("katulong", 42, "Run tests", "yolo", false);
        assert!(cmd.contains("cd /work/katulong &&"));
        assert!(!cmd.contains("worktrees"));
    }

    #[test]
    fn agent_command_escapes_single_quote_in_title() {
        // A naive title like "it's broken" must not close the prompt's
        // single-quoted argument. The standard sh idiom is `'\''`.
        let cmd = agent_command("katulong", 42, "it's broken", "yolo", false);
        assert!(
            cmd.contains(r"'Work on task #42: it'\''s broken'"),
            "expected escaped title in: {cmd}"
        );
    }

    #[test]
    fn agent_command_neutralizes_injection_attempt() {
        // A malicious title with shell metacharacters must not be able
        // to escape the prompt argument and run additional commands.
        let cmd = agent_command("katulong", 9, "'; rm -rf /tmp; '", "yolo", false);
        // The closing `'` of the prompt arg must come AFTER the escaped
        // payload — never inside it.
        let after_prompt = cmd.split("-p '").nth(1).expect("missing prompt arg");
        // The first unescaped `'` must be the very last char (the closer).
        assert!(
            after_prompt.ends_with('\''),
            "prompt argument is not properly closed: {cmd}"
        );
        // No bare `;` should appear between an unescaped `'` pair —
        // simplest check: the escape sequence appears at least twice
        // (once per single-quote in the title).
        assert!(
            cmd.matches(r"'\''").count() >= 2,
            "title not escaped: {cmd}"
        );
    }

    #[test]
    fn sh_single_quote_escape_is_identity_for_safe_strings() {
        assert_eq!(sh_single_quote_escape("Fix auth bug"), "Fix auth bug");
        assert_eq!(sh_single_quote_escape(""), "");
    }

    #[test]
    fn remote_config_load_from_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(
            &path,
            r#"{"url": "https://example.com", "apiKey": "test-key"}"#,
        )
        .unwrap();

        let config = RemoteConfig::load_from(&path).unwrap();
        assert_eq!(config.url, "https://example.com");
        assert_eq!(config.api_key, "test-key");
    }

    #[test]
    fn remote_config_load_from_missing_file() {
        let result = RemoteConfig::load_from(std::path::Path::new("/nonexistent/remote.json"));
        assert!(result.is_err());
    }

    #[test]
    fn remote_config_load_from_rejects_empty_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(&path, r#"{"url": "", "apiKey": "k"}"#).unwrap();
        let err = RemoteConfig::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("'url' is empty"), "got: {err}");
    }

    #[test]
    fn remote_config_load_from_rejects_empty_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(&path, r#"{"url": "https://x", "apiKey": ""}"#).unwrap();
        let err = RemoteConfig::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("'apiKey' is empty"), "got: {err}");
    }

    #[test]
    fn sub_url_encodes_topic_slashes_and_appends_seq() {
        let cfg = RemoteConfig {
            url: "https://k.example".into(),
            api_key: "k".into(),
        };
        assert_eq!(
            cfg.sub_url("crew/proj/role", 7),
            "https://k.example/sub/crew%2Fproj%2Frole?fromSeq=7"
        );
    }

    #[test]
    fn sessions_url_builds_create_and_list_endpoint() {
        assert_eq!(
            sessions_url("https://k.example"),
            "https://k.example/sessions"
        );
    }

    #[test]
    fn exec_url_uses_by_id_path() {
        assert_eq!(
            exec_url("https://k.example", "s_abc"),
            "https://k.example/sessions/by-id/s_abc/exec"
        );
    }

    #[test]
    fn status_url_uses_by_id_path() {
        assert_eq!(
            status_url("https://k.example", "s_abc"),
            "https://k.example/sessions/by-id/s_abc/status"
        );
    }

    #[test]
    fn kill_url_is_session_root_under_by_id() {
        assert_eq!(
            kill_url("https://k.example", "s_abc"),
            "https://k.example/sessions/by-id/s_abc"
        );
    }

    #[test]
    fn output_lines_url_includes_query_param() {
        assert_eq!(
            output_lines_url("https://k.example", "s_abc", 80),
            "https://k.example/sessions/by-id/s_abc/output?lines=80"
        );
    }

    #[test]
    fn claude_transcript_url_includes_uuid_and_limit() {
        assert_eq!(
            claude_transcript_url("https://k.example", "uuid-123", 500),
            "https://k.example/api/claude-transcript/uuid-123?limit=500"
        );
    }

    #[test]
    fn claude_respond_url_uses_uuid() {
        assert_eq!(
            claude_respond_url("https://k.example", "uuid-123"),
            "https://k.example/api/claude/respond/uuid-123"
        );
    }

    #[test]
    fn sub_url_strips_trailing_slash_from_base() {
        let cfg = RemoteConfig {
            url: "https://k.example/".into(),
            api_key: "k".into(),
        };
        assert_eq!(
            cfg.sub_url("topic", 0),
            "https://k.example/sub/topic?fromSeq=0"
        );
    }

    #[test]
    fn client_strips_trailing_slash() {
        let client = KatulongClient::new("https://example.com/".to_string(), "key".to_string());
        assert_eq!(client.url(), "https://example.com");
    }

    #[test]
    fn is_valid_session_id_accepts_observed_nanoid_shapes() {
        // Real ids seen in the e2e probe + tests.
        assert!(is_valid_session_id("Tj9HtvbDQ06zsCbu7dM6-"));
        assert!(is_valid_session_id("ykPzkUAf_vb8_UMnBXpgm"));
        assert!(is_valid_session_id("s_abc123"));
        assert!(is_valid_session_id("d5r4RNxP5Tu8HvXQge682"));
    }

    #[test]
    fn is_valid_session_id_rejects_path_traversal_and_metacharacters() {
        // Any URL-significant or shell-significant char is rejected.
        assert!(!is_valid_session_id("../admin"));
        assert!(!is_valid_session_id("foo?inject=1"));
        assert!(!is_valid_session_id("s_valid/../../../other"));
        assert!(!is_valid_session_id("with space"));
        assert!(!is_valid_session_id("trailing#fragment"));
        assert!(!is_valid_session_id("with.dot"));
        assert!(!is_valid_session_id("with%percent"));
    }

    #[test]
    fn is_valid_session_id_rejects_empty_and_oversized() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id(&"x".repeat(65)));
        assert!(is_valid_session_id(&"x".repeat(64))); // boundary: ok
    }

    #[test]
    fn is_valid_session_id_rejects_non_ascii() {
        // Unicode letters are valid in Rust's `char::is_alphabetic` but
        // not for this validator — we want pure ASCII so the URL path
        // segment is always 1 byte per char.
        assert!(!is_valid_session_id("café"));
        assert!(!is_valid_session_id("emoji😀"));
    }

    #[test]
    fn session_validate_id_returns_err_on_bad_id() {
        let bad = Session {
            id: "../admin".to_string(),
            name: "katulong--dev".to_string(),
        };
        let err = bad.validate_id().unwrap_err().to_string();
        assert!(err.contains("invalid session id"), "got: {err}");
    }

    #[test]
    fn session_validate_id_passes_on_real_id() {
        let ok = Session {
            id: "Tj9HtvbDQ06zsCbu7dM6-".to_string(),
            name: "katulong--dev".to_string(),
        };
        assert!(ok.validate_id().is_ok());
    }

    #[test]
    fn session_deserializes_and_ignores_extra_fields() {
        // katulong's GET /sessions returns the full session.toJSON() payload
        // with id/name/tmuxSession/tmuxPane/alive/etc. — we only need id+name.
        let payload = r#"{
            "id": "s_abc123",
            "name": "katulong--dev",
            "tmuxSession": "katulong--dev",
            "tmuxPane": "%1",
            "alive": true,
            "hasChildProcesses": false,
            "external": false
        }"#;
        let s: Session = serde_json::from_str(payload).unwrap();
        assert_eq!(s.id, "s_abc123");
        assert_eq!(s.name, "katulong--dev");
    }

    #[test]
    fn session_status_deserializes_with_camel_case() {
        // hasChildProcesses → has_child_processes via #[serde(rename)].
        let payload = r#"{
            "id": "s_abc123",
            "name": "katulong--dev",
            "alive": true,
            "hasChildProcesses": true,
            "childCount": 2,
            "pane": null,
            "agent": null
        }"#;
        let st: SessionStatus = serde_json::from_str(payload).unwrap();
        assert_eq!(st.id, "s_abc123");
        assert!(st.alive);
        assert!(st.has_child_processes);
    }

    #[test]
    fn is_success_classifies_2xx() {
        assert!(is_success(200));
        assert!(is_success(201));
        assert!(is_success(204));
        assert!(!is_success(199));
        assert!(!is_success(300));
        assert!(!is_success(404));
        assert!(!is_success(409));
        assert!(!is_success(500));
    }
}
