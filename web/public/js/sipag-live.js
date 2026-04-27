// sipag-live.js — DOM glue between transport.js and the maud-rendered UI.
//
// Subscribes to:
//   - workers/activity         → updates the top-right ticker, .working class
//   - <discourse topics>       → appends rows into open discourse drawers
//
// The HTMX-rendered board is the source of truth for static content;
// this script only handles live append / pulse animations / ticker.

(function () {
  if (!window.sipagTransport) return;
  const t = window.sipagTransport.connect("/ws");

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

  function appendDiscourse(topic, env) {
    const node = document.querySelector(`.discourse[data-topic="${cssEscape(topic)}"] .discourse-log`);
    if (!node) return;
    const li = document.createElement("li");
    li.className = "discourse-row";
    li.dataset.kind = env.kind;
    const role = roleLabel(env);
    const text = (env.payload && (env.payload.text || env.payload.message)) || "";
    li.innerHTML = `
      <span class="discourse-ts">${escapeHtml(env.ts)}</span>
      <span class="discourse-role">${escapeHtml(role)}</span>
      <span class="discourse-text">${escapeHtml(text)}</span>
    `;
    node.appendChild(li);
  }
  function roleLabel(env) {
    const w = (env.payload && env.payload.worker) || "?";
    switch (env.kind) {
      case "human.message": return "human";
      case "assistant.message": return "worker:" + w;
      case "worker.progress": return "worker:" + w + " (progress)";
      case "worker.complete": return "worker:" + w;
      case "worker.error": return "worker:" + w + " (error)";
      case "label.changed": return "system: label";
      case "done.toggled": return "system: done";
      default: return env.kind;
    }
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

  // 2. For every visible discourse panel, subscribe to its topic.
  function wireDiscoursePanels() {
    document.querySelectorAll(".discourse[data-topic]").forEach((node) => {
      const topic = node.dataset.topic;
      if (node.dataset.subscribed === "1") return;
      node.dataset.subscribed = "1";
      t.subscribe(topic, { fromSeq: 0 }, (env) => appendDiscourse(topic, env));
    });
  }
  document.addEventListener("DOMContentLoaded", wireDiscoursePanels);
  // Re-wire after each htmx swap because the board fragment can replace
  // discourse panels wholesale.
  document.body.addEventListener("htmx:afterSwap", wireDiscoursePanels);
})();
