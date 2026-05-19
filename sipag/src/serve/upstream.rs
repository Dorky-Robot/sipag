//! Sanitization helpers for upstream-supplied strings (katulong /
//! ollama / other) that sipag interpolates into its own outbound
//! responses or log lines.
//!
//! Extracted from the now-deleted `serve/katulong_proxy.rs` during
//! the #527 / Phase 2 #6 migration to [`katulong_client::KatulongAsyncClient`].
//! The proxy file went away because the async client now handles
//! HTTP transport + body-cap directly, but `sanitize_upstream_body`
//! is a sipag-side concern (what we put into *our* HTTP responses
//! and log lines), so it kept its home here in `serve/`.

/// Cap on how many chars of an upstream response body we echo back
/// to the caller. Long enough for a useful one-line JSON error,
/// short enough that pathological responses (large stack traces,
/// attacker-supplied content) can't bloat sipag's own response or
/// disrupt log forwarders. Char-bounded, not byte-bounded — see
/// [`sanitize_upstream_body`].
const UPSTREAM_BODY_MAX_CHARS: usize = 256;

/// Sanitize an upstream response body before interpolating it into
/// sipag's own **plain-text** response (axum tuple body, htmx error
/// string). Strips ASCII control characters (Rust's
/// [`char::is_control`] — covers C0 + DEL + C1) except `\n` and
/// `\t` so multi-line JSON errors stay readable, and truncates to
/// [`UPSTREAM_BODY_MAX_CHARS`] chars.
///
/// Caller responsibilities:
/// - **HTML contexts**: do NOT pass sanitized output to a `maud`
///   fragment or other HTML renderer. The filter strips terminal
///   escapes and bytes that disrupt log lines, but does not encode
///   `<`, `>`, `&`, or strip Unicode bidi-override chars
///   (U+202A-202E etc.) that can disrupt visual layout. For HTML
///   error paths, drop body forwarding entirely and rely on the
///   `warn!` log instead — see `observation_transcript_handler`.
/// - **Operator visibility**: the full body should be `warn!`-logged
///   *before* this sanitizer runs so operators retain the raw
///   diagnostic text. Sanitization governs only what crosses to the
///   HTTP caller.
pub(super) fn sanitize_upstream_body(body: &str) -> String {
    body.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(UPSTREAM_BODY_MAX_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_passes_short_well_formed_bodies() {
        assert_eq!(
            sanitize_upstream_body(r#"{"error":"Session already exists"}"#),
            r#"{"error":"Session already exists"}"#
        );
    }

    #[test]
    fn sanitize_keeps_newlines_and_tabs() {
        let s = "line one\nline two\twith tab";
        assert_eq!(sanitize_upstream_body(s), s);
    }

    #[test]
    fn sanitize_strips_other_control_chars() {
        // \r, NUL, ESC, BEL — anything that could disrupt a log line
        // or terminal rendering — is dropped.
        let s = "before\rafter\0nul\x1bescape\x07bell";
        assert_eq!(sanitize_upstream_body(s), "beforeafternulescapebell");
    }

    #[test]
    fn sanitize_truncates_to_cap() {
        let s: String = "x".repeat(UPSTREAM_BODY_MAX_CHARS + 100);
        let out = sanitize_upstream_body(&s);
        assert_eq!(out.chars().count(), UPSTREAM_BODY_MAX_CHARS);
        assert_eq!(out.len(), UPSTREAM_BODY_MAX_CHARS);
    }

    #[test]
    fn sanitize_truncates_to_cap_for_multibyte_chars() {
        let s: String = "😀".repeat(UPSTREAM_BODY_MAX_CHARS + 50);
        let out = sanitize_upstream_body(&s);
        assert_eq!(out.chars().count(), UPSTREAM_BODY_MAX_CHARS);
        assert_eq!(out.len(), UPSTREAM_BODY_MAX_CHARS * 4);
    }

    #[test]
    fn sanitize_truncates_after_filtering() {
        let mut s = String::new();
        for _ in 0..50 {
            s.push('\r');
        }
        for _ in 0..UPSTREAM_BODY_MAX_CHARS {
            s.push('x');
        }
        assert_eq!(
            sanitize_upstream_body(&s).chars().count(),
            UPSTREAM_BODY_MAX_CHARS
        );
    }
}
