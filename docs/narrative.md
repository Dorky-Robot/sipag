# sipag — the product narrative

> **The one thing.** Sipag is where humans steer an agentic dev fleet by writing what matters, not by coordinating who does what.

If that one sentence is true, everything else cascades. If it isn't, nothing else here matters.

This document is the **product narrative** — what sipag is, what using it feels like, why it exists, and what it deliberately isn't. It's written present-tense, as if the product is fully shipped, on the Amazon "Working Backwards" theory that you should be able to describe a product compellingly *before* you build it. Some of what's described here is real today; some is on the queue. The shape doesn't change.

For the strategic principles behind the shape, see [`../VISION.md`](../VISION.md). For the architecture, [`modules.md`](modules.md). For capability state, [`feature-matrix.md`](feature-matrix.md). For commands, [`cli-reference.md`](cli-reference.md). This doc is the spine; those are the ribs.

---

## A Tuesday morning

You open sipag.dev. Three Objectives across the top. Six Key Results below them. Most are green. One is yellow.

The yellow KR is the auth refactor. There's a small sidebar item on it: *"Two of the agents working on this are arriving at incompatible designs — cookie-domain wildcards vs explicit subdomain whitelisting. Which direction wins?"* The question came from gemma a few minutes ago; it noticed the divergence in their commit patterns and worktree branches before either agent was going to ask you.

You read both options for ninety seconds. You pick the explicit whitelisting. Close the tab.

That afternoon, you check back. The agents converged on your choice; you flip the KR green. Two more Objectives have new observations in their sidebars — one strategic, one a pattern someone's lens-worker noticed about test flakiness across three repos. You scan them. Nothing needs you. You close the tab again.

You never wrote a ticket. Never moved a card between columns. Never assigned anyone. You wrote what mattered up front; the fleet handled the rest; sipag surfaced the one thing in your day that actually needed a human.

That's what sipag is for.

---

## What sipag is, in one sentence

**For** software teams running an agentic dev fleet
**Who** want to spend their attention on strategic direction, not coordination,
**The** sipag is **a steering surface**
**That** lets humans write Objectives and Key Results and lets agents handle decomposition, execution, and the report-up,
**Unlike** kanban tools (which optimize for human-coordinating-with-human) or AI dev assistants (which optimize for single-session productivity),
**Our product** is the layer where the human's two questions live — *what are we optimizing for?* and *is it working?* — with everything else pushed below the surface.

---

## Why this exists

Software work today is task management theatre. Humans drag cards from todo to doing to done. Agents wait for instructions. Standups exist to coordinate humans who are coordinating other humans. The tools optimize for the user shape of 2015 — a team of people with overlapping context who need to align with each other.

The user shape is changing. With agents that can actually work, the human's role narrows to *direction*. What are we optimizing for. Is it working. Why or why not. The rest is execution — and execution can increasingly be delegated.

Tools that optimize for the old shape get *less* valuable as agents get better. Tools that draw the line at the OKR / execution boundary — humans above, agents below — get *more* valuable. Sipag is built for the second.

The contrarian bet:

> Task management shouldn't need to exist as a human responsibility.

If you find yourself dragging cards, that's a smell — the agent layer hasn't absorbed enough autonomy yet. Optimizing the workflow that flows out of that smell is optimizing for something that should not exist.

---

## How it works

You write **Objectives** — free-text, hypothesis-shaped, lived in until they don't matter. Under each Objective you write **Key Results** — also free-text, with a traffic-light stance (green / yellow / red / done). That's your entire surface.

Below the surface: every Objective and KR you write is also a **lens**. A lens is sipag's word for "a perspective through which to read what's happening." When you write a KR, a small background worker starts watching what the fleet is doing — through the lens of that KR. It looks at commits, agent transcripts, test results, anything else available, and keeps a running notebook of what it observes. When the worker sees something the human should know about, it surfaces it as a note, a question, or a proposed stance change under the KR's sidebar.

