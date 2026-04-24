(ns sipag.app
  "Objective-focused agent manager SPA.

   Philosophy: we optimize *for* something (the objective); everything
   we're doing right now lives under its objective; ideas (things we
   might pick up later) sit in a parked idea-box off the main surface.
   No kanban funnel, no backlog → todo → doing → done columns — if it
   is happening, it shows; if it is not happening, it is either an
   idea or archived.

   Status mapping (read-only, server stays compatible with the CLI's
   full status vocabulary):
     backlog             → idea box
     todo | in-progress  → active under objective
     review              → active under objective
     done                → hidden (archived)

   Data from the server:
     GET /api/projects           → [{name, repo, statuses, tasks}]
     GET /api/hosts              → [{id, url}]
     GET /api/hosts/:id/sessions → katulong /sessions proxied

   Cross-referencing which host is running a task is best-effort and
   lives in a small footnote on active items; it never dominates the
   view."
  (:require
    [clojure.string :as str]
    [goog.dom :as gdom]))

;; ── state ───────────────────────────────────────────────────────────

(defonce state
  (atom {:projects       []     ; [{name, repo, statuses, tasks}]
         :hosts          []     ; [{id, url}]
         :sessions       {}     ; host-id → [sessions]
         :idea-box-open? false
         :err            nil
         :phase          :boot}))

;; ── fetch helpers ───────────────────────────────────────────────────

