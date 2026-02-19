# sipag project instructions

## PR descriptions

Write PR descriptions in a product-centric style. Lead with what the change unlocks for users, not what code was modified. Follow this structure:

1. **What this unlocks** — Open with the user-facing outcome. Show concrete before/after examples (shell commands, workflows). Explain what friction is removed and why someone should care. No implementation details here.

2. **What changed** — Brief, bolded summaries of each capability added. One short paragraph per feature. Technical enough for a reviewer to know where to look, but framed around behavior not code.

3. **Files touched** — Flat list of files with one-line descriptions of what changed in each.

4. **Test plan** — Checklist of manual verification steps written as concrete commands a reviewer can run.

Keep the tone direct and concrete. Use code blocks for commands and terminal output. Avoid jargon like "refactor" or "cleanup" — describe what the user can now do that they couldn't before.

## Architecture

sipag is a standalone daemon that manages multiple projects from `~/.sipag/`, polls for work from pluggable sources (GitHub issues, ad-hoc tasks, tao), runs Claude Code on each, and opens PRs. It is a pure bash project (bash 4+) with an optional Rust TUI. Installable via Homebrew.

### Directory layout

```
~/.sipag/
├── config                     # Global defaults (poll interval, max workers, etc.)
├── sipag.pid                  # Single daemon PID
├── logs/
│   └── sipag.log              # Daemon log
├── projects/
│   ├── my-app/
│   │   ├── config             # Per-project config (SIPAG_REPO, source, concurrency, etc.)
│   │   ├── workers/           # PID files, task files, state JSON
│   │   └── logs/              # Per-worker logs
│   └── another-repo/
│       ├── config
│       ├── workers/
│       └── logs/
└── adhoc/
    ├── pending/               # Ad-hoc task JSON files waiting to be picked up
    ├── claimed/               # Currently being worked on
    └── done/                  # Completed ad-hoc tasks
```

### Module structure

```
bin/sipag                  CLI entry point (daemon/project/task commands + legacy compat)
  ↓
lib/core/log.sh            Logging utilities (debug, info, warn, error, die)
lib/core/config.sh         Config loader (global ~/.sipag/config + per-project config)
lib/core/project.sh        Project registry (add/remove/list/show)
lib/core/pool.sh           Multi-project daemon (round-robin polling, global concurrency cap)
lib/core/worker.sh         Individual worker (clone from URL → branch → claude → PR)
  ↓
lib/sources/_interface.sh  Source plugin interface (6 functions)
lib/sources/github.sh      GitHub Issues plugin (gh CLI, label-based workflow)
lib/sources/adhoc.sh       File-based ad-hoc task queue
lib/sources/tao.sh       tao suspended actions plugin (sqlite3)
  ↓
lib/hooks/safety-gate.sh   PreToolUse hook (rule-based + optional LLM safety gate)
  ↓
Formula/sipag.rb           Homebrew formula
tui/                       Rust TUI (multi-project view)
```

