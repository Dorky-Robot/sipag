// In-page debug panel.
//
// Reshaped around the polling paradigm: shows what's actually
// happening — HTMX requests, polled fragments, WS events when the
// socket happens to work, and how fresh the data is. Useful from a
// device that has no browser inspector.
//
// Floating 🐞 button bottom-right. Tap to slide a panel in from the
// right.
(function () {
  const PANEL_ID = "sipag-debug-panel";
  const TOGGLE_ID = "sipag-debug-toggle";
  const STORAGE_KEY = "sipag-debug-state";
  const MAX_ACTIVITY = 80;
  const STALE_AFTER_MS = 12_000; // 12s with no successful update = stalled

  const state = {
    open: false,
    activity: [],          // newest-first
    lastSuccessAt: null,   // wall-clock ms of most recent successful update
    inflight: 0,
    wsConnected: false,
    wsUrl: null,
    pollsByUrl: new Map(), // url → {count, lastStatus, lastDur}
  };

  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || "{}");
    if (saved.open) state.open = true;
  } catch (_) {}

  function persist() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ open: state.open }));
    } catch (_) {}
  }

  // ── styles ──────────────────────────────────────────────────────
  const style = document.createElement("style");
  style.textContent = `
    #${TOGGLE_ID} {
      position: fixed; bottom: 16px; right: 16px;
      width: 44px; height: 44px; border-radius: 22px;
      border: 1px solid #262c34; background: #14181d;
      color: #7aa2f7; font: 600 14px ui-monospace, monospace;
      cursor: pointer; z-index: 9999;
      box-shadow: 0 4px 12px rgba(0,0,0,0.35);
    }
    #${TOGGLE_ID}.live { color: #9ece6a; }
    #${TOGGLE_ID}.stale { color: #e0af68; }
    #${TOGGLE_ID}.err { color: #f7768e; }
    #${PANEL_ID} {
      position: fixed; top: 0; right: 0; bottom: 0;
      width: min(440px, 100vw); transform: translateX(100%);
      transition: transform 0.18s ease-out;
      background: #0e1013; color: #e6e8eb;
      border-left: 1px solid #262c34; z-index: 9998;
      display: flex; flex-direction: column;
      font: 13px/1.45 ui-sans-serif, system-ui, sans-serif;
    }
    #${PANEL_ID}.open { transform: translateX(0); }
    #${PANEL_ID} header {
      padding: 12px 16px; border-bottom: 1px solid #262c34;
      display: flex; align-items: center; gap: 8px;
    }
    #${PANEL_ID} header h2 { font-size: 14px; font-weight: 600; margin: 0; flex: 1; }
    #${PANEL_ID} header button {
      background: transparent; border: 1px solid #262c34;
      color: #9aa3ae; padding: 4px 10px; border-radius: 4px;
      font: inherit; font-size: 12px; cursor: pointer;
    }
    #${PANEL_ID} .body { flex: 1; overflow-y: auto; padding: 12px 16px; }
    #${PANEL_ID} section { margin-bottom: 18px; }
    #${PANEL_ID} h3 {
      font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em;
      color: #9aa3ae; font-weight: 600; margin: 0 0 8px;
    }
    #${PANEL_ID} .pill {
      display: inline-flex; gap: 6px; align-items: center;
      padding: 3px 8px; border-radius: 11px;
      font-family: ui-monospace, monospace; font-size: 11px;
      background: #14181d; border: 1px solid #262c34;
    }
    #${PANEL_ID} .pill .dot {
      display: inline-block; width: 7px; height: 7px; border-radius: 4px;
    }
    #${PANEL_ID} .pill.live  .dot { background: #9ece6a; }
    #${PANEL_ID} .pill.stale .dot { background: #e0af68; }
    #${PANEL_ID} .pill.err   .dot { background: #f7768e; }
    #${PANEL_ID} .meta {
      display: grid; grid-template-columns: auto 1fr; gap: 4px 10px;
      font-family: ui-monospace, monospace; font-size: 11.5px;
      color: #9aa3ae; margin-top: 8px;
    }
    #${PANEL_ID} .meta .k { color: #9aa3ae; }
    #${PANEL_ID} .meta .v { color: #c0caf5; word-break: break-all; }
    #${PANEL_ID} ul.activity {
      list-style: none; margin: 0; padding: 0;
      max-height: 60vh; overflow-y: auto;
      border: 1px solid #14181d; border-radius: 4px;
      background: #14181d;
    }
    #${PANEL_ID} li.act {
      padding: 6px 10px; border-bottom: 1px solid #0e1013;
      font-family: ui-monospace, monospace; font-size: 11px;
      display: grid; grid-template-columns: auto 1fr;
      gap: 8px; align-items: baseline;
    }
    #${PANEL_ID} li.act:last-child { border-bottom: none; }
    #${PANEL_ID} li.act .glyph {
      width: 16px; text-align: center;
    }
    #${PANEL_ID} li.act .ts { color: #9aa3ae; font-size: 10.5px; }
    #${PANEL_ID} li.act .text { word-break: break-word; color: #c0caf5; }
    #${PANEL_ID} li.act.req .glyph { color: #7aa2f7; }
    #${PANEL_ID} li.act.res .glyph { color: #9ece6a; }
    #${PANEL_ID} li.act.err .glyph { color: #f7768e; }
    #${PANEL_ID} li.act.ws  .glyph { color: #bb9af7; }
    #${PANEL_ID} li.act.err .text { color: #f7768e; }
    #${PANEL_ID} .probes { display: flex; gap: 6px; flex-wrap: wrap; }
    #${PANEL_ID} .probes button {
      background: #14181d; border: 1px solid #262c34;
      color: #7aa2f7; padding: 5px 10px; border-radius: 4px;
      font: inherit; font-family: ui-monospace, monospace;
      font-size: 11.5px; cursor: pointer;
    }
    #${PANEL_ID} .probes button:hover { border-color: #7aa2f7; }
  `;
  document.head.appendChild(style);

  // ── DOM ─────────────────────────────────────────────────────────
  const toggle = document.createElement("button");
  toggle.id = TOGGLE_ID;
  toggle.textContent = "🐞";
  toggle.title = "sipag debug panel";
  document.body.appendChild(toggle);

  const panel = document.createElement("div");
  panel.id = PANEL_ID;
  panel.innerHTML = `
    <header>
      <h2>sipag · debug</h2>
      <button data-act="close">close</button>
    </header>
    <div class="body">
      <section class="status">
        <h3>live updates</h3>
        <span class="pill" id="dbg-mode"><span class="dot"></span><span class="label">…</span></span>
        <div class="meta">
          <span class="k">freshness</span><span class="v" id="dbg-fresh">unknown</span>
          <span class="k">in-flight</span><span class="v" id="dbg-inflight">0</span>
          <span class="k">ws url</span><span class="v" id="dbg-wsurl">—</span>
          <span class="k">ws state</span><span class="v" id="dbg-wsstate">—</span>
        </div>
      </section>
      <section class="probes-section">
        <h3>probes</h3>
        <div class="probes">
          <button data-probe="board">board</button>
          <button data-probe="ticker">ticker</button>
          <button data-probe="attention">attention</button>
          <button data-probe="topics">topics</button>
          <button data-probe="repos">repos</button>
        </div>
      </section>
      <section class="poll-stats">
        <h3>poll endpoints</h3>
        <ul class="activity" id="dbg-polls"><li class="act"><span class="glyph">·</span><span class="text">no polls yet</span></li></ul>
      </section>
      <section class="activity-section">
        <h3>activity <span style="color:#9aa3ae;font-weight:400;text-transform:none;letter-spacing:0">(newest first, capped at ${MAX_ACTIVITY})</span></h3>
        <ul class="activity" id="dbg-activity"><li class="act"><span class="glyph">·</span><span class="text">waiting…</span></li></ul>
      </section>
    </div>
  `;
  document.body.appendChild(panel);
  if (state.open) panel.classList.add("open");

  const modeEl = panel.querySelector("#dbg-mode");
  const freshEl = panel.querySelector("#dbg-fresh");
  const inflightEl = panel.querySelector("#dbg-inflight");
  const wsUrlEl = panel.querySelector("#dbg-wsurl");
  const wsStateEl = panel.querySelector("#dbg-wsstate");
  const pollsList = panel.querySelector("#dbg-polls");
  const activityList = panel.querySelector("#dbg-activity");

  // ── helpers ─────────────────────────────────────────────────────
  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
  }

  function shortPath(url) {
    try {
      const u = new URL(url, location.href);
      return u.pathname + (u.search || "");
    } catch {
      return String(url);
    }
  }

  function timeShort(ms) {
    const d = new Date(ms);
    return d.toTimeString().slice(0, 8);
  }

  function relativeFreshness(ms) {
    if (!ms) return "unknown";
    const age = Date.now() - ms;
    if (age < 1000) return "just now";
    if (age < 60_000) return Math.round(age / 1000) + "s ago";
    if (age < 3_600_000) return Math.round(age / 60_000) + "m ago";
    return Math.round(age / 3_600_000) + "h ago";
  }

  function pushActivity(kind, text) {
    state.activity.unshift({ kind, text, ts: Date.now() });
    if (state.activity.length > MAX_ACTIVITY) state.activity.length = MAX_ACTIVITY;
    if (state.open) renderActivity();
  }

  function renderActivity() {
    if (!state.activity.length) {
      activityList.innerHTML = `<li class="act"><span class="glyph">·</span><span class="text">waiting…</span></li>`;
      return;
    }
    const glyphFor = (k) =>
      k === "req" ? "→" : k === "res" ? "←" : k === "ws" ? "📡" : k === "err" ? "⚠" : "·";
    activityList.innerHTML = state.activity
      .map(
        (a) =>
          `<li class="act ${a.kind}"><span class="glyph">${glyphFor(a.kind)}</span>` +
          `<div><div class="ts">${timeShort(a.ts)}</div>` +
          `<div class="text">${escapeHtml(a.text)}</div></div></li>`
      )
      .join("");
  }

  function renderStatus() {
    const live = state.wsConnected;
    const stale = !live && (!state.lastSuccessAt || Date.now() - state.lastSuccessAt > STALE_AFTER_MS);
    const recentErrors = state.activity.slice(0, 6).some((a) => a.kind === "err");

    let label, cls;
    if (live) { label = "websocket"; cls = "live"; }
    else if (recentErrors && stale) { label = "errors · stalled"; cls = "err"; }
    else if (stale) { label = "stalled (no recent updates)"; cls = "err"; }
    else { label = "polling"; cls = "live"; }

    modeEl.className = "pill " + cls;
    modeEl.querySelector(".label").textContent = label;
    toggle.classList.remove("live", "stale", "err");
    toggle.classList.add(cls);

    freshEl.textContent = relativeFreshness(state.lastSuccessAt);
    inflightEl.textContent = String(state.inflight);
    wsUrlEl.textContent = state.wsUrl || "—";
    wsStateEl.textContent = live ? "connected" : "—";
  }

  function renderPolls() {
    if (!state.pollsByUrl.size) {
      pollsList.innerHTML = `<li class="act"><span class="glyph">·</span><span class="text">no polls yet</span></li>`;
      return;
    }
    const rows = [];
    for (const [url, info] of state.pollsByUrl) {
      const status = info.lastStatus;
      const ok = status >= 200 && status < 400;
      const cls = info.lastWasError ? "err" : ok ? "res" : "req";
      const glyph = ok ? "✓" : "✗";
      rows.push(
        `<li class="act ${cls}"><span class="glyph">${glyph}</span>` +
          `<div><div class="text">${escapeHtml(shortPath(url))}</div>` +
          `<div class="ts">${info.count} polls · last ${status || "—"}` +
          (info.lastDur != null ? ` · ${info.lastDur}ms` : "") +
          `</div></div></li>`
      );
    }
    pollsList.innerHTML = rows.join("");
  }

  // ── HTMX request lifecycle ──────────────────────────────────────
  // beforeRequest fires for every HTMX-driven fetch (poll or click).
  document.body.addEventListener("htmx:beforeRequest", (ev) => {
    const url = ev.detail?.requestConfig?.path || ev.detail?.pathInfo?.requestPath;
    if (!url) return;
    state.inflight = Math.max(0, state.inflight + 1);
    pushActivity("req", `${ev.detail?.requestConfig?.verb || "GET"} ${shortPath(url)}`);
    if (state.open) renderStatus();
  });

  document.body.addEventListener("htmx:afterRequest", (ev) => {
    state.inflight = Math.max(0, state.inflight - 1);
    const xhr = ev.detail?.xhr;
    const url = ev.detail?.requestConfig?.path || ev.detail?.pathInfo?.requestPath || "";
    const status = xhr?.status ?? 0;
    const ok = status >= 200 && status < 400;
    const dur = ev.detail?.elapsedTime != null ? Math.round(ev.detail.elapsedTime) : null;

    if (ok) state.lastSuccessAt = Date.now();
    pushActivity(ok ? "res" : "err", `${status} ${shortPath(url)}` + (dur != null ? ` · ${dur}ms` : ""));

    // Track per-endpoint poll stats.
    const key = shortPath(url).split("?")[0];
    const info = state.pollsByUrl.get(key) || { count: 0 };
    info.count += 1;
    info.lastStatus = status;
    info.lastDur = dur;
    info.lastWasError = !ok;
    state.pollsByUrl.set(key, info);

    if (state.open) {
      renderStatus();
      renderPolls();
    }
  });

  document.body.addEventListener("htmx:responseError", (ev) => {
    const url = ev.detail?.requestConfig?.path || "";
    const status = ev.detail?.xhr?.status || 0;
    pushActivity("err", `${status} ${shortPath(url)}`);
  });

  // ── WS hookup (opportunistic — works on desktop, no-op on iPad) ─
  if (window.sipagTransport && window.sipagLive) {
    const tr = window.sipagLive.transport;
    if (tr) {
      tr.on("connect", () => {
        state.wsConnected = true;
        const d = tr.diag ? tr.diag() : null;
        state.wsUrl = d?.url || null;
        pushActivity("ws", "ws connected");
        if (state.open) renderStatus();
      });
      tr.on("disconnect", () => {
        state.wsConnected = false;
        pushActivity("ws", "ws disconnected");
        if (state.open) renderStatus();
      });
      const d0 = tr.diag ? tr.diag() : null;
      if (d0?.url) state.wsUrl = d0.url;
      // Subscribe to the activity firehose so worker events show up
      // in the panel even when WS works.
      tr.subscribe("workers/activity", { fromSeq: 0 }, (env) => {
        pushActivity(
          "ws",
          `${env.kind} @ ${env.topic}` +
            (env.payload ? " · " + JSON.stringify(env.payload).slice(0, 120) : "")
        );
        state.lastSuccessAt = Date.now();
        if (state.open) renderStatus();
      });
    }
  }

  // ── interactions ────────────────────────────────────────────────
  toggle.addEventListener("click", () => {
    state.open = !state.open;
    persist();
    panel.classList.toggle("open", state.open);
    if (state.open) {
      renderStatus();
      renderActivity();
      renderPolls();
    }
  });

  panel.addEventListener("click", (ev) => {
    const t = ev.target;
    if (!t || !t.getAttribute) return;
    if (t.getAttribute("data-act") === "close") {
      state.open = false;
      persist();
      panel.classList.remove("open");
      return;
    }
    const probe = t.getAttribute("data-probe");
    if (probe) {
      const map = {
        board: "/htmx/board",
        ticker: "/htmx/ticker",
        attention: "/htmx/attention",
        topics: "/htmx/debug/topics",
        repos: "/api/insights/repos",
      };
      const path = map[probe];
      if (!path) return;
      const url = window.sipagTransport && window.sipagTransport.pageUrl
        ? window.sipagTransport.pageUrl(path)
        : path;
      pushActivity("req", `manual probe ${shortPath(url)}`);
      const t0 = performance.now();
      fetch(url)
        .then(async (r) => {
          const body = await r.text();
          const dur = Math.round(performance.now() - t0);
          if (r.ok) state.lastSuccessAt = Date.now();
          pushActivity(
            r.ok ? "res" : "err",
            `${r.status} ${shortPath(url)} · ${dur}ms · ${body.length}B`
          );
          if (state.open) {
            renderStatus();
            renderPolls();
          }
        })
        .catch((e) => {
          pushActivity("err", `probe failed: ${e.message || e}`);
          if (state.open) renderStatus();
        });
    }
  });

  // Refresh status every second so freshness ages in real time.
  setInterval(() => { if (state.open) renderStatus(); }, 1000);

  // Initial paint.
  renderStatus();
})();
