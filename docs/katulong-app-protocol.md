# katulong-app — host/app handshake protocol

**Status**: draft for review
**Version**: 0.1
**Spec home (canonical)**: TBD `katulong/docs/katulong-app-protocol.md`
**Reference host implementation**: `katulong` (CLI + tile renderer)
**Reference app implementations**: `sipag`, `alon`

This is a proposed open contract for embedding **external web apps**
inside katulong as first-class tiles, while letting the apps run
independently as their own services on their own subdomains.

A katulong is the **host**. An external web app (sipag, alon, anything
else implementing this protocol) is the **app**. The protocol defines:

- How a host **discovers** an app (anonymous metadata fetch).
- How a host **installs** an app, with cryptographic proof of human
  consent on both sides via passkey assertions on each origin.
- How a host **renders** an installed app inside a tile.
- How an app **calls back** to its host for sanctioned actions
  (e.g., spawning crew sessions).
- How an app and host exchange **shortcuts and lifecycle events**
  via postMessage.
- How either side **uninstalls** the relationship.

The protocol is deliberately small: HTTP for discovery and install,
postMessage for runtime. No new daemons, no shared databases, no
auth federation. Each side keeps its own credential store; the
protocol just lets them sanction one specific machine-to-machine
relationship at a time.

---

## Goals and non-goals

**Goals**

- A host can install any compliant app with one CLI command — no
  config-file editing on either side, no copy-pasted secrets.
- An app can be installed on multiple hosts independently, each
  install creating a distinct trust relationship with its own keys.
- Human consent is proven on both origins via passkey, in one
  ~10-second ceremony.
- Apps stay first-class web apps with their own auth, their own
  deploy lifecycle, their own subdomain.
- The protocol is symmetric in spirit: any tool that publishes the
  app endpoints is installable; any tool that implements the host CLI
  is a valid host.

**Non-goals**

- Auth federation. Each side's user-facing auth (passkey login, etc.)
  remains independent. The protocol governs *machine-to-machine*
  authorization, not human-to-app session sharing.
- Plugin SDKs. The app does not run code inside the host process.
- Marketplace. App discovery is by URL; curation is the human's
  problem.
- Cross-host distribution of install state. Each host has its own
  `apps.toml`. Three macs, three independent installs.

---

## Roles

| Role     | Examples              | Owns                                                |
|----------|-----------------------|-----------------------------------------------------|
| **Host** | `katulong`            | The CLI driver; the picker; the tile renderer      |
| **App**  | `sipag`, `alon`       | The web app being embedded; declares its manifest  |

Both run as independent web services on independent subdomains
(e.g., `katulong-mini.felixflor.es`, `sipag.felixflor.es`). Both have
their own passkey login, credential store, and api-key system.

---

## Endpoints summary

The app publishes four endpoints under the standard well-known prefix:

```
GET    https://<app>/.well-known/katulong-app/manifest
POST   https://<app>/.well-known/katulong-app/intent-pull
GET    https://<app>/.well-known/katulong-app/install      ← user lands here
DELETE https://<app>/.well-known/katulong-app/install
GET    https://<app>/.well-known/katulong-app/health       (optional)
```

The host publishes one endpoint that apps fetch during install:

```
GET    https://<host>/.well-known/katulong-host/intent/:token
```

(Both well-known prefixes follow RFC 8615.)

---

## The double-passkey install ceremony

This is the protocol's central flow. The user is logged into both
`<host>` (with a host-side passkey) and (eventually) `<app>` (with
an app-side passkey). They want to install the app on this host.

### Step 1 — User initiates on host

```
$ katulong app install https://sipag.felixflor.es
```

Or: clicks "Install app" in the host UI, types the URL.

The host:

1. Requires a passkey assertion from the user (proves host's owner
   is acting). If they're already authenticated this session, no
   re-prompt; otherwise Face ID / fingerprint / etc.
2. Fetches `GET https://<app>/.well-known/katulong-app/manifest`.
   - On 4xx/5xx or wrong shape → abort, error message: "not a
     katulong-app: <reason>".
3. Mints a fresh api-key in its api-key store, named `app:<app.id>`.
4. Generates a fresh `intent_token` (random, 256 bits, single-use,
   5-minute TTL) and a fresh `state` (random, single-use, returned
   in callback to defeat CSRF).
