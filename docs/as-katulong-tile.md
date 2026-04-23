# sipag as a katulong tile — working doc

Status: **draft**, revised as we ship.

## North star

Make something that works, then make it less stupid.

This doc is the plan for making sipag a first-class app inside katulong,
with a matching plan for alon as the second concrete example. Two real
integrations, built ugly first, cleaned up in place. No plugin SDK until
the shape of it is forced by real friction.

## Why we're being careful

katulong already tried a plugin system once (the 2026-03 tile SDK). It
was stripped in `333aee3` with zero reachable capability lost:

- `docs/tile-sdk.md` pitched manifest discovery from `~/.katulong/tiles/`,
  a `canClose()` hook, community-installable tiles. None had a loader.
  None had a caller. Grep at strip time found zero references.
- The `+` menu only ever created `{ type: "terminal" }`. The non-terminal
  branch was dead code.
- `tile-chrome`'s sidebar and shelf zones had zero runtime consumers —
  deleted with no behavior change.
- The 50-line tile registry dispatched between two hard-coded factories.
  Deleted. Replaced with two local closures in `app.js`.
- Heterogeneous grids were a plugin concept. Once plugins were cut,
  `cluster-tile` got *simpler* — every slot is a terminal.

The codebase has a consistent simplification gravity. Every abstraction
built for hypothetical future users got deleted. Examples:

| What | Why cut |
|---|---|
| self-managed TLS (`8160acc`) | external tunnels were enough; –6600 lines |
| three platform renderers (`d282567`) | one universal carousel worked everywhere |
| SimplePeer wrapper (`347803e`) | raw browser API, fewer bugs |
| `alon-composer` (`0ae4ec6`) | "abandon extra complexity with alon composer" |

The positive signal is just as strong. The APIs that *stuck* were
shaped by concrete use:

- **Alon (`808a3e0`)** — the posts/chat example drove `html()` /
  `style()`, enter-to-submit, CSS custom properties. The Jasmine specs
  tested plumbing; the example shaped the API.
- **Tala (`e33edd7`)** — dual-mode from day one: standalone server *and*
  embeddable, via runtime `configure()`, injectable auth, namespaced
  WebSockets. "Plugin-ready architecture avoided a later painful
  refactor by making embedding a first-class concern early."

Method for this reboot: **example-as-spec**. Build sipag and alon
integrations first. Extract the plugin surface from what they
actually needed. Not before.

## Why sipag first, not alon-viewer

Both are valid plugins. Ordering by leverage:

- **Sipag** turns three macs into one agentic fleet. Every hour without
  it is an hour we can't parallelize work across mini + prime + og —
  an hour cost paid *per active project, times three hosts*. With it,
  a single board dispatches workers anywhere on the mesh and streams
  their output back.
- **Alon-viewer** is a nicer way to read `.md` files.

So sipag ships first, even though it teaches us less about the plugin
pattern than alon-viewer would. We pay the pattern-discovery tax later.
Alon-viewer comes in as the **second** example that proves the extension
points generalize beyond sipag's shape — the moment we have two plugins
competing for the same surface is the moment it's safe to build it.

## The real shape: plugins as handlers, not apps

Earlier drafts of this plan assumed a plugin is a standalone app
launched in an iframe from the command palette. That's the easy case,
and it's wrong as the starting model. What we actually want is:

> If I install alon, opening a file in katulong's file browser gives me
> "Open with Alon" as an option, right next to the built-in document
> viewer.

A plugin isn't a silo; it's a **handler that participates in decisions
katulong already makes**. The file browser today has hardcoded logic
that picks a tile based on extension: `.md → document-tile`, `.png →
image-tile`, else download. That branch is an extension point in
hiding. The plugin system's job is to turn that branch into a registry
where the built-ins are just the default handlers and plugins can
register alternatives.

This is the VSCode "Open With…" model, not the VSCode "Extensions
Panel" model. Starts small, grows by extension point.

## The primitives that already exist

We don't need to invent the windowing system. It's there:

- **`/_proxy/<port>/`** — authenticated reverse proxy to any localhost
  port. Iframe-safe. (`a92b5f8`)
- **`localhost-browser` tile renderer** — iframes the proxy URL.
  Props: `{ port }`. Lives in `public/lib/tile-renderers/`.
- **File browser → tile dispatch** — the hardcoded `path → tileType`
  logic in the file browser. *This is one extension point we can
  turn into a registry.*
- **Tile/session picker** — `Cmd+/`, "Go to tile or session…" fuzzy
  finder. Items carry `{ id, label, kind, action }` and dispatch
  through `openCommandPicker`. Open tiles, closed managed sessions,
  and unmanaged tmux sessions are the current item sources. Adding
  plugin-supplied items here is a natural extension point.
- **Command mode** — `Cmd+.`, vim-style modal chord tree
  (`command-mode.js` + `command-tree.js`). Separate system from the
  picker. Don't touch this yet; the picker is the right launcher for
  plugin apps.
- **Tile state model** — reducer-driven store + renderer capabilities.
  (`cbe7a8f`)

A plugin = a local HTTP service (like Tala was built to be) + a small
manifest declaring which extension points it hooks. Katulong renders
its output in the iframe renderer that already exists.

