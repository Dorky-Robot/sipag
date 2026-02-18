# sipag

Sleep while Claude writes your PRs.

**sipag** polls for GitHub issues labeled `sipag`, runs [Claude Code](https://docs.anthropic.com/en/docs/claude-code) on each one, and opens pull requests. You create issues, go to sleep, wake up to PRs.

## Requirements

- [Claude Code](https://docs.anthropic.com/en/docs/claude-code) CLI (`claude`)
- [GitHub CLI](https://cli.github.com/) (`gh`) — authenticated
- `git`, `jq`, `bash` 4+

## Install

```bash
git clone https://github.com/dorky-robot/sipag.git
cd sipag
make install
```

Or run directly from the repo:

```bash
./bin/sipag help
```

## Quick start

```bash
cd your-project

# Generate config
sipag init

# Create a GitHub issue with the 'sipag' label
gh issue create --title "Add input validation to signup form" --label sipag

# Start sipag
sipag start

# Check status
sipag status

# Stop
sipag stop
```

## How it works

1. sipag polls your repo for open issues with the configured label (default: `sipag`)
2. For each issue, it spins up a worker that:
   - Claims the issue (swaps the label to `sipag-wip`)
   - Creates a fresh git clone and branch
   - Runs Claude Code with the issue title + body as the prompt
   - Pushes the branch and opens a PR
   - Marks the issue as done (`sipag-done`) and closes it
3. Workers run in parallel (configurable concurrency)

## Config

Place a `.sipag` file in your project root. Run `sipag init` to generate one interactively, or copy from `.sipag.example`:

| Variable | Default | Description |
|---|---|---|
| `SIPAG_SOURCE` | `github` | Source plugin |
| `SIPAG_REPO` | — | GitHub repo (`owner/repo`) |
| `SIPAG_BASE_BRANCH` | `main` | Base branch for PRs |
| `SIPAG_CONCURRENCY` | `2` | Max parallel workers |
| `SIPAG_LABEL_READY` | `sipag` | Label for ready issues |
| `SIPAG_LABEL_WIP` | `sipag-wip` | Label for in-progress issues |
| `SIPAG_LABEL_DONE` | `sipag-done` | Label for completed issues |
| `SIPAG_TIMEOUT` | `600` | Claude Code timeout (seconds) |
| `SIPAG_POLL_INTERVAL` | `60` | Polling interval (seconds) |
| `SIPAG_SAFETY_MODE` | `strict` | Safety mode: `strict`, `balanced`, or `yolo` |
| `SIPAG_ALLOWED_TOOLS` | — | Comma-separated allowed tools for Claude |
| `SIPAG_PROMPT_PREFIX` | — | Prepended to every Claude prompt |

Add `.sipag.d/` to your `.gitignore` — that's where sipag stores runtime state.

## Safety modes

Workers run Claude Code unattended, so sipag uses a **PreToolUse hook** to auto-approve safe actions and auto-deny dangerous ones — no human prompts needed.

Set `SIPAG_SAFETY_MODE` in your `.sipag` config:

| Mode | Behavior |
|---|---|
| `strict` (default) | Rule-based only. Read-only tools are allowed. File writes must target the project directory. Bash commands are checked against allow/deny regex patterns. Anything ambiguous is denied. |
| `balanced` | Same rules as strict, but ambiguous commands that don't match any pattern are sent to Claude Haiku for a quick safety evaluation. Requires `ANTHROPIC_API_KEY`. |
| `yolo` | Uses `--dangerously-skip-permissions` — no restrictions at all. Use at your own risk. |

**How it works:** sipag writes a `.claude/settings.local.json` in each worker's clone directory, configuring a PreToolUse hook that points to `lib/hooks/safety-gate.sh`. Every tool call Claude makes is intercepted and evaluated before execution.

**Bash allow list** (partial): `git` (standard operations), `npm/yarn/pnpm test|run|install`, `cargo/go/python/pytest test|build`, read-only shell commands (`ls`, `cat`, `wc`, etc.), `mkdir`, `cp`, `mv`.

**Bash deny list** (partial): `sudo`, `rm -rf /`, `git push --force`, `git reset --hard`, writing to `/etc/` or `~/`, `chmod 777`, network writes (`curl -X POST`, `wget`), `ssh`, `eval`, global installs.

## CLI

```
sipag init              Generate .sipag config interactively
sipag start [-f]        Start polling (-f for foreground)
sipag status            Show active workers
sipag stop              Graceful shutdown
sipag version           Print version
sipag help              Show help
```

## License

MIT
