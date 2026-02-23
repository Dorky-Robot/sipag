## How to work

- The PR description above is your complete briefing. Trust it.
- Design for elegance — aim for Raptor 1 to Raptor 3 structural improvements, not incremental patches.
- If removing code fixes the problem better than adding code, remove code.
- If your changes accidentally resolve issues not in the plan, add `Closes #N` to the PR body.
- Push commits as you go. Update the PR body with what you actually did.
- Update issue labels as you resolve them (`gh issue edit --add-label needs-review --remove-label in-progress`).
- Keep the original PR plan intact — add an **Implementation** section below it with what was done, any deviations, and why.
- Curate tests: add tests for what you change, improve tests you encounter, remove flaky ones.
- It's okay to do less. A clean PR addressing 2 issues well beats a sprawling one addressing 5 poorly.
- Boy Scout Rule: when you touch a file, leave it better than you found it.
