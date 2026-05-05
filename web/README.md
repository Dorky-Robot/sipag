# sipag web

The browser frontend served by `sipag serve`. The page is rendered
server-side by maud templates in `sipag/src/serve/board_view.rs`; the
files in `public/` are static assets only.

## Stack

- **HTMX** for partial swaps (`htmx.min.js` is vendored — no npm).
- **Hand-written JS** in `public/js/` for the WebSocket transport
  (`transport.js`), live-DOM glue (`sipag-live.js`), and the in-page
  debug panel (`sipag-debug.js`).
- **Rust `sipag serve`** — axum + reqwest; proxies each katulong with
  `Authorization: Bearer <apiKey>` and serves this directory's
  `public/`.

No build step on this side. Edit a file in `public/`, refresh.

## One-time setup

```sh
# 1. Copy the hosts template and fill in one entry per katulong you
#    want sipag to manage.
mkdir -p ~/.sipag && cp extras/hosts.toml.example ~/.sipag/hosts.toml
chmod 600 ~/.sipag/hosts.toml    # it contains API keys

# Each katulong stores its own key at ~/.katulong/remote.json on that
# host. For a remote host:
#   ssh <your-host> 'jq -r .apiKey ~/.katulong/remote.json'
# For the local host:
#   jq -r .apiKey ~/.katulong/remote.json
# Paste each value into the matching `apiKey` field in hosts.toml.
```

## Run

```sh
cargo run -p sipag -- serve --port 7100
```

Open <http://localhost:7100>. You'll see live activity across every
host in `hosts.toml`, plus your projects and key results.

## History

This directory used to be a vanilla ClojureScript SPA built with
shadow-cljs. The htmx rewrite (commit on `spike/w1-htmx`) replaced
`web/src/sipag/app.cljs` with server-rendered maud templates so the
page works without a JS build step. See `docs/as-katulong-tile.md` for
the original Week-1 cljs design notes.
