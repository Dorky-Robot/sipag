// transport.js — minimal WebSocket pub/sub client for sipag.
//
// Stripped-down port of katulong/lib/client-transport.js. WebSocket-only
// for v1; auto-reconnect with exponential backoff; subscriptions are
// re-issued on reconnect using the highest seen seq per topic.
//
// Usage:
//   const t = sipagTransport.connect("/ws");
//   t.subscribe("workers/activity", { fromSeq: 0 }, (env) => {...});
//   t.publish("a/b", "kind", { ... });
//   t.on("connect", () => {});
//
// Standalone — assigns to window.sipagTransport. No deps.

(function () {
  // Directory prefix of the current page. Empty when served at root;
  // "/_proxy/7100" when reverse-proxied through katulong's port proxy
  // (whether or not the URL has a trailing slash — sipag's pathnames
  // are never filename-flavored, so we treat the whole pathname as a
  // directory).
  function pageDir() {
    return location.pathname.replace(/\/+$/, "");
  }

  function pageUrl(absPath) {
    if (!absPath || !absPath.startsWith("/")) return absPath;
    const prefix = pageDir();
    if (!prefix) return absPath;
    if (absPath.startsWith(prefix + "/") || absPath === prefix) return absPath;
    return prefix + absPath;
  }

  function connect(path) {
    // Compute the WS URL by appending to the current document's
    // directory. Load-bearing when sipag is iframed via katulong's
    // port proxy (e.g. https://katulong-mini.felixflor.es/_proxy/7100/)
    // — a hardcoded "/ws" would resolve to the host root and miss the
    // proxy prefix.
    let url;
    if (path && /^wss?:\/\//.test(path)) {
      url = path;
    } else {
      const tail = (path || "ws").replace(/^\/+/, "");
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      const dir = pageDir();
      url = proto + "//" + location.host + dir + "/" + tail;
    }

    const handlers = new Map(); // topic -> { fromSeq, lastSeq, callback }
    const events = new Map();   // event name -> Set<handler>
    let ws = null;
    let backoff = 250;
    let alive = true;
    let heartbeatTimer = null;
    const diag = { url, lastClose: null, lastError: null };

    function emit(name, ...args) {
      const set = events.get(name);
      if (!set) return;
      for (const h of set) try { h(...args); } catch {}
    }

    function open() {
      try {
        ws = new WebSocket(url);
      } catch (_) {
        scheduleReconnect();
        return;
      }
      ws.onopen = () => {
        backoff = 250;
        emit("connect");
        for (const [topic, sub] of handlers) {
          const fromSeq = sub.lastSeq != null ? sub.lastSeq + 1 : sub.fromSeq;
          send({ action: "subscribe", topic, from_seq: fromSeq });
        }
        startHeartbeat();
      };
      ws.onmessage = (event) => {
        let msg;
        try { msg = JSON.parse(event.data); } catch { return; }
        if (msg.error) {
          emit("error", msg.error);
          return;
        }
        if (msg.kind === "pong") return; // heartbeat ack from server
        if (typeof msg.topic === "string" && typeof msg.seq === "number") {
          const sub = handlers.get(msg.topic);
          if (sub) {
            sub.lastSeq = msg.seq;
            try { sub.callback(msg); } catch (e) {
              if (window.console && console.warn) console.warn("transport callback threw", e);
            }
          }
        }
      };
      ws.onclose = (ev) => {
        stopHeartbeat();
        diag.lastClose = { code: ev && ev.code, reason: (ev && ev.reason) || "" };
        emit("disconnect", diag.lastClose);
        if (alive) scheduleReconnect();
      };
      ws.onerror = () => {
        diag.lastError = "websocket error";
        try { ws.close(); } catch {}
      };
    }

    function send(obj) {
      if (!ws || ws.readyState !== 1) return false;
      try { ws.send(JSON.stringify(obj)); return true; } catch { return false; }
    }

    function scheduleReconnect() {
      const wait = Math.min(backoff, 30000);
      backoff = Math.min(backoff * 2, 30000);
      setTimeout(() => { if (alive) open(); }, wait);
    }

    function startHeartbeat() {
      stopHeartbeat();
      heartbeatTimer = setInterval(() => {
        if (!send({ action: "ping" })) stopHeartbeat();
      }, 25000);
    }
    function stopHeartbeat() {
      if (heartbeatTimer) clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }

    open();

    return {
      subscribe(topic, opts, cb) {
        const fromSeq = (opts && typeof opts.fromSeq === "number") ? opts.fromSeq : 0;
        const existing = handlers.get(topic);
        if (existing) {
          existing.callback = cb;
          return;
        }
        handlers.set(topic, { fromSeq, lastSeq: null, callback: cb });
        send({ action: "subscribe", topic, from_seq: fromSeq });
      },
      unsubscribe(topic) {
        if (handlers.delete(topic)) {
          send({ action: "unsubscribe", topic });
        }
      },
      publish(topic, kind, payload) {
        send({ action: "publish", topic, kind, payload });
      },
      on(event, handler) {
        if (!events.has(event)) events.set(event, new Set());
        events.get(event).add(handler);
      },
      off(event, handler) {
        const set = events.get(event);
        if (set) set.delete(handler);
      },
      close() {
        alive = false;
        try { ws && ws.close(); } catch {}
      },
      diag() { return Object.assign({}, diag); },
    };
  }

  // Auto-rewrite HTMX request URLs so absolute /htmx/... paths inherit
  // the proxy prefix when sipag is reverse-proxied (katulong's
  // /_proxy/7100/). One global hook covers attributes set in HTML
  // returned by swaps too.
  document.addEventListener("htmx:configRequest", function (ev) {
    if (ev.detail && typeof ev.detail.path === "string" && ev.detail.path.startsWith("/")) {
      ev.detail.path = pageUrl(ev.detail.path);
    }
  });

  window.sipagTransport = { connect, pageUrl, pageDir };
})();
