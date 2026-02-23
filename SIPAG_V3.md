# sipag v3

## What sipag is

sipag is a slow, relentless gardener for codebases. It watches the issues people file, finds the diseases underneath the symptoms, and fixes them — one careful PR at a time. Over cycles, this naturally stabilizes a project the way a good doctor treats a patient: not by chasing every ache, but by building real health.

sipag doesn't rush. It doesn't try to close 30 issues in one PR. It reads every issue, thinks about what they have in common, picks the deepest problem it can address well, and does that. Then it comes back tomorrow and does it again.

## The philosophy

**Find the disease, not the symptoms.** Three issues about different error messages probably mean there's no unified error handling. Five issues about Docker configuration probably mean the config boundary is wrong. sipag's job is to see these patterns and fix the root cause — which fixes the filed issues and prevents the ones nobody has filed yet.

**Optimize for the step function, not the checklist.** The goal of each cycle is a Raptor 1 to Raptor 3 improvement — a structural leap that changes the trajectory of the codebase. Not a v1.0.1 to v1.0.2 patch that ticks off issue checkboxes. A PR that fully resolves 2 issues and partially addresses 3 others by fixing their shared root cause is better than a PR that fully resolves all 5 with isolated patches. Partial progress on the right abstraction beats complete progress on the wrong one.

This means it's not just okay to partially address issues — it's often the right move. If the disease is "no unified error handling" and you build the error type and migrate 3 of 5 modules, that's a great PR. The remaining 2 modules are trivial follow-up next cycle. You moved the architecture forward.

**Kill accidental complexity.** Sometimes the most elegant fix isn't fixing the bug — it's removing the code that makes the bug possible. Ten issues about configuration edge cases might mean the config system is over-engineered. Replacing 500 lines with 50 that use a simpler model doesn't just fix the 10 issues — it makes entire categories of future issues impossible. Be creative. The best PRs make code disappear.

**Organic stabilization.** Every cycle, sipag reads the full backlog. Issues are the voice of the team (or users, or the codebase itself) saying "this hurts." By listening to that voice and responding with structural improvements, the project naturally converges on stability. You don't need a grand architecture plan — you need a disciplined listener.

**Slow is smooth, smooth is fast.** Quality compounds. Each cycle leaves the project measurably better. There's always tomorrow.

## Why Docker

The Docker worker exists for one reason: to give Claude Code a place to run `--dangerously-skip-permissions` without risking the host machine. Claude Code needs to read files, write files, run tests, install dependencies, execute arbitrary shell commands. That's inherently dangerous on a machine you care about. The container is a disposable sandbox — if Claude does something destructive, you throw the container away. The host is untouched.

This is why the thinking lives on the main session and the doing lives in Docker. The main Claude Code session runs on the host with normal permissions — it can read the codebase, call `gh`, analyze issues, but it doesn't write code or run untrusted commands. All that happens inside the container, where the blast radius is contained.

## The flow

Three phases, two execution contexts. The main Claude Code session handles intelligence (analysis, decisions, review). The Docker worker handles implementation (code changes, tests, commits).

```
                          ┌──────────────────────────────────────────┐
                          │            main claude code              │
                          │                                          │
                          │   1. fetch all issue titles               │
                          │   2. spin up 3 parallel agents to         │
                          │      identify disease clusters:           │
                          │      ┌──────────┬──────────┬──────────┐  │
                          │      │ security │  arch    │ usability│  │
                          │      └────┬─────┴────┬─────┴────┬─────┘  │
                          │           └──────────┼──────────┘        │
                          │   3. synthesize → pick best disease      │
                          │   4. read affected issues + codebase      │
                          │   5. craft a refined PR (title + body)    │
                          │      — architectural insight              │
                          │      — approach and key files             │
                          │      — constraints and gotchas            │
                          │      — validation criteria                │
                          │   6. mark issues in-progress, open PR     │
                          │                                          │
                          └──────────────────┬───────────────────────┘
                                             │
                          ┌──────────────────▼───────────────────────┐
                          │           docker sipag worker             │
                          │                                          │
                          │   7. start cold — read PR as assignment   │
                          │   8. implement, test, commit, push        │
                          │   9. keep PR and issues updated as it     │
                          │      works (progress, findings, status)   │
                          │  10. report state back to host via file   │
                          │                                          │
                          └──────────────────┬───────────────────────┘
                                             │
                          ┌──────────────────▼───────────────────────┐
                          │            main claude code              │
                          │                                          │
                          │  11. review the PR diff                  │
                          │  12. merge or close                      │
                          │  13. loop back to step 1                 │
                          │                                          │
                          └──────────────────────────────────────────┘
```