5. Stores an **intent record** keyed by `intent_token`:

   ```json
   {
     "katulong": {
       "id": "mini",
       "url": "https://katulong-mini.felixflor.es",
       "version": "0.58.7"
     },
     "apiKey": "<the api-key for the app to call back with>",
     "scope": ["sessions:read", "sessions:exec"],
     "expires_at": "2026-04-25T19:35:00Z"
   }
   ```

6. Redirects the browser to:

   ```
   https://sipag.felixflor.es/.well-known/katulong-app/install
     ?host_url=https://katulong-mini.felixflor.es
     &intent_token=<token>
     &return_to=https://katulong-mini.felixflor.es/apps/install/callback
     &state=<state>
   ```

The api-key is **never** in the URL. It's pulled by the app from the
host's intent endpoint (step 3 below), over HTTPS.

### Step 2 — Browser arrives on app

The app's `/install` endpoint:

1. Validates `host_url` is a syntactically valid HTTPS URL.
2. Validates `return_to` shares the registered hostname of `host_url`
   (defeats redirect smuggling — a host can only ever redirect back
   to itself).
3. Server-to-server: `POST <host_url>/.well-known/katulong-host/intent/<intent_token>`
   - The HTTPS connection's certificate authenticates the host's
     identity. No additional signing needed; if the cert is valid for
     `host_url`, the response is from that host.
   - Response is the intent record from Step 1.5. App stores it
     in memory keyed by intent_token until the user confirms.
4. Renders consent UI:

   ```
   ┌───────────────────────────────────────┐
   │  Install Sipag in this katulong?      │
   │                                       │
   │  Host:  mini                          │
   │         katulong-mini.felixflor.es    │
   │                                       │
   │  This will allow:                     │
   │   • mini's katulong to embed Sipag    │
   │     as a tile                         │
   │   • Sipag to call mini's katulong     │
   │     for: sessions:read, sessions:exec │
   │                                       │
   │  [Confirm with Face ID]  [Cancel]     │
   └───────────────────────────────────────┘
   ```

5. On Confirm: requires a passkey assertion from the user *on the
   app's origin* (proves the app's owner is acting — the second of
   the double-passkey).
6. App writes a record to its registered-hosts store:

   ```toml
   # ~/<app>/katulongs.toml
   [[katulong]]
   id = "mini"
   url = "https://katulong-mini.felixflor.es"
   apiKey = "<from the intent record>"
   scope = ["sessions:read", "sessions:exec"]
   installed_at = "2026-04-25T19:30:14Z"
   ```

7. App POSTs back to host's intent endpoint to mark the intent
   consumed (defeats replay), then redirects browser to
   `return_to?state=<state>&result=ok`.

### Step 3 — Browser returns to host

The host's `/apps/install/callback`:

1. Validates `state` matches a recently issued state (single-use).
2. Validates the matching intent record was marked consumed by the
   app. If not, abort: the app didn't follow the protocol.
3. Writes `apps.toml`:

   ```toml
   # ~/.katulong/apps.toml
   [[app]]
   id = "sipag"
   name = "Sipag"
   icon = "kanban"
   url = "https://sipag.felixflor.es"
   apikey_id = "8b6931dc..."   # references ~/.katulong/api-keys/
   shortcuts = ["openPicker"]
   installed_at = "2026-04-25T19:30:14Z"
   ```

4. Renders success: `✓ Sipag installed. Press Cmd+/ → "sipag" → ▷ live.`

End-to-end: ~10 seconds, two passkey assertions, no copy-paste.

---

## App manifest format

`GET /.well-known/katulong-app/manifest` returns JSON:

```json
{
  "protocol": "katulong-app/1",
  "app": {
    "id":      "sipag",
    "name":    "Sipag",
    "icon":    "kanban",
    "version": "3.0.10"
  },
  "embed": {
    "url":       "https://sipag.felixflor.es",
    "shortcuts": ["openPicker"]
  },
  "requires": ["sessions:read", "sessions:exec"],
  "endpoints": {
    "install":   "/.well-known/katulong-app/install",
    "uninstall": "/.well-known/katulong-app/install",
    "intent":    "/.well-known/katulong-app/intent-pull",
    "health":    "/.well-known/katulong-app/health"
  }
}
```

**Field semantics**

