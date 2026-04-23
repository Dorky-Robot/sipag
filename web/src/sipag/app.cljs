(ns sipag.app
  "Vanilla ClojureScript entry point for the agent-manager SPA.

   No Reagent, no re-frame — direct DOM interop via goog.dom, fetch()
   for HTTP, and a tiny render loop over an atom holding the last known
   state. Matches Alon's no-framework posture (commit 0ae4ec6 in alon:
   'abandon extra complexity with alon composer').

   The server (Rust `sipag serve`) owns:
     - /api/hosts                             → [{id, url}]
     - /api/hosts/:id/crew/status             → katulong /crew/status

   This file owns:
     - polling each host for /crew/status every 5s
     - rendering one column per host with its projects and workers"
  (:require
    [goog.dom :as gdom]))

;; ── state ───────────────────────────────────────────────────────────
;;
;; One atom, re-rendered on every change. No diffing, no vdom — the tree
;; is small and the spike only needs to prove the pipe.

(defonce state
  (atom {:hosts []
         :crew  {}   ; host-id → parsed /crew/status body
         :err   {}   ; host-id → last error string (or nil)
         :phase :boot}))

;; ── fetch helpers ───────────────────────────────────────────────────

(defn- fetch-json [url]
  (-> (js/fetch url)
      (.then (fn [resp]
               (if (.-ok resp)
                 (.json resp)
                 (throw (ex-info (str "HTTP " (.-status resp)) {:url url})))))
      (.then #(js->clj % :keywordize-keys true))))

(defn- load-hosts! []
  (-> (fetch-json "/api/hosts")
      (.then (fn [hosts]
               (swap! state assoc :hosts hosts :phase :ready)))
      (.catch (fn [err]
                (swap! state assoc :phase :error :boot-error (.-message err))))))

(defn- load-crew! [host-id]
  (-> (fetch-json (str "/api/hosts/" host-id "/crew/status"))
      (.then (fn [body]
               (swap! state (fn [s]
                              (-> s
                                  (assoc-in [:crew host-id] body)
                                  (assoc-in [:err host-id] nil))))))
      (.catch (fn [err]
                (swap! state assoc-in [:err host-id] (.-message err))))))

(defn- refresh-all! []
  (doseq [{:keys [id]} (:hosts @state)]
    (load-crew! id)))

;; ── rendering ───────────────────────────────────────────────────────
;;
;; Plain string templating. No DOM diffing. We replace innerHTML on
;; every state change, which is fine at three hosts × a handful of
;; workers and keeps the spike honest about what the data path costs.

(defn- escape-html [s]
  (when s
    (-> (str s)
        (.replace (js/RegExp "&" "g") "&amp;")
        (.replace (js/RegExp "<" "g") "&lt;")
        (.replace (js/RegExp ">" "g") "&gt;")
        (.replace (js/RegExp "\"" "g") "&quot;"))))

(defn- worker-state [w]
  ;; katulong /crew/status exposes { name, alive, hasChildProcesses, childCount }.
  ;; Derive a three-way state the way crew-tile did (superseded commit ac169c2):
  ;; no alive → exited, child processes → active, otherwise → idle.
  (cond
    (not (:alive w))             "exited"
    (:hasChildProcesses w)       "active"
    :else                        "idle"))

(defn- render-worker [w]
  (let [st (worker-state w)
        name (escape-html (:name w))
        kids (or (:childCount w) 0)]
    (str "<div class=\"worker " st "\">"
         "<span>" name "</span>"
         "<span>" st " · " kids "</span>"
         "</div>")))

(defn- render-project [[project-name workers]]
  (str "<div class=\"project\">"
       "<div class=\"project-name\">" (escape-html project-name) "</div>"
       (apply str (map render-worker workers))
       "</div>"))

(defn- render-host [{:keys [id url] :as _host} crew err]
  (let [projects (or (:projects crew) {})]
    (str "<section class=\"host\">"
         "<header class=\"host-head\">"
         "<div class=\"host-id\">" (escape-html id) "</div>"
         "<div class=\"host-url\">" (escape-html url) "</div>"
         "</header>"
         (if err
           (str "<div class=\"err\">" (escape-html err) "</div>")
           (if (empty? projects)
             "<div class=\"subtle\">no crew</div>"
             (str "<div class=\"crew\">"
                  (apply str (map render-project projects))
                  "</div>")))
         "</section>")))

(defn- render! [{:keys [hosts crew err phase boot-error]}]
  (let [status-el (gdom/getElement "status")
        board-el  (gdom/getElement "board")]
    (set! (.-textContent status-el)
          (case phase
            :boot   "loading…"
            :ready  (str (count hosts) " host(s)")
            :error  (str "error: " boot-error)))
    (set! (.-innerHTML board-el)
          (apply str (map #(render-host % (get crew (:id %)) (get err (:id %))) hosts)))))

;; ── wiring ──────────────────────────────────────────────────────────

(defn init
  "Entry point. shadow-cljs calls this from :init-fn."
  []
  (add-watch state ::render
             (fn [_ _ _ new-state]
               (render! new-state)))
  (render! @state)
  (.then (load-hosts!)
         (fn [_]
           (refresh-all!)
           (js/setInterval refresh-all! 5000))))