### Phase 1: Analysis (main Claude Code)

The main session — running on the host — does the thinking. It never writes code directly. Its job is:

1. **Fetch all open issue titles.** Just titles. This gives Claude the full landscape without overwhelming context. Titles are the signal — they tell you what the team is experiencing.

2. **Identify disease clusters from three perspectives.** The main session spins up three parallel agents, each given the same list of issue titles but asked to evaluate from a different lens:
   - **Security & correctness** — What clusters represent vulnerabilities, data integrity risks, or correctness bugs? Which disease, if left unfixed, could cause real damage?
   - **Architecture & simplification** — What clusters point at accidental complexity, missing abstractions, or leaky boundaries? Where would a structural change eliminate entire categories of issues?
   - **Completeness & usability** — What clusters represent gaps in the feature set, broken workflows, or poor developer experience? What's making the project hard to use or contribute to?

   Each agent returns its top disease clusters ranked by impact. Three perspectives surface trade-offs a single analysis would miss — a security lens might prioritize a token-handling fix, while the architecture lens sees that the token handling is tangled because the auth boundary is wrong, and fixing the boundary also resolves 3 usability issues.

   Claude Code already knows how to run parallel agents via the Task tool. The prompt just needs to ask for it — no additional orchestration code in sipag.

3. **Synthesize and pick the best disease.** The main session reads all three perspectives and chooses. Not the biggest cluster, not the scariest vulnerability, but the one where a well-scoped fix yields the most structural improvement across concerns. The synthesis often reveals a deeper disease that no single perspective identified alone.

4. **Mark affected issues as in-progress.** Only the issues the chosen disease cluster covers. This is visible to humans and prevents duplicate work.

5. **Craft and open a draft PR.** This is the critical handoff point. The main session has full context — it has read the issues, understood the codebase, identified the disease. It pours all of that into the PR:

   - **Title** names the disease, not the symptoms. Not "Fix issues #101, #103, #107" but "Unified config validation with structured error reporting."
   - **Body** is a refined ticket — the kind you'd write for a senior engineer joining the project cold. It includes:
     - The architectural insight: what's actually wrong and why these issues are connected
     - The affected issues with full context on each (not just numbers — what each one means for this fix)
     - The recommended approach: which files to touch, what the target design looks like, what patterns to follow
     - What "done" looks like for this PR — which might be partial. If the right move is building the abstraction and migrating 3 of 5 callers, say that. The remaining 2 are next cycle.
     - Creative options: if removing complexity would negate issues entirely, propose that. "Instead of fixing the 4 config edge cases, replace the config parser with a 50-line version that can't have these bugs."
     - Constraints and gotchas: things the worker should watch out for, existing tests that cover this area, related code that shouldn't be touched
     - Validation criteria: how to know the fix is correct (specific tests to run, behaviors to verify)
     - `Closes #N` for issues fully resolved, `Partially addresses #M` for issues where this PR makes structural progress

   This is the main session's most important job. The Docker worker starts with a raw context — no conversation history, no prior analysis. The PR description is the *entire* briefing. A well-written PR description sets the worker up for success; a vague one wastes a cycle.

### Phase 2: Implementation (Docker worker)

Once the container starts, it's a black box. The main Claude Code session can't see inside it and doesn't try to control it. The worker runs autonomously until it finishes or fails.

Inside the container, Claude Code starts fresh — no conversation history, no prior analysis — and reads the PR description as its entire briefing. It clones the repo, checks out the PR branch, and gets to work.

The worker is backgrounded — the main Claude Code session may be doing something else entirely (reviewing another PR, analyzing a different repo, or idle). Nobody is watching the container. So the worker keeps GitHub current as it goes, not just at the end:

