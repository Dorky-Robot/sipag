#!/usr/bin/env bash
# sipag — worker: pick issue → branch → claude → PR

_worker_write_state() {
  local run_dir="$1" task_id="$2" status="$3"
  local title="${4:-}" url="${5:-}" branch="${6:-}"
  local pr_url="${7:-}" error="${8:-}"

  local state_file="${run_dir}/workers/${task_id}.json"
  local now
  now=$(date -u '+%Y-%m-%dT%H:%M:%SZ')

  # Preserve started_at from existing file
  local started_at="$now"
  local finished_at="null"
  if [[ -f "$state_file" ]]; then
    local prev_started
    prev_started=$(grep -o '"started_at":"[^"]*"' "$state_file" 2>/dev/null | cut -d'"' -f4)
    [[ -n "$prev_started" ]] && started_at="$prev_started"
  fi

  if [[ "$status" == "done" || "$status" == "failed" ]]; then
    finished_at="\"$now\""
  fi

  local pr_json="null"
  [[ -n "$pr_url" ]] && pr_json="\"$pr_url\""

  local err_json="null"
  [[ -n "$error" ]] && err_json="$(printf '%s' "$error" | sed 's/\\/\\\\/g; s/"/\\"/g; s/\t/\\t/g' | tr '\n' ' ')"
  [[ "$err_json" != "null" ]] && err_json="\"$err_json\""

  cat > "$state_file" << JSONEOF
{"task_id":${task_id},"title":"${title}","url":"${url}","branch":"${branch}","status":"${status}","started_at":"${started_at}","finished_at":${finished_at},"pr_url":${pr_json},"error":${err_json}}
JSONEOF
}

