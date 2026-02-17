#!/usr/bin/env bash
# sipag — pool manager: spawn up to N workers, reap finished ones

pool_start() {
  local project_dir="$1"
  local run_dir="$2"
  local foreground="${3:-false}"

  local pid_file="${run_dir}/sipag.pid"

  # Check if already running
  if [[ -f "$pid_file" ]]; then
    local existing_pid
    existing_pid=$(cat "$pid_file")
    if kill -0 "$existing_pid" 2>/dev/null; then
      die "sipag is already running (PID ${existing_pid}). Use 'sipag stop' first."
    else
      log_warn "Stale PID file found, removing"
      rm -f "$pid_file"
    fi
  fi

  if [[ "$foreground" == "true" ]]; then
    echo $$ > "$pid_file"
    trap 'pool_cleanup "$run_dir"; exit 0' INT TERM
    log_info "sipag started in foreground (PID $$)"
    log_info "Polling ${SIPAG_REPO} for issues labeled '${SIPAG_LABEL_READY}' every ${SIPAG_POLL_INTERVAL}s"
    log_info "Concurrency: ${SIPAG_CONCURRENCY}"
    _pool_loop "$project_dir" "$run_dir"
  else
    _pool_daemonize "$project_dir" "$run_dir" "$pid_file"
  fi
}

_pool_daemonize() {
  local project_dir="$1"
  local run_dir="$2"
  local pid_file="$3"
  local log_file="${run_dir}/logs/sipag.log"

  (
    echo $$ > "$pid_file"
    trap 'pool_cleanup "$run_dir"; exit 0' INT TERM

    log_info "sipag started as daemon (PID $$)"
    log_info "Polling ${SIPAG_REPO} for issues labeled '${SIPAG_LABEL_READY}' every ${SIPAG_POLL_INTERVAL}s"
    log_info "Concurrency: ${SIPAG_CONCURRENCY}"

    _pool_loop "$project_dir" "$run_dir"
  ) >> "$log_file" 2>&1 &

  local daemon_pid=$!
  disown "$daemon_pid" 2>/dev/null

  # Wait briefly so the inner shell writes its PID
  sleep 1

  if [[ -f "$pid_file" ]]; then
    local actual_pid
    actual_pid=$(cat "$pid_file")
    log_info "sipag started (PID ${actual_pid}). Logs: ${log_file}"
  else
    log_info "sipag started (PID ${daemon_pid}). Logs: ${log_file}"
  fi
}

_pool_loop() {
  local project_dir="$1"
  local run_dir="$2"

  while true; do
    _pool_reap_workers "$run_dir"

    local active_count
    active_count=$(_pool_active_count "$run_dir")

    if [[ "$active_count" -lt "$SIPAG_CONCURRENCY" ]]; then
      local slots=$(( SIPAG_CONCURRENCY - active_count ))

      # Fetch ready tasks
      local tasks
      tasks=$(source_list_tasks "$SIPAG_REPO" "$SIPAG_LABEL_READY" 2>/dev/null)

      if [[ -n "$tasks" ]]; then
        while IFS= read -r task_id && [[ "$slots" -gt 0 ]]; do
          # Skip if already being worked on
          if [[ -f "${run_dir}/workers/${task_id}.pid" ]]; then
            continue
          fi

          _pool_spawn_worker "$task_id" "$project_dir" "$run_dir"
          slots=$(( slots - 1 ))
        done <<< "$tasks"
      fi
    fi

    sleep "$SIPAG_POLL_INTERVAL"
  done
}

_pool_spawn_worker() {
  local task_id="$1"
  local project_dir="$2"
  local run_dir="$3"

  (
    worker_run "$task_id" "$project_dir" "$run_dir"
  ) &

  local worker_pid=$!
  echo "$worker_pid" > "${run_dir}/workers/${task_id}.pid"
  echo "$task_id" > "${run_dir}/workers/${worker_pid}.task"

  log_info "Spawned worker for task #${task_id} (PID ${worker_pid})"
}

