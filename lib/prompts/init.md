You are setting up Claude Code for a new project. The `.claude/` directory
structure has been created. Your job is to explore this project, understand
its domain and tech stack, then write customized agents, commands, and hooks.

## Procedure

1. Explore the project: read README, CLAUDE.md, config files (package.json,
   Cargo.toml, pyproject.toml, go.mod, Makefile, etc.), and scan the source
   directory structure. Understand the domain, tech stack, conventions, and
   workflows.

2. Generate customized agents (`.claude/agents/*.md`):
   - 3-5 agents tailored to this project's domain
   - Every project needs at minimum: a security reviewer, a correctness
     reviewer, and a code quality reviewer — but focused on THIS project's
     specific attack surfaces, edge cases, and conventions
   - Use the reference agents below for format and spirit, not as copy targets

3. Generate customized commands (`.claude/commands/*.md`):
   - 2-4 slash commands for this project's actual workflows
   - Think about what review, deployment, triage, or testing workflows
     matter for THIS project
   - Use the reference commands below for format and spirit

4. Generate a customized `.claude/hooks/safety-gate.toml`:
   - Project-specific deny patterns and path restrictions

5. Print a summary of what you generated and why.

{FORCE_INSTRUCTION}

## Agent format

Every agent file needs YAML frontmatter:

    ---
    name: <kebab-case>
    description: <one sentence — when to use this agent>
    ---

    <agent body with ## sections>

## Reference templates (use as inspiration, not copy targets)

### Reference agent: security-reviewer
{AGENT_SECURITY_REVIEWER}

### Reference agent: architecture-reviewer
{AGENT_ARCHITECTURE_REVIEWER}

### Reference agent: correctness-reviewer
{AGENT_CORRECTNESS_REVIEWER}

### Reference agent: backlog-triager
{AGENT_BACKLOG_TRIAGER}

### Reference agent: issue-analyst
{AGENT_ISSUE_ANALYST}

### Reference command: dispatch
{COMMAND_DISPATCH}

### Reference command: review
{COMMAND_REVIEW}

### Reference command: triage
{COMMAND_TRIAGE}

### Reference config: safety-gate.toml
{HOOK_SAFETY_GATE_TOML}

## Constraints

- Write ONLY inside `.claude/`
- Keep each agent focused — 4 sharp agents beat 6 diffuse ones
- Match the quality bar of the reference templates
