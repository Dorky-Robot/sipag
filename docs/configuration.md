# Configuration

sipag uses a layered configuration system: environment variables override config file values, which override hardcoded defaults.

---

## Config file

Create `~/.sipag/config` with key=value pairs (one per line):

```
image=ghcr.io/dorky-robot/sipag-worker:latest
timeout=7200
work_label=ready
max_open_prs=3
poll_interval=120
heartbeat_interval=30
heartbeat_stale=90
```

Lines starting with `#` are ignored.

---

## Config reference

| Key | Env var | Default | Description |
|-----|---------|---------|-------------|
| `image` | `SIPAG_IMAGE` | `ghcr.io/dorky-robot/sipag-worker:latest` | Docker image for workers |
| `timeout` | `SIPAG_TIMEOUT` | `7200` | Worker timeout in seconds (2 hours). Minimum: 1 |
| `work_label` | `SIPAG_WORK_LABEL` | `ready` | Issue label that marks work ready for dispatch |
| `max_open_prs` | `SIPAG_MAX_OPEN_PRS` | `3` | Max active workers before dispatch is paused. 0 disables the limit |
| `poll_interval` | `SIPAG_POLL_INTERVAL` | `120` | Seconds between polling cycles. Minimum: 10 |
| `heartbeat_interval` | `SIPAG_HEARTBEAT_INTERVAL` | `30` | Seconds between heartbeat writes. Minimum: 5 |
| `heartbeat_stale` | `SIPAG_HEARTBEAT_STALE` | `90` | Seconds before a heartbeat is considered stale. Minimum: 15 |

The sipag data directory defaults to `~/.sipag/` and can be overridden with `SIPAG_DIR`.

---

## Resolution order

For each config key, sipag checks in this order:

1. **Environment variable** (e.g. `SIPAG_IMAGE`) — highest priority
2. **Config file** (`~/.sipag/config`) — middle priority
3. **Hardcoded default** — lowest priority

---

## Credentials

Workers need credentials for GitHub and Claude Code. These are passed as environment variables into the Docker container.

### GitHub

sipag resolves a GitHub token in this order:

1. `GH_TOKEN` environment variable
2. `gh auth token` (GitHub CLI's stored token)

The token needs write access to the repos you dispatch workers against.

```bash
# Option A: GitHub CLI (recommended)
gh auth login

# Option B: Environment variable
export GH_TOKEN=ghp_...
```

### Claude Code

sipag resolves Claude credentials in this order:

1. `CLAUDE_CODE_OAUTH_TOKEN` environment variable
2. `~/.sipag/token` file (OAuth token)
3. `ANTHROPIC_API_KEY` environment variable

```bash
# Option A: OAuth token (recommended)
# Run `claude` and complete the OAuth flow, then save the token:
echo 'YOUR_OAUTH_TOKEN' > ~/.sipag/token
chmod 600 ~/.sipag/token

# Option B: API key
export ANTHROPIC_API_KEY=sk-ant-...
```

---

## Custom Docker image

To use a locally built image instead of the published one:

```bash
docker build -t sipag-worker:local .
export SIPAG_IMAGE=sipag-worker:local
sipag dispatch https://github.com/owner/repo/pull/42
```

Or set it permanently in the config file:

```
image=sipag-worker:local
```

---

## File layout

```
~/.sipag/
├── config          # Optional key=value config file
├── token           # Optional Claude OAuth token (mode 0600)
├── workers/        # PR-keyed state JSON files + heartbeat files
├── events/         # Append-only lifecycle event files
├── logs/           # Worker stdout/stderr ({owner}--{repo}--pr-{N}.log)
└── lessons/        # Per-repo learning from failures ({owner}--{repo}.md)
```

These directories are created automatically by `sipag doctor` or the first `sipag dispatch`.

---

## Back-pressure

The `max_open_prs` setting controls how many workers can run concurrently. When the count of active (non-terminal) workers equals or exceeds this limit, `sipag dispatch` refuses to start new work.

The policy is **fail closed**: if the count can't be verified, dispatch refuses to start. Set `max_open_prs=0` to disable the limit entirely.

---

## Timeouts

The `timeout` setting wraps the Docker container in a system timeout command (`timeout` on Linux, `gtimeout` on macOS). When the timeout expires, the container is killed and the worker is marked as failed.

The default of 7200 seconds (2 hours) is generous. Most workers finish in 15-45 minutes. Increase this if your repo has a long build/test cycle.