- **PR updates throughout** — The worker updates the PR body as it works, not just when it's done. Early on it might add "Investigating: the config parser is more tangled than expected, considering approach B from the plan." After committing, it updates with what was actually done. When finished, it marks the PR ready for review. Anyone watching the PR (human or the main session) can see progress in real time.
- **Issue updates as it discovers** — The worker transitions issue labels as it resolves them. If it finds that its changes accidentally fix #118, it adds `Closes #118` to the PR and transitions that issue too. It's closer to the code than the main session — it knows which issues were actually addressed vs. which the plan was optimistic about.
- **sipag state file** — The worker writes heartbeats and phase updates to a file on a host-mounted volume. This is how the sipag runtime tracks progress without peeking inside the container. Just a text file: phase, heartbeat timestamp, exit code when done.

The PR is the worker's live journal. By the time the main session comes back to review, the PR tells the full story — what was planned, what was discovered, what was done, and why.

### The black box and crash recovery

The Docker worker and the sipag runtime are decoupled by design. They communicate through two channels:

1. **GitHub** (PR state, issue labels) — survives everything. If both sipag and the container crash, the PR and labels are still there when you come back.
2. **State file** (`~/.sipag/workers/*.json`) — a text file on a host-mounted volume. The container writes to it; the sipag runtime reads it.

This means:

- **If sipag crashes or restarts**, the container keeps running. It doesn't know or care that the host process died. It's writing code, pushing commits, updating the PR. When sipag comes back online, it reads the state file and picks up where it left off — "oh, this worker is still running" or "this worker finished while I was down."
- **If the container crashes**, the state file records the failure. sipag reads it, marks the issues back to `ready`, and the next cycle can try again with a different approach.
- **If both crash**, GitHub is the source of truth. The PR exists, the labels exist. A new sipag instance can reconcile by looking at what's in-progress and what has matching state files.

The state file is intentionally simple — a JSON blob with phase, heartbeat, exit code, PR number. No RPC, no socket, no daemon protocol. Just a file. This makes recovery trivial and debugging easy: `cat ~/.sipag/workers/Dorky-Robot--sipag--391.json`.

### Phase 3: Review (main Claude Code)

When the worker finishes (sipag detects exit via the state file), the main session picks up:

7. **Review the PR.** Read the diff. Check if the architectural insight was actually addressed. Verify tests pass. Look for things the worker missed.

8. **Merge or close.** If the PR is good, merge it. If it's not, close it — the issues return to the backlog and will be re-analyzed next cycle. No partial merges, no "fix it later." Either the PR made the codebase better or it didn't.

9. **Loop.** Go back to step 1. The backlog has changed — some issues are resolved, new ones may have appeared, and the codebase is (hopefully) healthier. The next cycle's analysis will be different because the project is different.

## sipag is infrastructure, Claude Code is intelligence

This is the core architectural boundary.

**sipag** (the Rust binary) is plumbing. It provides:

- **Container management** — spinning up Docker workers, mounting volumes, passing credentials
- **State tracking** — reading worker state files, surfacing progress in the TUI
- **Polling loop** — checking state files, detecting completion, back-pressure, drain signals
- **Recovery** — reconciling state when sipag restarts (reading state files, checking GitHub)

**Main Claude Code** (on the host) provides:

- **Disease analysis** — reading issues, finding patterns, identifying root causes
- **Decision-making** — which disease to address, how to scope the PR
- **PR crafting** — writing the refined ticket that briefs the worker
- **Review** — evaluating the PR, deciding to merge or close

**Worker Claude Code** (in Docker) provides:

- **Implementation** — writing code, running tests, committing
- **PR and issue updates** — updating the PR body, transitioning issue labels
- **State reporting** — writing heartbeats and phase to the state file

sipag never shells out to `claude --print` or tries to be smart. It doesn't analyze, decide, or generate. It spins up containers, reads files, and shows you what's happening. The intelligence is split between two Claude Code instances: the main session (thinking) and the worker (doing), connected by a well-crafted PR description and a simple state file.

## What a cycle looks like in practice

A human (or a cron job, or a parent orchestrator) starts a Claude Code session and says "run sipag work on this repo." Here's what happens:

1. sipag fetches the list of issues labeled `ready` from the repo.
2. The main session spins up three parallel agents, each given the same 40 issue titles:
   - **Security agent** flags: #397 (token in git URL), #443 (token in plaintext), #440 (token file permissions) — "the credential handling is scattered and leaky."
   - **Architecture agent** flags: #101 (config crash), #103 (timeout=0 accepted), #107 (invalid keys), #112 (cryptic file-not-found), #115 (silent env override) — "the config system has no validation boundary." Also notes #397/#443 share a root cause with config: "credentials are config, and config has no structure."
   - **Usability agent** flags: #112 (cryptic error), #115 (silent override), #408 (args with spaces break) — "configuration is confusing and error-prone."
3. The main session synthesizes: all three perspectives converge on the config system. Security sees scattered credentials. Architecture sees no validation. Usability sees confusing behavior. The architecture agent's insight is deepest — credentials *are* config, and the config system has no structure. Fix that and you address issues from all three lenses.
4. The main session reads the full bodies and relevant source files. It realizes the config parser is 400 lines of ad-hoc string matching with no validation layer. Patching each issue individually would add 400 more lines of edge-case handling. The creative move: replace the parser with an 80-line version that uses typed fields with built-in validation. This would fully resolve #101, #103, #107, and make #112 and #115 trivial follow-ups.
5. The main session crafts a draft PR: title "Replace ad-hoc config parser with typed validation layer." The body explains the architectural insight, names the 5 issues, says this PR targets #101/#103/#107 fully and lays the groundwork for #112/#115 next cycle. It describes the target design, lists the files to change, and notes that 8 existing tests cover the current parser.
6. sipag marks all 5 issues `in-progress` and opens the draft PR.
7. sipag spins up a Docker worker, pointing it at the PR.
8. The worker starts cold. It reads the PR description and knows: replace the parser, migrate callers, keep the 8 tests green, add validation tests.
9. The worker implements. While working, it realizes the old parser's complexity was also causing #118 (a race condition during reload). The simpler parser eliminates the race entirely. It adds `Closes #118` to the PR body — an issue it wasn't assigned but that the new design makes impossible.
10. The worker finishes. PR body now says: `Closes #101, Closes #103, Closes #107, Closes #118. Partially addresses #112, #115 — typed config makes these straightforward follow-ups.`
11. The main session reviews. The 400-line parser is now 80 lines. 4 issues closed, 2 partially addressed, and a race condition nobody assigned was eliminated. Tests pass.
12. Merge. Four issues close automatically. #112 and #115 stay `ready`.
13. Next cycle: #112 and #115 are now trivial — just add a friendly error message and a log line on override. The hard part (the parser rewrite) is done.

## Key design decisions

### Scope for the step function, not the issue count

The PR isn't optimizing for "close the most issues." It's optimizing for the biggest structural improvement that fits in one coherent PR. Sometimes that closes 5 issues. Sometimes it closes 2 and partially addresses 4 others. Sometimes it closes 1 issue but eliminates an entire class of future bugs by removing accidental complexity. The constraint is "does this PR leave the codebase meaningfully healthier?" not "did we clear the backlog?"

### The PR is a refined ticket, not a stub

The draft PR is created by the main session *before* the worker starts. The main session has context the worker never will — it has the conversation history, codebase familiarity, and the analytical reasoning that identified the disease. All of that goes into the PR description.

This is why the handoff works despite the worker starting cold:
- The PR description is the complete briefing — architectural insight, approach, files to touch, gotchas, test plan
- The worker doesn't need to re-derive the analysis. It reads the ticket and executes.
- Humans can see the plan before any code is written and intervene if it's wrong
- A vague PR description wastes a cycle. A precise one produces a great PR on the first try.

### Labels are the coordination protocol

- `ready` — issue is approved for work
- `in-progress` — a worker is actively addressing this
- `needs-review` — a PR exists that addresses this
- These are the only labels sipag cares about. Priorities, categories, etc. are human concerns.

### File-based orchestration

The sipag runtime and the Docker worker never talk to each other directly. They choreograph through two shared surfaces:

1. **GitHub** (durable, survives everything) — PR state, issue labels, commits. This is the source of truth. If you lost everything else, you could reconstruct the project's state from GitHub alone.
2. **State file** (`~/.sipag/workers/*.json`) — a JSON file on a host-mounted volume. The container writes heartbeats, phase, exit code. The sipag runtime reads it to track progress and detect completion.

