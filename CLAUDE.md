# CLAUDE.md — sipag workflow instructions

## What is sipag?

sipag is a sandbox launcher for Claude Code. It spins up isolated Docker containers, each running `claude --dangerously-skip-permissions` on a specific task. Claude Code is the orchestrator — deciding what to work on, in what order, and handling retries. sipag does one thing: spin up sandboxes and make progress visible.

## Issue management is conversation

When discussing features, bugs, or tasks with the human, create GitHub issues directly via `gh`. Don't ask permission — issues are cheap and editable. Share the link so the human can see it immediately.

```bash
gh issue create --title "Fix task tracking race condition" --body "..." --label "bug"
```

## The label workflow

Issues move through a pipeline tracked by labels:

| Stage | Label | What it means |
|---|---|---|
| Created | _(none)_ | Backlog — not yet triaged |
| Triaged | `P0`–`P3` + category | Priority and category assigned |
| Refined | `approved` | Well-defined, ready for implementation |
| In progress | _(PR open)_ | A sipag worker is implementing it |
| Review | _(PR open)_ | PR ready for human review |
| Done | _(closed)_ | Merged |

### sipag start commands

- **`sipag start triage`** — interactive session where Claude and human prioritize together. Claude adds `P0`–`P3` and category labels based on the conversation.
- **`sipag start refinement`** — interactive session where Claude and human refine issues together. Well-defined issues get the `approved` label.
- **`sipag work`** — Docker workers pick up `approved` issues, implement them, and open PRs.
- **`sipag start review`** — interactive session where Claude and human review open PRs together.
- **`sipag merge`** — mechanical merge of approved PRs.

## The conversational principle

`sipag start` commands launch interactive sessions. During these sessions, Claude should:

- Ask broad product or architectural questions — don't walk tickets one by one
- Propose batch actions based on the human's answers (e.g. "Based on what you said, I'll label these 5 issues P1 and close these 2 as duplicates — ok?")
- The human might be on a phone with no screen — keep the interaction voice-friendly and low-friction
- Apply changes via `gh` during the conversation, not after

## Setting up CLAUDE.md in managed repos

Each repo that sipag manages should have its own `CLAUDE.md` at the repo root. This file is read automatically by Claude Code when it starts inside a sandbox. Include:

1. **What the project does** — one paragraph describing the purpose and users
2. **Current priorities** — what matters most right now (features, stability, performance)
3. **Architectural constraints** — module boundaries, patterns to follow or avoid, anything fragile
4. **Labels and conventions** — any repo-specific labels, branch naming, commit message format, PR conventions

Example:

```markdown
# CLAUDE.md

## What is this project?

A billing microservice that handles subscription lifecycle for the SaaS platform.
It owns the subscriptions table and publishes events to the billing-events Kafka topic.

## Current priorities

1. Reliability of webhook delivery (see label: `reliability`)
2. Moving off the legacy Stripe v2 API before EOL

## Architectural constraints

- Never write directly to the subscriptions table from outside this service
- All side effects must be idempotent — retries happen
- Use the internal `EventPublisher` abstraction, not the Kafka client directly

## Conventions

- Branch names: `feat/<issue-number>-slug` or `fix/<issue-number>-slug`
- Commit messages: conventional commits (`feat:`, `fix:`, `chore:`)
- PR labels: add `needs-migration` if the PR includes a DB migration
```

## Development commands (sipag repo itself)

```bash
make build    # release build
make test     # cargo test
make lint     # cargo clippy -D warnings
make fmt      # cargo fmt
make dev      # fmt + lint + test
```
