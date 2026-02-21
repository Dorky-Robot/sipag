You are resolving merge conflicts in a pull request branch.

The repository is at /work.

PR #{{PR_NUM}}: {{PR_TITLE}}
Branch: {{BRANCH}}

A `git merge origin/main` was attempted on this branch and produced merge conflicts.
The conflict markers are already present in the working tree.

Your task:
1. Run `git status` to see which files have conflicts
2. For each conflicted file:
   - Read the file and understand both sides of the conflict
   - Keep changes from both sides where possible
   - When changes conflict directly, preserve the intent of this PR's changes (the `HEAD`
     side) while incorporating relevant changes from main (the `origin/main` side)
   - Never simply discard either side without understanding what it does
3. Stage all resolved files with `git add <file>`
4. Run `git merge --continue --no-edit` to create the merge commit
5. Run `git push origin {{BRANCH}}` to update the pull request

Critical rules — NEVER violate these:
- NEVER run `git rebase` (rebase rewrites history and causes problems for reviewers)
- NEVER run `git push --force` or `git push -f` (force push destroys history)
- NEVER amend any existing commits on this branch
- ONLY use `git merge --continue` to complete the merge, not `git commit` directly

Context about this PR (to help understand what changes to preserve):
{{PR_BODY}}
