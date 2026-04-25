(ns sipag.app
  "OKR-shaped agent manager SPA.

   Three top-level sections:

     OBJECTIVES — projects with kind='objective'.
       Each has key results (KRs) with a traffic-light stance.
       Active tasks render under each KR they advance (duplicated if
       a task touches multiple KRs — that duplication is honest;
       you're seeing each KR's progress, not a task funnel).
       Tasks not bound to any KR render in a 'loose' bucket inside
       the objective.

     STANDING — projects with kind='standing'.
       Perpetual upkeep and one-off firefights. Just tasks, no KRs.

     IDEAS — every task with status='backlog' across all projects.
       The parking lot. Collapsible drawer at the bottom.

   Above-the-line (Objectives + KRs) is the human's surface.
   Below-the-line (Tasks) is meant for agents to own — but right now
   the human still drives task creation and stance changes.

   Backed by:
     GET    /api/projects
     POST   /api/projects                              {name, repo, kind?}
     POST   /api/projects/:n/key-results               {title}
     PATCH  /api/projects/:n/key-results/:id           {title?, stance?}
     POST   /api/projects/:n/tasks                     {title, role?, labels?, key_results?}
     PATCH  /api/projects/:n/tasks/:id                 {status?, title?, key_results?, ...}
     GET    /api/hosts
     GET    /api/hosts/:id/sessions"
  (:require
    [clojure.string :as str]
    [goog.dom :as gdom]))

;; ── state ───────────────────────────────────────────────────────────

(defonce state
  (atom {:projects []
         :hosts []
         :sessions {}
         :idea-box-open? false
         :forms {}      ; form-key (e.g. ":new-objective" or ":new-task::agent-manager") → input map
         :err nil
         :phase :boot}))

;; ── http helpers ───────────────────────────────────────────────────

