// Embedded template files installed by `sipag configure`.

// Agents
pub const AGENT_SECURITY_REVIEWER: &str =
    include_str!("../../lib/templates/agents/security-reviewer.md");
pub const AGENT_ARCHITECTURE_REVIEWER: &str =
    include_str!("../../lib/templates/agents/architecture-reviewer.md");
pub const AGENT_CORRECTNESS_REVIEWER: &str =
    include_str!("../../lib/templates/agents/correctness-reviewer.md");
pub const AGENT_BACKLOG_TRIAGER: &str =
    include_str!("../../lib/templates/agents/backlog-triager.md");
pub const AGENT_ISSUE_ANALYST: &str = include_str!("../../lib/templates/agents/issue-analyst.md");
pub const AGENT_ROOT_CAUSE_ANALYST: &str =
    include_str!("../../lib/templates/agents/root-cause-analyst.md");
pub const AGENT_SIMPLICITY_ADVOCATE: &str =
    include_str!("../../lib/templates/agents/simplicity-advocate.md");

// Commands
pub const COMMAND_DISPATCH: &str = include_str!("../../lib/templates/commands/dispatch.md");
pub const COMMAND_REVIEW: &str = include_str!("../../lib/templates/commands/review.md");
pub const COMMAND_TRIAGE: &str = include_str!("../../lib/templates/commands/triage.md");
pub const COMMAND_SHIP_IT: &str = include_str!("../../lib/templates/commands/ship-it.md");
