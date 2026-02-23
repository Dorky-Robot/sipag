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
issues an `allow` or `deny` decision before the tool runs. It uses a
**deny-list-only** model: known-dangerous operations are denied,
everything else is allowed.

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
Any tool:
  1. Read-only tools (Read, Glob, Grep, Task, etc.)  → allow
  2. Edit / Write:
       → deny if path on config [paths] deny list
       → deny if path outside safe dirs (project, ~/.sipag, ~/.claude)
       → allow
  3. Bash:
       → deny if empty command
       → rm: deny if any target outside safe dirs, allow otherwise
       → deny if matches deny patterns
       → allow
  4. Unknown tools → allow
```

### Built-in deny patterns

| Pattern | Rationale |
|---|---|
| `^sudo` / `^doas` | privilege escalation |
| `rm -rf /` / `rm -rf ~` | recursive host deletion |
| `git push --force` / `-f` | destructive remote history rewrite |
| `git reset --hard` | destructive local state change |
| Redirects to `/proc/` or `/sys/` | kernel interface manipulation |
| `chmod 777` / `chown` | overly permissive permission change |
| `^curl -X POST/PUT/DELETE` / `^wget` | outbound write requests |
| `^ssh` / `^scp` / `^rsync` | lateral movement / data exfiltration |
| `^(npm\|pip\|gem) install -g` | global package mutation |
| `^eval` / `^exec` | arbitrary code execution |
| Pipes into `sh` / `bash` | shell injection |
| `docker run --privileged` / `--cap-add` | container escape |
| `^mount` / `^umount` | filesystem manipulation |
| `^iptables` / `^ip route` / `^ip link` | network rule manipulation |
| `^kill -9` / `^killall` | uncontrolled process termination |
| `^dd if=` | raw disk write |
| `^mkfs.*` / `^fdisk` / `^parted` | filesystem creation/destruction |
| `^apt`/`^yum`/`^apk` install/remove | package manager mutation |

Patterns are anchored to command start (`^`) where possible to avoid
false positives when keywords appear in string arguments (e.g.,
`gh issue create --body "use ssh keys"` is allowed).

### Optional config file

Create `.claude/hooks/safety-gate.toml` in the project root to extend
the built-in deny list or add path restrictions. All fields are optional.

```toml
# .claude/hooks/safety-gate.toml

[deny]
# Extra patterns appended to the built-in deny list (ERE syntax)
patterns = [
  "docker run --network host",
  "nc -l",
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
Each tool call appends one JSON object per line:

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
| `CLAUDE_PROJECT_DIR` | `$(pwd)` | Project root (set by Claude Code) |
| `SIPAG_AUDIT_LOG` | — | Path to NDJSON audit log; unset = no logging |

### Testing

```bash
make test           # run all tests (includes safety-gate.bats)
bats test/unit/safety-gate.bats   # run only safety-gate tests
```
