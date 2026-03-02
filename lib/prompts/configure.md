You are setting up Claude Code for a new project. The `.claude/` directory
structure has been created.

## Goal

Create project-specific **agents**, **commands**, and **git hooks** for this
project in the current working directory. Agents are specialized reviewers that
understand this project's domain. Commands are slash-command workflows (like
`/review`, `/test`) that invoke those agents. Git hooks enforce quality gates
(secrets, formatting, linting, tests) before commits and pushes. Together they
give Claude Code project-aware tooling.

Because Claude Code also has global commands, every project command description
MUST include the project name so users can distinguish project-specific commands
from global ones. For example: "Run tests for MyApp" not just "Run tests".

CRITICAL: You will receive a "Project context" section in the initial message
containing the actual directory listing and config file contents discovered by
sipag. Base ALL your work on that context. Do NOT invent project names, tech
stacks, or domain concepts that are not present in the discovered context.

## Procedure

1. **Review the project context** provided in the initial message. This is the
   ground truth about what exists in this project. You may read additional files
   in the current working directory to deepen your understanding, but NEVER read
   files outside it (no parent directories, no sibling projects, no home
   directory files).

2. **State what you found** before writing anything. Print a short summary:
   - Project name (from package.json, Cargo.toml, etc. — or "unknown")
   - Tech stack (languages, frameworks, tools — ONLY what config files reveal)
   - Key directories
   - If the project is minimal or empty, say so explicitly

3. Generate customized agents (`.claude/agents/*.md`):
   - 3-5 agents tailored to this project's ACTUAL domain and tech stack
   - Every project needs at minimum: a security reviewer, a correctness
     reviewer, and a code quality reviewer — but focused on THIS project's
     specific attack surfaces, edge cases, and conventions
   - Use the reference agents below for format and spirit, not as copy targets
   - Every agent description must reference technologies ACTUALLY found in step 2

4. Generate customized commands (`.claude/commands/*.md`):
   - 3-5 slash commands that drive the agents you created in step 3
   - Every project MUST have a `/ship-it` command — the commit→PR→review→merge
     workflow that invokes your review agents. This is the most important command.
   - The first line of each command file is the description shown in the
     command picker — it MUST include the project name (e.g.,
     "Review a pull request for CoolBeans." not "Review a pull request.")
   - Think about what review, deployment, triage, or testing workflows
     matter for THIS project based on what you found in step 2
   - Commands should invoke the agents by name (e.g., launch a Task with
     `subagent_type: "security-reviewer"`) so the agents actually get used
   - Use the reference commands below for format and spirit

5. Generate git hooks (`.husky/pre-commit` and `.husky/pre-push`):
   - Start from the reference hooks below and customize for this project
   - Always include gitleaks (secrets scan) and typos (spell check) — these are
     universal and must stay in every hook
   - Replace the placeholder formatting and lint steps with real commands for the
     tech stack discovered in step 2 (e.g., `cargo fmt` for Rust, `npx prettier`
     for Node, `ruff format` for Python, `gofmt` for Go)
   - Replace the placeholder test suite step in pre-push with the real test
     runner (e.g., `cargo test --workspace`, `npm test`, `pytest`, `go test ./...`)
   - Follow the exact bash structure: `set -euo pipefail`, `warn()`/`fail()`/`step()`
     helpers, numbered steps, visual box formatting
   - After writing both hook files, run `chmod +x .husky/pre-commit .husky/pre-push`
     to make them executable
   - Tell the user how to activate: `git config core.hooksPath .husky`

6. Print a summary of what you generated and why, citing specific files or
   config entries that motivated each choice.

## Re-running on an existing project

If `.claude/` already contains files from a previous run:
- Read existing agents and commands first
- Update agents if the project's tech stack or structure has changed
- Add new agents or commands for capabilities that have grown
- Remove agents that no longer apply
- Always overwrite — you are the source of truth for these files

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

### Reference agent: root-cause-analyst
{AGENT_ROOT_CAUSE_ANALYST}

### Reference agent: simplicity-advocate
{AGENT_SIMPLICITY_ADVOCATE}

### Reference command: dispatch
{COMMAND_DISPATCH}

### Reference command: review
{COMMAND_REVIEW}

### Reference command: triage
{COMMAND_TRIAGE}

### Reference command: ship-it
{COMMAND_SHIP_IT}

### Reference command: work
{COMMAND_WORK}

### Reference command: consult
{COMMAND_CONSULT}

### Reference git hook: pre-commit
{HOOK_PRE_COMMIT}

### Reference git hook: pre-push
{HOOK_PRE_PUSH}

## Constraints

- Read ONLY files inside the current working directory — never read files outside it
- Write ONLY inside `.claude/` and `.husky/`
- Do NOT explore parent directories, sibling projects, or home directory files
- Do NOT invent or hallucinate project details — if a technology is not in the
  config files or source code, do not reference it in agents or commands
- Every agent and command must be justified by something you actually found
- If the project is minimal (few files, no framework), generate minimal but
  useful agents — do not fabricate a complex domain
- Keep each agent focused — 4 sharp agents beat 6 diffuse ones
- Match the quality bar of the reference templates
