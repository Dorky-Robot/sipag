You are iterating on PR #{{PR_NUM}} in {{REPO}}.

Original issue:
{{ISSUE_BODY}}

Current PR diff:
{{PR_DIFF}}

Review feedback:
{{REVIEW_FEEDBACK}}

Instructions:
- You are on branch {{BRANCH}} which already has work in progress
- Read the review feedback carefully and address every point raised
- Make targeted changes that address the feedback
- Do NOT rewrite the PR from scratch — make surgical fixes
- Run `make dev` (fmt + clippy + test) before committing to validate your changes
- Commit with a message that references the feedback (do NOT amend existing commits)
- Push to the same branch (git push origin {{BRANCH}}) — do NOT force push
