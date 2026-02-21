# Running sipag in CI

sipag can run in GitHub Actions to automatically dispatch workers when issues are labeled — no local machine required.

## Overview

A CI-based sipag setup looks like this:

1. Someone labels a GitHub issue `approved`
2. A GitHub Actions workflow triggers
3. The workflow runs `sipag work` which picks up the issue
4. A Docker container starts inside the runner
5. Claude works the issue and opens a PR

This is different from the interactive `sipag start` session — there's no conversation layer, just automated dispatch on label events.

## Basic workflow

Create `.github/workflows/sipag-worker.yml`:

```yaml
name: sipag Worker

on:
  issues:
    types: [labeled]

jobs:
  dispatch:
    # Only run when the 'approved' label is applied
    if: github.event.label.name == 'approved'
    runs-on: ubuntu-latest

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install sipag
        run: |
          curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash

      - name: Install Docker
        uses: docker/setup-buildx-action@v3

      - name: Run worker for this issue
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          sipag run \
            --repo ${{ github.repository_url }} \
            --issue ${{ github.event.issue.number }} \
            "${{ github.event.issue.title }}"
```

## Required secrets

Set these in your repository's Settings → Secrets and variables → Actions:

| Secret | Value |
|---|---|
| `ANTHROPIC_API_KEY` | Your Anthropic API key |

`GITHUB_TOKEN` is provided automatically by GitHub Actions with write permissions to the repo.

!!! note "Token permissions"
    The default `GITHUB_TOKEN` can push branches and open PRs. Make sure your workflow's permissions include `contents: write` and `pull-requests: write`:

    ```yaml
    permissions:
      contents: write
      pull-requests: write
      issues: write
    ```

## Parallel dispatch

The basic workflow above runs one worker per label event. If you label multiple issues at the same time, each triggers a separate workflow run — GitHub Actions handles the parallelism.

For batched processing (picking up all currently-approved issues at once), use a scheduled workflow:

```yaml
name: sipag Batch Worker

on:
  schedule:
    - cron: '0 * * * *'  # hourly
  workflow_dispatch:      # also triggerable manually

jobs:
  work:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
      issues: write

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install sipag
        run: |
          curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash

      - name: Process approved issues
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          # sipag work polls until there are no more approved issues
          sipag work ${{ github.repository }}
```

## Controlling costs

Workers use Claude API tokens. A few settings to keep costs predictable:

**Set a timeout** — workers stop after a fixed time whether or not they finish:

```yaml
env:
  SIPAG_TIMEOUT: 1800  # 30 minutes (default)
```

**Limit concurrency** — GitHub Actions job concurrency can cap parallel workers:

```yaml
concurrency:
  group: sipag-workers
  cancel-in-progress: false
```

**Use issue size labels** — Create labels like `size/small`, `size/medium`, `size/large` and filter which ones get dispatched to CI vs. interactive sessions.

## Notifications

Add a step after the worker runs to notify on success or failure:

```yaml
- name: Notify on success
  if: success()
  env:
    GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  run: |
    gh issue comment ${{ github.event.issue.number }} \
      --body "Worker completed. PR opened." \
      --repo ${{ github.repository }}

- name: Notify on failure
  if: failure()
  env:
    GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  run: |
    gh issue comment ${{ github.event.issue.number }} \
      --body "Worker failed. Check the [workflow run](${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}) for logs." \
      --repo ${{ github.repository }}
```

## Self-hosted runners

For repos with larger codebases or longer-running tasks, self-hosted runners give you more control:

```yaml
runs-on: self-hosted
```

Self-hosted runners need Docker installed. The worker container pulls from `ghcr.io/dorky-robot/sipag-worker:latest` — make sure the runner has internet access to pull it.

## Security considerations

**Never put credentials in workflow files.** Always use GitHub Actions secrets.

**The worker container is isolated** — it only has access to the cloned repo, not the runner's filesystem or other secrets. Docker's isolation is the safety boundary.

**`GITHUB_TOKEN` scope** — the automatically-provided token is scoped to the current repository. Workers can't access other repos unless you pass a personal access token with broader scope.

**Review before merge** — even with automated dispatch, PRs still require human review before merging. Don't set up auto-merge unless you've verified the worker output quality for your repo.

## Example: full production setup

A complete setup combining label-triggered dispatch with notifications:

```yaml
name: sipag Worker

on:
  issues:
    types: [labeled]

permissions:
  contents: write
  pull-requests: write
  issues: write

jobs:
  dispatch:
    if: github.event.label.name == 'approved'
    runs-on: ubuntu-latest
    timeout-minutes: 45

    steps:
      - uses: actions/checkout@v4

      - name: Install sipag
        run: curl -fsSL https://raw.githubusercontent.com/Dorky-Robot/sipag/main/scripts/install.sh | bash

      - name: Run worker
        id: worker
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          SIPAG_TIMEOUT: 2400
        run: |
          sipag run \
            --repo "${{ github.server_url }}/${{ github.repository }}" \
            --issue "${{ github.event.issue.number }}" \
            "${{ github.event.issue.title }}"

      - name: Comment on success
        if: success()
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          gh issue comment ${{ github.event.issue.number }} \
            --body ":white_check_mark: Worker completed. Check for a new PR." \
            --repo ${{ github.repository }}

      - name: Remove approved label and re-add on failure
        if: failure()
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          gh issue edit ${{ github.event.issue.number }} \
            --remove-label "in-progress" \
            --repo ${{ github.repository }}
          gh issue comment ${{ github.event.issue.number }} \
            --body ":x: Worker failed. [View logs](${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }})" \
            --repo ${{ github.repository }}
```

---

[FAQ →](../faq.md){ .md-button .md-button--primary }
[Configuration reference →](../configuration.md){ .md-button }