## Week-by-week plan

Method: **build standalone with real data, then port into katulong.**
No mocked-fixture step. The three macs already run katulong and their
API keys already exist at `~/.katulong/remote.json` on each host —
mocking would just be ceremony. UX gets discovered cheaper by using
real output streams than by imagining what fake ones should look like.

### Week 1 — Real standalone sipag (vanilla ClojureScript, hits the live mesh)

No katulong integration yet. Just a browser app, a tiny backend, and
three real katulong servers on the other end.

1. **Stack: vanilla ClojureScript.** Same philosophical stance as
   Alon — platform primitives, no framework. `shadow-cljs` for the
   compile loop, direct DOM interop for rendering, `js/fetch` and
   `js/EventSource` for the network. No Reagent, no re-frame, no
   component-layering libraries.
2. `sipag serve --port 7100` — small backend (Bun is fine). It does
   three things:
   - serves the cljs bundle + static assets
   - proxies `/api/hosts/:id/crew/...` → that host's katulong,
     keeping api keys server-side so the browser never sees them
   - exposes `/events` — SSE fan-in merging `/crew/output` streams
     from all three hosts into one client stream
3. `~/.sipag/hosts.toml` — one entry per mac, api keys copied from
   each host's `~/.katulong/remote.json`:
   ```toml
   [[host]]
   id     = "mini"
   url    = "https://katulong-mini.felixflor.es"
   apiKey = "..."

   [[host]]
   id     = "prime"
   url    = "https://katulong-prime.felixflor.es"
   apiKey = "..."

   [[host]]
   id     = "og"
   url    = "https://katulong-og.felixflor.es"
   apiKey = "..."
   ```
4. Board UX, all against real data from day one:
   - projects + tasks from `sipag-core`
   - host swimlanes showing live `/crew/status` per host
   - click task → pick host → `POST /crew/spawn` on that host
   - live worker tail reading `/events` (SSE)
   - keyboard-first nav, command palette shape
5. Daily use: `sipag serve`, open `localhost:7100` in any browser.
   The agent manager is real, working, dispatching across three macs.

Katulong changes this week: **zero**. This is Booster 4 — the agentic
fleet works, wires exposed, port typed by hand.

### Week 2 — Port into katulong (first extension point: picker items)

The "port it in just like alon does" step. The standalone thing
exists; make it installable.

1. `~/.katulong/apps.toml`:
   ```toml
   [[app]]
   id   = "sipag"
   name = "Sipag"
   icon = "kanban"
   port = 7100
   ```
2. Katulong reads `apps.toml` at boot. `openTilePicker()` grows a
   new item source that emits one entry per `app`:
   ```js
   { id: "app:sipag", label: "Sipag", kind: "app", action: "open" }
   ```
3. The picker's `onPick` dispatcher handles `action: "open"` by
   adding a `localhost-browser` tile with that `app`'s port, title,
   and icon.
4. `Cmd+/ → type "sipag" → Enter` opens the board.

One extension point (picker items from a plugin source). Four
manifest fields, all demanded by sipag. Command mode (`Cmd+.`)
untouched.

### Week 3 — Second extension point: file handlers (sipag + alon-viewer)

Two plugins competing for the same surface is when it's safe to build
it. This week they arrive together.

1. **Refactor katulong's file-open dispatch** into a `fileHandlers`
   registry. The built-ins (document-tile, image-tile, etc.) become
   default registrations — no user-visible change on its own.
   ```js
   fileHandlers.register({
     id: "katulong.document",
     accepts: (path) => /\.(md|txt|json|toml|...)$/i.test(path),
     open:    (path) => openDocumentTile(path),
   });
   ```
2. **Extend the manifest** so plugins can register handlers:
   ```toml
   [[app]]
   id   = "sipag"
   port = 7100
   [[app.handles.files]]
   accepts = [".task.toml"]
   open    = "/task?path=${path}"

   [[app]]
   id   = "alon-viewer"
   name = "Alon Viewer"
   icon = "wave"
   port = 7200
   [[app.handles.files]]
   accepts = [".md", ".html", ".habi"]
   open    = "/view?path=${path}"
   ```
3. **Build `alon-viewer`** — small Node/Bun server with one endpoint,
   `GET /view?path=<file>`, returning HTML built with habiscript +
   tanaw + alon. Run it at port 7200.
4. **Sipag gains `/task?path=<file>`** — opens the task card view for
   a specific `.task.toml` instead of the whole board.
5. File browser now offers: `.task.toml` → **Open with Sipag**,
   `.md` → **Open with Alon Viewer**, and the built-ins remain for
   everything else. Right-click for "Open with…" submenu when
   multiple handlers accept.

The manifest has earned five fields (`id`, `name`, `icon`, `port`,
`handles.files`) because two real plugins needed exactly those.

### Week 4+ — friction-driven iteration

Predicted friction (to be replaced by actual friction as we feel it):

- Starting `sipag serve` / `alon-viewer` by hand → lifecycle
  (`entry.command`, `entry.healthcheck`) earns its way in
