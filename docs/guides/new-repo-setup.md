# Setting Up a New Repo for sipag

This guide covers what a repo needs before you can dispatch workers against it.

---

## 1. Add a CLAUDE.md

Create a `CLAUDE.md` at the repo root. This file primes Claude Code sessions with project-specific context. Include:

- **Project overview** — what the project does, its architecture
- **Build and test commands** — how to build, run tests, lint
- **Key directories** — where the important code lives
- **Conventions** — coding style, naming patterns, PR workflow

Example:

```markdown
# CLAUDE.md

## Project

A REST API built with Rust (Actix Web) and PostgreSQL.

## Commands

- `cargo build` — build
- `cargo test` — run tests (requires DATABASE_URL)
- `cargo clippy -- -D warnings` — lint
- `cargo fmt -- --check` — format check

## Structure

- `src/routes/` — HTTP handlers
- `src/models/` — database models
- `src/services/` — business logic
- `migrations/` — SQL migrations

## Conventions

- All handlers return structured JSON errors
- Tests use a shared test database (see `tests/common/mod.rs`)
- PRs require passing CI before merge
```

Workers read this file when they start. The more accurate it is, the better workers perform.

---

## 2. Configure agents and commands

Run sipag configure to generate project-specific review agents:

```bash
cd ~/Projects/my-repo
sipag configure
```

This creates `.claude/agents/` and `.claude/commands/` tailored to your project. Commit these files so they're available inside worker containers.

---

## 3. Ensure GitHub token access

The GitHub token used by sipag needs write access to the repo:

- **Contents** — push commits to PR branches
- **Pull requests** — read PR body, update PR state
- **Issues** — read issue bodies, update labels (optional)

If you use `gh auth login`, the default scopes cover this. If you use a personal access token via `GH_TOKEN`, make sure it has the `repo` scope.

For organization repos, ensure the token has access to the org. Fine-grained tokens need explicit repository access.

---

## 4. Pull the worker image

Make sure the Docker image is available locally:

```bash
docker pull ghcr.io/dorky-robot/sipag-worker:latest
```

Or build a local image if you need custom tooling:

```bash
docker build -t sipag-worker:local .
export SIPAG_IMAGE=sipag-worker:local
```

---

## 5. Verify with sipag doctor

```bash
sipag doctor
```

Fix anything marked FAIL or MISSING.

---

## Tips for effective PR descriptions

The PR body is the worker's complete assignment. Good descriptions produce good results.

**Include:**

- Which issues it addresses (`Closes #N`)
- The relevant files and modules
- The implementation approach or constraints
- What "done" looks like (specific tests, behaviors)

**Avoid:**

- Vague instructions ("improve the code")
- Assumptions about context the worker won't have
- Multiple unrelated tasks in one PR

---

## Repos with special requirements

### Environment variables

If your test suite needs environment variables (database URLs, API keys), document them in CLAUDE.md. Workers can't access your local `.env` file.

### Private dependencies

If the repo pulls private packages, the worker container needs access. You may need to build a custom Docker image that includes the right credentials or registry configuration.

### Large repos

Very large repos (multi-GB) may hit the default 2-hour timeout during clone. Consider increasing the timeout:

```
# ~/.sipag/config
timeout=10800
```
