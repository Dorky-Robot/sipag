# Configuration

sipag is configured through environment variables and the optional `~/.sipag/config` file.

## Environment variables

| Variable | Default | Required | Purpose |
|---|---|---|---|
| `ANTHROPIC_API_KEY` | — | Yes | Passed into worker containers |
| `GH_TOKEN` | — | Yes | GitHub token for repo access and PR creation |
| `SIPAG_DIR` | `~/.sipag` | No | sipag data directory |
| `SIPAG_IMAGE` | `ghcr.io/dorky-robot/sipag-worker:latest` | No | Worker Docker image |
| `SIPAG_TIMEOUT` | `1800` | No | Per-container timeout in seconds |
| `SIPAG_MODEL` | _(claude default)_ | No | Override the Claude model inside workers |
| `SIPAG_WORK_LABEL` | `approved` | No | GitHub label that marks issues ready for workers |

Set these in your shell profile (`.zshrc`, `.bashrc`, etc.) or export them before running sipag.

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export GH_TOKEN="ghp_..."
```

## Config file

`~/.sipag/config` accepts the same settings in `key=value` format. Values here are used as defaults; environment variables take precedence.

```ini
# ~/.sipag/config

# Worker Docker image (useful for local builds)
image=ghcr.io/dorky-robot/sipag-worker:latest

# Container timeout in seconds (30 minutes)
timeout=1800

# GitHub label for approved issues
work_label=approved

# How many workers to run in parallel
batch_size=3

# How often to poll GitHub for new issues (seconds)
poll_interval=60
```

## Directory layout

sipag stores all state under `~/.sipag/`:

```
~/.sipag/
  queue/         # pending tasks
  running/       # actively executing (tracking file + .log per task)
  done/          # completed tasks
  failed/        # tasks that need attention
  hooks/         # lifecycle hook scripts
  repos.conf     # registered repos (name → URL)
  config         # optional configuration
  seen           # worker dedup list (issue numbers already dispatched)
  token          # Claude OAuth token for worker containers
```

Create these directories by running:

```bash
sipag init
```

Or use the setup wizard:

```bash
sipag setup
```

## Repos registry

Register repos by name to avoid typing full URLs:

```bash
sipag repo add myapp https://github.com/org/myapp
sipag repo list
```

Once registered, you can use the short name anywhere a repo URL is accepted.

The registry lives at `~/.sipag/repos.conf`:

```
myapp=https://github.com/org/myapp
backend=https://github.com/org/backend
```

## Worker image

Workers use `ghcr.io/dorky-robot/sipag-worker:latest` by default — a pre-built Ubuntu image with `claude`, `gh`, `git`, Node.js, and npm installed.

The image is published automatically to GHCR on each release and on Dockerfile changes to main.

To test with a locally built image:

```bash
docker build -t sipag-worker:local .
SIPAG_IMAGE=sipag-worker:local sipag work owner/repo
```

## Lifecycle hooks

Drop executable scripts into `~/.sipag/hooks/` to react to worker events. See [How It Works — Lifecycle hooks](how-it-works.md#lifecycle-hooks) for the full event reference.

```bash
mkdir -p ~/.sipag/hooks

cat > ~/.sipag/hooks/on-worker-completed << 'EOF'
#!/usr/bin/env bash
echo "$(date) PR opened: ${SIPAG_PR_URL}" >> ~/.sipag/events.log
EOF

chmod +x ~/.sipag/hooks/on-worker-completed
```

## CLAUDE.md — per-repo configuration

Repos control how Claude behaves inside the sandbox by adding a `CLAUDE.md` file. Claude Code reads it automatically when it starts in the repo directory.

Add it to your repo root:

```
CLAUDE.md            # read by Claude Code
.claude/CLAUDE.md    # alternative location, also read
```

### What to include

A good `CLAUDE.md` answers four questions for Claude before it reads a single line of code:

| Section | What to write |
|---|---|
| **Project** | One paragraph: what the repo does, who uses it |
| **Priorities** | What matters right now — stability, specific feature areas, migrations in progress |
| **Architecture** | Tech stack, key modules, patterns to follow or avoid |
| **Testing** | Exact commands to run tests; what "passing" looks like |

Keep it short. Claude reads `CLAUDE.md` before writing code, so dense prose slows it down. Bullet points and short paragraphs work best.

### Example

```markdown
## Project
dorky_robot is a personal mesh network for self-hosted services.
Rust/Axum backend, HTMX frontend, SQLite database. Passkey auth, no passwords.

## Priorities
Stability > features. Hardening auth before adding new mesh capabilities.
Do not change the passkey flow without a green test suite first.

## Architecture
- src/auth/     — passkey registration and assertion
- src/api/      — Axum route handlers (thin, business logic lives in src/domain/)
- src/domain/   — pure Rust, no async, no framework dependencies
- migrations/   — SQLite migrations via sqlx (never edit existing migrations)

## Testing
cargo test                   # unit + integration
npx playwright test          # E2E (requires running dev server)
make ci                      # full suite used in CI

All tests must pass before opening a PR.
```

### Labels

Workers only pick up issues labeled `approved`. Add your project's label conventions to `CLAUDE.md` so Claude applies them correctly when opening PRs:

```markdown
## Labels
- `approved`    — ready for a sipag worker to implement
- `needs-spec`  — issue needs more detail before approval
- `blocked`     — waiting on external dependency
```

---

[CLI reference →](cli-reference.md){ .md-button .md-button--primary }
[Setting up a new repo →](guides/new-repo-setup.md){ .md-button }