- **bin/sipag** — Resolves `SIPAG_ROOT`, sources all libraries, dispatches commands. New command tree: `daemon {start|stop|status}`, `project {add|remove|list|show}`, `task {add|list|show}`. Legacy `start/stop/status/init` commands auto-register projects and delegate.
- **lib/core/** — Core runtime. Config (global + per-project loading), logging, project registry, pool management, and the worker lifecycle.
- **lib/sources/** — Pluggable work sources. Each plugin implements 6 functions: `source_list_tasks`, `source_claim_task`, `source_get_task`, `source_complete_task`, `source_fail_task`, `source_comment`.
- **lib/hooks/** — Claude Code hook scripts for the safety gate system.
- **tui/** — Rust TUI that reads from `~/.sipag/` to show all projects in a single view. Supports `--project <slug>` filtering and legacy single-project mode.

## Design principles

- **Source everything, execute nothing.** Libraries are sourced by `bin/sipag`. No library runs directly.
- **Standalone daemon.** One daemon process manages all projects from `~/.sipag/`. No local repo checkout required.
- **Worker isolation.** Each worker gets a fresh git clone from URL. Workers share nothing except config and log directories.
- **Multi-source architecture.** GitHub issues, ad-hoc tasks (stdin/CLI), and tao suspended actions are all pluggable sources.
- **Label-based state machine.** For GitHub source, issue state (ready → wip → done) is tracked through labels. Ad-hoc source uses filesystem (pending → claimed → done). tao source uses SQLite status field.
- **Fail-safe by default.** Safety mode defaults to `strict`. Workers deny ambiguous commands rather than allow them.
- **Pluggable sources.** The `_interface.sh` contract means new backends can be added without touching core code.
- **No global state.** Config vars are globals set once per project load. Worker subshells inherit a frozen copy.

## Shell coding conventions

- All scripts use `bash 4+`. Shebang: `#!/usr/bin/env bash`.
- All sourced libraries set no options. Only `bin/sipag` sets `set -euo pipefail`.
- Use `local` for all function-scoped variables.
- Quote all variable expansions: `"$var"`, `"${var}"`.
- Use `[[ ]]` for conditionals, never `[ ]`.
- Use `$(command)` for command substitution, never backticks.
- Functions are namespaced by module: `config_load`, `worker_run`, `pool_start`, `source_list_tasks`, `project_add`.
- Private functions (not part of public API) are prefixed with `_`: `_worker_write_state`, `_worker_setup_hooks`.
- Error handling: `|| { log_error "msg"; return 1; }` pattern for recoverable errors, `die` for fatal.

## Development rules

- Never use `--no-verify`. Fix the issue.
- `make dev` (`lint` → `fmt-check` → `test`) before opening PRs.
- `make check` (`typos` → `lint` → `fmt-check`) mirrors pre-commit.
- New source plugins must implement all 6 functions from `_interface.sh`.
- The safety gate hook must be tested with `echo '...' | bash lib/hooks/safety-gate.sh` before push.

## Quality gates

Hooks are the sole quality gate. Fast checks on commit, heavy checks on push.

**Pre-commit** (~10s): gitleaks, typos, shellcheck, shfmt --diff, smart-mapped BATS tests

**Pre-push** (~2-3min): smoke tests, gitleaks, parallel BATS tests (unit + integration), 3-agent AI review

### Tools required

```
brew install gitleaks typos-cli shellcheck shfmt bats-core
```

### Security review

**Layer 1: gitleaks** (deterministic, instant) — 600+ secret patterns, entropy analysis.

**Layer 2: shellcheck** (deterministic, instant) — Static analysis for shell scripts.

**Layer 3: AI-powered analysis** (Claude review, pre-push only)

Shell-specific attack surfaces:
- **Command injection** — Unquoted variables in commands, eval, word splitting on user input.
- **Path traversal** — File paths from external input (issue titles, config values) used without validation.
- **Privilege escalation** — sudo, setuid, writing to system paths.
- **Information disclosure** — API keys, tokens, or secrets in logs, error messages, or git history.
- **Unsafe patterns** — Unquoted `$()`, `eval`, piping to `sh`, `rm -rf` with variable paths.

## Testing

Tests use [BATS](https://github.com/bats-core/bats-core) (Bash Automated Testing System). Install with `brew install bats-core`.

### Test structure

```
test/
  helpers/
    test-helpers.bash    Shared setup/teardown, assertions, config helpers
    mock-commands.bash   Mock creation for gh, git, claude, curl
  unit/
    safety-gate.bats     Security boundary (deny/allow patterns, path validation)
    config.bats          Config loading, validation, fallbacks, project-scoped loading
    log.bats             Log levels, filtering, output format
    pool.bats            Multi-project pool counting, reaping, spawning
    project.bats         Project registry CRUD (add/remove/list/show)
    worker-state.bats    JSON state file writes, transitions
  integration/
    worker.bats          Full worker_run lifecycle with URL-based cloning
    github-source.bats   GitHub source plugin with mocked gh
    adhoc-source.bats    Ad-hoc file-based task queue plugin
    tao-source.bats    tao SQLite source plugin
    cli.bats             CLI entry point (daemon/project/task commands)
    hooks.bats           End-to-end safety gate scenarios
```

### Running tests

```bash
make test              # all tests (unit + integration)
make test-unit         # unit tests only
make test-integ        # integration tests only
make test-parallel     # parallel execution on multiple cores
```

### Writing new tests

1. Every `.bats` file loads the shared helpers:
   ```bash
   load ../helpers/test-helpers
   load ../helpers/mock-commands   # if mocking external commands
   ```
2. Use `setup_common` / `teardown_common` for temp dirs, PATH isolation, SIPAG_HOME isolation, and config defaults.
3. Use `create_mock "cmd" [exit_code] [output]` for simple mocks.
4. Use `create_gh_mock` + `set_gh_response` for `gh` subcommand dispatch.
5. Use `create_project_config "slug" [overrides...]` for project configs in SIPAG_HOME.
6. Use `assert_json_field "$json" ".path" "expected"` for JSON assertions.

### Smart test mapping (pre-commit)

The pre-commit hook maps staged files to their corresponding test files:

| Source file | Test file(s) |
|---|---|
| `lib/hooks/safety-gate.sh` | `test/unit/safety-gate.bats` |
| `lib/core/config.sh` | `test/unit/config.bats` |
| `lib/core/log.sh` | `test/unit/log.bats` |
| `lib/core/pool.sh` | `test/unit/pool.bats` |
| `lib/core/project.sh` | `test/unit/project.bats` |
| `lib/core/worker.sh` | `test/unit/worker-state.bats` + `test/integration/worker.bats` |
| `lib/sources/github.sh` | `test/integration/github-source.bats` |
| `lib/sources/adhoc.sh` | `test/integration/adhoc-source.bats` |
| `lib/sources/tao.sh` | `test/integration/tao-source.bats` |
| `bin/sipag` | `test/integration/cli.bats` |
| `test/helpers/*.bash` | all tests |

### Pre-push parallel execution

Pre-push runs unit and integration tests in parallel on half-CPU each, alongside review agents. If tests fail, review agents are killed and push is blocked.

## Code review checklist

- **Module boundaries:** does the new code respect the source/hook architecture?
- **Variable quoting:** are all variable expansions properly quoted?
- **Error handling:** does the function handle failures and clean up resources?
- **Security:** no command injection, no unvalidated paths, no secrets in code?
- **Consistency:** does the function follow existing naming and patterns?
