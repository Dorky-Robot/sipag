#!/usr/bin/env bash
# sipag — ad-hoc file-based task queue source plugin
#
# Tasks are JSON files in ~/.sipag/adhoc/{pending,claimed,done,failed}/
# Each file: {"id":"...","slug":"...","prompt":"...","created_at":"..."}

_adhoc_dir() {
  echo "$(config_get_home)/adhoc"
}

source_list_tasks() {
  local _repo="$1" _label="$2"
  local adhoc_dir
  adhoc_dir="$(_adhoc_dir)/pending"

  if [[ ! -d "$adhoc_dir" ]]; then
    return 0
  fi

  local slug="${SIPAG_PROJECT_SLUG:-}"

  for f in "${adhoc_dir}"/*.json; do
    [[ -f "$f" ]] || continue
    local task_slug task_id
    task_slug=$(jq -r '.slug // ""' "$f" 2>/dev/null)
    task_id=$(jq -r '.id // ""' "$f" 2>/dev/null)

    # Only return tasks matching this project
    if [[ -n "$slug" && "$task_slug" != "$slug" ]]; then
      continue
    fi

    echo "$task_id"
  done
}

source_claim_task() {
  local _repo="$1" task_id="$2" _wip_label="$3" _ready_label="$4"
  local adhoc_dir
  adhoc_dir="$(_adhoc_dir)"

  local src="${adhoc_dir}/pending/${task_id}.json"
  local dst="${adhoc_dir}/claimed/${task_id}.json"

  if [[ ! -f "$src" ]]; then
    log_error "Ad-hoc task ${task_id} not found in pending"
    return 1
  fi

  mkdir -p "${adhoc_dir}/claimed"
  mv "$src" "$dst"
}

source_get_task() {
  local _repo="$1" task_id="$2"
  local adhoc_dir
  adhoc_dir="$(_adhoc_dir)"

  local task_file="${adhoc_dir}/claimed/${task_id}.json"

  if [[ ! -f "$task_file" ]]; then
    log_error "Ad-hoc task ${task_id} not found in claimed"
    return 1
  fi

  local prompt slug
  prompt=$(jq -r '.prompt // ""' "$task_file")
  slug=$(jq -r '.slug // ""' "$task_file")

  echo "TASK_TITLE=adhoc: ${prompt:0:60}"
  echo "TASK_BODY=${prompt}"
  echo "TASK_NUMBER=${task_id}"
  echo "TASK_URL=adhoc://${slug}/${task_id}"
}

source_complete_task() {
  local _repo="$1" task_id="$2" _done_label="$3" _wip_label="$4" pr_url="$5"
  local adhoc_dir
  adhoc_dir="$(_adhoc_dir)"

  local src="${adhoc_dir}/claimed/${task_id}.json"
  local dst="${adhoc_dir}/done/${task_id}.json"

  if [[ ! -f "$src" ]]; then
    return 0
  fi

  mkdir -p "${adhoc_dir}/done"

  # Append result to the JSON
  local now
  now=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
  local tmp
  tmp=$(jq --arg pr "$pr_url" --arg ts "$now" \
    '. + {completed_at: $ts, pr_url: $pr}' "$src")
  echo "$tmp" >"$dst"
  rm -f "$src"
}

source_fail_task() {
  local _repo="$1" task_id="$2" _ready_label="$3" _wip_label="$4" error_msg="$5"
  local adhoc_dir
  adhoc_dir="$(_adhoc_dir)"

  local src="${adhoc_dir}/claimed/${task_id}.json"

  if [[ ! -f "$src" ]]; then
    return 0
  fi

  # Move back to pending for retry
  mv "$src" "${adhoc_dir}/pending/${task_id}.json"

  if [[ -n "$error_msg" ]]; then
    log_warn "Ad-hoc task ${task_id} failed: ${error_msg}"
  fi
}

source_comment() {
  local _repo="$1" task_id="$2" message="$3"
  # Ad-hoc tasks don't have a comment mechanism; log instead
  log_debug "Ad-hoc task ${task_id}: ${message}"
}