worker_run() {
  local task_id="$1"
  local project_dir="$2"
  local run_dir="$3"

  export SIPAG_WORKER_ID="${task_id}"

  log_info "Starting work on task #${task_id}"

  # Get task details
  local task_info
  task_info=$(source_get_task "$SIPAG_REPO" "$task_id") || {
    log_error "Failed to fetch task #${task_id}"
    return 1
  }

  local task_title task_body task_number task_url
  task_title=$(echo "$task_info" | grep '^TASK_TITLE=' | cut -d= -f2-)
  task_body=$(echo "$task_info" | grep '^TASK_BODY=' | cut -d= -f2-)
  task_number=$(echo "$task_info" | grep '^TASK_NUMBER=' | cut -d= -f2-)
  task_url="https://github.com/${SIPAG_REPO}/issues/${task_number}"

  # Claim the task
  source_claim_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_WIP" "$SIPAG_LABEL_READY" || {
    log_error "Failed to claim task #${task_id}"
    _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "" "" "Failed to claim task"
    return 1
  }

  log_info "Claimed task #${task_id}: ${task_title}"
  _worker_write_state "$run_dir" "$task_id" "claimed" "$task_title" "$task_url"

  # Create a working clone
  local work_dir="${run_dir}/workers/clone-${task_id}"
  rm -rf "$work_dir"

  log_info "Cloning repo into ${work_dir}"
  git clone "$project_dir" "$work_dir" 2>/dev/null || {
    log_error "Failed to clone repo"
    source_fail_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_READY" "$SIPAG_LABEL_WIP" "Failed to clone repo"
    _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "" "" "Failed to clone repo"
    return 1
  }

  cd "$work_dir" || return 1

  # Set up remote to point at the real origin
  local origin_url
  origin_url=$(git -C "$project_dir" remote get-url origin 2>/dev/null)
  if [[ -n "$origin_url" ]]; then
    git remote set-url origin "$origin_url"
  fi

  # Create branch
  local branch_name="sipag/${task_number}-$(echo "$task_title" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g' | sed 's/--*/-/g' | sed 's/^-//' | sed 's/-$//' | head -c 50)"

  git checkout -b "$branch_name" "origin/${SIPAG_BASE_BRANCH}" 2>/dev/null || {
    git checkout -b "$branch_name" "${SIPAG_BASE_BRANCH}" 2>/dev/null || {
      log_error "Failed to create branch ${branch_name}"
      source_fail_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_READY" "$SIPAG_LABEL_WIP" "Failed to create branch"
      _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "" "" "Failed to create branch"
      return 1
    }
  }

  log_info "Created branch: ${branch_name}"
  _worker_write_state "$run_dir" "$task_id" "running" "$task_title" "$task_url" "$branch_name"

  # Build the prompt for Claude
  local prompt="${SIPAG_PROMPT_PREFIX:+${SIPAG_PROMPT_PREFIX}\n\n}"
  prompt+="GitHub Issue #${task_number}: ${task_title}"
  prompt+=$'\n\n'
  prompt+="${task_body}"
  prompt+=$'\n\n'
  prompt+="Implement what the issue asks for. When done, make sure all changes are committed."

  # Build claude args
  local claude_args=(--print)
  if [[ -n "$SIPAG_ALLOWED_TOOLS" ]]; then
    IFS=',' read -ra tool_list <<< "$SIPAG_ALLOWED_TOOLS"
    for tool in "${tool_list[@]}"; do
      claude_args+=(--allowedTools "$tool")
    done
  else
    claude_args+=(--dangerously-skip-permissions)
  fi

  # Run Claude Code
  local log_file="${run_dir}/logs/worker-${task_id}.log"

  log_info "Running Claude Code..."
  source_comment "$SIPAG_REPO" "$task_id" "sipag is working on this..."

  local claude_exit=0
  timeout "${SIPAG_TIMEOUT}" claude \
    "${claude_args[@]}" \
    -p "$prompt" \
    > "$log_file" 2>&1 || claude_exit=$?

  if [[ "$claude_exit" -ne 0 ]]; then
    log_error "Claude Code exited with status ${claude_exit}"
    local error_snippet
    error_snippet=$(tail -20 "$log_file")
    source_fail_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_READY" "$SIPAG_LABEL_WIP" \
      "Claude Code failed (exit ${claude_exit}). Last output:\n\`\`\`\n${error_snippet}\n\`\`\`"
    _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "$branch_name" "" "Claude Code failed (exit ${claude_exit})"
    return 1
  fi

  # Check if there are any commits
  local commit_count
  commit_count=$(git rev-list --count "${SIPAG_BASE_BRANCH}..HEAD" 2>/dev/null || echo "0")

  if [[ "$commit_count" -eq 0 ]]; then
    log_warn "No commits produced for task #${task_id}"
    source_fail_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_READY" "$SIPAG_LABEL_WIP" \
      "Claude Code ran but produced no commits."
    _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "$branch_name" "" "No commits produced"
    return 1
  fi

  log_info "Claude produced ${commit_count} commit(s)"
  _worker_write_state "$run_dir" "$task_id" "pushing" "$task_title" "$task_url" "$branch_name"

  # Push the branch
  git push origin "$branch_name" 2>/dev/null || {
    log_error "Failed to push branch ${branch_name}"
    source_fail_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_READY" "$SIPAG_LABEL_WIP" "Failed to push branch"
    _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "$branch_name" "" "Failed to push branch"
    return 1
  }

  log_info "Pushed branch: ${branch_name}"

  # Open a PR
  local pr_body="Resolves #${task_number}"
  pr_body+=$'\n\n'
  pr_body+="Generated by [sipag](https://github.com/dorky-robot/sipag) via Claude Code."

  local pr_url
  pr_url=$(gh pr create \
    --repo "$SIPAG_REPO" \
    --base "$SIPAG_BASE_BRANCH" \
    --head "$branch_name" \
    --title "$task_title" \
    --body "$pr_body" 2>/dev/null) || {
    log_error "Failed to create PR"
    source_fail_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_READY" "$SIPAG_LABEL_WIP" "Failed to create PR"
    _worker_write_state "$run_dir" "$task_id" "failed" "$task_title" "$task_url" "$branch_name" "" "Failed to create PR"
    return 1
  }

  log_info "PR opened: ${pr_url}"

  # Mark task as complete
  source_complete_task "$SIPAG_REPO" "$task_id" "$SIPAG_LABEL_DONE" "$SIPAG_LABEL_WIP" "$pr_url"

  log_info "Task #${task_id} completed successfully"
  _worker_write_state "$run_dir" "$task_id" "done" "$task_title" "$task_url" "$branch_name" "$pr_url"

  # Clean up work dir
  rm -rf "$work_dir"

  return 0
}
