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
  function connect(path) {
    const url = (location.protocol === "https:" ? "wss://" : "ws://")
      + location.host + path;

    const handlers = new Map(); // topic -> { fromSeq, lastSeq, callback }
    const events = new Map();   // event name -> Set<handler>
    let ws = null;
    let backoff = 250;
    let alive = true;
    let heartbeatTimer = null;

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
      ws.onclose = () => {
        stopHeartbeat();
        emit("disconnect");
        if (alive) scheduleReconnect();
      };
      ws.onerror = () => {
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
    };
  }

  window.sipagTransport = { connect };
})();
