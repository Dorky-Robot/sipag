// Embedded template files installed by `sipag init`.

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

// Commands
pub const COMMAND_DISPATCH: &str = include_str!("../../lib/templates/commands/dispatch.md");
pub const COMMAND_REVIEW: &str = include_str!("../../lib/templates/commands/review.md");
pub const COMMAND_TRIAGE: &str = include_str!("../../lib/templates/commands/triage.md");

// Hooks
pub const HOOK_SAFETY_GATE_SH: &str = include_str!("../../lib/templates/hooks/safety-gate.sh");
pub const HOOK_SAFETY_GATE_TOML: &str = include_str!("../../lib/templates/hooks/safety-gate.toml");
pub const HOOK_README: &str = include_str!("../../lib/templates/hooks/README.md");

// Settings
pub const SETTINGS_LOCAL_JSON: &str = include_str!("../../lib/templates/settings.local.json");
