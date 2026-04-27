//! Session cookie primitives.
//!
//! One cookie name (`sipag_session`), one set of flags, one parser. All
//! flag decisions are centralised here so "what flags does the session
//! cookie carry?" has one answer visible to every reviewer.

/// The HTTP cookie name that carries the session token.
///
/// Fixed string — never construct this ad-hoc at a call site, because a
/// typo or case-mismatch will silently authenticate no one.
pub const SESSION_COOKIE: &str = "sipag_session";

/// Extract the session token from the `Cookie:` header value, if
/// present. Returns `None` on any parse failure — a malformed cookie
/// header is indistinguishable from a missing one at the auth boundary.
pub fn extract_session_token(header_value: &str) -> Option<String> {
    for pair in header_value.split(';') {
        let pair = pair.trim();
        if let Some((name, value)) = pair.split_once('=') {
            if name.trim() == SESSION_COOKIE {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Build the `Set-Cookie` header value for a freshly minted session.
///
/// Flags:
/// - `HttpOnly` — never readable from JavaScript
/// - `SameSite=Lax` — top-level navigations send the cookie, cross-site
///   POSTs don't (blocks CSRF on state-changing endpoints)
/// - `Secure` — set when `secure` is true (remote/tunnel access). We
///   intentionally omit it for loopback so dev over `http://localhost`
///   still works.
/// - `Path=/` — valid for every route on the origin
/// - `Max-Age` — seconds; client drops the cookie when it elapses
///
/// The token is written verbatim — no URL-escaping is needed since our
/// tokens are hex-only.
pub fn build_set_cookie(token: &str, max_age_secs: u64, secure: bool) -> String {
    format!(
        "{name}={token}; Max-Age={max_age_secs}{flags}",
        name = SESSION_COOKIE,
        flags = cookie_flags(secure),
    )
}

/// Build a `Set-Cookie` header that clears the session cookie. Same
/// name + path as the live cookie, `Max-Age=0` to evict immediately.
pub fn build_clear_cookie(secure: bool) -> String {
    format!(
        "{name}=; Max-Age=0{flags}",
        name = SESSION_COOKIE,
        flags = cookie_flags(secure),
    )
}

fn cookie_flags(secure: bool) -> String {
    let mut s = String::from("; HttpOnly; SameSite=Lax; Path=/");
    if secure {
        s.push_str("; Secure");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_pulls_named_cookie_from_multi_cookie_header() {
        let header = "other=foo; sipag_session=abc123; yet_another=bar";
        assert_eq!(
            extract_session_token(header),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extract_handles_whitespace_variants() {
        assert_eq!(
            extract_session_token("sipag_session=t"),
            Some("t".to_string())
        );
        assert_eq!(
            extract_session_token("sipag_session=t  "),
            Some("t".to_string())
        );
    }

    #[test]
    fn extract_returns_none_when_absent() {
        assert_eq!(extract_session_token(""), None);
        assert_eq!(extract_session_token("a=1; b=2"), None);
        assert_eq!(extract_session_token("sipag_session_other=decoy"), None);
    }

    #[test]
    fn build_sets_secure_only_when_requested() {
        assert!(build_set_cookie("abc", 60, true).contains("Secure"));
        assert!(!build_set_cookie("abc", 60, false).contains("Secure"));
    }

    #[test]
    fn build_always_sets_httponly_and_samesite() {
        for secure in [true, false] {
            let c = build_set_cookie("t", 60, secure);
            assert!(c.contains("HttpOnly"));
            assert!(c.contains("SameSite=Lax"));
            assert!(c.contains("Path=/"));
        }
    }

    #[test]
    fn clear_cookie_uses_max_age_zero() {
        let c = build_clear_cookie(true);
        assert!(c.starts_with("sipag_session=;"));
        assert!(c.contains("Max-Age=0"));
        assert!(c.contains("Secure"));
    }
}
