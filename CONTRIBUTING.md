# Contributing to sipag

## Development setup

Install the required tools:

```bash
# macOS
brew install bats-core shellcheck shfmt

# Ubuntu / Debian
sudo apt-get install shellcheck
# shfmt
curl -sSL https://github.com/mvdan/sh/releases/download/v3.8.0/shfmt_v3.8.0_linux_amd64 \
  -o /usr/local/bin/shfmt && chmod +x /usr/local/bin/shfmt
# bats-core
git clone https://github.com/bats-core/bats-core.git /tmp/bats-core
sudo /tmp/bats-core/install.sh /usr/local
```

## Make targets

| Target | Description |
|--------|-------------|
| `make check` | Run lint + format checks (same as pre-commit hook) |
| `make test` | Run full BATS test suite (same as pre-push hook) |
| `make lint` | Run shellcheck on all shell files |
| `make fmt-check` | Check shell formatting with shfmt |
| `make fmt` | Auto-format shell files in place |
| `make dev` | Run all checks and tests (lint + fmt-check + test) |

Run `make dev` before pushing to verify everything passes locally.

## Workflow

1. Fork or branch from `main`
2. Make your changes
3. Run `make dev` locally
4. Push and open a pull request

## CI/CD

Every pull request runs the following jobs automatically:

### lint
Runs `make check` — shellcheck and shfmt on all shell scripts. PRs that fail
lint are blocked from merge.

### test
Installs bats-core and runs `make test` — the full BATS unit and integration
suite. PRs that fail tests are blocked from merge.

### security
Scans the PR diff with [gitleaks](https://github.com/gitleaks/gitleaks) to
detect accidentally committed secrets. PRs that expose secrets are blocked from
merge.

### ai-review (optional)
If the `ANTHROPIC_API_KEY` repository secret is configured, Claude Haiku
reviews the diff and posts an advisory comment. This job **never blocks merge**
— it is purely informational. The job is silently skipped when no API key is
present.

### Rust CI (conditional)
A separate workflow runs when `*.rs`, `Cargo.toml`, or `Cargo.lock` files
change. It runs `cargo fmt`, `cargo clippy`, `cargo test`, and `cargo deny`.

## Branch protection rules for `main`

Configure these settings in **Settings → Branches → Branch protection rules**
on GitHub:

| Rule | Setting |
|------|---------|
| **Require status checks to pass before merging** | Enabled |
| Required checks | `Lint`, `Test`, `Security` |
| **Require branches to be up to date** | Enabled |
| **Require pull request reviews before merging** | 1 approval required |
| **Dismiss stale reviews when new commits are pushed** | Enabled |
| **Do not allow force pushes** | Enabled |
| **Do not allow deletions** | Enabled |

> The AI Review job is intentionally excluded from required checks so it never
> blocks a merge.

## Code style

- Shell scripts are formatted with `shfmt -i 0` (tabs)
- All shell files pass `shellcheck -S warning`
- New features need BATS tests in `test/unit/` or `test/integration/`
- Keep function names lowercase with underscores, prefixed by the module name
  (e.g. `task_parse_next`, `executor_run_task`)
