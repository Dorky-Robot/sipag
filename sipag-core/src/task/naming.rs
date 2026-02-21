/// Convert text to a URL-safe slug (lowercase, hyphens only).
///
/// Behavior matches the Bash `worker_slugify` function in `lib/worker/config.sh`
/// for ASCII input. The only divergence is that `worker_slugify` additionally
/// truncates the result to 50 characters for branch name use; this function
/// has no length limit (callers truncate as needed, e.g. `generate_task_id`).
pub fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut slug = String::new();
    let mut prev_hyphen = false;

    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }

    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn test_slugify_special_chars() {
        assert_eq!(slugify("Fix Bug #1!"), "fix-bug-1");
    }

    #[test]
    fn test_slugify_multiple_separators() {
        assert_eq!(slugify("hello   world"), "hello-world");
    }

    #[test]
    fn test_slugify_leading_trailing() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn test_slugify_already_slug() {
        assert_eq!(slugify("fix-bug"), "fix-bug");
    }

    // Cross-validation: these cases must match `worker_slugify` in lib/worker/config.sh.
    // The Bash implementation (tr lower | sed s/[^a-z0-9]/-/g | tr -s '-' | trim) produces
    // identical output for ASCII input. The only divergence is that worker_slugify additionally
    // truncates to 50 chars via `cut -c1-50` for branch-name use.
    #[test]
    fn test_slugify_matches_bash_worker_slugify() {
        assert_eq!(slugify("Fix Bug #1!"), "fix-bug-1");
        assert_eq!(slugify("hello   world"), "hello-world");
        assert_eq!(slugify("  hello  "), "hello");
        assert_eq!(slugify("Code hygiene: remove dead code"), "code-hygiene-remove-dead-code");
        // Multi-char specials: parens + colon collapse to a single hyphen each group
        assert_eq!(slugify("feat(worker): detect stale PRs"), "feat-worker-detect-stale-prs");
    }

    #[test]
    fn test_slugify_50_char_truncation_for_branch_names() {
        // Branches use worker_slugify which truncates at 50 chars via `cut -c1-50`.
        // Verify that taking the first 50 chars of the Rust slug matches what
        // worker_slugify produces (both agree on the pre-truncation portion).
        let long_title = "This is a very long issue title that exceeds fifty characters easily";
        let slug = slugify(long_title);
        let truncated: String = slug.chars().take(50).collect();
        assert_eq!(
            truncated,
            "this-is-a-very-long-issue-title-that-exceeds-fifty"
        );
    }
}
