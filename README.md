# sipag

<div align="center">

<video src="sipag.mp4" width="600" controls></video>

</div>

Task queue feeder for Claude Code. Reads a markdown checklist, feeds the next unchecked item to `claude`, marks it done, moves on.

## Quick start

```bash
echo "- [ ] Add dark mode support" > tasks.md
sipag           # runs the task
sipag list      # shows [x] done
```

## Install

Clone this repo and add `bin/` to your PATH:

```bash
git clone https://github.com/anthropics/sipag.git
export PATH="$PWD/sipag/bin:$PATH"
```

## Usage

```
sipag                        Run next unchecked task (same as sipag next)
sipag next [-c] [-n] [-f]   Find first - [ ], run claude, mark - [x]
sipag list [-f path]         Print all tasks with status
sipag add "task" [-f path]   Append - [ ] task to file
sipag version                Print version
sipag help                   Show help
```

### Flags

| Flag | Description |
|---|---|
| `-c, --continue` | After completing, loop to the next task |
| `-n, --dry-run` | Show what would run, don't invoke claude |
| `-f, --file <path>` | Task file (default: `./tasks.md` or `$SIPAG_FILE`) |

## Task file format

Standard markdown checklist:

```markdown
# My Project Tasks

- [ ] Implement user authentication
- [x] Set up project scaffolding
- [ ] Add input validation to signup form

  The form at /signup needs server-side validation.
  Check email format and password strength.

- [ ] Fix the memory leak in the WebSocket handler
```

- `- [ ] text` = pending task
- `- [x] text` = done
- First unchecked item (top to bottom) is "next"
- Indented lines (2+ spaces) after a task = body/context sent to Claude
- Headings, blank lines, non-checklist text = preserved but ignored

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `SIPAG_FILE` | `./tasks.md` | Task file path |
| `SIPAG_TIMEOUT` | `600` | Claude timeout (seconds) |
| `SIPAG_MODEL` | _(claude default)_ | Model override |
| `SIPAG_PROMPT_PREFIX` | _(none)_ | Prepended to every prompt |
| `SIPAG_SKIP_PERMISSIONS` | `1` | Set `0` for interactive mode |
| `SIPAG_CLAUDE_ARGS` | _(none)_ | Extra raw args to claude |

## Safety gate (optional)

`extras/safety-gate.sh` is a PreToolUse hook that auto-approves safe actions and auto-denies dangerous ones. To enable it, add to `.claude/settings.local.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/sipag/extras/safety-gate.sh"
          }
        ]
      }
    ]
  }
}
```

Set `SIPAG_SAFETY_MODE=balanced` and provide `ANTHROPIC_API_KEY` for LLM-assisted evaluation of ambiguous commands. Default is `strict` (deny anything not on the allow list).

## Development

```bash
brew install bats-core shellcheck shfmt
make dev     # lint + fmt-check + test
make test    # all tests
make lint    # shellcheck
```

## Works with kubo

kubo can export chains as markdown checklists. sipag doesn't need to know about kubo — it just reads the file:

```bash
kubo show plan-dinner --export >> tasks.md
sipag next --continue
```
