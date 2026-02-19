#!/usr/bin/env bash
# sipag — pool manager: multi-project daemon with worker pool

pool_start() {
  local home="$1"
  local foreground="${2:-false}"

  local pid_file="${home}/sipag.pid"
  mkdir -p "${home}/logs"

  # Check if already running
  if [[ -f "$pid_file" ]]; then
    local existing_pid
    existing_pid=$(cat "$pid_file")
    if kill -0 "$existing_pid" 2>/dev/null; then
      die "sipag daemon is already running (PID ${existing_pid}). Use 'sipag daemon stop' first."
    else
      log_warn "Stale PID file found, removing"
      rm -f "$pid_file"
    fi
  fi

  if [[ "$foreground" == "true" ]]; then
    echo $$ >"$pid_file"
    trap 'pool_cleanup "$home"; exit 0' INT TERM
    log_info "sipag daemon started in foreground (PID $$)"
    log_info "Polling all projects every ${SIPAG_POLL_INTERVAL}s"
    log_info "Global max workers: ${SIPAG_MAX_WORKERS}"
    _pool_loop "$home"
  else
    _pool_daemonize "$home" "$pid_file"
  fi
}

_pool_daemonize() {
  local home="$1"
  local pid_file="$2"
  local log_file="${home}/logs/sipag.log"

  (
    echo $$ >"$pid_file"
    trap 'pool_cleanup "$home"; exit 0' INT TERM

    log_info "sipag daemon started (PID $$)"
    log_info "Polling all projects every ${SIPAG_POLL_INTERVAL}s"
    log_info "Global max workers: ${SIPAG_MAX_WORKERS}"

    _pool_loop "$home"
  ) >>"$log_file" 2>&1 &

  local daemon_pid=$!
  disown "$daemon_pid" 2>/dev/null

  # Wait briefly so the inner shell writes its PID
  sleep 1

  if [[ -f "$pid_file" ]]; then
    local actual_pid
    actual_pid=$(cat "$pid_file")
    log_info "sipag daemon started (PID ${actual_pid}). Logs: ${log_file}"
  else
    log_info "sipag daemon started (PID ${daemon_pid}). Logs: ${log_file}"
  fi
}

_pool_loop() {
  local home="$1"

  while true; do
    # Count total active workers across all projects
    local total_active=0

    for slug in $(config_list_projects); do
      local project_dir
      project_dir="$(config_get_project_dir "$slug")"
      config_ensure_project_dir "$slug" >/dev/null

      _pool_reap_workers "$project_dir"

      local project_active
      project_active=$(_pool_active_count "$project_dir")
      total_active=$((total_active + project_active))
    done

    # Now spawn workers for each project
    for slug in $(config_list_projects); do
      # Load this project's config
      export SIPAG_PROJECT_SLUG="$slug"
      config_load_project "$slug"
      _load_source_plugin

      local project_dir
      project_dir="$(config_get_project_dir "$slug")"

      local project_active
      project_active=$(_pool_active_count "$project_dir")

      # Respect per-project concurrency AND global max
      local project_slots=$((SIPAG_CONCURRENCY - project_active))
      local global_slots=$((SIPAG_MAX_WORKERS - total_active))
      local slots=$((project_slots < global_slots ? project_slots : global_slots))

      if [[ "$slots" -le 0 ]]; then
        continue
      fi

      # Fetch ready tasks
      local tasks
      tasks=$(source_list_tasks "$SIPAG_REPO" "$SIPAG_LABEL_READY" 2>/dev/null) || true

      if [[ -n "$tasks" ]]; then
        while IFS= read -r task_id && [[ "$slots" -gt 0 ]]; do
          [[ -z "$task_id" ]] && continue

          # Skip if already being worked on
          if [[ -f "${project_dir}/workers/${task_id}.pid" ]]; then
            continue
          fi

          _pool_spawn_worker "$task_id" "$project_dir"
          slots=$((slots - 1))
          total_active=$((total_active + 1))
        done <<<"$tasks"
      fi
    done

    # Reload global config for poll interval
    config_load_global 2>/dev/null || true
    sleep "${SIPAG_POLL_INTERVAL:-60}"
  done
}

_pool_spawn_worker() {
  local task_id="$1"
  local run_dir="$2"

  (
    worker_run "$task_id" "$run_dir"
  ) &

  local worker_pid=$!
  echo "$worker_pid" >"${run_dir}/workers/${task_id}.pid"
  echo "$task_id" >"${run_dir}/workers/${worker_pid}.task"

  log_info "Spawned worker for task #${task_id} (PID ${worker_pid})"
}