- `protocol` — protocol version. Hosts MUST refuse to install apps
  whose major version they don't implement.
- `app.id` — globally identifying string within a host's apps.toml.
  Lowercase, kebab-case. Hosts SHOULD refuse install if `app.id` is
  already present in apps.toml.
- `app.icon` — Phosphor icon name (or other host-supported icon set).
  Renders in the picker.
- `embed.url` — the URL the host iframes when the app is opened.
  May differ from the install URL if the app wants its install
  endpoint hosted separately. Same-origin recommended.
- `embed.shortcuts` — the postMessage shortcut actions the app
  forwards (see "Runtime postMessage" below).
- `requires` — capability strings the app needs from the host. The
  host's CLI MUST surface these to the user during install consent.
- `endpoints` — paths under the same origin. Useful if implementations
  want to host endpoints on different paths.

---

## Intent record

Stored short-term (≤5 min TTL) on the host, fetched once by the app.

```json
{
  "katulong": {
    "id":      "mini",
    "url":     "https://katulong-mini.felixflor.es",
    "version": "0.58.7"
  },
  "apiKey":   "<api-key the app uses to call the host>",
  "scope":    ["sessions:read", "sessions:exec"],
  "expires_at": "2026-04-25T19:35:00Z",
  "consumed":   false
}
```

Pulled by the app via:

```
POST https://<host>/.well-known/katulong-host/intent/<intent_token>
Body: {}
→ 200 { ...intent record... }
→ 410 Gone if expired or already consumed
→ 404 if token unknown
```

(POST instead of GET: avoids accidental browser caching, and lets us
update `consumed=true` server-side as a side effect — though we may
prefer a separate POST to mark consumed; see open questions.)

---

## Embedding (runtime)

After install, the host can render the app as a tile.

### Tile renderer

A new built-in renderer `external-app` (modeled on `localhost-browser`):

```js
// public/lib/tile-renderers/external-app.js
export const externalAppRenderer = {
  type: "external-app",
  describe(props) { return { title: props.name, icon: props.icon, ... } },
  mount(el, { props, dispatch, ctx }) {
    const iframe = document.createElement("iframe");
    iframe.src = props.url;
    iframe.setAttribute(
      "sandbox",
      "allow-same-origin allow-scripts allow-forms allow-popups"
    );
    el.appendChild(iframe);
    // shortcut postMessage handler installed at app.js boot
  }
};
```

Picker grows a new item kind `"app"` reading directly from `apps.toml`.

### Runtime postMessage

The protocol uses `window.postMessage` for app↔host runtime calls.
All messages have shape `{ type: "katulong.<kind>", ... }`.

**App → host: forward a shortcut**

```js
window.parent.postMessage(
  { type: "katulong.shortcut", action: "openPicker" },
  hostOrigin   // pulled from a query param on the iframe URL or
               // an environment endpoint exposed by the host
);
```

The host listens on its own window, validates `event.origin` matches
an installed app's origin, validates `action` is in the app's
declared `embed.shortcuts`, and dispatches the matching action.

**Host → app: lifecycle event** (planned, future)

```js
iframe.contentWindow.postMessage(
  { type: "katulong.event", event: "blur" },
  appOrigin
);
```

For an MVP, only the app→host direction is required. Host→app can
ship later.

---

## Calls (host ↔ app over HTTP)

### App calls host

The app uses the api-key it received during install:

```
GET https://<host>/sessions
Authorization: Bearer <apiKey>
```

The host's existing api-key auth path validates the bearer, checks
the key's scope against the requested route, and either serves or
401s. **No new auth code on host's side** — this reuses the host's
existing api-key system.

### Host calls app

In the MVP, the host doesn't make API calls to the app. It just
iframes `embed.url` and forwards shortcuts. If a future feature
needs host→app HTTP calls, we add a symmetric `apiKey` direction
(app issues a key during install for the host to use).

---

## Uninstall

Symmetric, can be initiated from either side.

### From host

```
$ katulong app uninstall sipag
```