(defn- fetch-json [url]
  (-> (js/fetch url)
      (.then (fn [resp]
               (if (.-ok resp)
                 (.json resp)
                 (throw (ex-info (str "HTTP " (.-status resp)) {:url url})))))
      (.then #(js->clj % :keywordize-keys true))))

(defn- load-projects! []
  (-> (fetch-json "/api/projects")
      (.then (fn [ps] (swap! state assoc :projects ps :phase :ready)))
      (.catch (fn [err] (swap! state assoc :err (.-message err) :phase :error)))))

(defn- load-hosts! []
  (-> (fetch-json "/api/hosts")
      (.then (fn [hs] (swap! state assoc :hosts hs)))
      (.catch (fn [_err] nil))))

(defn- load-sessions! [host-id]
  (-> (fetch-json (str "/api/hosts/" host-id "/sessions"))
      (.then (fn [ss] (swap! state assoc-in [:sessions host-id] ss)))
      (.catch (fn [_err] nil))))

(defn- refresh-all! []
  (load-projects!)
  (doseq [{:keys [id]} (:hosts @state)]
    (load-sessions! id)))

;; ── classification ──────────────────────────────────────────────────

(def ^:private active-statuses #{"todo" "in-progress" "review"})
(def ^:private idea-statuses   #{"backlog"})
(def ^:private archived-statuses #{"done"})

(defn- active? [t]  (contains? active-statuses (:status t)))
(defn- idea?   [t]  (contains? idea-statuses (:status t)))

(defn- task-running-on
  "Return the host id currently running this task's dispatch session,
   or nil. Match on the katulong crew naming convention:
   `<project>--<role>`. Best-effort; the view never fails if this
   returns nil."
  [task project-name sessions-by-host]
  (let [needle (str project-name "--" (:role task))]
    (some (fn [[host-id sessions]]
            (when (some #(= needle (:name %)) sessions)
              host-id))
          sessions-by-host)))

;; ── rendering ───────────────────────────────────────────────────────

(defn- escape-html [s]
  (when s
    (-> (str s)
        (.replace (js/RegExp "&" "g") "&amp;")
        (.replace (js/RegExp "<" "g") "&lt;")
        (.replace (js/RegExp ">" "g") "&gt;")
        (.replace (js/RegExp "\"" "g") "&quot;"))))

(defn- render-labels [labels]
  (when (seq labels)
    (str "<span class=\"labels\">"
         (apply str (for [l labels]
                      (str "<span class=\"label\">" (escape-html l) "</span>")))
         "</span>")))

(defn- render-active-task [task project-name sessions]
  (let [host (task-running-on task project-name sessions)
        status (:status task)
        status-chip (when (not= status "todo")
                      (str "<span class=\"status-chip " status "\">"
                           (escape-html status) "</span>"))]
    (str "<li class=\"task\">"
         "<div class=\"task-head\">"
         "<span class=\"task-id\">#" (:id task) "</span>"
         "<span class=\"task-title\">" (escape-html (:title task)) "</span>"
         status-chip
         (render-labels (:labels task))
         "</div>"
         (when host
           (str "<div class=\"task-foot\">"
                "▸ running on " (escape-html host)
                "</div>"))
         "</li>")))

(defn- render-objective [{:keys [name tasks] :as proj} sessions]
  (let [active (filter active? tasks)]
    (str "<section class=\"objective\">"
         "<header class=\"objective-head\">"
         "<h2>" (escape-html name) "</h2>"
         "<span class=\"subtle\">"
         (count active) " active · " (count tasks) " total"
         "</span>"
         "</header>"
         (if (empty? active)
           "<div class=\"objective-empty\">nothing now</div>"
           (str "<ul class=\"tasks\">"
                (apply str (map #(render-active-task % name sessions) active))
                "</ul>"))
         "</section>")))

(defn- render-idea [task proj-name]
  (str "<li class=\"idea\">"
       "<span class=\"task-id\">#" (:id task) "</span>"
       "<span class=\"task-title\">" (escape-html (:title task)) "</span>"
       "<span class=\"subtle\"> · " (escape-html proj-name) "</span>"
       (render-labels (:labels task))
       "</li>"))

(defn- render-idea-box [projects open?]
  (let [all-ideas (mapcat (fn [p] (map #(vector % (:name p)) (filter idea? (:tasks p))))
                          projects)]
    (if (empty? all-ideas)
      "<aside class=\"idea-box\"><button class=\"idea-toggle\" disabled>idea box · empty</button></aside>"
      (str "<aside class=\"idea-box" (when open? " open") "\">"
           "<button class=\"idea-toggle\" data-action=\"toggle-ideas\">"
           (if open? "▾" "▸") " idea box · " (count all-ideas)
           "</button>"
           (when open?
             (str "<ul class=\"ideas\">"
                  (apply str (map (fn [[t pn]] (render-idea t pn)) all-ideas))
                  "</ul>"))
           "</aside>"))))

(defn- render-topbar [{:keys [hosts projects phase err]}]
  (let [total-active (count (mapcat #(filter active? (:tasks %)) projects))
        host-count (count hosts)]
    (str "<header class=\"topbar\">"
         "<h1>sipag</h1>"
         "<span class=\"subtle\"> · what are we optimizing for</span>"
         "<span class=\"spacer\"></span>"
         (case phase
           :boot   "<span class=\"subtle\">loading…</span>"
           :error  (str "<span class=\"err-chip\">error: " (escape-html err) "</span>")
           (str "<span class=\"mesh-chip\" title=\"mesh\">" host-count
                (if (= 1 host-count) " host" " hosts")
                "</span>"
                "<span class=\"subtle\"> · " total-active " active</span>"))
         "</header>")))

(defn- render! [{:keys [projects sessions idea-box-open?] :as s}]
  (let [root (gdom/getElement "app")]
    (when root
      (set! (.-innerHTML root)
            (str (render-topbar s)
                 "<main class=\"board\">"
                 (cond
                   (empty? projects)
                   (str "<div class=\"empty-state\">"
                        "<h2>no objectives yet</h2>"
                        "<p>what are you optimizing for?</p>"
                        "<pre><code>sipag project add &lt;name&gt; --repo owner/repo\nsipag add &quot;first thing to ship&quot;</code></pre>"
                        "</div>")

                   :else
                   (apply str (map #(render-objective % sessions) projects)))
                 "</main>"
                 (render-idea-box projects idea-box-open?))))))

;; ── events ──────────────────────────────────────────────────────────

(defn- handle-click [ev]
  (let [t (.-target ev)
        btn (.closest t "[data-action]")]
    (when btn
      (let [action (.getAttribute btn "data-action")]
        (case action
          "toggle-ideas" (swap! state update :idea-box-open? not)
          nil)))))

;; ── wiring ──────────────────────────────────────────────────────────

(defn init
  "Entry point. shadow-cljs calls this from :init-fn."
  []
  (add-watch state ::render (fn [_ _ _ ns] (render! ns)))
  (render! @state)
  (.addEventListener js/document "click" handle-click)
  (.then (load-hosts!)
         (fn [_]
           (refresh-all!)
           (js/setInterval refresh-all! 5000))))