No sockets, no RPC, no daemon protocol. The sipag runtime is just a loop that reads files and checks GitHub. The container is just a process that writes files and pushes to GitHub. Either can crash independently and the other continues or recovers.

### One worker at a time per repo

sipag doesn't parallelize workers on the same repo. One disease at a time. This avoids merge conflicts, competing PR strategies, and context fragmentation. Multiple repos can run in parallel — they're independent.

### Failure is cheap

If a worker fails, the issues go back to `ready`. Next cycle, the analysis might pick them up again with a different approach, or it might pick different issues entirely. There's no state to clean up except the state file (which records the failure for TUI visibility).

## The worker prompt

The worker prompt (inside the Docker container) is intentionally lean. The heavy lifting — the analysis, the disease identification, the approach — already lives in the PR description. The worker prompt sets the *disposition*, not the *direction*.

Key principles from the 356722a prompt that should carry forward:

- **The PR description is your assignment.** Read it first. It contains the architectural insight, the approach, the affected issues, and the constraints. Trust it — it was written by a session with full context.
- **Design for elegance.** Each PR should be a step function improvement — Raptor 1 to Raptor 3. Prefer one architectural change over isolated patches. If removing code is more elegant than adding code, remove code.
- **Be creative about what "fixed" means.** If a simpler design makes an issue impossible rather than handling it, that's better than a fix. If you discover that your changes accidentally resolve issues you weren't assigned, claim them. The PR should capture all the good it did.
- **Reference everything you touch.** `Closes #N`, `Partially addresses #M`, `Related to #K`. The PR body tells the full story. Partial progress is real progress — name it explicitly so the next cycle knows where to pick up.
- **Boy Scout Rule.** When you touch a file, leave it better. Fix nearby smells, improve naming, tighten types.
- **Curate the test suite.** Add tests for what you change. Improve tests you encounter. Remove flaky ones.
- **It's okay to do less.** A beautiful PR addressing 2 issues well is better than a sprawling one. Quality compounds.
- **Append, don't overwrite.** When updating the PR body, keep the original plan intact. Add an "Implementation" section below it with what was actually done, any deviations from the plan, and why.

## What sipag is not

- **Not a CI system.** sipag doesn't run on every commit. It runs when you tell it to, looking at the backlog.
- **Not a project manager.** It doesn't prioritize issues, set milestones, or track velocity. It reads what's there and does the most impactful work.
- **Not a code generator.** It's a code improver. It looks at what hurts and fixes the underlying cause.
- **Not autonomous.** A human (or parent orchestrator) starts it, reviews the PRs, and decides what to merge. sipag proposes; humans dispose.
- **Not fast.** It's relentless. One careful cycle at a time, every cycle leaving the project measurably better.

## Open questions for v3

- **What data does sipag provide to the main session?** The main session needs issue titles for disease clustering, and full issue bodies for the selected cluster when writing the PR. Does sipag provide this via commands (`sipag issues`, `sipag issue #N`), or does the main session just use `gh` directly? sipag commands would be more ergonomic and could handle caching/formatting.

- **How does the worker receive its assignment?** Currently via env vars (PROMPT, ISSUE_NUMS, etc.). In v3, the worker should receive a PR URL/branch and work on that PR. The PR description *is* the assignment. The worker prompt just sets the disposition (boy scout, elegance, test curation).

- **What's the review experience?** The main session reviews via `gh pr diff` + `gh pr review`. Should sipag provide a structured review command, or is that just Claude Code being Claude Code?

- **How do we handle the "issues that don't need code" case?** Some issues are duplicates, some are invalid, some need discussion. Should the analysis phase recommend closing/adjusting issues, or just skip them?

- **How much of the flow does sipag automate vs. the main session orchestrate?** Two extremes: (a) `sipag work` is a single command that runs the full loop automatically, or (b) `sipag` provides building blocks (`sipag issues`, `sipag dispatch`, `sipag review`) and the main session strings them together with its own judgment. The diagram suggests (b) — the main Claude Code session is in the driver's seat.

- **Scaling beyond one repo.** Currently `sipag work repo1 repo2` polls both. Should repos be fully independent (separate cycles), or should the analysis consider cross-repo patterns?