That's it. Write what matters. Wait. Read what surfaces. Steer.

Mechanical execution lives in *katulong* (long-running terminal sessions for the agents) and runs underneath. *kubo* handles the chain-of-thought planning agents need. *hulma* scaffolds review agents and slash commands. Sipag is the layer where you tell the stack what to optimize for and where it tells you what's happening.

You also get **Standing** — a separate surface for upkeep and firefights that don't ladder up to an Objective (architecture review, security patches, "this just broke"). And an **Idea box** for parking lots of thoughts before they become Objectives. Those are the only other places work lives.

---

## What sipag deliberately isn't

Each absence is a feature. They're listed here because the absences are how sipag stays honest about which layer the human belongs at.

- **You never drag a card between columns.** Tasks exist in the data model but live below the human surface. If you notice yourself moving cards, that's the agent layer not absorbing enough work.
- **There are no sprints, no velocity, no points.** An Objective is open until it isn't. Cadence is irrelevant.
- **There are no standups, retros, or planning meetings inside sipag.** Those are human-coordinating-with-human ceremonies; sipag is human-coordinating-with-an-agentic-fleet.
- **There are no assignees.** Ownership lives at the KR level. Whichever agent picks a task up runs it.
- **There are no "blocked" or "ready for review" lanes.** Those are kanban artifacts. If a KR is at risk, its stance turns yellow or red. That is the only signal.
- **There is no portfolio / program / roadmap layer above Objectives.** Above Objectives there is nothing. If you ever feel the need to add a layer, the move is *up* (decide what we are not optimizing for), not sideways into more management.

Each of these would make sipag superficially more "complete." Each would also pull the human back down into work the fleet should be doing.

---

## Frequently asked

**Is this a kanban tool?**
No. Sipag refuses to be a kanban tool by design — every list item in the "deliberately isn't" section above is a kanban affordance we won't build. You can use sipag without ever seeing a column, a lane, a swim, a backlog, or a velocity chart.

**What if the agents do the wrong thing?**
They will, regularly. The point is that the human catches it at the KR level — "this stance is yellow, the fleet's stuck on cookie-domain handling, here's the question" — and redirects with a sentence, not by reassigning tasks or rewriting tickets. The blast radius of any single agent's wrong direction is small because the human checks in at KR-stance granularity, not task granularity.

**How does sipag know what's important enough to surface?**
A small local LLM (by default, gemma running against a local ollama daemon — your private code never has to leave your machine) reads through what the fleet has been doing and decides. Each KR you write is also a system prompt for a background worker that reads through this lens. When the worker sees something material to that KR, it writes a note, a blocker, a question, or proposes a stance change. Nothing gets sent unless the worker thinks it matters. When you see a question, gemma flagged it; when you don't, gemma didn't.

That said: gemma will miss things. Lens-workers are pattern-matchers, not omniscient — when a worker doesn't think something matters, it won't surface. Two safety nets: you can always open the live attach to a session and watch directly, and KR stances are reviewable on whatever cadence you set (weekly is a reasonable default). When something gets missed, you'll usually see it as a KR drifting yellow without a corresponding question — the absence is itself a signal.

**Doesn't running gemma continuously cost a lot?**
Gemma is local — there's no per-token cost, just CPU/GPU on your own hardware. The bigger concern is *call volume*. Lens-workers don't fire on a wall-clock schedule regardless of activity; they fire on threshold-crossing events (a new permission request arrives, a session has been silent for a while, the corpus has accumulated enough new content to warrant a derivation pass). A quiet day costs essentially nothing. A busy day costs whatever your hardware can handle. With a GPU, gemma is near-free; on CPU-only hardware, the practical trade-off is observation latency vs. contention with your other work — both tunable.

**Can I see what the agents are actually doing?**
Yes. Each session has a live attach you can open — same as opening a terminal tab on the agent's shell. Watching is fine; you just shouldn't *have* to watch in order to know whether things are working. The KR stance + the surfaced notes are designed to tell you that without you having to attach.

