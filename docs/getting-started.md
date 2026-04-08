# Getting started

This page walks through installing sipag, registering a project, adding a
task, and dispatching it to a katulong session.

## Prerequisites

- A reachable [katulong](https://github.com/Dorky-Robot/katulong) server
- The katulong connection details written to `~/.katulong/remote.json`:

  ```json
  { "url": "https://katulong.example", "apiKey": "..." }
  ```

That is sipag's only runtime dependency. If you don't yet run katulong, set
it up first — sipag's dispatcher has nothing to talk to without it.

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

Confirm the install:

```bash
sipag version
```

## 1. Register a project

A project is a named bundle of tasks and roles. Create one for the repo you
want to dispatch work for:

```bash
sipag project add my-app --repo owner/my-app
```

The first project you register becomes the default; subsequent commands can
omit `--project` and target it automatically.

## 2. Add a role

Roles are templates that say "when you dispatch a task tagged with this role,
run *this* command in *that* session." Create one at:

```
~/.sipag/projects/my-app/roles/dev.toml
```

with contents like:

```toml
name = "dev"
command = "claude --dangerously-skip-permissions"
worktree = true
```

`command` is what katulong runs in the session for each task. `worktree =
true` tells sipag to create an isolated git worktree for each task before
launching the agent.

## 3. Add a task

```bash
sipag add "Wire up the settings page" --role dev
```

This writes a TOML file under `~/.sipag/projects/my-app/tasks/` with an
auto-incrementing id. List the board to see it:

```bash
sipag list
```

## 4. Dispatch the task

```bash
sipag dispatch 1
```

sipag will:

1. Look up task `#1` and its role
2. Open (or reuse) the katulong session for that role
3. Optionally create a worktree for the task
4. Exec the role's command in the session
5. Move the task to `in-progress`

## 5. Watch the board

```bash
sipag tui
```

The TUI is a kanban view of every project's tasks. Use the arrow keys to
move between cards; hotkeys add, move, and dispatch tasks without leaving the
board.

## Next steps

- [CLI reference](cli-reference.md) — every command and flag
