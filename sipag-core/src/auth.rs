use anyhow::Result;
use std::fs;
use std::path::Path;

/// Resolve the Claude OAuth token.
///
/// Checks `CLAUDE_CODE_OAUTH_TOKEN` env var first, then `sipag_dir/token` file.
/// Returns `None` if no OAuth token is found (does not check `ANTHROPIC_API_KEY`).
pub(crate) fn resolve_token(sipag_dir: &Path) -> Option<String> {
    if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }
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
