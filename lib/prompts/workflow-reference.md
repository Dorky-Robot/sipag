<!-- sipag:workflow:start -->
## sipag workflow

**PR-only rule** — The host machine is for conversation and commands only.
All code changes must go through Docker workers that open PRs. Never edit
files, commit, or push directly from the host.

### Label lifecycle

| Label | Meaning |
|-------|---------|
| `ready` | Queued for a sipag worker to pick up |
| `in-progress` | Worker is actively building — do not edit |
| `needs-review` | Worker finished — PR awaits human review |
| `P0`–`P3` | Priority (critical → low) |

### Key commands

```
sipag work <owner/repo>           Start/resume worker polling
sipag status                      Global worker overview
gh pr diff N --repo OWNER/REPO    Review a PR
gh pr merge N --repo OWNER/REPO --squash --delete-branch
gh issue edit N --repo OWNER/REPO --add-label "ready"
```

### Triage protocol

1. Group backlog by theme — present clusters, not individual tickets
2. Ask broad questions: priorities, blockers, stale items
3. Recommend P0–P3 labels; surface dependencies
4. Batch-apply `ready` + priority labels after agreement

### Making changes

1. Create or update a GitHub issue describing the change
2. Label it `ready`
3. `sipag work` dispatches a Docker worker → PR
4. Review with `gh pr diff` → merge with `gh pr merge`
<!-- sipag:workflow:end -->
