// sipag-live.js — DOM glue between transport.js and the maud-rendered UI.
//
// Subscribes to:
//   - workers/activity         → updates the top-right ticker, .working class
//   - observations/activity    → pulses the matching session row + ticker
//
// The HTMX-rendered board is the source of truth for static content;
// this script only handles live append / pulse animations / ticker.

// Preserve <details data-key=...> open state across HTMX swaps. Without
// this every 5s board poll would slam every expanded session-detail
// closed. We track the user's intent in a Set keyed by data-key and
// re-apply it after each swap.
(function () {
  const openKeys = new Set();

  function captureOpen(root) {
    if (!root || !root.querySelectorAll) return;
    root.querySelectorAll("details[data-key]").forEach((d) => {
      const k = d.getAttribute("data-key");
      if (!k) return;
      if (d.open) openKeys.add(k);
      else openKeys.delete(k);
    });
  }

  function reapplyOpen(root) {
    if (!root || !root.querySelectorAll) return;
    root.querySelectorAll("details[data-key]").forEach((d) => {
      const k = d.getAttribute("data-key");
      if (k && openKeys.has(k)) d.open = true;
    });
  }

  // Track user toggles directly so we don't lose state if a swap
  // happens with no opened details in the outgoing tree.
  document.addEventListener("toggle", (e) => {
    const d = e.target;
    if (!d || d.tagName !== "DETAILS") return;
    const k = d.getAttribute && d.getAttribute("data-key");
    if (!k) return;
    if (d.open) openKeys.add(k);
    else openKeys.delete(k);
  }, true);

  document.addEventListener("htmx:beforeSwap", (e) => {
    captureOpen(e.detail && e.detail.target);
  });
  document.addEventListener("htmx:afterSwap", (e) => {
    reapplyOpen(e.detail && e.detail.target);
  });
  // Also re-apply after the very first board render (in case the user
  // hard-refreshed with details open in the previous session — we can't
  // restore *those*, but we want to be idempotent if they re-toggle).
  document.addEventListener("DOMContentLoaded", () => reapplyOpen(document));
})();

(function () {
  if (!window.sipagTransport) return;
  const t = window.sipagTransport.connect("/ws");
  // Make the live transport reachable from the debug panel so it can
  // expose URL + lastClose info without re-connecting.
  window.sipagLive = { transport: t };

  const TICKER_MAX = 5;

  function ensureTicker() {
    let el = document.getElementById("ticker");
    if (!el) {
      el = document.createElement("div");
      el.id = "ticker";
      el.className = "ticker";
      document.body.appendChild(el);
    }
    return el;
  }

  function tickerRow(env) {
    const row = document.createElement("div");
    row.className = "ticker-row";
    row.dataset.kind = env.kind;
    const ts = env.ts || "";
    const worker = (env.payload && env.payload.worker) || "";
    const targetKind = (env.payload && env.payload.kind) || "";
    const targetProject = (env.payload && env.payload.project) || "";
    const targetId = (env.payload && env.payload.id) || "";
    row.innerHTML = `
      <span class="ticker-ts">${escapeHtml(ts)}</span>
      <span class="ticker-kind">${escapeHtml(env.kind)}</span>
      <span class="ticker-worker">${escapeHtml(worker)}</span>
      <span class="ticker-target">${escapeHtml(targetKind)} ${escapeHtml(targetProject)} #${escapeHtml(String(targetId))}</span>
    `;
    return row;
  }

  function setItemWorking(kind, project, id, working) {
    const sel = `[data-kind="${cssEscape(kind)}"][data-project="${cssEscape(project)}"][data-id="${cssEscape(String(id))}"]`;
    const el = document.querySelector(sel);
    if (!el) return;
    if (working) el.classList.add("working");
    else el.classList.remove("working");
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
    })[c]);
  }
  function cssEscape(s) {
    if (window.CSS && CSS.escape) return CSS.escape(s);
    return String(s).replace(/[^a-zA-Z0-9_-]/g, "\\$&");
  }

  // 1. Subscribe to workers/activity from seq=0 — replay + live.
  t.subscribe("workers/activity", { fromSeq: 0 }, (env) => {
    const ticker = ensureTicker();
    ticker.appendChild(tickerRow(env));
    while (ticker.children.length > TICKER_MAX) {
      ticker.removeChild(ticker.firstChild);
    }
    if (env.payload && typeof env.payload.id === "number") {
      const kind = env.payload.kind;
      const project = env.payload.project;
      const id = env.payload.id;
      if (env.kind === "worker.start") setItemWorking(kind, project, id, true);
      if (env.kind === "worker.complete" || env.kind === "worker.error") {
        setItemWorking(kind, project, id, false);
      }
    }
  });

  // 2. observations/activity — every observer / categorize / kr-assign
  // event. We pulse the matching <li.live-obs> row so the misc tray
  // visibly reacts when gemma4 finishes a proposal or a new session
  // appears.
  function pulseObservation(host, session) {
    if (!host || !session) return;
    const sel = `li.live-obs[data-host="${cssEscape(host)}"][data-session="${cssEscape(session)}"]`;
    const el = document.querySelector(sel);
    if (!el) return;
    el.classList.remove("pulse"); // reset if already mid-animation
    // Force reflow so re-adding the class restarts the animation
    void el.offsetWidth;
    el.classList.add("pulse");
    setTimeout(() => el.classList.remove("pulse"), 1600);
  }
  t.subscribe("observations/activity", { fromSeq: 0 }, (env) => {
    const ticker = ensureTicker();
    ticker.appendChild(tickerRow(env));
    while (ticker.children.length > TICKER_MAX) {
      ticker.removeChild(ticker.firstChild);
    }
    const host = env.payload && env.payload.host;
    const session = env.payload && env.payload.session;
    pulseObservation(host, session);
  });

})();
