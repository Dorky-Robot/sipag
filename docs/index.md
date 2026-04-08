# sipag

**Board-driven work dispatcher for Claude Code crews.**

---

sipag owns the project board (tasks, statuses, roles) and ships work to
running terminal sessions managed by [katulong](https://github.com/Dorky-Robot/katulong).
Each task knows which role it belongs to; dispatching a task tells katulong
to launch the role's command in the right session.

## Two commands for humans

```bash
sipag dispatch <task_id>                  # Send a task to its katulong session
sipag tui                                 # Open the kanban board
```

Everything else (`sipag add`, `sipag list`, `sipag move`, `sipag projects`,
`sipag up`) is for managing the board from the command line.

To scaffold review agents and slash commands into a project's `.claude/`
directory, use [hulma](https://github.com/Dorky-Robot/hulma).

## How it works

```
sipag add ...           Title becomes a task on the board
        ↓
sipag dispatch <id>     Sends the task to its role's katulong session
        ↓
katulong session        Agent runs the role's command, picks up the task
        ↓
sipag move <id> review  You move work along as the agent finishes
```

sipag itself does not run code — it is a board and a dispatcher. Long-running
terminal sessions live in katulong; sipag just tells katulong what to do
next.

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
kubo (think)  →  sipag (board)  →  katulong (sessions)  →  agents
```

- [kubo](https://github.com/Dorky-Robot/kubo) — chain-of-thought reasoning
- [katulong](https://github.com/Dorky-Robot/katulong) — long-running terminal sessions
- [hulma](https://github.com/Dorky-Robot/hulma) — scaffolds review agents into a project
- **sipag** — board + dispatcher

---

[Get started →](getting-started.md){ .md-button .md-button--primary }
[CLI reference →](cli-reference.md){ .md-button }
