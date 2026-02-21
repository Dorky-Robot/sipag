use crate::task::slugify;
use chrono::{DateTime, Utc};

/// Build the Claude prompt for a task.
pub fn build_prompt(title: &str, body: &str, issue: Option<&str>) -> String {
    let mut prompt =
        format!("You are working on the repository at /work.\n\nYour task:\n{title}\n");
    if !body.is_empty() {
        prompt.push_str(body);
        prompt.push('\n');
    }
    prompt.push_str("\nInstructions:\n");
    prompt.push_str("- Create a new branch with a descriptive name\n");
    prompt.push_str("- Before writing any code, open a draft pull request with this body:\n");
    prompt.push_str(&format!(
        "    > This PR is being worked on by sipag. Commits will appear as work progresses.\n    Task: {title}\n"
    ));
    if let Some(iss) = issue {
        prompt.push_str(&format!("    Issue: #{iss}\n"));
    }
    prompt.push_str("- The PR title should match the task title\n");
    prompt.push_str("- Commit after each logical unit of work (not just at the end)\n");
    prompt.push_str("- Push after each commit so GitHub reflects progress in real time\n");
    prompt.push_str("- Run any existing tests and make sure they pass\n");
    prompt.push_str(
        "- When all work is complete, update the PR body with a summary of what changed and why\n",
    );
    prompt.push_str("- When all work is complete, mark the pull request as ready for review\n");
    prompt
}

/// Generate a task ID from a timestamp and a slugified description.
///
/// Accepts an injectable `now` for deterministic testing.
pub fn generate_task_id(description: &str, now: DateTime<Utc>) -> String {
    let slug = slugify(description);
    let ts = now.format("%Y%m%d%H%M%S");
    let truncated = slug.get(..30.min(slug.len())).unwrap_or(&slug);
    let id = format!("{ts}-{truncated}");
    id.trim_end_matches('-').to_string()
}

/// Format a duration in seconds as a human-readable string.
pub fn format_duration(secs: i64) -> String {
    if secs < 0 {
        return "-".to_string();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_build_prompt_basic() {
        let prompt = build_prompt("Fix the bug", "", None);
        assert!(prompt.contains("Fix the bug"));
        assert!(prompt.contains("repository at /work"));
        assert!(prompt.contains("pull request"));
    }

    #[test]
    fn test_build_prompt_with_issue() {
        let prompt = build_prompt("Fix the bug", "", Some("42"));
        assert!(prompt.contains("Issue: #42"));
    }

    #[test]
    fn test_build_prompt_with_body() {
        let prompt = build_prompt("Fix the bug", "Some detailed body", None);
        assert!(prompt.contains("Some detailed body"));
    }

    #[test]
    fn test_build_prompt_no_issue() {
        let prompt = build_prompt("Fix the bug", "", None);
        assert!(!prompt.contains("Issue: #"));
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(30), "30s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(90), "1m30s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3661), "1h1m");
    }

    #[test]
    fn test_format_duration_negative() {
        assert_eq!(format_duration(-1), "-");
    }

    #[test]
    fn test_generate_task_id() {
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
        let id = generate_task_id("Fix the authentication bug", now);
        assert!(id.contains("fix-the-authentication-bug"));
        // Should start with the injected timestamp
        assert!(id.starts_with("20240101120000"));
    }

    #[test]
    fn test_generate_task_id_truncates_long_slug() {
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let long_description =
            "this is a very long description that exceeds thirty characters easily";
        let id = generate_task_id(long_description, now);
        // slug portion should be at most 30 chars
        let slug_part = id.strip_prefix("20240101000000-").unwrap();
        assert!(slug_part.len() <= 30);
    }
}
