You are working on the repository at /work.

Your task:
{{TITLE}}

{{BODY}}

Instructions:
- You are on branch {{BRANCH}} — do NOT create a new branch
- A draft PR is already open for this branch — do not open another one
- Implement the changes
- Run `make dev` (fmt + clippy + test) before committing to validate your changes
- Run any existing tests and make sure they pass
- Commit your changes with a clear commit message and push to origin
- After pushing, update the PR description with a structured summary using `gh pr edit {{BRANCH}} --repo $REPO --body <body>`. Include: a **Summary** section (bullet points of what changed and why), a **Changes** section (files modified), and a **Test plan** section (how changes were validated). Keep `Closes #{{ISSUE_NUM}}` at the top.
- The PR will be marked ready for review automatically when you finish
