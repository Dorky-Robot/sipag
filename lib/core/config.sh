#!/usr/bin/env bash
# sipag — config loader

SIPAG_CONFIG_FILE=".sipag"

# Defaults
SIPAG_SOURCE="${SIPAG_SOURCE:-github}"
SIPAG_REPO="${SIPAG_REPO:-}"
SIPAG_BASE_BRANCH="${SIPAG_BASE_BRANCH:-main}"
SIPAG_CONCURRENCY="${SIPAG_CONCURRENCY:-2}"
SIPAG_LABEL_READY="${SIPAG_LABEL_READY:-sipag}"
SIPAG_LABEL_WIP="${SIPAG_LABEL_WIP:-sipag-wip}"
SIPAG_LABEL_DONE="${SIPAG_LABEL_DONE:-sipag-done}"
SIPAG_TIMEOUT="${SIPAG_TIMEOUT:-600}"
SIPAG_POLL_INTERVAL="${SIPAG_POLL_INTERVAL:-60}"
SIPAG_ALLOWED_TOOLS="${SIPAG_ALLOWED_TOOLS:-}"
SIPAG_PROMPT_PREFIX="${SIPAG_PROMPT_PREFIX:-}"

config_load() {
  local config_path="${1:-.}/${SIPAG_CONFIG_FILE}"

  if [[ ! -f "$config_path" ]]; then
    die "No ${SIPAG_CONFIG_FILE} found in ${1:-.}. Run 'sipag init' first."
  fi

  # Source the config (it's just bash variable assignments)
  # shellcheck disable=SC1090
  source "$config_path"

  # Validate required fields
  if [[ -z "$SIPAG_REPO" ]]; then
    die "SIPAG_REPO is required in ${SIPAG_CONFIG_FILE}"
  fi

  log_debug "Config loaded from $config_path"
  log_debug "  SIPAG_SOURCE=$SIPAG_SOURCE"
  log_debug "  SIPAG_REPO=$SIPAG_REPO"
  log_debug "  SIPAG_CONCURRENCY=$SIPAG_CONCURRENCY"
}

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
