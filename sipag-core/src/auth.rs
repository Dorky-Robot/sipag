use anyhow::Result;
use std::fs;
use std::path::Path;

/// Resolve the Claude OAuth token.
///
/// Checks `CLAUDE_CODE_OAUTH_TOKEN` env var first, then `sipag_dir/token` file.
/// Returns `None` if no OAuth token is found (does not check `ANTHROPIC_API_KEY`).
pub(crate) fn resolve_token(sipag_dir: &Path) -> Option<String> {
    resolve_token_with_env(sipag_dir, |k| std::env::var(k).ok())
}

/// Testable token resolution — accepts an env-var lookup function.
fn resolve_token_with_env(
    sipag_dir: &Path,
    get_env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(token) = get_env("CLAUDE_CODE_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }
    read_token_file(sipag_dir)
}

/// Read the OAuth token from `sipag_dir/token`, warning if permissions are too open.
///
/// Returns `None` if the file does not exist or is empty.
/// This is the single source of truth for token-file reading — both
/// [`resolve_token`] and `Credentials::resolve_oauth_token` delegate here.
pub(crate) fn read_token_file(sipag_dir: &Path) -> Option<String> {
    let token_file = sipag_dir.join("token");
    if token_file.exists() {
        warn_if_token_world_readable(&token_file);
        if let Ok(contents) = fs::read_to_string(&token_file) {
            let trimmed = contents.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// Emit a warning to stderr if the token file is readable by group or others.
///
/// On shared systems this token grants full Claude API access, so it must be
/// owner-readable only (mode 0600).  We warn rather than bail so that the
/// worker still starts — a misconfigured permission is bad but not fatal.
pub(crate) fn warn_if_token_world_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                eprintln!(
                    "Warning: {} has permissions {:04o} — token is readable by group/others. \
                     Run: chmod 600 {}",
                    path.display(),
                    mode & 0o777,
                    path.display()
                );
            }
        }
    }
}

/// Check that Claude authentication is available.
///
/// Checks `CLAUDE_CODE_OAUTH_TOKEN` env, then `sipag_dir/token` (OAuth), then
/// `ANTHROPIC_API_KEY` as a fallback.
pub fn preflight_auth(sipag_dir: &Path) -> Result<()> {
    if resolve_token(sipag_dir).is_some() {
        return Ok(());
    }
    // ANTHROPIC_API_KEY is a valid fallback (API key auth)
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            eprintln!("Note: Using ANTHROPIC_API_KEY. For OAuth instead, run:");
            eprintln!("  claude setup-token");
            eprintln!("  cp ~/.claude/token {}/token", sipag_dir.display());
            return Ok(());
        }
    }
    anyhow::bail!(
        "Error: No Claude authentication found.\n\n  To fix, run these two commands:\n\n    claude setup-token\n    cp ~/.claude/token {}/token\n\n  The first command opens your browser to authenticate with Anthropic.\n  The second copies the token to where sipag workers can use it.\n\n  Alternative: export ANTHROPIC_API_KEY=sk-ant-... (if you have an API key)",
        sipag_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_token_prefers_env_over_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-token\n").unwrap();

        let token = resolve_token_with_env(dir.path(), |k| match k {
            "CLAUDE_CODE_OAUTH_TOKEN" => Some("env-token".to_string()),
            _ => None,
        });
        assert_eq!(token, Some("env-token".to_string()));
    }

    #[test]
    fn resolve_token_falls_back_to_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-token\n").unwrap();

        let token = resolve_token_with_env(dir.path(), |_| None);
        assert_eq!(token, Some("file-token".to_string()));
    }

    #[test]
    fn resolve_token_skips_empty_env() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-token\n").unwrap();

        let token = resolve_token_with_env(dir.path(), |k| match k {
            "CLAUDE_CODE_OAUTH_TOKEN" => Some(String::new()),
            _ => None,
        });
        assert_eq!(token, Some("file-token".to_string()));
    }

    #[test]
    fn token_file_trims_whitespace() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "  my-token  \n").unwrap();

        let token = resolve_token_with_env(dir.path(), |_| None);
        assert_eq!(token, Some("my-token".to_string()));
    }
}
