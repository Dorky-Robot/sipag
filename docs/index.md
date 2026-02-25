# sipag

**Template installer and sandbox launcher for Claude Code.**

---

sipag installs review agents and safety hooks into your project, launches isolated Docker workers to implement PRs, and provides a TUI to monitor everything. You write the assignment; workers do the work.

## Three commands for humans

```bash
sipag init                                # Install agents + hooks into .claude/
sipag dispatch --repo owner/repo --pr N   # Launch a Docker worker for a PR
sipag tui                                 # Monitor all workers
```

Everything else (`sipag ps`, `sipag logs`, `sipag kill`) is for managing workers from the command line.

## How it works

```
sipag init           Install review agents + safety hooks
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
    curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash
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
