# sipag

**The OKR layer for an agentic fleet.**

> For the product story (a day with sipag, the FAQ, what it deliberately
> isn't), see [`narrative.md`](narrative.md). This page is the landing-page
> overview + install instructions.

---

sipag owns the project board (tasks, statuses, roles) and ships the work
to running terminal sessions managed by
[katulong](https://github.com/Dorky-Robot/katulong). Each task knows which
role it belongs to; dispatching a task tells katulong to launch the role's
command in the right session.

## Two commands carry most of the weight

```bash
sipag dispatch <task_id>   # send a task to its katulong session
sipag tui                  # open the interactive board (default with no args)
```

Everything else (`sipag add`, `sipag list`, `sipag move`, `sipag projects`,
`sipag up`) is for managing the board from the command line.

To scaffold review agents and slash commands into a project's `.claude/`
directory, use [hulma](https://github.com/Dorky-Robot/hulma).

## How it works

```
sipag add ...            Add a task to the board
        ↓
sipag dispatch <id>      Send the task to its role's katulong session
        ↓
katulong session         Agent runs the role's command, picks up the task
        ↓
sipag move <id> review   You move work along as the agent finishes
```

sipag drives the work; the long-running terminal sessions where agents
actually run live in katulong. sipag tells katulong what to do next.

!!! note "Refinement pipeline deprecated 2026-05-17"

    Earlier versions exposed `sipag feature add` and `sipag refine
    <feature>` for turning raw ideas into actionable tickets via a
    background `claude` subprocess. That pipeline was retired in favor
    of a new **Experimentation** work model (spike → observe →
    derive). See
    [`docs/architecture.md`](architecture.md) for the current shape.
    The deprecated source is preserved in
    `sipag-core/src/{feature,refine}.rs` with deprecation banners.

---

## Install

=== "Homebrew (recommended)"

    ```bash
    brew tap Dorky-Robot/sipag
    brew install sipag
    ```

=== "One-line script"

    ```bash
    curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | sh
    ```

=== "From source"

    ```bash
    cargo install --path sipag
    ```

---

## Part of the dorky robot stack

```
hulma (scaffold)  →  sipag (board + dispatch)  →  katulong (sessions)  →  agents
```

- [hulma](https://github.com/Dorky-Robot/hulma) — project-aware Claude Code scaffolder
- [katulong](https://github.com/Dorky-Robot/katulong) — long-running terminal sessions
- [kubo](https://github.com/Dorky-Robot/kubo) — isolated dev environments in Docker
- **sipag** — board + dispatcher

---

[Get started →](getting-started.md){ .md-button .md-button--primary }
[CLI reference →](cli-reference.md){ .md-button }
