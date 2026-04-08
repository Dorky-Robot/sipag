# sipag

**Board-driven work dispatcher for Claude Code crews.**

---

sipag owns the project board (tasks, statuses, roles), refines raw feature
ideas into actionable tickets, and ships the work to running terminal
sessions managed by [katulong](https://github.com/Dorky-Robot/katulong).
Each task knows which role it belongs to; dispatching a task tells katulong
to launch the role's command in the right session.

## The two halves

```bash
# Refine raw ideas into tickets
sipag feature add "wire up the new dispatcher"   # capture an idea
sipag refine f-...                               # turn it into actionable tickets

# Dispatch tickets to running sessions
sipag dispatch <task_id>                         # send a task to its katulong session
sipag tui                                        # open the kanban board
```

Everything else (`sipag add`, `sipag list`, `sipag move`, `sipag projects`,
`sipag up`) is for managing the board from the command line.

To scaffold review agents and slash commands into a project's `.claude/`
directory, use [hulma](https://github.com/Dorky-Robot/hulma).

## How it works

```
sipag feature add ...    Capture a raw idea in the dispatch store
        ↓
sipag refine <feature>   Spawn Claude to turn raw ideas into ticket bullets
        ↓
sipag add ...            Title becomes a task on the board
        ↓
sipag dispatch <id>      Send the task to its role's katulong session
        ↓
katulong session         Agent runs the role's command, picks up the task
        ↓
sipag move <id> review   You move work along as the agent finishes
```

sipag drives the work, but the long-running terminal sessions where agents
actually run live in katulong. sipag tells katulong what to do next; the
refinement step is the one place sipag itself spawns a `claude` subprocess
to chew through raw ideas in the background.

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
hulma (scaffold)  →  sipag (refine + board + dispatch)  →  katulong (sessions)  →  agents
```

- [hulma](https://github.com/Dorky-Robot/hulma) — project-aware Claude Code scaffolder
- [katulong](https://github.com/Dorky-Robot/katulong) — long-running terminal sessions
- [kubo](https://github.com/Dorky-Robot/kubo) — isolated dev environments in Docker
- **sipag** — refinement + board + dispatcher

---

[Get started →](getting-started.md){ .md-button .md-button--primary }
[CLI reference →](cli-reference.md){ .md-button }
