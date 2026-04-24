(ns sipag.app
  "Vanilla ClojureScript entry point for the agent-manager SPA.

   No Reagent, no re-frame — direct DOM interop via goog.dom, fetch()
   for HTTP, and a tiny render loop over an atom holding the last known
   state.

   The server (Rust `sipag serve`) owns:
     - GET /api/hosts                           → [{id, url}]
     - GET /api/hosts/:id/sessions              → proxied katulong /sessions

   Katulong session names follow the `<project>--<worker>` convention
   when crewed; this view groups by that prefix so a katulong spawned
   with `crew spawn myapp frontend` renders under a `myapp` column."
  (:require
    [clojure.string :as str]
    [goog.dom :as gdom]))

;; ── state ───────────────────────────────────────────────────────────

(defonce state
  (atom {:hosts    []       ; [{id, url}]
         :sessions {}       ; host-id → vec of session maps
         :err      {}       ; host-id → last error string
         :phase    :boot
         :boot-err nil}))

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
                (swap! state assoc :phase :error :boot-err (.-message err))))))

(defn- load-sessions! [host-id]
  (-> (fetch-json (str "/api/hosts/" host-id "/sessions"))
      (.then (fn [body]
               (swap! state (fn [s]
                              (-> s
                                  (assoc-in [:sessions host-id] body)
                                  (assoc-in [:err host-id] nil))))))
      (.catch (fn [err]
                (swap! state assoc-in [:err host-id] (.-message err))))))

(defn- refresh-all! []
  (doseq [{:keys [id]} (:hosts @state)]
    (load-sessions! id)))

;; ── deriving crew structure ─────────────────────────────────────────

(defn- session-state [sess]
  (cond
    (not (:alive sess))          "exited"
    (:hasChildProcesses sess)    "active"
    :else                        "idle"))

(defn- crew-key
  "Split a session name on `--` into [project worker]. Un-crewed
   sessions — those without the separator — land in a synthetic
   '_loose_' bucket so they still appear in the view."
  [sess]
  (let [name (or (:name sess) "")
        idx (str/index-of name "--")]
    (if idx
      [(subs name 0 idx) (subs name (+ idx 2))]
      ["_loose_" name])))

(defn- group-by-project [sessions]
  (->> sessions
       (map (fn [s]
              (let [[proj worker] (crew-key s)]
                (assoc s ::project proj ::worker worker))))
       (group-by ::project)))

;; ── rendering ───────────────────────────────────────────────────────
;;
;; Plain string templating, full innerHTML replace per tick. Small tree,
;; not worth a diff — and keeps the spike honest about what the data
;; path costs.

(defn- escape-html [s]
  (when s
    (-> (str s)
        (.replace (js/RegExp "&" "g") "&amp;")
        (.replace (js/RegExp "<" "g") "&lt;")
        (.replace (js/RegExp ">" "g") "&gt;")
        (.replace (js/RegExp "\"" "g") "&quot;"))))

(defn- render-worker [sess]
  (let [st    (session-state sess)
        label (escape-html (or (::worker sess) (:name sess)))
        kids  (or (:childCount sess) 0)]
    (str "<div class=\"worker " st "\">"
         "<span>" label "</span>"
         "<span>" st (when (pos? kids) (str " · " kids)) "</span>"
         "</div>")))

(defn- render-project [[project-name sessions]]
  (str "<div class=\"project\">"
       "<div class=\"project-name\">" (escape-html project-name) "</div>"
       (apply str (map render-worker sessions))
       "</div>"))

(defn- render-host [{:keys [id url]} sessions err]
  (let [projects (group-by-project (or sessions []))
        proj-count (count projects)]
    (str "<section class=\"host\">"
         "<header class=\"host-head\">"
         "<div class=\"host-id\">" (escape-html id)
         " <span class=\"subtle\">(" (count sessions) ")</span></div>"
         "<div class=\"host-url\">" (escape-html url) "</div>"
         "</header>"
         (cond
           err
           (str "<div class=\"err\">" (escape-html err) "</div>")

           (zero? proj-count)
           "<div class=\"subtle\">no sessions</div>"

           :else
           (str "<div class=\"crew\">"
                (apply str (map render-project projects))
                "</div>"))
         "</section>")))

(defn- render! [{:keys [hosts sessions err phase boot-err]}]
  (let [status-el (gdom/getElement "status")
        board-el  (gdom/getElement "board")]
    (when status-el
      (set! (.-textContent status-el)
            (case phase
              :boot   "loading…"
              :ready  (str (count hosts) " host(s)")
              :error  (str "error: " boot-err))))
    (when board-el
      (set! (.-innerHTML board-el)
            (apply str (map #(render-host % (get sessions (:id %)) (get err (:id %))) hosts))))))

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
