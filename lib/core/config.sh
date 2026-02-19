#!/usr/bin/env bash
# sipag — config loader (global + per-project)

SIPAG_CONFIG_FILE=".sipag"

# Defaults
SIPAG_SOURCE="${SIPAG_SOURCE:-github}"
SIPAG_REPO="${SIPAG_REPO:-}"
SIPAG_CLONE_URL="${SIPAG_CLONE_URL:-}"
SIPAG_BASE_BRANCH="${SIPAG_BASE_BRANCH:-main}"
SIPAG_CONCURRENCY="${SIPAG_CONCURRENCY:-2}"
SIPAG_MAX_WORKERS="${SIPAG_MAX_WORKERS:-8}"
SIPAG_LABEL_READY="${SIPAG_LABEL_READY:-sipag}"
SIPAG_LABEL_WIP="${SIPAG_LABEL_WIP:-sipag-wip}"
SIPAG_LABEL_DONE="${SIPAG_LABEL_DONE:-sipag-done}"
SIPAG_TIMEOUT="${SIPAG_TIMEOUT:-600}"
SIPAG_POLL_INTERVAL="${SIPAG_POLL_INTERVAL:-60}"
SIPAG_ALLOWED_TOOLS="${SIPAG_ALLOWED_TOOLS:-}"
SIPAG_PROMPT_PREFIX="${SIPAG_PROMPT_PREFIX:-}"
SIPAG_SAFETY_MODE="${SIPAG_SAFETY_MODE:-strict}"
SIPAG_TAO_DB="${SIPAG_TAO_DB:-}"
SIPAG_TAO_ACTION="${SIPAG_TAO_ACTION:-}"

config_get_home() {
  echo "${SIPAG_HOME:-${HOME}/.sipag}"
}

config_get_project_dir() {
  local slug="$1"
  echo "$(config_get_home)/projects/${slug}"
}

config_ensure_home() {
  local home
  home="$(config_get_home)"
  mkdir -p "$home"
  echo "$home"
}

config_ensure_project_dir() {
  local slug="$1"
  local project_dir
  project_dir="$(config_get_project_dir "$slug")"
  mkdir -p "${project_dir}/workers" "${project_dir}/logs"
  echo "$project_dir"
}

config_list_projects() {
  local home
  home="$(config_get_home)"
  local projects_dir="${home}/projects"
  if [[ ! -d "$projects_dir" ]]; then
    return 0
  fi
  for dir in "${projects_dir}"/*/; do
    [[ -d "$dir" ]] || continue
    [[ -f "${dir}config" ]] || continue
    basename "$dir"
  done
}

config_load_global() {
  local home
  home="$(config_get_home)"
  local global_config="${home}/config"
  if [[ -f "$global_config" ]]; then
    # shellcheck disable=SC1090
    source "$global_config"
  fi
}

config_load_project() {
  local slug="$1"
  local project_dir
  project_dir="$(config_get_project_dir "$slug")"
  local config_path="${project_dir}/config"

  if [[ ! -f "$config_path" ]]; then
    die "No config found for project '${slug}'. Run 'sipag project add ${slug}' first."
  fi

  # shellcheck disable=SC1090
  source "$config_path"

  _config_validate
  log_debug "Config loaded for project: ${slug}"
}

config_save_project() {
  local slug="$1"
  local project_dir
  project_dir="$(config_ensure_project_dir "$slug")"
  local config_path="${project_dir}/config"

  cat >"$config_path" <<CONF
SIPAG_SOURCE=${SIPAG_SOURCE}
SIPAG_REPO=${SIPAG_REPO}
SIPAG_CLONE_URL=${SIPAG_CLONE_URL}
SIPAG_BASE_BRANCH=${SIPAG_BASE_BRANCH}
SIPAG_CONCURRENCY=${SIPAG_CONCURRENCY}
SIPAG_LABEL_READY=${SIPAG_LABEL_READY}
SIPAG_LABEL_WIP=${SIPAG_LABEL_WIP}
SIPAG_LABEL_DONE=${SIPAG_LABEL_DONE}
SIPAG_TIMEOUT=${SIPAG_TIMEOUT}
SIPAG_POLL_INTERVAL=${SIPAG_POLL_INTERVAL}
SIPAG_SAFETY_MODE=${SIPAG_SAFETY_MODE}
SIPAG_ALLOWED_TOOLS="${SIPAG_ALLOWED_TOOLS}"
SIPAG_PROMPT_PREFIX="${SIPAG_PROMPT_PREFIX}"
SIPAG_TAO_DB=${SIPAG_TAO_DB}
SIPAG_TAO_ACTION=${SIPAG_TAO_ACTION}
CONF

  log_debug "Config saved for project: ${slug}"
}

# Legacy: load config from a .sipag file in a project directory
config_load() {
  local config_path="${1:-.}/${SIPAG_CONFIG_FILE}"

  if [[ ! -f "$config_path" ]]; then
    die "No ${SIPAG_CONFIG_FILE} found in ${1:-.}. Run 'sipag init' first."
  fi

  # shellcheck disable=SC1090
  source "$config_path"

  _config_validate
  log_debug "Config loaded from $config_path"
}

_config_validate() {
  # Validate safety mode
  case "$SIPAG_SAFETY_MODE" in
    strict | balanced | yolo) ;;
    *)
      log_warn "Invalid SIPAG_SAFETY_MODE '${SIPAG_SAFETY_MODE}', falling back to strict"
      SIPAG_SAFETY_MODE="strict"
      ;;
  esac

  if [[ "$SIPAG_SAFETY_MODE" == "balanced" && -z "${ANTHROPIC_API_KEY:-}" ]]; then
    log_warn "SIPAG_SAFETY_MODE=balanced requires ANTHROPIC_API_KEY; falling back to strict"
    SIPAG_SAFETY_MODE="strict"
  fi

  # Derive clone URL from repo if not explicitly set
  if [[ -z "$SIPAG_CLONE_URL" && -n "$SIPAG_REPO" ]]; then
    SIPAG_CLONE_URL="https://github.com/${SIPAG_REPO}.git"
  fi
}

# Legacy helpers (still used by old-style per-repo layout)
config_get_run_dir() {
  local base="${1:-.}"
  echo "${base}/.sipag.d"
}

config_ensure_run_dir() {
  local run_dir
  run_dir=$(config_get_run_dir "$@")
  mkdir -p "$run_dir"/{workers,logs}
  echo "$run_dir"
}
