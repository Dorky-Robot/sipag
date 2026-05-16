# tests-web — Playwright suite for the notebook UI

End-to-end tests for `katulong-client serve`'s notebook page. The
`webServer` config spawns the binary (which itself spawns a real
katulong subprocess), and each test drives the page in a real
Chromium so behavior is asserted at the layer a developer
actually interacts with.

## Prerequisites

- Node + npm (the project already requires them for katulong itself).
- A katulong checkout — set `KATULONG_REPO=/path/to/katulong`.
- Tmux on PATH (katulong's PTY backend).

## First time

```sh
cd katulong-client/tests-web
npm install
npx playwright install chromium
```

## Running

```sh
KATULONG_REPO=~/Projects/dorky_robot/katulong npm test          # headless
KATULONG_REPO=~/Projects/dorky_robot/katulong npm run test:headed   # see the browser
KATULONG_REPO=~/Projects/dorky_robot/katulong npm run test:ui   # interactive UI mode
```

The first run compiles `katulong-client` (~30s); subsequent runs are
near-instant. Total suite runtime ≈ 15-20s once cached.

## What's covered

- Page loads, all 7 cells render, header populates.
- Each cell's Play button drives its respective library call:
  create → paste → press → wait-for → lines → sessions → close.
- The "exactly one session after create" assertion regresses the
  PR #534 phantom-session-spawn bug at the UI layer.
- The "close terminates" assertion regresses the close-deadlock.
- The "live-view reflects rolling buffer" assertion catches
  regressions in the polled `/api/lines` viewer.

## Test isolation

Tests share a long-lived `serve` instance for speed. `beforeEach`
calls `POST /api/reset` to close the persistent attach AND delete
every session on the underlying katulong, so we don't accumulate
towards katulong's `MAX_SESSIONS=20` cap.
