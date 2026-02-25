Triage the open issue backlog for the current repository.

## Step 1: Identify the repository

Resolve `owner/repo` from the git remote of the current working directory:

```bash
gh repo view --json nameWithOwner --jq .nameWithOwner
```

Store this as `REPO` for subsequent commands.

## Step 2: Fetch all open issues

```bash
gh issue list --repo <REPO> --state open --json number,title,labels,assignees,createdAt,updatedAt --limit 200
```

Count the total and note how many carry the `ready` label. This data provides context for synthesis later — the agents will fetch their own copies.

## Step 3: Launch 2 agents in parallel

Send a **single message** with 2 Task tool calls so they run concurrently:

1. **backlog-triager** (`backlog-triager` agent) — Pass it:
   ```
   Triage all open issues for <REPO>. Follow your full procedure: fetch vision/architecture docs, fetch open issues, evaluate each one, and produce a structured triage report with CLOSE/ADJUST/KEEP/MERGE recommendations.
   ```

2. **issue-analyst** (`issue-analyst` agent) — Pass it:
   ```
   Analyze all open issues for <REPO>. Follow your full procedure: fetch issues, cluster by theme, evaluate from 3 perspectives, and recommend the highest-impact next PR. Focus on issues labeled `ready`.
   ```

Wait for both agents to complete before proceeding.

## Step 4: Synthesize a triage report

Combine both agent outputs into a single structured report:

```
## Triage Report for <REPO>

### Issues to close
<From backlog-triager CLOSE recommendations. Include issue number, title, and reason.>

### Issues to adjust
<From backlog-triager ADJUST recommendations. Include what labels or wording changes are needed.>

### Issues to merge
<From backlog-triager MERGE recommendations. Identify the primary issue and the duplicate.>

### Priority ranking
<From issue-analyst clustering and scoring. List the top clusters with their scores and constituent issues.>

### Recommended next PR
<From issue-analyst recommendation. Include the goal, issues addressed, approach summary, and key files.>

### Summary
- Total open issues: N
- Close: N
- Adjust: N
- Merge: N
- Keep: N
- Ready for dispatch: N
```

## Step 5: Offer to apply changes

Present the triage results and ask the user which actions to take:

- **Label changes** — For issues recommended as `ready` that aren't already labeled, offer to apply:
  ```bash
  gh issue edit <N> --repo <REPO> --add-label ready
  ```

- **Closing issues** — For issues recommended to CLOSE, list them and **ask for explicit confirmation before closing any**. Only close after the user approves:
  ```bash
  gh issue close <N> --repo <REPO> --comment "<reason from triage>"
  ```

- **Merging duplicates** — For MERGE recommendations, offer to close the duplicate with a comment pointing to the primary issue. Requires user confirmation.

Do not take any destructive action (closing, editing) without explicit user approval.
