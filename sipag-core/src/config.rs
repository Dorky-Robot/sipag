//! Runtime configuration for sipag.
//!
//! v4 is filesystem-driven: the only thing this module owns is the path to
//! the sipag state directory. Everything else (project config, board config)
//! lives under `~/.sipag/` as TOML files managed by the `board` module.

use std::env;
use std::path::PathBuf;

/// Return the default sipag directory (`~/.sipag`).
///
/// Resolution: `SIPAG_DIR` env var > `$HOME/.sipag` > `./.sipag`.
pub fn default_sipag_dir() -> PathBuf {
    env::var("SIPAG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("HOME")
                .map(|h| PathBuf::from(h).join(".sipag"))
                .unwrap_or_else(|_| PathBuf::from(".sipag"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sipag_dir_uses_sipag_dir_env() {
        // Save and restore env var so this test is hermetic.
        let prev = env::var("SIPAG_DIR").ok();
        env::set_var("SIPAG_DIR", "/tmp/sipag-test-dir");
        assert_eq!(default_sipag_dir(), PathBuf::from("/tmp/sipag-test-dir"));
        match prev {
            Some(v) => env::set_var("SIPAG_DIR", v),
            None => env::remove_var("SIPAG_DIR"),
        }
    }
}
