# Getting Started

From zero to your first autonomous PR in a few steps.

## Prerequisites

- macOS or Linux
- [Docker](https://docs.docker.com/get-docker/) installed and running
- [Claude Code](https://claude.ai/code) installed (`npm install -g @anthropic-ai/claude-code`)
- A GitHub account with a personal access token (`gh auth login` or `GH_TOKEN`)
- An Anthropic API key (`ANTHROPIC_API_KEY`)

## 1. Install sipag

=== "Homebrew (recommended)"

    ```bash
    brew tap Dorky-Robot/sipag
    brew install sipag
    ```

    To also use the bash helper commands (`sipag start`, `sipag work`, `sipag merge`, `sipag setup`), add this to your shell profile:

    ```bash
    export PATH="$(brew --prefix sipag)/libexec/bin:$PATH"
    ```

=== "One-line script"

    ```bash
    curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash
    ```

    Supports macOS (Intel and Apple Silicon) and Linux (x86_64 and ARM64).

=== "From source"

    Requires [Rust](https://rustup.rs/) and Cargo.

    ```bash
    git clone https://github.com/Dorky-Robot/sipag
    cd sipag
    make install
    ```

## 2. Run setup

```bash
sipag setup
```

The setup wizard walks you through:

- Setting your `ANTHROPIC_API_KEY` and `GH_TOKEN`
- Creating `~/.sipag/` directories (queue, running, done, failed)
- Creating `~/.sipag/hooks/` for lifecycle hooks
- Optionally registering repos by name

## 3. Prepare your repo

For sipag to pick up issues, they need the `approved` label. Create it in your GitHub repo:

```bash
gh label create approved --color 0075ca --description "Ready for a sipag worker" --repo owner/repo
```

You can also add a `CLAUDE.md` file to your repo so workers know how to work with it — what test commands to run, architecture constraints, coding conventions. See [Setting Up a New Repo](guides/new-repo-setup.md) for a complete guide.

## 4. Create and approve an issue

Create a GitHub issue describing a concrete, self-contained task:

```bash
gh issue create \
  --title "Add a health check endpoint" \
  --body "Add GET /health that returns 200 OK with {status: ok} JSON. No auth required." \
  --repo owner/repo
```

Then label it `approved`:

```bash
gh issue edit <number> --add-label approved --repo owner/repo
```

!!! tip "Good first issues for sipag"
    Start with something small and contained: add a missing test, fix a linting error, add a new endpoint, update a dependency. sipag works best when the task is well-defined — clear acceptance criteria, no ambiguity about what "done" looks like.

## 5. Start a session

Open Claude Code and start a sipag session:

```bash
claude
```

Inside the Claude Code session:

```
sipag start owner/repo
```

Claude reads your GitHub board — open issues, PRs, labels, recent activity — and starts working with you on priorities.

## 6. Let Claude work

The conversation from here is natural. Claude might:

- **Ask product questions** if issues need more clarity
- **Start workers** for approved, well-defined issues
- **Review PRs** that workers have opened
- **Triage your backlog** and suggest what to approve next

You decide what gets approved. Claude and the workers do the rest.

## 7. Watch workers in the TUI

In a separate terminal, open the sipag TUI to observe worker activity:

```bash
sipag
```

The TUI shows all tasks across their lifecycle — queued, running, done, failed — with color-coded status and keyboard navigation.

## 8. Merge when ready

When PRs are stacking up and you're ready to review:

```
sipag merge owner/repo
```

Claude walks through the open PRs with you. You decide what ships.

---

## What just happened?

```
you: "sipag start owner/repo"
   ↓
Claude reads GitHub board
   ↓
you: approved issues, set priorities
   ↓
Claude runs sipag work (in background)
   ↓
Docker workers opened PRs
   ↓
you: "sipag merge owner/repo"
   ↓
Claude walks you through PRs
   ↓
you: merged
```

The next time you run `sipag start`, Claude picks up where things left off.

---

## Troubleshooting

**Worker fails immediately**

Check that Docker is running and your API keys are set:

```bash
docker ps
echo $ANTHROPIC_API_KEY
echo $GH_TOKEN
```

**No issues getting picked up**

Workers only pick up issues labeled `approved`. Check the label exists and is applied:

```bash
gh issue list --label approved --repo owner/repo
```

**Claude can't push to the repo**

The `GH_TOKEN` needs write access to the repo. Check your token scopes:

```bash
gh auth status
```

---

[See a full session walkthrough →](guides/first-session.md){ .md-button .md-button--primary }
[Configure sipag →](configuration.md){ .md-button }
