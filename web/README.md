# sipag web — Week-1 agent-manager spike

Vanilla ClojureScript SPA, served by `sipag serve`. Displays live crew
status across the three-mac mesh defined in `~/.sipag/hosts.toml`.

## Stack

- **shadow-cljs** via npm — Java required (already present on macOS with
  Xcode; otherwise `brew install openjdk`).
- **Vanilla ClojureScript** — no Reagent, no re-frame, no libs. Direct
  DOM interop through `goog.dom`. One atom + `add-watch` → re-render.
- **Rust `sipag serve`** — axum + reqwest; proxies to each katulong with
  `Authorization: Bearer <apiKey>` and ships this directory's `public/`.

## One-time setup

```sh
# 1. Copy the hosts template and fill in API keys for mini/prime/og.
cp extras/hosts.toml.example ~/.sipag/hosts.toml

# On each host, pull the apiKey and paste it in:
#   ssh mini   'jq -r .apiKey ~/.katulong/remote.json'
#   ssh mac2024 'jq -r .apiKey ~/.katulong/remote.json'
#   ssh mac2019 'jq -r .apiKey ~/.katulong/remote.json'

# 2. Install shadow-cljs + its deps.
cd web
npm install
```

## Run (two shells)

```sh
# shell A — keep the cljs bundle fresh
cd web
npm run watch        # shadow-cljs watch app → public/js/app.js

# shell B — the backplane
cargo run -p sipag -- serve --port 7100
```

Open <http://localhost:7100>. You should see one column per host with
its projects and worker statuses. Everything is real — the browser
talks to a Rust server, which talks to three real katulongs.

## What this proves

- End-to-end pipe from a cljs SPA through a Rust proxy to live katulong
  APIs across the mesh.
- API keys never leave the Rust process.
- Zero katulong changes — the browser tile in katulong can point at
  <http://localhost:7100> today.

## What's deliberately missing (Booster 4 on purpose)

- No SSE fan-in of `/crew/output` — polling every 5s.
- No dispatch (`POST /crew/spawn`) — read-only.
- No projects/tasks/roles view — just what `/crew/status` returns.
- No katulong integration (`apps.toml`, picker entry) — that's Week 2.

See `docs/as-katulong-tile.md` for the week-by-week plan.
