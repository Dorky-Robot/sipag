#!/usr/bin/env bats
# sipag — pool module tests

load ../helpers/test-helpers
load ../helpers/mock-commands

# Helper: get a guaranteed-dead PID
_dead_pid() {
  bash -c 'exit 0' &
  local pid=$!
  wait "$pid" 2>/dev/null
  echo "$pid"
}

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
  source "${SIPAG_ROOT}/lib/core/worker.sh"
  source "${SIPAG_ROOT}/lib/core/pool.sh"

  export RUN_DIR="${TEST_TMPDIR}/run"
  mkdir -p "${RUN_DIR}/workers" "${RUN_DIR}/logs"
}

teardown() {
  [[ -d "${RUN_DIR:-}" ]] || { teardown_common; return 0; }
  # Kill any background processes we spawned
  for pid_file in "${RUN_DIR}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue
    local pid
    pid=$(cat "$pid_file" 2>/dev/null)
    kill "$pid" 2>/dev/null || true
  done
  # Kill any sleep processes spawned during tests
  jobs -p 2>/dev/null | while read -r p; do kill "$p" 2>/dev/null || true; done
  teardown_common
}

# --- _pool_active_count ---

@test "_pool_active_count: no workers → 0" {
  local count
  count=$(_pool_active_count "$RUN_DIR")
  [[ "$count" -eq 0 ]]
}

@test "_pool_active_count: live workers counted" {
  sleep 300 &
  local pid=$!
  echo "$pid" > "${RUN_DIR}/workers/42.pid"

  local count
  count=$(_pool_active_count "$RUN_DIR")
  [[ "$count" -eq 1 ]]

  kill "$pid" 2>/dev/null
}

@test "_pool_active_count: dead workers not counted" {
  local dead
  dead=$(_dead_pid)
  echo "$dead" > "${RUN_DIR}/workers/42.pid"

  local count
  count=$(_pool_active_count "$RUN_DIR")
  [[ "$count" -eq 0 ]]
}

@test "_pool_active_count: mix of live and dead" {
  sleep 300 &
  local live_pid=$!
  echo "$live_pid" > "${RUN_DIR}/workers/42.pid"

  local dead
  dead=$(_dead_pid)
  echo "$dead" > "${RUN_DIR}/workers/43.pid"

  local count
  count=$(_pool_active_count "$RUN_DIR")
  [[ "$count" -eq 1 ]]

  kill "$live_pid" 2>/dev/null
}

@test "_pool_active_count: ignores non-numeric pid files" {
  echo "content" > "${RUN_DIR}/workers/something.pid"
  sleep 300 &
  local pid=$!
  echo "$pid" > "${RUN_DIR}/workers/42.pid"

  local count
  count=$(_pool_active_count "$RUN_DIR")
  [[ "$count" -eq 1 ]]

  kill "$pid" 2>/dev/null
}

# --- _pool_reap_workers ---

@test "_pool_reap_workers: cleans dead PIDs" {
  local dead
  dead=$(_dead_pid)
  echo "$dead" > "${RUN_DIR}/workers/42.pid"
  echo "42" > "${RUN_DIR}/workers/${dead}.task"

  _pool_reap_workers "$RUN_DIR"

  [[ ! -f "${RUN_DIR}/workers/42.pid" ]]
  [[ ! -f "${RUN_DIR}/workers/${dead}.task" ]]
}

@test "_pool_reap_workers: preserves live PIDs" {
  sleep 300 &
  local pid=$!
  echo "$pid" > "${RUN_DIR}/workers/42.pid"
  echo "42" > "${RUN_DIR}/workers/${pid}.task"

  _pool_reap_workers "$RUN_DIR"

  [[ -f "${RUN_DIR}/workers/42.pid" ]]
  [[ -f "${RUN_DIR}/workers/${pid}.task" ]]

  kill "$pid" 2>/dev/null
}

# --- _pool_spawn_worker ---

@test "_pool_spawn_worker: creates PID + task files" {
  # Mock worker_run to just sleep
  worker_run() { sleep 300; }

  _pool_spawn_worker "42" "$PROJECT_DIR" "$RUN_DIR"

  # Poll for PID file (up to 2s) instead of fixed sleep
  local tries=0
  while [[ ! -f "${RUN_DIR}/workers/42.pid" ]] && [[ "$tries" -lt 20 ]]; do
    sleep 0.1
    tries=$((tries + 1))
  done

  [[ -f "${RUN_DIR}/workers/42.pid" ]]
  local worker_pid
  worker_pid=$(cat "${RUN_DIR}/workers/42.pid")
  [[ -f "${RUN_DIR}/workers/${worker_pid}.task" ]]

  local task_id
  task_id=$(cat "${RUN_DIR}/workers/${worker_pid}.task")
  [[ "$task_id" == "42" ]]

  kill "$worker_pid" 2>/dev/null
}

# --- pool_start ---

@test "pool_start: detects already-running instance" {
  sleep 300 &
  local pid=$!
  echo "$pid" > "${RUN_DIR}/sipag.pid"

  run pool_start "$PROJECT_DIR" "$RUN_DIR" "true"
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"already running"* ]]

  kill "$pid" 2>/dev/null
}

@test "pool_start: handles stale PID file" {
  local dead
  dead=$(_dead_pid)
  echo "$dead" > "${RUN_DIR}/sipag.pid"

  # Override _pool_loop to just exit
  _pool_loop() { return 0; }

  pool_start "$PROJECT_DIR" "$RUN_DIR" "true"

  # Stale PID was removed and new one written
  [[ -f "${RUN_DIR}/sipag.pid" ]]
}

# --- pool_stop ---

@test "pool_stop: kills workers and removes PID file" {
  sleep 300 &
  local main_pid=$!
  echo "$main_pid" > "${RUN_DIR}/sipag.pid"

  sleep 300 &
  local worker_pid=$!
  echo "$worker_pid" > "${RUN_DIR}/workers/42.pid"
  echo "42" > "${RUN_DIR}/workers/${worker_pid}.task"

  # Mock source plugin functions needed by pool_stop
  source_fail_task() { return 0; }

  pool_stop "$RUN_DIR"

  [[ ! -f "${RUN_DIR}/sipag.pid" ]]
  [[ ! -f "${RUN_DIR}/workers/42.pid" ]]

  # Workers should be dead
  ! kill -0 "$worker_pid" 2>/dev/null
}

@test "pool_stop: not running → exit 1" {
  run pool_stop "$RUN_DIR"
  [[ "$status" -ne 0 ]]
}

# --- pool_cleanup ---

@test "pool_cleanup: removes all tracking files" {
  sleep 300 &
  local pid=$!
  echo "$pid" > "${RUN_DIR}/workers/42.pid"
  echo "42" > "${RUN_DIR}/workers/${pid}.task"
  echo "$$" > "${RUN_DIR}/sipag.pid"

  pool_cleanup "$RUN_DIR"

  [[ ! -f "${RUN_DIR}/sipag.pid" ]]
  local remaining
  remaining=$(ls "${RUN_DIR}/workers/"*.pid 2>/dev/null | wc -l | tr -d ' ')
  [[ "$remaining" -eq 0 ]]

  kill "$pid" 2>/dev/null || true
}