**What if I want to micro-manage one task?**
You can — sipag still has the `sipag dispatch <task_id>` and similar mechanics underneath, and a board view in the TUI. The product opinion is that you mostly *shouldn't*, but the product doesn't get in your way when you do.

**How is this different from Linear / Jira / Asana?**
Those tools optimize for human-coordinating-with-human and add AI as a feature on top. Sipag does the inverse: it starts from "agents do execution; humans steer" and builds outward. The deliberate absences (no cards, no sprints, no standups) are the visible difference; the underlying architecture (gemma-driven lens-workers per Steering entry; insights surface up to the KR sidebar) is the load-bearing difference.

**How is this different from a generic AI dev assistant like Cursor or Copilot?**
Those operate at the keystroke or single-session level. Sipag operates at the *fleet* level — directing many agents over many objectives over weeks or months. It does not write code. It is the layer between the human's strategic intent and the agents that do the work.

**Where does the data live?**
Locally, under `~/.sipag/`. TOML files for the human-written content; a vector corpus for the agent-derived observations (Phase 1 work — see [`feature-matrix.md`](feature-matrix.md)); a file-backed pub/sub broker for the in-flight events. There is no SaaS dependency. You point sipag at a [katulong](https://github.com/Dorky-Robot/katulong) server for session management — that can be your own machine or a server on your mesh.

**What happens if I leave sipag running and walk away for a week?**
The fleet continues. Lens-workers continue deriving. Sipag accumulates insights in the corpus, surfaces what the workers think is material, lets the rest sit. When you come back, the KR sidebars tell you the story of the week. If something needed you and you weren't there, that KR's stance went yellow or red; if everything went well, it stayed green.

**Who is this for, really?**
Software teams (or solo developers) running an agentic dev fleet — i.e., multiple Claude / agent instances working in parallel on tracked work. Sipag is overkill if you're using AI assistance for autocomplete; sipag is the right shape if you're treating agents as semi-autonomous coworkers and need a way to direct them without coordinating them.

**Is sipag finished?**
No. Several capabilities described above are aspirational at this writing — see [`feature-matrix.md`](feature-matrix.md) for the per-capability scorecard (✅ / 🟡 / 🟧 / 🔴 / ⏳). The product *shape* is settled. The implementation is staged in phases per [`modules.md`](modules.md) §9.

**What's the alternative I should consider instead?**
Build your own coordination by hand using shell scripts + the existing Claude CLI + some glue. That's what most people are doing today. It works. It also doesn't compound — every project ends up with bespoke glue. Sipag is the bet that the glue is worth standardizing.

---

## What's in this repo's docs

Now that you have the narrative spine, here's where each supporting doc fits:

| Doc | Reads as |
|---|---|
| [`narrative.md`](narrative.md) | **You are here.** The product story; the spine everything else hangs on. |
| [`../VISION.md`](../VISION.md) | The strategic principles — why this shape, what we refuse to be |
| [`modules.md`](modules.md) | The architecture — DDD bounded contexts, lens-worker abstraction, phase queue |
| [`feature-matrix.md`](feature-matrix.md) | The capability scorecard — what's shipped, what's gapped, what's deliberately not in scope |
| [`getting-started.md`](getting-started.md) | The tactical how-to — install, register a project, dispatch a task |
| [`dispatch.md`](dispatch.md) | What actually happens when you click "Dispatch" — flow diagrams + file:line refs |
| [`cli-reference.md`](cli-reference.md) | Every command + flag |
| [`../README.md`](../README.md) | Repo-root overview + install instructions |
| [`../CLAUDE.md`](../CLAUDE.md) | Internal priming for Claude Code sessions working on sipag itself |
| [`as-katulong-tile.md`](as-katulong-tile.md), [`katulong-app-protocol.md`](katulong-app-protocol.md) | Adjacent: how sipag appears as a tile inside katulong, and katulong's wire protocol |
