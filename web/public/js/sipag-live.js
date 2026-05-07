// sipag-live.js — DOM glue between transport.js and the maud-rendered UI.
//
// Subscribes to:
//   - observations/activity    → pulses the matching session row
//
// The HTMX-rendered board is the source of truth for static content;
// this script only handles live pulse animations.

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
  document.addEventListener("DOMContentLoaded", () => reapplyOpen(document));
})();

(function () {
  if (!window.sipagTransport) return;
  const t = window.sipagTransport.connect("/ws");
  // Make the live transport reachable from the debug panel so it can
  // expose URL + lastClose info without re-connecting.
  window.sipagLive = { transport: t };

  function cssEscape(s) {
    if (window.CSS && CSS.escape) return CSS.escape(s);
    return String(s).replace(/[^a-zA-Z0-9_-]/g, "\\$&");
  }

  // observations/activity — every observer / categorize / kr-assign
  // event pulses the matching <li.live-obs> row so the board visibly
  // reacts when gemma4 finishes a proposal or a new session appears.
  function pulseObservation(host, session) {
    if (!host || !session) return;
    const sel = `li.live-obs[data-host="${cssEscape(host)}"][data-session="${cssEscape(session)}"]`;
    const el = document.querySelector(sel);
    if (!el) return;
    el.classList.remove("pulse");
    void el.offsetWidth;
    el.classList.add("pulse");
    setTimeout(() => el.classList.remove("pulse"), 1600);
  }
  t.subscribe("observations/activity", { fromSeq: 0 }, (env) => {
    const host = env.payload && env.payload.host;
    const session = env.payload && env.payload.session;
    pulseObservation(host, session);
  });
})();
