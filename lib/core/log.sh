#!/usr/bin/env bash
# sipag — logging helpers

SIPAG_LOG_LEVEL="${SIPAG_LOG_LEVEL:-info}"

_log_levels() {
  case "$1" in
    debug) echo 0 ;;
    info) echo 1 ;;
    warn) echo 2 ;;
    error) echo 3 ;;
    *) echo 1 ;;
  esac
}

_should_log() {
  local msg_level
  msg_level=$(_log_levels "$1")
  local current_level
  current_level=$(_log_levels "$SIPAG_LOG_LEVEL")
  [[ "$msg_level" -ge "$current_level" ]]
}

_log() {
  local level="$1"
  shift
  if _should_log "$level"; then
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    local prefix=""
    [[ -n "${SIPAG_WORKER_ID:-}" ]] && prefix="[worker:${SIPAG_WORKER_ID}] "
    [[ -n "${SIPAG_PROJECT_SLUG:-}" && -z "${SIPAG_WORKER_ID:-}" ]] && prefix="[${SIPAG_PROJECT_SLUG}] "
    printf '%s [%-5s] %s%s\n' "$timestamp" "$level" "$prefix" "$*" >&2
  fi
}

log_debug() { _log debug "$@"; }
log_info() { _log info "$@"; }
log_warn() { _log warn "$@"; }
log_error() { _log error "$@"; }

die() {
  log_error "$@"
  exit 1
}
