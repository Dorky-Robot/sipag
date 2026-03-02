# Running sipag in CI

sipag dispatch can run in CI pipelines to automatically launch workers for PRs. This guide covers common patterns.

---

## Basic setup

sipag needs three things in CI:

1. **Docker** — to run worker containers
2. **GitHub token** — with write access to the repo
3. **Claude credentials** — OAuth token or API key

---

## GitHub Actions example

```yaml
name: sipag dispatch
on:
  pull_request:
    types: [labeled]

jobs:
  dispatch:
    if: github.event.label.name == 'ready'
    runs-on: ubuntu-latest
    steps:
      - name: Install sipag
        run: |
          curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash

      - name: Pull worker image
        run: docker pull ghcr.io/dorky-robot/sipag-worker:latest

      - name: Dispatch worker
        env:
          GH_TOKEN: ${{ secrets.GH_TOKEN }}
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
        run: |
          sipag dispatch ${{ github.event.pull_request.html_url }}
```

This triggers when a PR is labeled `ready`. The worker runs inside the CI runner's Docker daemon.

---

## Considerations

### Docker-in-Docker

sipag launches Docker containers, so the CI runner needs Docker access. On GitHub Actions with `ubuntu-latest`, Docker is available by default. On other CI platforms, you may need to enable Docker-in-Docker or use a privileged runner.

### Credentials

Store your Claude token and GitHub token as CI secrets. Never hardcode them in workflow files.

- `GH_TOKEN` — a personal access token or GitHub App token with repo write access
- `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` — for Claude Code inside the container

### Timeouts

CI runners have their own timeouts (e.g., 6 hours on GitHub Actions). Make sure `SIPAG_TIMEOUT` is shorter than the runner timeout to allow graceful cleanup.

### Back-pressure

In CI, you may want to adjust `max_open_prs` to control how many workers run concurrently:

```bash
export SIPAG_MAX_OPEN_PRS=5
```

Or set it to 0 to disable the limit if your CI infrastructure can handle it.

---

## Monitoring from CI

sipag writes state files to `~/.sipag/workers/`. In a CI context, these are ephemeral. If you need to monitor workers across runs, consider:

- Checking the PR on GitHub for worker activity (commits, comments)
- Using the lifecycle events in `~/.sipag/events/` to trigger notifications
- Running `sipag ps` at the end of the CI job to report status

---

## Alternative: dispatch from a long-running server

Instead of dispatching from CI, you can run sipag on a dedicated server that watches for labeled PRs:

```bash
# Poll for PRs labeled 'ready' and dispatch workers
while true; do
  gh pr list --label ready --json url --jq '.[].url' | while read url; do
    sipag dispatch "$url"
  done
  sleep 120
done
```

This gives you persistent state files, lessons, and a TUI you can connect to. The `/work` command template automates this pattern with back-pressure and parallel dispatch.
