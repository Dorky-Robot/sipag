// In-page debug panel for sipag's WebSocket transport.
//
// Built for poking from devices without a Web Inspector (iPad, kiosk,
// etc.). Floating ⓘ button at bottom-right. Tap it to slide a panel in
// from the right showing:
//   • connection state
//   • known topics (one toggle per topic to subscribe)
//   • live event tail (newest-first, capped at MAX_EVENTS)
//   • a tiny publish form for round-tripping a manual event
//
// Hooks into the global `sipagTransport` exposed by transport.js.
(function () {
  if (!window.sipagTransport) {
    console.warn('[sipag-debug] transport.js not loaded; debug panel skipped');
    return;
  }

  const MAX_EVENTS = 80;
  const PANEL_ID = 'sipag-debug-panel';
  const TOGGLE_ID = 'sipag-debug-toggle';
  const STORAGE_KEY = 'sipag-debug-state';

  const state = {
    open: false,
    subs: new Set(),     // topics currently subscribed
    events: [],          // newest at index 0
    topics: [],          // last fetched list
    lastError: null,
  };

  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || '{}');
    if (saved.open) state.open = true;
    if (Array.isArray(saved.subs)) saved.subs.forEach((t) => state.subs.add(t));
  } catch (_) { /* ignore */ }

  function persist() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({
        open: state.open,
        subs: Array.from(state.subs),
      }));
    } catch (_) { /* ignore */ }
  }

  // ── styles ──────────────────────────────────────────────────────
  const style = document.createElement('style');
  style.textContent = `
    #${TOGGLE_ID} {
      position: fixed; bottom: 16px; right: 16px;
      width: 44px; height: 44px; border-radius: 22px;
      border: 1px solid #262c34; background: #14181d;
      color: #7aa2f7; font: 600 14px ui-monospace, monospace;
      cursor: pointer; z-index: 9999;
      box-shadow: 0 4px 12px rgba(0,0,0,0.35);
    }
    #${TOGGLE_ID}.connected { color: #9ece6a; }
    #${TOGGLE_ID}.disconnected { color: #f7768e; }
    #${PANEL_ID} {
      position: fixed; top: 0; right: 0; bottom: 0;
      width: min(420px, 100vw); transform: translateX(100%);
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
    #${PANEL_ID} header h2 {
      font-size: 14px; font-weight: 600; margin: 0; flex: 1;
    }
    #${PANEL_ID} header button {
      background: transparent; border: 1px solid #262c34;
      color: #9aa3ae; padding: 4px 10px; border-radius: 4px;
      font: inherit; font-size: 12px; cursor: pointer;
    }
    #${PANEL_ID} .body { flex: 1; overflow-y: auto; padding: 12px 16px; }
    #${PANEL_ID} section { margin-bottom: 16px; }
    #${PANEL_ID} h3 {
      font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em;
      color: #9aa3ae; font-weight: 600; margin: 0 0 6px;
    }
    #${PANEL_ID} .status-row {
      display: flex; gap: 8px; align-items: baseline;
      font-family: ui-monospace, monospace; font-size: 12px;
    }
    #${PANEL_ID} .status-row .dot {
      display: inline-block; width: 8px; height: 8px; border-radius: 4px;
    }
    #${PANEL_ID} .status-row .dot.connected { background: #9ece6a; }
    #${PANEL_ID} .status-row .dot.disconnected { background: #f7768e; }
    #${PANEL_ID} .topic-row {
      display: flex; align-items: center; justify-content: space-between;
      padding: 4px 0; border-bottom: 1px solid #14181d;
      font-family: ui-monospace, monospace; font-size: 12px;
    }
    #${PANEL_ID} .topic-row .name { flex: 1; word-break: break-all; }
    #${PANEL_ID} .topic-row button {
      background: transparent; border: 1px solid #262c34;
      color: #7aa2f7; padding: 2px 8px; border-radius: 3px;
      font: inherit; cursor: pointer;
    }
    #${PANEL_ID} .topic-row.subscribed button { color: #9ece6a; }
    #${PANEL_ID} .events {
      list-style: none; margin: 0; padding: 0;
      max-height: 360px; overflow-y: auto;
      border: 1px solid #14181d; border-radius: 4px;
      background: #14181d;
    }
    #${PANEL_ID} .event {
      padding: 6px 8px; border-bottom: 1px solid #0e1013;
      font-family: ui-monospace, monospace; font-size: 11px;
    }
    #${PANEL_ID} .event:last-child { border-bottom: none; }
    #${PANEL_ID} .event .meta { color: #9aa3ae; }
    #${PANEL_ID} .event pre {
      margin: 4px 0 0; white-space: pre-wrap; word-break: break-word;
      color: #c0caf5; font-size: 10.5px;
    }
    #${PANEL_ID} form.publish {
      display: grid; gap: 6px;
    }
    #${PANEL_ID} form.publish input,
    #${PANEL_ID} form.publish textarea {
      background: #14181d; border: 1px solid #262c34;
      color: #e6e8eb; padding: 6px 8px; border-radius: 3px;
      font: inherit; font-family: ui-monospace, monospace;
      font-size: 12px;
    }
    #${PANEL_ID} form.publish button {
      background: #7aa2f7; color: #0e1013; border: none;
      padding: 6px 12px; border-radius: 4px;
      font: inherit; font-weight: 600; cursor: pointer;
      justify-self: start;
    }
    #${PANEL_ID} .err {
      color: #f7768e; font-family: ui-monospace, monospace;
      font-size: 11px; padding: 6px 8px;
      background: #14181d; border-radius: 3px;
    }
  `;
  document.head.appendChild(style);

  // ── DOM scaffolding ─────────────────────────────────────────────
  const toggle = document.createElement('button');
  toggle.id = TOGGLE_ID;
  toggle.textContent = '🐞';
  toggle.title = 'sipag debug panel';
  document.body.appendChild(toggle);

  const panel = document.createElement('div');
  panel.id = PANEL_ID;
  panel.innerHTML = `
    <header>
      <h2>sipag · debug</h2>
      <button data-act="refresh-topics">refresh</button>
      <button data-act="close">close</button>
    </header>
    <div class="body">
      <section class="status">
        <h3>connection</h3>
        <div class="status-row">
          <span class="dot disconnected"></span>
          <span class="state">unknown</span>
          <span class="meta"></span>
        </div>
      </section>
      <section class="topics">
        <h3>topics</h3>
        <div class="list"></div>
      </section>
      <section class="events">
        <h3>events <span class="meta" style="color:#9aa3ae;font-weight:400">(newest first, capped at ${MAX_EVENTS})</span></h3>
        <ul class="events"></ul>
      </section>
      <section class="publish">
        <h3>publish</h3>
        <form class="publish">
          <input data-name="topic" placeholder="topic, e.g. workers/activity">
          <input data-name="kind" placeholder="kind, e.g. test">
          <textarea data-name="payload" placeholder='{"hello":"world"}' rows="3"></textarea>
          <button type="submit">publish</button>
        </form>
        <div class="err" hidden></div>
      </section>
    </div>
  `;
  document.body.appendChild(panel);

  if (state.open) panel.classList.add('open');

  // ── refs ────────────────────────────────────────────────────────
  const dot = panel.querySelector('.status .dot');
  const stateEl = panel.querySelector('.status .state');
  const stateMeta = panel.querySelector('.status .meta');
  const topicList = panel.querySelector('.topics .list');
  const eventsUl = panel.querySelector('ul.events');
  const errEl = panel.querySelector('.publish .err');
  const publishForm = panel.querySelector('form.publish');

  // ── connection status ───────────────────────────────────────────
  function diagSummary() {
    const live = window.sipagLive && window.sipagLive.transport;
    const d = live && live.diag ? live.diag() : null;
    if (!d) return '';
    const parts = [];
    if (d.url) parts.push('url=' + d.url);
    if (d.lastClose && d.lastClose.code) {
      parts.push('lastClose=' + d.lastClose.code + ' "' + (d.lastClose.reason || '') + '"');
    }
    if (d.lastError) parts.push('err=' + d.lastError);
    return parts.join(' · ');
  }

  function paintConnection(connected, detail) {
    dot.classList.toggle('connected', connected);
    dot.classList.toggle('disconnected', !connected);
    stateEl.textContent = connected ? 'connected' : 'disconnected';
    const more = diagSummary();
    let suffix = '';
    if (detail && detail.reason) suffix = '· ' + detail.reason;
    else if (typeof detail === 'string') suffix = '· ' + detail;
    if (more) suffix = (suffix ? suffix + ' · ' : '· ') + more;
    stateMeta.textContent = suffix;
    toggle.classList.toggle('connected', connected);
    toggle.classList.toggle('disconnected', !connected);
  }

  paintConnection(false, 'opening…');
  // Update connection meta periodically so user sees latest URL / errors.
  setInterval(() => {
    if (state.open && stateEl.textContent === 'disconnected') {
      paintConnection(false, 'opening…');
    }
  }, 1000);

  if (typeof sipagTransport.on === 'function') {
    sipagTransport.on('connect', () => paintConnection(true));
    sipagTransport.on('disconnect', (info) => paintConnection(false, info && info.reason));
  }

  // ── events ───────────────────────────────────────────────────────
  function pushEvent(env) {
    state.events.unshift(env);
    if (state.events.length > MAX_EVENTS) state.events.length = MAX_EVENTS;
    if (state.open) renderEvents();
  }

  function renderEvents() {
    const html = state.events.map((env) => {
      const ts = (env.ts || '').replace('T', ' ').slice(0, 19);
      const payload = env.payload === undefined
        ? ''
        : typeof env.payload === 'string'
          ? env.payload
          : JSON.stringify(env.payload, null, 2);
      return `<li class="event">
        <div class="meta">#${env.seq} ${ts} <strong>${escapeHtml(env.kind || '')}</strong> @ ${escapeHtml(env.topic || '')}</div>
        ${payload ? `<pre>${escapeHtml(payload)}</pre>` : ''}
      </li>`;
    }).join('');
    eventsUl.innerHTML = html;
  }

  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  // ── topics ───────────────────────────────────────────────────────
  function paintTopics() {
    if (!state.topics.length) {
      topicList.innerHTML = '<div class="meta" style="color:#9aa3ae;font-size:12px">no topics yet</div>';
      return;
    }
    topicList.innerHTML = state.topics.map((t) => {
      const subscribed = state.subs.has(t);
      return `<div class="topic-row${subscribed ? ' subscribed' : ''}">
        <span class="name">${escapeHtml(t)}</span>
        <button data-topic="${escapeHtml(t)}">${subscribed ? 'unsub' : 'sub'}</button>
      </div>`;
    }).join('');
  }

  function subscribe(topic) {
    if (state.subs.has(topic)) return;
    state.subs.add(topic);
    persist();
    sipagTransport.subscribe(topic, { fromSeq: 0 }, (env) => pushEvent(env));
    paintTopics();
  }

  function unsubscribe(topic) {
    if (!state.subs.has(topic)) return;
    state.subs.delete(topic);
    persist();
    if (typeof sipagTransport.unsubscribe === 'function') {
      sipagTransport.unsubscribe(topic);
    }
    paintTopics();
  }

  async function refreshTopics() {
    try {
      const r = await fetch(sipagTransport.pageUrl('/htmx/debug/topics'));
      if (!r.ok) throw new Error('HTTP ' + r.status);
      const list = await r.json();
      state.topics = Array.isArray(list) ? list.slice().sort() : [];
    } catch (e) {
      state.topics = [];
      stateMeta.textContent = '· topics fetch failed: ' + (e.message || e);
    }
    // Subscribe to anything in saved state but not yet wired.
    for (const t of state.subs) {
      if (state.topics.indexOf(t) >= 0) {
        sipagTransport.subscribe(t, { fromSeq: 0 }, (env) => pushEvent(env));
      }
    }
    paintTopics();
  }

  // ── interactions ─────────────────────────────────────────────────
  toggle.addEventListener('click', () => {
    state.open = !state.open;
    persist();
    panel.classList.toggle('open', state.open);
    if (state.open) {
      renderEvents();
      refreshTopics();
    }
  });

  panel.addEventListener('click', (ev) => {
    const t = ev.target;
    const act = t && t.getAttribute && t.getAttribute('data-act');
    if (act === 'close') {
      state.open = false;
      persist();
      panel.classList.remove('open');
      return;
    }
    if (act === 'refresh-topics') { refreshTopics(); return; }
    const topic = t && t.getAttribute && t.getAttribute('data-topic');
    if (topic) {
      if (state.subs.has(topic)) unsubscribe(topic);
      else subscribe(topic);
    }
  });

  publishForm.addEventListener('submit', (ev) => {
    ev.preventDefault();
    errEl.hidden = true;
    const topic = publishForm.querySelector('[data-name="topic"]').value.trim();
    const kind = publishForm.querySelector('[data-name="kind"]').value.trim();
    const payloadRaw = publishForm.querySelector('[data-name="payload"]').value.trim();
    if (!topic || !kind) {
      errEl.textContent = 'topic and kind are required';
      errEl.hidden = false;
      return;
    }
    let payload = {};
    if (payloadRaw) {
      try { payload = JSON.parse(payloadRaw); }
      catch (e) {
        errEl.textContent = 'payload must be valid JSON: ' + (e.message || e);
        errEl.hidden = false;
        return;
      }
    }
    try {
      sipagTransport.publish(topic, kind, payload);
    } catch (e) {
      errEl.textContent = 'publish failed: ' + (e.message || e);
      errEl.hidden = false;
    }
  });

  // Initial fetch — even if panel is closed, so the toggle reflects
  // connection state.
  refreshTopics();
})();