_pool_reap_workers() {
  local run_dir="$1"

  for pid_file in "${run_dir}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue

    local task_id
    task_id=$(basename "$pid_file" .pid)
    # Skip non-numeric (like PID.task files matched by glob)
    [[ "$task_id" =~ ^[0-9]+$ ]] || continue

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
    [[ "$task_id" =~ ^[0-9]+$ ]] || continue

    local worker_pid
    worker_pid=$(cat "$pid_file")

    if kill -0 "$worker_pid" 2>/dev/null; then
      count=$(( count + 1 ))
    fi
  done

  echo "$count"
}

pool_status() {
  local run_dir="$1"
  local pid_file="${run_dir}/sipag.pid"

  if [[ ! -f "$pid_file" ]]; then
    echo "sipag is not running"
    return 1
  fi

  local main_pid
  main_pid=$(cat "$pid_file")

  if ! kill -0 "$main_pid" 2>/dev/null; then
    echo "sipag is not running (stale PID file)"
    return 1
  fi

  echo "sipag is running (PID ${main_pid})"
  echo ""

  local active=0
  for pid_file in "${run_dir}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue

    local task_id
    task_id=$(basename "$pid_file" .pid)
    [[ "$task_id" =~ ^[0-9]+$ ]] || continue

    local worker_pid
    worker_pid=$(cat "$pid_file")

    if kill -0 "$worker_pid" 2>/dev/null; then
      echo "  Worker PID ${worker_pid} → task #${task_id} (running)"
      active=$(( active + 1 ))
    else
      echo "  Worker PID ${worker_pid} → task #${task_id} (finished)"
    fi
  done

  if [[ "$active" -eq 0 ]]; then
    echo "  No active workers (waiting for tasks)"
  fi

  echo ""
  echo "Concurrency: ${SIPAG_CONCURRENCY} | Poll interval: ${SIPAG_POLL_INTERVAL}s"
}

pool_stop() {
  local run_dir="$1"
  local pid_file="${run_dir}/sipag.pid"

  if [[ ! -f "$pid_file" ]]; then
    echo "sipag is not running"
    return 1
  fi

  local main_pid
  main_pid=$(cat "$pid_file")

  if ! kill -0 "$main_pid" 2>/dev/null; then
    echo "sipag is not running (cleaning up stale PID file)"
    rm -f "$pid_file"
    return 1
  fi

  echo "Stopping sipag (PID ${main_pid})..."

  # Kill workers first
  for wpid_file in "${run_dir}/workers/"*.pid; do
    [[ -f "$wpid_file" ]] || continue

    local task_id
    task_id=$(basename "$wpid_file" .pid)
    [[ "$task_id" =~ ^[0-9]+$ ]] || continue

    local worker_pid
    worker_pid=$(cat "$wpid_file")

    if kill -0 "$worker_pid" 2>/dev/null; then
      log_info "Stopping worker PID ${worker_pid} (task #${task_id})"
      kill "$worker_pid" 2>/dev/null
    fi

    rm -f "$wpid_file"
    rm -f "${run_dir}/workers/${worker_pid}.task"
  done

  # Kill main process
  kill "$main_pid" 2>/dev/null
  rm -f "$pid_file"

  echo "sipag stopped"
}

pool_cleanup() {
  local run_dir="$1"

  log_info "Cleaning up..."

  for wpid_file in "${run_dir}/workers/"*.pid; do
    [[ -f "$wpid_file" ]] || continue

    local worker_pid
    worker_pid=$(cat "$wpid_file")

    if kill -0 "$worker_pid" 2>/dev/null; then
      kill "$worker_pid" 2>/dev/null
    fi
  done

  rm -f "${run_dir}/workers/"*.pid
  rm -f "${run_dir}/workers/"*.task
  rm -f "${run_dir}/sipag.pid"

  log_info "Cleanup complete"
}