- Host requires passkey assertion (cheap; just the host owner's).
- Host calls `DELETE https://<app>/.well-known/katulong-app/install`
  with the app's `apiKey` as bearer auth.
- App removes its `katulongs.toml` row for this katulong, returns 204.
- Host revokes the api-key, removes the apps.toml row.

### From app

```
$ sipag katulong remove mini
```

- App requires passkey assertion (app owner's).
- App calls `DELETE https://<host>/<host's app uninstall path>`
  with the host's `apiKey` as bearer auth.
- The exact uninstall endpoint on the host is part of the host's
  spec; reference (katulong): `DELETE /api/apps/:app_id`.
- Host revokes the api-key, removes the apps.toml row.

Either side may end the relationship; the other side reflects it on
its next refresh / health check / call attempt.

---

## Health (optional)

```
GET https://<app>/.well-known/katulong-app/health
→ 200 { "ok": true, "version": "3.0.10" }
```

Hosts MAY ping this to mark apps "up" or "unreachable" in their
picker. Not required.

---

## Storage schemas

### Host: `~/.katulong/apps.toml`

```toml
[[app]]
id           = "sipag"
name         = "Sipag"
icon         = "kanban"
url          = "https://sipag.felixflor.es"
apikey_id    = "8b6931dc..."          # ref into ~/.katulong/api-keys/
shortcuts    = ["openPicker"]
installed_at = "2026-04-25T19:30:14Z"
```

The api-key value lives in the existing api-keys store, never in
`apps.toml`.

### App: `~/<app>/katulongs.toml` (e.g., `~/.sipag/katulongs.toml`)

```toml
[[katulong]]
id           = "mini"
url          = "https://katulong-mini.felixflor.es"
apiKey       = "<secret>"             # mode 0600
scope        = ["sessions:read", "sessions:exec"]
installed_at = "2026-04-25T19:30:14Z"
```

The api-key is a secret in the file — `chmod 600` enforced. App's
`serve` process loads these into memory at boot; the value is
never exposed to its own browser surface.

---

## Security model

### What's protected

- **Human intent on both origins.** A passkey assertion at the host's
  origin and a separate one at the app's origin during install
  proves the same device's authenticators are in use, and that the
  human visited both pages within a 5-minute window.
- **CSRF on the redirect.** The host's `state` param is single-use
  and validated on callback.
- **Token replay.** Intent tokens are single-use and TTL'd to 5 min;
  marked consumed once pulled.
- **Cross-origin postMessage abuse.** Hosts validate `event.origin`
  against installed apps' origins, and the action against the
  declared shortcuts.

### What isn't (and why)

- **Stolen api-keys.** If the app's `katulongs.toml` is exfiltrated,
  the attacker can call the host's API as the app. Mitigation: file
  is mode 0600; key can be rotated by the host (open question:
  rotation flow).
- **Compromised origin TLS.** If `<app>` or `<host>` is MitM'd, the
  install can be tampered. Mitigated only by the underlying TLS PKI;
  out of scope for this protocol.
- **Compromised client device.** Both passkeys live on the same
  device by default. If that device is compromised, both ends are
  compromised. WebAuthn cross-device flows mitigate (e.g., Mac with
  iPhone passkey via Bluetooth). Out of scope here.

### Threat model summary

The protocol assumes:

- HTTPS is intact between all parties.
- The user controls the device performing the assertions.
- The host's api-key store has standard secret hygiene.
- The app's `katulongs.toml` has 0600 perms.

It does not assume any pre-shared key, any third-party identity
provider, or any centralized registry.

---

## Versioning

`protocol` field in the manifest is the contract. Reference:
`katulong-app/1` is this document. Future `/2` changes are evaluated
case-by-case for backwards compatibility:

- Adding optional manifest fields → minor, no version bump.
- Adding a new endpoint → minor, no version bump if hosts can ignore
  unknown endpoints.
- Changing required HTTP semantics → major version bump.

A host MUST refuse install if `protocol` major version is unknown.

---

## Open questions for review

1. **Intent endpoint host path.** Currently
   `/.well-known/katulong-host/intent/:token`. Should this be on the
   katulong-app prefix (since it's part of the app handshake) or
   separate (since it's host-side state)?
2. **Consume vs pull semantics.** Should the intent record be marked
   consumed when pulled, or only when the app POSTs back after consent?
   The latter is more robust against accidental cancellation but adds
   a round trip.
3. **Capability strings.** The `requires` field is a list of strings
   like `sessions:read`. Should these be standardized in this spec or
   left as host-specific? Probably host-specific; the spec describes
   the field shape, not the vocabulary.
4. **Key rotation.** No flow defined yet. Add a `POST .../install/rotate`
   endpoint that mints a new api-key and invalidates the old, with
   passkey re-assertion required?
5. **Multiple installs of same app on one host.** Should we allow it
   (different scopes / tile configs)? Current design refuses — first
   install wins, second collides on `app.id`.
6. **Cross-device install.** What happens if the user is on a device
   without an authenticator for one of the origins? Currently: install
   fails. WebAuthn cross-device flows (BLE-paired phone) work
   transparently if available, no spec change needed.
7. **App→host calls without an api-key.** Should the app be able to
   make some calls anonymously (e.g., a public manifest endpoint
   on the host)? Probably yes; the current spec doesn't require
   bearer auth on the host's `/intent/:token` endpoint, since the
   token itself is a capability.
8. **Tile renderer scope.** Should the `external-app` renderer be the
   only one, or should apps be able to declare an alternate renderer
   (e.g., split-screen, popout, fullscreen-only)? Current design:
   one renderer, one shape. Apps can change their *embed.url* but not
   the rendering itself.

---

## Appendix A — Reference flow diagram

```
 ┌─ user initiates install on host ──────────────────────────┐
 │                                                            │
 │  $ katulong app install https://sipag.felixflor.es         │
 │                                                            │
 │  ┌─ Face ID #1 (host origin) ────────────────────────────┐ │
 │  │  prove katulong's owner is acting                     │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ┌─ host pre-flight ────────────────────────────────────┐  │
 │  │  • GET app/manifest                                   │  │
 │  │  • mint api-key  → store                              │  │
 │  │  • create intent record  → store                      │  │
 │  │  • generate state                                      │  │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ┌─ redirect ────────────────────────────────────────────┐ │
 │  │  GET app/install?host_url=…&intent_token=…&state=…    │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │              ↓                                             │
 │                                                            │
 │  ┌─ app receives redirect ──────────────────────────────┐ │
 │  │  • validate host_url + return_to                       │ │
 │  │  • POST host/intent/<token>  → fetch intent record    │ │
 │  │  • render consent screen                               │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ┌─ Face ID #2 (app origin) ────────────────────────────┐ │
 │  │  prove app's owner is acting                          │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ┌─ app commits ─────────────────────────────────────────┐ │
 │  │  • write katulongs.toml row                            │ │
 │  │  • POST host/intent/<token>/consume                    │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ┌─ redirect back ───────────────────────────────────────┐ │
 │  │  GET host/apps/install/callback?state=…&result=ok      │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ┌─ host commits ────────────────────────────────────────┐ │
 │  │  • validate state                                       │ │
 │  │  • write apps.toml row                                  │ │
 │  │  • render success                                        │ │
 │  └────────────────────────────────────────────────────────┘ │
 │                                                            │
 │  ✓ Sipag installed. Cmd+/ → "sipag" → live tile.           │
 └────────────────────────────────────────────────────────────┘
```

## Appendix B — Sequence diagrams (text)

Numbered message sequences for the four major flows. The vertical
flow in Appendix A is the user-facing ceremony; these are the
engineer-facing message exchanges. Plain text so they render in any
markdown viewer; we can add a Mermaid sibling appendix later when
diagram rendering is available.

### B.1 Install — the double-passkey ceremony

Participants: **User** · **Browser** · **Host** (katulong) · **App** (sipag)

```
 1. User    → Host        POST /apps/install (url)
 2. Host    → User        prompt passkey (host origin)
 3. User    → Host        assertion (Face ID #1) — host owner proven
 4. Host    → App         GET /.well-known/katulong-app/manifest
 5. App     → Host        200 { manifest }
 6. Host                  mint apiKey, create intent record, generate state
 7. Host    → Browser     302 → <app>/.well-known/katulong-app/install
                                ?intent_token=…&state=…
                                 &return_to=…&host_url=…
 8. Browser → App         GET <app>/.well-known/katulong-app/install?…
 9. App     → Host        POST <host>/.well-known/katulong-host/intent/<token>
10. Host    → App         200 { intent record + apiKey }
11. App     → User        render consent screen
12. User    → App         Confirm
13. App     → User        prompt passkey (app origin)
14. User    → App         assertion (Face ID #2) — app owner proven
15. App                   write ~/<app>/katulongs.toml row
16. App     → Host        POST <host>/.well-known/katulong-host/intent/<token>/consume
17. Host    → App         204
18. App     → Browser     302 → <return_to>?state=…&result=ok
19. Browser → Host        GET <host>/apps/install/callback?state=…&result=ok
20. Host                  validate state, confirm intent consumed,
                          write ~/.katulong/apps.toml row
21. Host    → User        ✓ Sipag installed
```

### B.2 Uninstall — initiated from host

Participants: **User** · **Host** (katulong) · **App** (sipag)

```
1. User → Host       katulong app uninstall sipag
2. Host → User       prompt passkey (host owner only)
3. User → Host       assertion
4. Host → App        DELETE <app>/.well-known/katulong-app/install
                     Authorization: Bearer <apiKey>
5. App               remove ~/<app>/katulongs.toml row for this host
6. App  → Host       204
7. Host              revoke apiKey, remove ~/.katulong/apps.toml row
8. Host → User       ✓ sipag uninstalled
```

### B.3 Runtime call — app dispatches via host

The "live" path the install enables. After install, the app's
backend uses the stored apiKey to call the host's API on behalf of
a user action in the embedded app UI.

Participants: **User** · **App UI** (iframe) · **App server** · **Host**

```
1. User    → App UI      clicks ▷ dispatch on task #N
2. App UI  → App server  POST /api/projects/<p>/tasks/<N>/dispatch
3. App server            load task, role; pick host
4. App     → Host        POST /sessions { name }
                         Authorization: Bearer <apiKey>
5. Host    → App         200 { id, name }
6. App     → Host        POST /sessions/by-id/<id>/exec { input }
                         Authorization: Bearer <apiKey>
7. Host    → App         200
8. App                   move task to in-progress
9. App     → App UI      200 { task, host, session_name }
10. App UI → User        toast "dispatched #N on mini"
```

### B.4 Runtime shortcut — postMessage from app to host

The mechanism that fixes the iframe-eats-keyboard problem cleanly:
the embedded app re-publishes known shortcuts as postMessage events;
the host listens, validates origin and action, and runs them.

Participants: **User** · **App UI** (iframe at app origin) · **Host** (parent window)

```
1. User   → App UI    presses Cmd+/
2. App UI → Host      window.parent.postMessage(
                        { type: "katulong.shortcut", action: "openPicker" },
                        hostOrigin)
3. Host               validate event.origin against installed apps' origins,
                      validate action against embed.shortcuts
4. Host               openTilePicker()
5. Host  → User       picker visible
```

---

## Appendix C — Reference error responses

All endpoints return JSON errors with shape `{ "error": "<code>", "detail": "<human-readable>" }`.

| Code                         | When                                                     |
|------------------------------|----------------------------------------------------------|
| `manifest.invalid`           | Host rejected app's manifest shape                       |
| `manifest.protocol-mismatch` | Host doesn't implement the app's protocol version        |
| `intent.expired`             | App tried to pull an intent_token past its TTL           |
| `intent.consumed`            | App tried to pull an already-consumed token              |
| `intent.unknown`             | App sent an unrecognized intent_token                    |
| `state.invalid`              | Host received a callback with bad/expired state          |
| `consent.denied`             | App's consent UI returned cancel                         |
| `app.duplicate`              | Host already has an installed app with this app.id       |
| `auth.unauthorized`          | Bearer apiKey missing, expired, revoked, or out of scope |
| `host.unreachable`           | App couldn't reach host's intent endpoint                |

---

## Appendix D — Implementation order (suggested)

1. **B0 (this doc)** — settle the spec on review.
2. **App side: sipag implements the four endpoints** + a small
   `katulongs.toml` reader, in parallel with native passkey login
   (Track A).
3. **Host side: katulong CLI** — `app install`, `app uninstall`,
   `app list`, plus the intent endpoint.
4. **Host side: katulong UI** — picker source from `apps.toml`,
   the `external-app` tile renderer, the postMessage shortcut handler.
5. **alon implements the protocol** — second concrete app validates
   the spec generalizes.
6. **Optional later**: `app install --rotate`, cross-host install
   helpers, capability-scoped api-keys.

This doc is the artifact (1) — once it reads cleanly to both the
intended host author and the intended app author, the rest is
mechanical.