_pool_reap_workers() {
  local run_dir="$1"

  for pid_file in "${run_dir}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue

    local task_id
    task_id=$(basename "$pid_file" .pid)
    # Skip non-task files (PID.task files matched by glob)
    [[ "$task_id" =~ ^[a-zA-Z0-9]+$ ]] || continue

    local worker_pid
    worker_pid=$(cat "$pid_file")

    if ! kill -0 "$worker_pid" 2>/dev/null; then
      log_debug "Reaped worker for task #${task_id} (PID ${worker_pid})"
      rm -f "$pid_file"
      rm -f "${run_dir}/workers/${worker_pid}.task"
    fi
  done
}

_pool_active_count() {
  local run_dir="$1"
  local count=0

  for pid_file in "${run_dir}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue

    local task_id
    task_id=$(basename "$pid_file" .pid)
    [[ "$task_id" =~ ^[a-zA-Z0-9]+$ ]] || continue

    local worker_pid
    worker_pid=$(cat "$pid_file")

    if kill -0 "$worker_pid" 2>/dev/null; then
      count=$((count + 1))
    fi
  done

  echo "$count"
}

pool_status() {
  local home="$1"
  local pid_file="${home}/sipag.pid"

  if [[ ! -f "$pid_file" ]]; then
    echo "sipag daemon is not running"
    return 1
  fi

  local main_pid
  main_pid=$(cat "$pid_file")

  if ! kill -0 "$main_pid" 2>/dev/null; then
    echo "sipag daemon is not running (stale PID file)"
    return 1
  fi

  echo "sipag daemon is running (PID ${main_pid})"
  echo ""

  local total_active=0
  for slug in $(config_list_projects); do
    local project_dir
    project_dir="$(config_get_project_dir "$slug")"

    local active=0
    echo "  Project: ${slug}"

    for pid_file_w in "${project_dir}/workers/"*.pid; do
      [[ -f "$pid_file_w" ]] || continue

      local task_id
      task_id=$(basename "$pid_file_w" .pid)
      [[ "$task_id" =~ ^[a-zA-Z0-9]+$ ]] || continue

      local worker_pid
      worker_pid=$(cat "$pid_file_w")

      if kill -0 "$worker_pid" 2>/dev/null; then
        echo "    Worker PID ${worker_pid} → task #${task_id} (running)"
        active=$((active + 1))
      fi
    done

    if [[ "$active" -eq 0 ]]; then
      echo "    No active workers"
    fi
    total_active=$((total_active + active))
  done

  echo ""
  echo "Total active workers: ${total_active}/${SIPAG_MAX_WORKERS}"
  echo "Poll interval: ${SIPAG_POLL_INTERVAL:-60}s"
}

pool_stop() {
  local home="$1"
  local pid_file="${home}/sipag.pid"

  if [[ ! -f "$pid_file" ]]; then
    echo "sipag daemon is not running"
    return 1
  fi

  local main_pid
  main_pid=$(cat "$pid_file")

  if ! kill -0 "$main_pid" 2>/dev/null; then
    echo "sipag daemon is not running (cleaning up stale PID file)"
    rm -f "$pid_file"
    return 1
  fi

  echo "Stopping sipag daemon (PID ${main_pid})..."

  # Kill workers across all projects
  for slug in $(config_list_projects); do
    local project_dir
    project_dir="$(config_get_project_dir "$slug")"

    for wpid_file in "${project_dir}/workers/"*.pid; do
      [[ -f "$wpid_file" ]] || continue

      local task_id
      task_id=$(basename "$wpid_file" .pid)
      [[ "$task_id" =~ ^[a-zA-Z0-9]+$ ]] || continue

      local worker_pid
      worker_pid=$(cat "$wpid_file")

      if kill -0 "$worker_pid" 2>/dev/null; then
        log_info "Stopping worker PID ${worker_pid} (task #${task_id})"
        kill "$worker_pid" 2>/dev/null
      fi

      rm -f "$wpid_file"
      rm -f "${project_dir}/workers/${worker_pid}.task"
    done
  done

  # Kill main process
  kill "$main_pid" 2>/dev/null
  rm -f "$pid_file"

  echo "sipag daemon stopped"
}

pool_cleanup() {
  local home="$1"

  log_info "Cleaning up..."

  for slug in $(config_list_projects 2>/dev/null); do
    local project_dir
    project_dir="$(config_get_project_dir "$slug")"

    for wpid_file in "${project_dir}/workers/"*.pid; do
      [[ -f "$wpid_file" ]] || continue

      local worker_pid
      worker_pid=$(cat "$wpid_file")

      if kill -0 "$worker_pid" 2>/dev/null; then
        kill "$worker_pid" 2>/dev/null
      fi
    done

    rm -f "${project_dir}/workers/"*.pid
    rm -f "${project_dir}/workers/"*.task
  done

  rm -f "${home}/sipag.pid"

  log_info "Cleanup complete"
}