(defn- fetch-json [url]
  (-> (js/fetch url)
      (.then (fn [resp]
               (if (.-ok resp)
                 (.json resp)
                 (throw (ex-info (str "HTTP " (.-status resp)) {:url url})))))
      (.then #(js->clj % :keywordize-keys true))))

(defn- send-json
  "POST or PATCH a JSON body. Returns a promise resolving to the parsed
   response body. Rejects with the error message text on non-2xx."
  [method url body]
  (let [opts #js {:method method
                  :headers #js {"Content-Type" "application/json"}
                  :body (.stringify js/JSON (clj->js body))}]
    (-> (js/fetch url opts)
        (.then (fn [resp]
                 (if (.-ok resp)
                   (.json resp)
                   (.then (.text resp)
                          (fn [t] (throw (ex-info t {:status (.-status resp)})))))))
        (.then #(js->clj % :keywordize-keys true)))))

;; ── data fetch ─────────────────────────────────────────────────────

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

;; ── derivations ────────────────────────────────────────────────────

(def ^:private active-statuses #{"todo" "in-progress" "review"})
(def ^:private idea-statuses   #{"backlog"})

(defn- active? [t]  (contains? active-statuses (:status t)))
(defn- idea?   [t]  (contains? idea-statuses (:status t)))

(defn- objective? [p] (= "objective" (:kind p)))
(defn- standing?  [p] (= "standing"  (:kind p)))

(defn- task-running-on
  "Return host id running this task's `<project>--<role>` session, or nil."
  [task project-name sessions-by-host]
  (let [needle (str project-name "--" (:role task))]
    (some (fn [[host-id sessions]]
            (when (some #(= needle (:name %)) sessions)
              host-id))
          sessions-by-host)))

(def ^:private stance-cycle
  ;; Click cycles: green → yellow → red → done → green
  {"green" "yellow", "yellow" "red", "red" "done", "done" "green"})

(def ^:private status-cycle
  ;; Click cycles: todo → in-progress → review → done → backlog → todo
  {"todo"        "in-progress"
   "in-progress" "review"
   "review"      "done"
   "done"        "backlog"
   "backlog"     "todo"})

;; ── escaping + small DOM helpers ───────────────────────────────────

(defn- escape-html [s]
  (when s
    (-> (str s)
        (.replace (js/RegExp "&" "g") "&amp;")
        (.replace (js/RegExp "<" "g") "&lt;")
        (.replace (js/RegExp ">" "g") "&gt;")
        (.replace (js/RegExp "\"" "g") "&quot;"))))

(defn- attr [s] (escape-html s))

;; ── rendering: tasks ───────────────────────────────────────────────

(defn- render-labels [labels]
  (when (seq labels)
    (str "<span class=\"labels\">"
         (apply str (for [l labels]
                      (str "<span class=\"label\">" (escape-html l) "</span>")))
         "</span>")))

(defn- render-task [task project-name sessions-by-host]
  (let [host (task-running-on task project-name sessions-by-host)
        status (:status task)
        ;; Status chip is always rendered now — it's a clickable
        ;; cycle button. Hidden-by-default `todo` becomes visible
        ;; so the user has a target to click on.
        chip (str "<button class=\"status-chip " status "\""
                  " data-action=\"cycle-status\""
                  " data-project=\"" (attr project-name) "\""
                  " data-task=\"" (:id task) "\""
                  " data-status=\"" status "\""
                  " title=\"" status " — click to cycle\">"
                  status "</button>")]
    (str "<li class=\"task\">"
         "<div class=\"task-head\">"
         "<span class=\"task-id\">#" (:id task) "</span>"
         "<span class=\"task-title\">" (escape-html (:title task)) "</span>"
         chip
         (render-labels (:labels task))
         "<button class=\"row-delete\""
         " data-action=\"delete-task\""
         " data-project=\"" (attr project-name) "\""
         " data-task=\"" (:id task) "\""
         " title=\"delete task\">×</button>"
         "</div>"
         (when host
           (str "<div class=\"task-foot\">▸ running on " (escape-html host) "</div>"))
         "</li>")))

;; ── rendering: KR row ──────────────────────────────────────────────

(defn- stance-symbol [stance]
  (case stance
    "green"  "●"
    "yellow" "◐"
    "red"    "○"
    "done"   "✓"
    "·"))

(defn- render-kr-row [kr proj-name kr-tasks sessions]
  (str "<div class=\"kr\">"
       "<div class=\"kr-head\">"
       "<button class=\"kr-stance " (escape-html (:stance kr)) "\""
       " data-action=\"cycle-stance\""
       " data-project=\"" (attr proj-name) "\""
       " data-kr=\"" (:id kr) "\""
       " title=\"" (escape-html (:stance kr)) " — click to cycle\">"
       (stance-symbol (:stance kr))
       "</button>"
       "<span class=\"kr-title\">" (escape-html (:title kr)) "</span>"
       "<button class=\"row-delete\""
       " data-action=\"delete-kr\""
       " data-project=\"" (attr proj-name) "\""
       " data-kr=\"" (:id kr) "\""
       " title=\"delete KR\">×</button>"
       "</div>"
       (when (seq kr-tasks)
         (str "<ul class=\"tasks kr-tasks\">"
              (apply str (map #(render-task % proj-name sessions) kr-tasks))
              "</ul>"))
       "</div>"))

;; ── rendering: forms ───────────────────────────────────────────────

(defn- render-form
  "Inline form keyed by `form-key`. open? controls visibility; the
   form's input fields read from state.forms[form-key]."
  [form-key open? fields submit-action submit-label]
  (str "<div class=\"form" (when open? " open") "\">"
       (if-not open?
         (str "<button class=\"form-toggle\" data-action=\"open-form\""
              " data-form=\"" (attr form-key) "\">+ " (escape-html submit-label) "</button>")
         (str "<div class=\"form-body\">"
              (apply str
                     (for [{:keys [name label placeholder]} fields]
                       (str "<input class=\"form-input\""
                            " data-action=\"form-input\""
                            " data-form=\"" (attr form-key) "\""
                            " data-field=\"" (attr name) "\""
                            " placeholder=\"" (attr placeholder) "\""
                            " aria-label=\"" (attr label) "\">")))
              "<div class=\"form-actions\">"
              "<button data-action=\"" (attr submit-action) "\""
              " data-form=\"" (attr form-key) "\""
              " class=\"form-submit\">" (escape-html submit-label) "</button>"
              "<button data-action=\"close-form\""
              " data-form=\"" (attr form-key) "\""
              " class=\"form-cancel\">cancel</button>"
              "</div>"
              "</div>"))
       "</div>"))

;; ── rendering: objective card ──────────────────────────────────────

(defn- render-objective [{:keys [name tasks key_results]} sessions forms]
  (let [active-tasks (filter active? tasks)
        kr-form-key (str ":new-kr::" name)
        task-form-key (str ":new-task::" name)
        kr-form-open? (contains? forms (keyword kr-form-key))
        task-form-open? (contains? forms (keyword task-form-key))
        ;; Tasks under each KR (duplicated across KRs they advance)
        kr-bucket (into {}
                        (for [kr key_results]
                          [(:id kr)
                           (filter #(some #{(:id kr)} (:key_results %))
                                   active-tasks)]))
        loose (filter #(empty? (:key_results %)) active-tasks)]
    (str "<section class=\"objective\">"
         "<header class=\"objective-head\">"
         "<h2>" (escape-html name) "</h2>"
         "<span class=\"subtle\">"
         (count active-tasks) " active · " (count key_results) " KR"
         (when (not= 1 (count key_results)) "s")
         "</span>"
         "<button class=\"row-delete\""
         " data-action=\"delete-project\""
         " data-project=\"" (attr name) "\""
         " title=\"delete objective\">×</button>"
         "</header>"

         (if (empty? key_results)
           "<div class=\"objective-empty\">no key results yet — what does success look like?</div>"
           (apply str
                  (for [kr key_results]
                    (render-kr-row kr name (get kr-bucket (:id kr)) sessions))))

         (when (seq loose)
           (str "<div class=\"loose\">"
                "<div class=\"loose-head\">loose <span class=\"subtle\">no KR</span></div>"
                "<ul class=\"tasks\">"
                (apply str (map #(render-task % name sessions) loose))
                "</ul>"
                "</div>"))

         "<div class=\"objective-actions\">"
         (render-form kr-form-key kr-form-open?
                      [{:name "title" :label "KR title"
                        :placeholder "what does success look like for this objective?"}]
                      "submit-kr" "key result")
         (render-form task-form-key task-form-open?
                      [{:name "title" :label "task title"
                        :placeholder "what's the next thing to ship?"}
                       {:name "kr" :label "KR id (optional)"
                        :placeholder "kr id, e.g. 1"}
                       {:name "labels" :label "labels"
                        :placeholder "labels, comma separated"}]
                      "submit-task" "task")
         "</div>"
         "</section>")))

;; ── rendering: standing card ───────────────────────────────────────

(defn- render-standing [{:keys [name tasks]} sessions forms]
  (let [active-tasks (filter active? tasks)
        task-form-key (str ":new-task::" name)
        task-form-open? (contains? forms (keyword task-form-key))]
    (str "<section class=\"standing\">"
         "<header class=\"objective-head\">"
         "<h2>" (escape-html name) "</h2>"
         "<span class=\"subtle\">" (count active-tasks) " active</span>"
         "<button class=\"row-delete\""
         " data-action=\"delete-project\""
         " data-project=\"" (attr name) "\""
         " title=\"delete standing\">×</button>"
         "</header>"
         (if (empty? active-tasks)
           "<div class=\"objective-empty\">nothing now</div>"
           (str "<ul class=\"tasks\">"
                (apply str (map #(render-task % name sessions) active-tasks))
                "</ul>"))
         "<div class=\"objective-actions\">"
         (render-form task-form-key task-form-open?
                      [{:name "title" :label "concern"
                        :placeholder "what's the upkeep or the firefight?"}
                       {:name "labels" :label "labels"
                        :placeholder "labels, comma separated"}]
                      "submit-task" "concern")
         "</div>"
         "</section>")))

;; ── topbar + idea box + empty state ────────────────────────────────

(defn- render-topbar [{:keys [hosts projects phase err]}]
  (let [total-active (count (mapcat #(filter active? (:tasks %)) projects))
        host-count (count hosts)]
    (str "<header class=\"topbar\">"
         "<h1>sipag</h1>"
         "<span class=\"subtle\"> · what we're optimizing for</span>"
         "<span class=\"spacer\"></span>"
         (case phase
           :boot   "<span class=\"subtle\">loading…</span>"
           :error  (str "<span class=\"err-chip\">error: " (escape-html err) "</span>")
           (str "<span class=\"mesh-chip\">" host-count
                (if (= 1 host-count) " host" " hosts")
                "</span>"
                "<span class=\"subtle\"> · " total-active " active</span>"))
         "</header>")))

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
                  (apply str
                         (for [[t pn] all-ideas]
                           (str "<li class=\"idea\">"
                                "<span class=\"task-id\">#" (:id t) "</span>"
                                "<span class=\"task-title\">" (escape-html (:title t)) "</span>"
                                "<span class=\"subtle\"> · " (escape-html pn) "</span>"
                                "<button class=\"idea-activate\""
                                " data-action=\"activate-idea\""
                                " data-project=\"" (attr pn) "\""
                                " data-task=\"" (:id t) "\""
                                " title=\"promote to active\">→ activate</button>"
                                "<button class=\"row-delete\""
                                " data-action=\"delete-task\""
                                " data-project=\"" (attr pn) "\""
                                " data-task=\"" (:id t) "\""
                                " title=\"delete\">×</button>"
                                "</li>")))
                  "</ul>"))
           "</aside>"))))

(defn- render-empty []
  (str "<div class=\"empty-state\">"
       "<h2>no objectives yet</h2>"
       "<p>what are you optimizing for?</p>"
       "<pre><code>sipag project add &lt;name&gt; --repo owner/repo</code></pre>"
       "<p class=\"subtle\">or use the + objective button below.</p>"
       "</div>"))

;; ── top-level render ───────────────────────────────────────────────

(defn- render! [{:keys [projects sessions idea-box-open? forms] :as s}]
  (let [root (gdom/getElement "app")
        objs (filter objective? projects)
        stand (filter standing? projects)
        new-obj-form-open? (contains? forms :new-objective)
        new-standing-form-open? (contains? forms :new-standing)]
    (when root
      (set! (.-innerHTML root)
            (str (render-topbar s)
                 "<main class=\"board\">"

                 ;; OBJECTIVES section
                 "<div class=\"section-head\">objectives</div>"
                 (if (empty? objs)
                   (render-empty)
                   (apply str (map #(render-objective % sessions forms) objs)))
                 "<div class=\"section-actions\">"
                 (render-form ":new-objective" new-obj-form-open?
                              [{:name "name" :label "name"
                                :placeholder "objective name"}
                               {:name "repo" :label "repo"
                                :placeholder "owner/repo (optional)"}]
                              "submit-objective" "objective")
                 "</div>"

                 ;; STANDING section
                 "<div class=\"section-head\">standing</div>"
                 (if (empty? stand)
                   "<div class=\"objective-empty subtle\">no standing concerns yet</div>"
                   (apply str (map #(render-standing % sessions forms) stand)))
                 "<div class=\"section-actions\">"
                 (render-form ":new-standing" new-standing-form-open?
                              [{:name "name" :label "name"
                                :placeholder "standing concern (e.g. ops, hygiene)"}]
                              "submit-standing" "standing concern")
                 "</div>"

                 "</main>"
                 (render-idea-box projects idea-box-open?))))))

;; ── form input handling ────────────────────────────────────────────

(defn- form-key->kw [s] (keyword s))

(defn- get-form [form-key]
  (get-in @state [:forms (form-key->kw form-key)] {}))

(defn- assoc-form-field! [form-key field value]
  (swap! state assoc-in
         [:forms (form-key->kw form-key) (keyword field)]
         value))

(defn- open-form! [form-key]
  (swap! state assoc-in [:forms (form-key->kw form-key)] {}))

(defn- close-form! [form-key]
  (swap! state update :forms dissoc (form-key->kw form-key)))

;; ── event handlers ─────────────────────────────────────────────────

(defn- parse-labels [s]
  (->> (str/split (or s "") #",")
       (map str/trim)
       (remove str/blank?)
       (vec)))

(defn- after-mutation! []
  (load-projects!))

(defn- handle-submit-objective [_form-key]
  (let [{:keys [name repo]} (get-form ":new-objective")]
    (when-not (str/blank? name)
      (-> (send-json "POST" "/api/projects"
                     {:name name :repo (or repo "") :kind "objective"})
          (.then (fn [_]
                   (close-form! ":new-objective")
                   (after-mutation!)))
          (.catch (fn [err]
                    (js/alert (str "create objective failed: " (.-message err)))))))))

(defn- handle-submit-standing [_form-key]
  (let [{:keys [name]} (get-form ":new-standing")]
    (when-not (str/blank? name)
      (-> (send-json "POST" "/api/projects"
                     {:name name :repo "" :kind "standing"})
          (.then (fn [_]
                   (close-form! ":new-standing")
                   (after-mutation!)))
          (.catch (fn [err]
                    (js/alert (str "create standing failed: " (.-message err)))))))))

(defn- handle-submit-kr [form-key]
  (let [{:keys [title]} (get-form form-key)
        proj (last (str/split form-key #"::"))]
    (when-not (str/blank? title)
      (-> (send-json "POST" (str "/api/projects/" (js/encodeURIComponent proj) "/key-results")
                     {:title title})
          (.then (fn [_]
                   (close-form! form-key)
                   (after-mutation!)))
          (.catch (fn [err]
                    (js/alert (str "create KR failed: " (.-message err)))))))))

(defn- handle-submit-task [form-key]
  (let [{:keys [title kr labels]} (get-form form-key)
        proj (last (str/split form-key #"::"))
        kr-id (when-not (str/blank? kr) (js/parseInt kr 10))
        body (cond-> {:title title}
               (and kr-id (not (js/isNaN kr-id))) (assoc :key_results [kr-id])
               (seq labels)                       (assoc :labels (parse-labels labels)))]
    (when-not (str/blank? title)
      (-> (send-json "POST" (str "/api/projects/" (js/encodeURIComponent proj) "/tasks")
                     body)
          (.then (fn [_]
                   (close-form! form-key)
                   (after-mutation!)))
          (.catch (fn [err]
                    (js/alert (str "create task failed: " (.-message err)))))))))

(defn- handle-cycle-stance [proj kr-id]
  (let [project (some #(when (= proj (:name %)) %) (:projects @state))
        kr (some #(when (= (js/parseInt kr-id 10) (:id %)) %) (:key_results project))
        next (get stance-cycle (:stance kr) "green")]
    (-> (send-json "PATCH"
                   (str "/api/projects/" (js/encodeURIComponent proj)
                        "/key-results/" kr-id)
                   {:stance next})
        (.then (fn [_] (after-mutation!)))
        (.catch (fn [err]
                  (js/alert (str "stance update failed: " (.-message err))))))))

(defn- handle-cycle-status [proj task-id current]
  (let [next-status (get status-cycle current "todo")]
    (-> (send-json "PATCH"
                   (str "/api/projects/" (js/encodeURIComponent proj)
                        "/tasks/" task-id)
                   {:status next-status})
        (.then (fn [_] (after-mutation!)))
        (.catch (fn [err]
                  (js/alert (str "status update failed: " (.-message err))))))))

(defn- delete-with-confirm
  [method url confirm-msg]
  (when (js/confirm confirm-msg)
    (-> (js/fetch url #js {:method method})
        (.then (fn [resp]
                 (if (.-ok resp)
                   (after-mutation!)
                   (.then (.text resp)
                          (fn [t] (js/alert (str "delete failed: " t))))))))))

(defn- handle-delete-project [proj]
  (delete-with-confirm "DELETE"
                       (str "/api/projects/" (js/encodeURIComponent proj))
                       (str "Delete '" proj "'? This removes all KRs and tasks under it.")))

(defn- handle-delete-kr [proj kr-id]
  (delete-with-confirm "DELETE"
                       (str "/api/projects/" (js/encodeURIComponent proj)
                            "/key-results/" kr-id)
                       "Delete this key result? Tasks attached to it will become loose."))

(defn- handle-delete-task [proj task-id]
  (delete-with-confirm "DELETE"
                       (str "/api/projects/" (js/encodeURIComponent proj)
                            "/tasks/" task-id)
                       "Delete this task?"))

(defn- handle-activate-idea [proj task-id]
  (-> (send-json "PATCH"
                 (str "/api/projects/" (js/encodeURIComponent proj)
                      "/tasks/" task-id)
                 {:status "todo"})
      (.then (fn [_] (after-mutation!)))
      (.catch (fn [err]
                (js/alert (str "activate failed: " (.-message err)))))))

(defn- handle-click [ev]
  (let [t (.-target ev)
        btn (.closest t "[data-action]")]
    (when btn
      (let [action (.getAttribute btn "data-action")
            form (.getAttribute btn "data-form")
            project (.getAttribute btn "data-project")
            kr (.getAttribute btn "data-kr")
            task (.getAttribute btn "data-task")
            status (.getAttribute btn "data-status")]
        (case action
          "toggle-ideas"     (swap! state update :idea-box-open? not)
          "open-form"        (open-form! form)
          "close-form"       (close-form! form)
          "submit-objective" (handle-submit-objective form)
          "submit-standing"  (handle-submit-standing form)
          "submit-kr"        (handle-submit-kr form)
          "submit-task"      (handle-submit-task form)
          "cycle-stance"     (handle-cycle-stance project kr)
          "cycle-status"     (handle-cycle-status project task status)
          "delete-project"   (handle-delete-project project)
          "delete-kr"        (handle-delete-kr project kr)
          "delete-task"      (handle-delete-task project task)
          "activate-idea"    (handle-activate-idea project task)
          "form-input"       nil
          nil)))))

(defn- handle-input [ev]
  (let [t (.-target ev)]
    (when (and (.-getAttribute t)
               (= "form-input" (.getAttribute t "data-action")))
      (let [form (.getAttribute t "data-form")
            field (.getAttribute t "data-field")
            value (.-value t)]
        (assoc-form-field! form field value)))))

(defn- handle-keydown [ev]
  (let [t (.-target ev)]
    (when (and (.-getAttribute t)
               (= "form-input" (.getAttribute t "data-action"))
               (= "Enter" (.-key ev)))
      (let [form (.getAttribute t "data-form")
            ;; Find the form's submit button to dispatch the right action.
            container (.closest t ".form")
            submit (.querySelector container ".form-submit")]
        (when submit
          (.preventDefault ev)
          (.click submit))))))

;; ── wiring ─────────────────────────────────────────────────────────

(defn init
  "Entry point. shadow-cljs calls this from :init-fn."
  []
  (add-watch state ::render
             (fn [_ _ old new]
               ;; Skip a re-render if only :sessions changed (form
               ;; inputs would lose focus). Compare everything except
               ;; :sessions and :forms detail.
               (when (or (not= (dissoc old :sessions)
                               (dissoc new :sessions))
                         (not= (count (:forms old))
                               (count (:forms new))))
                 (render! new))))
  (render! @state)
  (.addEventListener js/document "click" handle-click)
  (.addEventListener js/document "input" handle-input)
  (.addEventListener js/document "keydown" handle-keydown)
  (.then (load-hosts!)
         (fn [_]
           (refresh-all!)
           (js/setInterval refresh-all! 5000))))
