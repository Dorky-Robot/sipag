use std::fmt;

/// Lifecycle status of a worker container.
///
/// State machine:
///   Running → Done | Failed
///   Recovering → Running (reset) | Done | Failed (finalized)
///
/// `Recovering` is a legacy transitional state from old code; new code never
/// sets it, but we handle it gracefully for backward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerStatus {
    Running,
    Recovering,
    Done,
    Failed,
}

impl WorkerStatus {
    /// Whether this status represents a terminal (final) state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }

    /// Whether this status represents an active (non-terminal) state.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Recovering)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Recovering => "recovering",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "recovering" => Some(Self::Recovering),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

impl fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_valid_statuses() {
        assert_eq!(WorkerStatus::parse("running"), Some(WorkerStatus::Running));
        assert_eq!(
            WorkerStatus::parse("recovering"),
            Some(WorkerStatus::Recovering)
        );
        assert_eq!(WorkerStatus::parse("done"), Some(WorkerStatus::Done));
        assert_eq!(WorkerStatus::parse("failed"), Some(WorkerStatus::Failed));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(WorkerStatus::parse(""), None);
        assert_eq!(WorkerStatus::parse("pending"), None);
        assert_eq!(WorkerStatus::parse("RUNNING"), None);
        assert_eq!(WorkerStatus::parse("queued"), None);
    }

    #[test]
    fn display_round_trips_through_parse() {
        for status in [
            WorkerStatus::Running,
            WorkerStatus::Recovering,
            WorkerStatus::Done,
            WorkerStatus::Failed,
        ] {
            let s = status.to_string();
            assert_eq!(WorkerStatus::parse(&s), Some(status));
        }
    }

    #[test]
    fn terminal_statuses() {
        assert!(!WorkerStatus::Running.is_terminal());
        assert!(!WorkerStatus::Recovering.is_terminal());
        assert!(WorkerStatus::Done.is_terminal());
        assert!(WorkerStatus::Failed.is_terminal());
    }

    #[test]
    fn active_statuses() {
        assert!(WorkerStatus::Running.is_active());
        assert!(WorkerStatus::Recovering.is_active());
        assert!(!WorkerStatus::Done.is_active());
        assert!(!WorkerStatus::Failed.is_active());
    }

    #[test]
    fn terminal_and_active_are_complementary() {
        for status in [
            WorkerStatus::Running,
            WorkerStatus::Recovering,
            WorkerStatus::Done,
            WorkerStatus::Failed,
        ] {
            assert_ne!(
                status.is_terminal(),
                status.is_active(),
                "{:?} should be either terminal or active, not both or neither",
                status
            );
        }
    }
}
