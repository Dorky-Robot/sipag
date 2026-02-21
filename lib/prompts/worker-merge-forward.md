You are resolving merge conflicts in PR #{{PR_NUM}} ({{TITLE}}) in {{REPO}}.

The branch {{BRANCH}} has fallen behind main and has merge conflicts.
Your job is to merge main forward into this branch — NEVER rebase.

Instructions:
- You are on branch {{BRANCH}} which already has work in progress
- Run: git fetch origin && git merge origin/main --no-edit
- If there are merge conflicts, resolve them carefully:
  - Keep the work from this branch where possible
  - Use the main branch version where the branch work is clearly superseded
  - After resolving all conflicts, stage the resolved files: git add <files>
  - Complete the merge commit: git merge --continue
- Run make dev (if available) or any existing tests to confirm nothing is broken
- Push the updated branch: git push origin {{BRANCH}}
- Never use git rebase
- Never use git push --force or git push --force-with-lease
