#!/usr/bin/env bash
# sipag — project registry (add/remove/list/show)

_project_validate_slug() {
  local slug="$1"
  if [[ -z "$slug" ]]; then
    log_error "Project slug is required"
    return 1
  fi
  if [[ ! "$slug" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]]; then
    log_error "Invalid slug '${slug}': must be alphanumeric with dots, dashes, underscores"
    return 1
  fi
}

project_add() {
  local slug="$1"
  shift

  _project_validate_slug "$slug" || return 1

  local project_dir
  project_dir="$(config_get_project_dir "$slug")"

  if [[ -f "${project_dir}/config" ]]; then
    log_error "Project '${slug}' already exists. Remove it first or edit its config."
    return 1
  fi

  # Parse key=val arguments (these globals are used by config_save_project)
  # shellcheck disable=SC2034
  for arg in "$@"; do
    case "$arg" in
      --repo=*) SIPAG_REPO="${arg#*=}" ;;
      --source=*) SIPAG_SOURCE="${arg#*=}" ;;
      --concurrency=*) SIPAG_CONCURRENCY="${arg#*=}" ;;
      --clone-url=*) SIPAG_CLONE_URL="${arg#*=}" ;;
      --branch=*) SIPAG_BASE_BRANCH="${arg#*=}" ;;
      --safety=*) SIPAG_SAFETY_MODE="${arg#*=}" ;;
      --poll-interval=*) SIPAG_POLL_INTERVAL="${arg#*=}" ;;
      --timeout=*) SIPAG_TIMEOUT="${arg#*=}" ;;
      --label-ready=*) SIPAG_LABEL_READY="${arg#*=}" ;;
      --label-wip=*) SIPAG_LABEL_WIP="${arg#*=}" ;;
      --label-done=*) SIPAG_LABEL_DONE="${arg#*=}" ;;
      --tao-db=*) SIPAG_TAO_DB="${arg#*=}" ;;
      --tao-action=*) SIPAG_TAO_ACTION="${arg#*=}" ;;
      *) log_warn "Unknown option: ${arg}" ;;
    esac
  done

  # Validate source-specific requirements
  if [[ "$SIPAG_SOURCE" == "github" && -z "$SIPAG_REPO" ]]; then
    log_error "GitHub source requires --repo=owner/repo"
    return 1
  fi

  config_save_project "$slug"
  config_ensure_project_dir "$slug" >/dev/null

  log_info "Project '${slug}' added"
  echo "Project '${slug}' registered at $(config_get_project_dir "$slug")"
}

project_remove() {
  local slug="$1"

  _project_validate_slug "$slug" || return 1

  local project_dir
  project_dir="$(config_get_project_dir "$slug")"

  if [[ ! -d "$project_dir" ]]; then
    log_error "Project '${slug}' does not exist"
    return 1
  fi

  # Check for active workers
  local active=0
  for pid_file in "${project_dir}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue
    local worker_pid
    worker_pid=$(cat "$pid_file")
    if kill -0 "$worker_pid" 2>/dev/null; then
      active=$((active + 1))
    fi
  done

  if [[ "$active" -gt 0 ]]; then
    log_error "Project '${slug}' has ${active} active worker(s). Stop them first."
    return 1
  fi

  rm -rf "$project_dir"
  log_info "Project '${slug}' removed"
  echo "Project '${slug}' removed"
}

project_list() {
  local home
  home="$(config_get_home)"
  local projects_dir="${home}/projects"

  if [[ ! -d "$projects_dir" ]]; then
    echo "No projects registered."
    return 0
  fi

  local found=0
  printf "%-20s %-10s %-30s %s\n" "PROJECT" "SOURCE" "REPO" "STATUS"
  printf "%-20s %-10s %-30s %s\n" "-------" "------" "----" "------"

  for dir in "${projects_dir}"/*/; do
    [[ -d "$dir" ]] || continue
    [[ -f "${dir}config" ]] || continue

    local slug
    slug=$(basename "$dir")

    # Read config vars without polluting global scope
    local p_source p_repo p_active
    p_source=$(grep '^SIPAG_SOURCE=' "${dir}config" 2>/dev/null | cut -d= -f2-)
    p_repo=$(grep '^SIPAG_REPO=' "${dir}config" 2>/dev/null | cut -d= -f2-)

    # Count active workers
    p_active=0
    for pid_file in "${dir}workers/"*.pid; do
      [[ -f "$pid_file" ]] || continue
      local task_id
      task_id=$(basename "$pid_file" .pid)
      [[ "$task_id" =~ ^[a-zA-Z0-9]+$ ]] || continue
      local worker_pid
      worker_pid=$(cat "$pid_file")
      if kill -0 "$worker_pid" 2>/dev/null; then
        p_active=$((p_active + 1))
      fi
    done

    local status_str
    if [[ "$p_active" -gt 0 ]]; then
      status_str="${p_active} worker(s) active"
    else
      status_str="idle"
    fi

    printf "%-20s %-10s %-30s %s\n" "$slug" "${p_source:-?}" "${p_repo:--}" "$status_str"
    found=$((found + 1))
  done

  if [[ "$found" -eq 0 ]]; then
    echo "No projects registered."
  fi
}

project_show() {
  local slug="$1"

  _project_validate_slug "$slug" || return 1

  local project_dir
  project_dir="$(config_get_project_dir "$slug")"

  if [[ ! -f "${project_dir}/config" ]]; then
    log_error "Project '${slug}' does not exist"
    return 1
  fi

  echo "Project: ${slug}"
  echo "Path:    ${project_dir}"
  echo ""
  echo "--- Config ---"
  cat "${project_dir}/config"
  echo ""

  # Active workers
  echo "--- Workers ---"
  local active=0
  for pid_file in "${project_dir}/workers/"*.pid; do
    [[ -f "$pid_file" ]] || continue
    local task_id
    task_id=$(basename "$pid_file" .pid)
    [[ "$task_id" =~ ^[a-zA-Z0-9]+$ ]] || continue
    local worker_pid
    worker_pid=$(cat "$pid_file")
    if kill -0 "$worker_pid" 2>/dev/null; then
      echo "  Worker PID ${worker_pid} → task #${task_id} (running)"
      active=$((active + 1))
    fi
  done

  if [[ "$active" -eq 0 ]]; then
    echo "  No active workers"
  fi
}
