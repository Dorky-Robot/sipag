# sipag

**Autonomous dev agents that evolve with your project.**

---

sipag generates project-aware review agents, ships work through isolated Docker containers, and learns from failures. You write the assignment; workers do the work.

## Three commands for humans

```bash
sipag configure                           # Configure agents + commands for .claude/
sipag dispatch <PR_URL>                   # Launch a Docker worker for a PR
sipag tui                                 # Monitor all workers
```

Everything else (`sipag ps`, `sipag logs`, `sipag kill`) is for managing workers from the command line.

## How it works

```
sipag configure      Configure review agents + commands
       ↓
create PR            Describe the work in the PR body
       ↓
sipag dispatch       Launch a Docker worker
       ↓
worker executes      clone → read PR → claude → push
       ↓
review + merge       You decide what ships
```

Workers run in isolated Docker containers. Claude has full autonomy inside them — no approval dialogs, no interruptions. The container is the safety boundary.

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
kubo (think)  →  sipag (do)  →  GitHub PRs (review)
                    ↑
tao (decide)  ─────┘
```

sipag is the execution layer. [kubo](https://github.com/Dorky-Robot/kubo) handles chain-of-thought planning; [tao](https://github.com/Dorky-Robot/tao) surfaces suspended decisions.

---

[Get started →](getting-started.md){ .md-button .md-button--primary }
[How it works →](how-it-works.md){ .md-button }
