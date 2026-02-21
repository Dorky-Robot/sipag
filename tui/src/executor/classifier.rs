/// Pure log-line classification — zero I/O, easily unit-testable.

#[derive(Debug, Clone, PartialEq)]
pub enum LogKind {
    Normal,
    Commit,
    Test,
    Pr,
    Error,
    Summary(bool), // true = success
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub text: String,
    pub kind: LogKind,
}

impl LogLine {
    /// Classify a raw log line by scanning for key event patterns.
    pub fn classify(text: &str) -> Self {
        let lower = text.to_lowercase();
        let kind = if is_commit(&lower) {
            LogKind::Commit
        } else if is_test(&lower) {
            LogKind::Test
        } else if is_pr(&lower) {
            LogKind::Pr
        } else if is_error(&lower) {
            LogKind::Error
        } else {
            LogKind::Normal
        };
        LogLine {
            text: text.to_string(),
            kind,
        }
    }
}

fn is_commit(lower: &str) -> bool {
    lower.contains("git commit")
        || lower.contains("committed")
        || (lower.contains("commit") && lower.contains("sha"))
        || lower.contains("[new branch]")
        || (lower.contains("pushing") && lower.contains("branch"))
        || lower.contains("git push")
}

fn is_test(lower: &str) -> bool {
    lower.contains("cargo test")
        || lower.contains("npm test")
        || lower.contains("running tests")
        || lower.contains("test result")
        || lower.contains("tests passed")
        || lower.contains("tests failed")
        || (lower.contains("running") && lower.contains("test"))
}

fn is_pr(lower: &str) -> bool {
    lower.contains("pull request")
        || lower.contains("draft pr")
        || lower.contains("gh pr")
        || lower.contains("ready for review")
        || lower.contains("pr created")
        || lower.contains("pr #")
}

fn is_error(lower: &str) -> bool {
    lower.starts_with("error")
        || lower.contains("error:")
        || lower.contains("error[e")
        || lower.contains("panicked")
        || lower.contains("fatal:")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_normal_line() {
        let line = LogLine::classify("Cloning into '/work'...");
        assert_eq!(line.kind, LogKind::Normal);
        assert_eq!(line.text, "Cloning into '/work'...");
    }

    #[test]
    fn classify_commit_line() {
        let line = LogLine::classify("Ran git commit -m 'Add feature'");
        assert_eq!(line.kind, LogKind::Commit);
    }

    #[test]
    fn classify_test_line() {
        let line = LogLine::classify("Running cargo test --all-features");
        assert_eq!(line.kind, LogKind::Test);
    }

    #[test]
    fn classify_pr_line() {
        let line = LogLine::classify("Creating draft pull request #42");
        assert_eq!(line.kind, LogKind::Pr);
    }

    #[test]
    fn classify_error_line() {
        let line = LogLine::classify("error[E0308]: mismatched types");
        assert_eq!(line.kind, LogKind::Error);
    }

    #[test]
    fn classify_error_fatal() {
        let line = LogLine::classify("fatal: repository not found");
        assert_eq!(line.kind, LogKind::Error);
    }

    #[test]
    fn classify_git_push() {
        let line = LogLine::classify("Running git push origin my-branch");
        assert_eq!(line.kind, LogKind::Commit);
    }
}
