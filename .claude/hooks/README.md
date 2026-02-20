# sipag — Claude Code Hooks

This directory contains Claude Code hook scripts registered in
`.claude/settings.local.json`.

## Hooks overview

| File | Event | Purpose |
|---|---|---|
| `safety-gate.sh` | `PreToolUse` | Allow/deny tool calls before execution |

---

## safety-gate.sh — PreToolUse safety gate

The safety gate intercepts every tool call Claude Code would make and
issues an `allow` or `deny` decision before the tool runs.  It is designed
for **unattended Docker worker** environments where Claude operates
autonomously on a GitHub issue.

### How it is registered

`.claude/settings.local.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "",
        "hooks": [{ "type": "command", "command": ".claude/hooks/safety-gate.sh" }]
      }
    ]
  }
}
```

An empty `matcher` causes the hook to run for every tool.

### Decision logic

```
Read / Glob / Grep / Task / WebSearch / WebFetch
  → always allow (read-only, no side effects)

Edit / Write
  → check config [paths] deny list  → deny if matched
  → check path is inside $CLAUDE_PROJECT_DIR → allow if yes, deny if no

Bash
  → check built-in + config deny patterns → deny if matched
  → check built-in + config allow patterns → allow if matched
  → strict mode  → deny ("ambiguous command")
  → balanced mode → ask LLM (claude-haiku) → allow or deny

Any other tool
  → strict mode  → deny
  → balanced mode → ask LLM
```

### Modes

| Mode | Behaviour for ambiguous commands |
|---|---|
| `strict` (default) | Deny anything not in the explicit allow list |
| `balanced` | Call the Anthropic API (claude-haiku) for a second opinion |

Set the mode via the environment variable **or** the config file (env var
takes precedence):

```bash
export SIPAG_SAFETY_MODE=balanced
```

### Built-in deny patterns

| Pattern | Rationale |
|---|---|
| `sudo` / `doas` | privilege escalation |
| `rm -rf /` / `rm -rf ~` | recursive host deletion |
| `git push --force` / `-f` | destructive remote history rewrite |
| `git reset --hard` | destructive local state change |
| Writes to `/etc/`, `/usr/`, `~/.ssh/` … | host system modification |
| `chmod 777` / `chown` | overly permissive permission change |
| `curl -X POST/PUT/DELETE` / `wget` | outbound write requests |
| `ssh` / `scp` / `rsync` | lateral movement / data exfiltration |
| `(npm\|pip\|gem) install -g` | global package mutation |
| `eval` / `exec` | arbitrary code execution |
| Pipes into `sh` / `bash` | shell injection |
| `docker run --privileged` / `--cap-add` | container escape |
| `mount` / `umount` | filesystem manipulation |
| `iptables` / `ip route` / `ip link` | network rule manipulation |
| `kill -9` / `killall` | uncontrolled process termination |
| `dd if=` | raw disk write |
| `mkfs.*` / `fdisk` / `parted` | filesystem creation/destruction |
| Writes to `/proc/` or `/sys/` | kernel interface manipulation |
| `apt`/`yum`/`apk` install/remove | package manager mutation (image should be immutable) |

### Built-in allow patterns

| Pattern | Rationale |
|---|---|
| `git add/commit/status/diff/log/…` | routine VCS operations |
| `git push` (non-force) | publish commits |
| `npm/yarn/pnpm test/run/exec` | test runners |
| `cargo/go/python/pytest/make/…` test/build/lint | standard build tooling |
| `ls/pwd/which/echo/cat/head/tail/…` | read-only Unix utilities |
| `mkdir` / `cp` / `mv` | filesystem organisation |
| `npm/yarn/pnpm install` / `pip install` | project dependency install |
| `chmod NNN` (3-digit octal) | safe permission change |
| `node/python/ruby` | script execution |
| `bats` | BATS test runner |
| `make test/check/lint/fmt/dev/build/clean/…` | standard Makefile targets |
| `gh issue/pr/repo/release/…` | GitHub CLI read+write ops |

### Optional config file

Create `.claude/hooks/safety-gate.toml` in the project root to extend or
override the built-in behaviour.  All fields are optional.

```toml
# .claude/hooks/safety-gate.toml

[mode]
# "strict" (default) or "balanced"
default = "strict"

[deny]
# Extra patterns appended to the built-in deny list (ERE syntax)
patterns = [
  "docker run --network host",
  "nc -l",
]

[allow]
# Extra patterns appended to the built-in allow list (ERE syntax)
patterns = [
  "^my-internal-tool ",
  "^./scripts/",
]

[paths]
# Absolute path prefixes that Write/Edit should always deny,
# even when inside $CLAUDE_PROJECT_DIR.
deny = [
  "/etc",
  "/usr",
  "/var/run",
]
```

Pattern strings follow **extended regular expression** (ERE) syntax as
accepted by `grep -E`.

### Audit logging

Set `$SIPAG_AUDIT_LOG` to a file path to enable NDJSON audit logging.
Each tool call appended one JSON object per line:

```bash
export SIPAG_AUDIT_LOG=/var/log/sipag/audit.ndjson
```

Example entry:

```json
{"timestamp":"2026-02-20T12:34:56Z","tool_name":"Bash","decision":"deny","reason":"Command matches deny pattern: rm -rf /","command":"rm -rf /"}
```

Fields:

| Field | Type | Description |
|---|---|---|
| `timestamp` | string (ISO-8601 UTC) | When the decision was made |
| `tool_name` | string | Claude Code tool name |
| `decision` | `"allow"` or `"deny"` | Gate decision |
| `reason` | string | Human-readable reason |
| `command` | string | Command / file path (truncated to 200 chars) |

Parse with `jq`:

```bash
# Show all denials
jq 'select(.decision=="deny")' /var/log/sipag/audit.ndjson

# Count by tool
jq -r '.tool_name' /var/log/sipag/audit.ndjson | sort | uniq -c
```

### Environment variables

| Variable | Default | Description |
|---|---|---|
| `SIPAG_SAFETY_MODE` | `strict` | `strict` or `balanced` |
| `CLAUDE_PROJECT_DIR` | `$(pwd)` | Project root (set by Claude Code) |
| `ANTHROPIC_API_KEY` | — | Required for balanced mode LLM evaluation |
| `SIPAG_AUDIT_LOG` | — | Path to NDJSON audit log; unset = no logging |

### Testing

```bash
make test           # run all tests (includes safety-gate.bats)
bats test/unit/safety-gate.bats   # run only safety-gate tests
```