- `apps.toml` edited by hand → `katulong app install <path-or-url>`
  writes an entry
- Plugins die silently → status indicator in the palette and file
  browser
- No preview-vs-edit distinction → manifest grows `mode`
- Plugin needs a background task, not a tile → headless mode earns in

Each gets added as its own commit, justified by one named friction.

## Stop-signs (things we will NOT do first)

1. **Not a manifest spec up front — beyond what alon-viewer actually
   needs.** The 2026-03 SDK was specced before anyone wrote a plugin.
   The Week 2 manifest has four fields (`id`, `name`, `port`,
   `handles.files`) because alon-viewer needs exactly those. Every
   additional field has to be earned by a specific second plugin
   demanding it.
2. **Not a tile-type registry.** In this model there's only one new
   tile type (`localhost-browser`, already shipping). A plugin is a
   URL, not a JS module that gets dispatched by type.
3. **Not `~/.katulong/tiles/` manifest discovery.** That's the ghost of
   the old system — tempting because it sounds right. Won't have a
   loader for months; that's exactly how it dies.
4. **Not chrome zones (sidebar, shelf) for plugins.** The plugin owns
   its iframe; it draws its own UI. The old sidebar and shelf had zero
   callers before they were deleted.
5. **Not a marketplace or settings page.** Two plugins, edited in a
   TOML, for at least a year.
6. **Not a "trusted mode" / in-process SDK.** Defer hard. Iframe is
   fine until it provably isn't. When it does bite, we'll know exactly
   which capability to promote.
7. **Not calling this a "plugin system".** Call them what they are:
   "sipag integration", "alon playground integration". A system
   implies generality; generality is the failure mode we're avoiding.

## The Booster 4 → Booster 19 principle

The first sipag tile is going to look like Booster 4: visible wires,
manual port numbers, a hosts.toml edited by hand, `sipag serve` in a
terminal the user has to remember to start. Every engine is doing its
job, just ugly.

Each week after, we replace one wire with something cleaner. Same
function, fewer loose ends. By the time we get to Booster 19, someone
who walks up to it sees "an app that lives in katulong and manages
work across three macs." They don't see the pipeline of decisions
that got us there — and that's the point.

## Revisions log

- 2026-04-23: first draft. Captured history lessons from `333aee3`,
  `e33edd7`, `808a3e0`, `0ae4ec6`. Four-week plan sketched.
- 2026-04-23: reframed plugins as **handlers plugging into katulong's
  existing decision points**, not standalone apps in iframes. Week 1
  changed from "sipag serve + iframe" to "refactor file-browser open
  into a handler registry." alon-viewer moved to Week 2 as the first
  real plugin. sipag moved to Week 3 as the generalization test.
  Prompted by: "if we install alon we should be able to open our
  files through it instead of the built-in file viewer."
- 2026-04-23: re-sequenced by leverage. Sipag ships first
  (W1, zero katulong changes, iframe-only) to unlock agentic
  management across the mesh. Palette provider becomes the first
  extension point (W2, driven by sipag's friction). File handlers
  move to W3, where sipag + alon-viewer arrive together — two
  plugins demanding the same surface is when it's safe to build it.
  Prompted by: "prioritize sipag — this would unlock agentic
  management and multiply impact across projects."
- 2026-04-23: adopted Alon's **spike-then-port** method. W1 is now a
  standalone UI spike with mocked data (no mesh, no katulong), to
  nail the agent-manager UX cheaply. W2 wires the spike to real
  katulong APIs (still no katulong integration). W3 ports it into
  katulong via the palette provider. W4 adds file handlers with
  alon-viewer. Prompted by: "spin up a localhost spike of what our
  agent manager UI could look like first, then port into katulong
  just like alon does."
- 2026-04-23: dropped the mocked-fixture spike step and compressed
  W1+W2 into a single W1. The three katulong API keys are already
  available at `~/.katulong/remote.json` on each host, so mocks are
  ceremony. Also pinned the frontend stack to **vanilla
  ClojureScript** (shadow-cljs + direct DOM interop), matching Alon's
  no-framework posture instead of layering habiscript/tanaw/alon.
  Weeks renumbered: W2 = port to katulong via palette, W3 = file
  handlers + alon-viewer, W4+ = friction-driven. Prompted by: "we
  don't have to have a fake thing — we can get api keys from each
  katulong instance and interact for real; and just like alon, we
  should go vanilla ClojureScript instead of complicating the
  frontend."
- 2026-04-23: corrected katulong's launcher surface. The installed
  katulong (v0.58.7) binds **`Cmd+/`** to the fuzzy "Go to tile or
  session…" picker (`openTilePicker()` in `public/app.js`, items
  carry `{ id, label, kind, action }`) and **`Cmd+.`** to a separate
  vim-style command mode (`command-mode.js` + `command-tree.js`).
  Earlier drafts cited `Option+Space` from abandoned commit
  `5f34a8a`; not what ships. W2 now describes plugins as a new
  picker item source (`kind: "app"`, `action: "open"`), and the
  primitives section distinguishes the picker from command mode.
  Prompted by: user's "I invoke the command palette with Cmd+/"
  screenshot of the actual picker.
