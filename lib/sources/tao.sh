#!/usr/bin/env bash
# sipag — tao source plugin
#
# Reads suspended actions from tao's SQLite database and uses them as work items.
# Requires: sqlite3 CLI (pre-installed on macOS)
#
# Config vars:
#   SIPAG_TAO_DB     — path to tao.db (default: ~/.tao/tao.db)
#   SIPAG_TAO_ACTION — action name to filter on (e.g., "implement-issue")

_tao_db_path() {
  echo "${SIPAG_TAO_DB:-${HOME}/.tao/tao.db}"
}

_tao_check_db() {
  local db
  db="$(_tao_db_path)"
  if [[ ! -f "$db" ]]; then
    log_error "tao database not found: ${db}"
    return 1
  fi
  if ! command -v sqlite3 &>/dev/null; then
    log_error "sqlite3 is required for the tao source plugin"
    return 1
  fi
}

source_list_tasks() {
  local _repo="$1" _label="$2"

  _tao_check_db || return 1

  local db action_filter
  db="$(_tao_db_path)"
  action_filter="${SIPAG_TAO_ACTION:-}"

  local query="SELECT tracking_id FROM tao_suspended_actions WHERE status='waiting_for_reply' AND archived=0"
  if [[ -n "$action_filter" ]]; then
    query+=" AND action_name='${action_filter}'"
  fi

  sqlite3 "$db" "$query" 2>/dev/null
}

source_claim_task() {
  local _repo="$1" task_id="$2" _wip_label="$3" _ready_label="$4"

  _tao_check_db || return 1

  # If tao CLI is available, use it to pause the action
  if command -v tao &>/dev/null; then
    tao pause "$task_id" 2>/dev/null || {
      log_warn "tao pause failed for ${task_id}, updating DB directly"
      _tao_update_status "$task_id" "paused"
    }
  else
    _tao_update_status "$task_id" "paused"
  fi
}

source_get_task() {
  local _repo="$1" task_id="$2"

  _tao_check_db || return 1

  local db
  db="$(_tao_db_path)"

  local prompt_text stdin_data action_name
  prompt_text=$(sqlite3 "$db" "SELECT prompt_text FROM tao_suspended_actions WHERE tracking_id='${task_id}'" 2>/dev/null)
  stdin_data=$(sqlite3 "$db" "SELECT stdin_data FROM tao_suspended_actions WHERE tracking_id='${task_id}'" 2>/dev/null)
  action_name=$(sqlite3 "$db" "SELECT action_name FROM tao_suspended_actions WHERE tracking_id='${task_id}'" 2>/dev/null)

  local title="${action_name}: ${prompt_text:0:60}"
  local body="${prompt_text}"
  if [[ -n "$stdin_data" ]]; then
    body="${body}\n\nContext:\n${stdin_data}"
  fi

  echo "TASK_TITLE=${title}"
  echo "TASK_BODY=${body}"
  echo "TASK_NUMBER=${task_id}"
  echo "TASK_URL=tao://${task_id}"
}

source_complete_task() {
  local _repo="$1" task_id="$2" _done_label="$3" _wip_label="$4" pr_url="$5"

  local response="Completed."
  [[ -n "$pr_url" ]] && response="PR opened: ${pr_url}"

  # If tao CLI is available, use it to resume
  if command -v tao &>/dev/null; then
    tao resume "$task_id" --response "$response" 2>/dev/null || {
      log_warn "tao resume failed for ${task_id}, updating DB directly"
      _tao_update_status "$task_id" "completed"
    }
  else
    _tao_update_status "$task_id" "completed"
  fi
}

source_fail_task() {
  local _repo="$1" task_id="$2" _ready_label="$3" _wip_label="$4" error_msg="$5"

  # Return to waiting state so it can be retried
  _tao_update_status "$task_id" "waiting_for_reply"

  if [[ -n "$error_msg" ]]; then
    log_warn "tao task ${task_id} failed: ${error_msg}"
  fi
}

source_comment() {
  local _repo="$1" _task_id="$2" _message="$3"
  # tao doesn't have a comment concept; no-op
  :
}

_tao_update_status() {
  local task_id="$1" new_status="$2"
  local db
  db="$(_tao_db_path)"
  sqlite3 "$db" "UPDATE tao_suspended_actions SET status='${new_status}' WHERE tracking_id='${task_id}'" 2>/dev/null
}
