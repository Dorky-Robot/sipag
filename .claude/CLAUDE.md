# sipag project instructions

## PR descriptions

Write PR descriptions in a product-centric style. Lead with what the change unlocks for users, not what code was modified. Follow this structure:

1. **What this unlocks** — Open with the user-facing outcome. Show concrete before/after examples (shell commands, workflows). Explain what friction is removed and why someone should care. No implementation details here.

2. **What changed** — Brief, bolded summaries of each capability added. One short paragraph per feature. Technical enough for a reviewer to know where to look, but framed around behavior not code.

3. **Files touched** — Flat list of files with one-line descriptions of what changed in each.

4. **Test plan** — Checklist of manual verification steps written as concrete commands a reviewer can run.

Keep the tone direct and concrete. Use code blocks for commands and terminal output. Avoid jargon like "refactor" or "cleanup" — describe what the user can now do that they couldn't before.

## Architecture

sipag is an unattended GitHub issue worker that polls for labeled issues, runs Claude Code on each, and opens PRs. It is a pure bash project (bash 4+) with an optional Rust TUI.

Module structure:

```
bin/sipag              CLI entry point (commands: init, start, status, stop)
  ↓
lib/core/config.sh     Configuration loader (defaults, validation, .sipag file)
lib/core/log.sh        Logging utilities (debug, info, warn, error, die)
lib/core/pool.sh       Worker pool manager (concurrency, polling, signal handling)
lib/core/worker.sh     Individual worker (clone → branch → claude → PR)
  ↓
lib/sources/_interface.sh   Source plugin interface (6 functions)
lib/sources/github.sh       GitHub Issues plugin (gh CLI, label-based workflow)
  ↓
lib/hooks/safety-gate.sh    PreToolUse hook (rule-based + optional LLM safety gate)
```

- **bin/sipag** — Resolves `SIPAG_ROOT`, sources all libraries, dispatches commands. The only executable entry point.
- **lib/core/** — Core runtime. Config, logging, pool management, and the worker lifecycle.
- **lib/sources/** — Pluggable issue sources. Each plugin implements 6 functions: `source_list_tasks`, `source_claim_task`, `source_get_task`, `source_complete_task`, `source_fail_task`, `source_comment`.
- **lib/hooks/** — Claude Code hook scripts for the safety gate system.

## Design principles

- **Source everything, execute nothing.** Libraries are sourced by `bin/sipag`. No library runs directly.
- **Worker isolation.** Each worker gets a fresh git clone. Workers share nothing except config and log directories.
- **Label-based state machine.** Issue state (ready → wip → done) is tracked entirely through GitHub labels.
- **Fail-safe by default.** Safety mode defaults to `strict`. Workers deny ambiguous commands rather than allow them.
- **Pluggable sources.** The `_interface.sh` contract means new backends can be added without touching core code.
- **No global state.** Functions receive all inputs as arguments. Config vars are set once at startup.

## Shell coding conventions

- All scripts use `bash 4+`. Shebang: `#!/usr/bin/env bash`.
- All sourced libraries set no options. Only `bin/sipag` sets `set -euo pipefail`.
- Use `local` for all function-scoped variables.
- Quote all variable expansions: `"$var"`, `"${var}"`.
- Use `[[ ]]` for conditionals, never `[ ]`.
- Use `$(command)` for command substitution, never backticks.
- Functions are namespaced by module: `config_load`, `worker_run`, `pool_start`, `source_list_tasks`.
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
    config.bats          Config loading, validation, fallbacks
    log.bats             Log levels, filtering, output format
    pool.bats            Worker pool counting, reaping, spawning
    worker-state.bats    JSON state file writes, transitions
  integration/
    worker.bats          Full worker_run lifecycle with mocked externals
    github-source.bats   GitHub source plugin with mocked gh
    cli.bats             CLI entry point (help, version, error cases)
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
2. Use `setup_common` / `teardown_common` for temp dirs, PATH isolation, and config defaults.
3. Use `create_mock "cmd" [exit_code] [output]` for simple mocks.
4. Use `create_gh_mock` + `set_gh_response` for `gh` subcommand dispatch.
5. Use `assert_json_field "$json" ".path" "expected"` for JSON assertions.

### Smart test mapping (pre-commit)

The pre-commit hook maps staged files to their corresponding test files:

| Source file | Test file(s) |
|---|---|
| `lib/hooks/safety-gate.sh` | `test/unit/safety-gate.bats` |
| `lib/core/config.sh` | `test/unit/config.bats` |
| `lib/core/log.sh` | `test/unit/log.bats` |
| `lib/core/pool.sh` | `test/unit/pool.bats` |
| `lib/core/worker.sh` | `test/unit/worker-state.bats` + `test/integration/worker.bats` |
| `lib/sources/github.sh` | `test/integration/github-source.bats` |
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
