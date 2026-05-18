# sipag — Product Vision

> *Companion to [`docs/narrative.md`](docs/narrative.md): that doc is the customer-facing product story; this is the strategic principles behind the shape — the bets, the deliberate absences, the user shape we're building toward.*

## One-liner

The OKR layer for an agentic fleet. Humans steer at "what are we
optimizing for"; agents handle everything underneath.

## The bet — OKRs over tasks

sipag is built around **objectives and key results**, not tasks. This
is a deliberate choice that defines what we will and will not become.

Most project tools optimize for being a great task manager: kanban
funnels, sprint planning, velocity charts, WIP limits, backlog grooming,
retro-then-replan. **sipag refuses to build any of that.**

The thesis is contrarian:

> Task management shouldn't need to exist as a human responsibility.

If you find yourself dragging cards from todo to doing to done, that
is a smell — the agent layer hasn't absorbed enough autonomy yet.
Optimizing the workflow that flows out of that smell is **optimizing
for something that should not exist**. Every feature that polishes
human task-management makes the workflow more entrenched, harder to
remove, and less honest about where things are heading.

So the human surface in sipag is two questions, and only two:

1. **What are we optimizing for?** → Objectives. Free-text, hypothesis-
   shaped, lived in until they no longer matter.
2. **Is it working?** → Key Results. Free-text. Traffic-light stance:
   green / yellow / red / done.

Tasks still exist in the data model — they're how a KR gets realized.
But they sit *below* the human surface, meant to evolve into
agent-managed scheduling units, not items the human grooms.

## Why this matters for the trajectory

Software tools age out when the user changes shape. A great kanban
tool from 2015 is mediocre today, not because the kanban broke, but
because the user — surrounded by AI agents — needs less of it. We're
betting on a near-future user who:

- writes objectives, not tickets
- sets KRs, not sprints
- delegates execution, not just sub-tasks
- spends their attention on direction, not coordination

If we built the kanban tool first, every feature request would pull
us deeper into task-management theatre — swimlanes, points, cycle
time, WIP limits — each one a small bet that today's user shape
persists. As agents take more of the work, those features get *less*
valuable, not more.

By starting one level up, we build for a user shape that **appreciates
as agents get better**. The human surface stays put. The substrate
underneath gets more autonomous over time. The product doesn't have
to be rewritten when the agent layer matures; it gets quieter at the
human edge and louder underneath.

This is how we keep sipag relevant for years instead of months.

## The human / agent boundary

sipag draws the boundary at the OKR / task line:

| | Owned by | Posture |
|---|---|---|
| Objectives, Key Results | **Human** | Strategic, qualitative, durable |
| Tasks, execution        | **Agent** | Tactical, decomposable, verifiable |
| Across the line         | Mixed   | Agent makes a case; human accepts or redirects at the *KR* level |

Today the human still creates tasks because the agent layer isn't
ready to. That's the spike posture; we'll narrate it through the
seams. As agent capability grows, sipag's job is to keep the human
surface stable while the substrate absorbs more responsibility — not
to invite the human back down.

## What sipag deliberately doesn't have

- **No kanban funnel.** Status fields exist (todo, in-progress, review,
  done) but the UI shows tasks under their KR, not in columns. There
  is no swim, there are no lanes.
- **No sprints, no velocity, no points.** Cadence is irrelevant. An
  objective is open until it isn't.
- **No standups, retros, planning rituals.** These are
  human-coordinating-with-human ceremonies. sipag is
  human-coordinating-with-agents.
- **No assignees.** Ownership lives at the KR level. A task exists to
  advance a KR; whichever agent picks it up runs it.
- **No "blocked" / "ready for review" lanes.** Those are kanban
  artifacts. If a KR is at risk, its stance turns yellow or red.
  That is the only signal.
- **No portfolio / program / roadmap layer.** Above objectives there
  is nothing. If we ever add a layer, it goes *up* (decide what we
  are not optimizing for), not sideways into more management.

Each absence is a feature. Together they keep the surface honest about
what humans should and shouldn't be doing.

## What sipag does have

- **Objectives.** Free-text. Open or closed. No hierarchy above them.
- **Key Results.** Free-text title, traffic-light stance, many tasks
  per KR.
- **Standing.** Separate top-level surface for upkeep and firefights —
  work that doesn't ladder up to an outcome (architecture review, dep
  audits, "this just broke, fix it").
- **Idea box.** Parking lot. One click promotes an idea to active.
- **Agent surface (planned).** The same `POST /tasks` and `PATCH
  /tasks/:id` endpoints the human UI uses, exposed for an agent loop
  to decompose KRs into tasks, execute them, and report back at the
  KR-stance level.
- **Mesh-aware execution.** A task running on host X surfaces as a
  small footnote on its KR row — not a panel of its own. The
  machinery is visible only when load-bearing.

## sipag in the stack

```
kubo (think)  →  sipag (objectives + KRs)  →  katulong (sessions)  →  agents do the work
```

- **kubo** — chain-of-thought reasoning, decomposes problems
- **sipag** — the OKR layer; humans steer here, agents work below
- **katulong** — long-running terminal sessions where agents execute
- **hulma** — scaffolds review agents and slash commands into a
  project

Each tool composes; any can be replaced. sipag's only runtime
dependency is a reachable katulong server (configured at
`~/.katulong/remote.json` per host, aggregated through
`~/.sipag/hosts.toml`).

## What success looks like

You open sipag in a katulong tile. You see three objectives, six KRs,
eleven active tasks. Two KRs are yellow; one is green; one is done.
The tasks under the yellow KRs are the ones to look at. You drill in
if a task seems stuck; you don't otherwise. Agents propose new tasks
under each KR; you accept, redirect, or close at the **KR** level —
never the task level.

You spend your time deciding what to optimize for.
The fleet handles the rest.
