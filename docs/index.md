# sipag

**Conversational agile for Claude Code. You talk; workers ship PRs.**

---

You open a conversation, describe what needs to happen, and sipag handles the rest — spinning up isolated Docker workers that plan, code, test, commit, and open pull requests autonomously. You make the calls. Workers do the work.

## Two commands for humans

```bash
sipag start <repo>   # Begin a sipag session — Claude reads your board and gets to work
sipag merge <repo>   # Review what's ready — Claude walks you through open PRs
```

Everything else (`sipag work`, `sipag run`, `sipag ps`) is Claude's domain.

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

## How it works

```
you type: sipag start <repo>
          ↓
Claude reads GitHub board (issues, PRs, labels)
          ↓
conversation: priorities, triage, refinement
          ↓
Claude creates/approves issues via gh
          ↓
Claude runs: sipag work <repo>  (in background)
          ↓
Docker workers → clone → claude --dangerously-skip-permissions → PR
          ↓
you type: sipag merge <repo>
          ↓
conversation: review, decide, merge
```

Workers run in isolated Docker containers. Claude has full autonomy inside them — no approval dialogs, no interruptions. The container is the safety boundary.

---

## The model

sipag is built around a simple idea: **you own the decisions, Claude does the work**.

- **You** decide what matters, what's approved, what ships
- **Claude** reads context, triages issues, writes specs, spins workers, reviews PRs
- **Workers** implement changes autonomously inside isolated containers

This isn't Claude writing code while you watch. It's Claude acting as your engineering team while you have a conversation about what to build.

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
