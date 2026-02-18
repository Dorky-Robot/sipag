#!/usr/bin/env bats
# sipag — config module tests

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
}

teardown() {
  teardown_common
}

@test "config_load: valid config loads all variables" {
  create_test_config "$PROJECT_DIR" "SIPAG_REPO=myorg/myrepo" "SIPAG_CONCURRENCY=4"
  config_load "$PROJECT_DIR"

  [[ "$SIPAG_REPO" == "myorg/myrepo" ]]
  [[ "$SIPAG_CONCURRENCY" == "4" ]]
  [[ "$SIPAG_SOURCE" == "github" ]]
  [[ "$SIPAG_BASE_BRANCH" == "main" ]]
}

@test "config_load: missing file → exit 1" {
  run config_load "$PROJECT_DIR"
  [[ "$status" -ne 0 ]]
}

@test "config_load: missing SIPAG_REPO → exit 1" {
  cat > "${PROJECT_DIR}/.sipag" <<'EOF'
SIPAG_SOURCE=github
SIPAG_REPO=
EOF
  run config_load "$PROJECT_DIR"
  [[ "$status" -ne 0 ]]
}

@test "config_load: invalid safety mode → fallback to strict + warning" {
  create_test_config "$PROJECT_DIR" "SIPAG_SAFETY_MODE=invalid"
  export SIPAG_LOG_LEVEL="warn"
  config_load "$PROJECT_DIR"
  [[ "$SIPAG_SAFETY_MODE" == "strict" ]]
}

@test "config_load: balanced without ANTHROPIC_API_KEY → fallback to strict" {
  create_test_config "$PROJECT_DIR" "SIPAG_SAFETY_MODE=balanced"
  unset ANTHROPIC_API_KEY 2>/dev/null || true
  export SIPAG_LOG_LEVEL="warn"
  config_load "$PROJECT_DIR"
  [[ "$SIPAG_SAFETY_MODE" == "strict" ]]
}

@test "config_load: balanced with ANTHROPIC_API_KEY → stays balanced" {
  create_test_config "$PROJECT_DIR" "SIPAG_SAFETY_MODE=balanced"
  export ANTHROPIC_API_KEY="sk-test-key"
  config_load "$PROJECT_DIR"
  [[ "$SIPAG_SAFETY_MODE" == "balanced" ]]
}

@test "config_load: yolo mode loads correctly" {
  create_test_config "$PROJECT_DIR" "SIPAG_SAFETY_MODE=yolo"
  config_load "$PROJECT_DIR"
  [[ "$SIPAG_SAFETY_MODE" == "yolo" ]]
}

@test "config_load: custom labels load correctly" {
  create_test_config "$PROJECT_DIR" \
    "SIPAG_LABEL_READY=ready" \
    "SIPAG_LABEL_WIP=working" \
    "SIPAG_LABEL_DONE=complete"
  config_load "$PROJECT_DIR"
  [[ "$SIPAG_LABEL_READY" == "ready" ]]
  [[ "$SIPAG_LABEL_WIP" == "working" ]]
  [[ "$SIPAG_LABEL_DONE" == "complete" ]]
}

@test "config_get_run_dir: returns correct path format" {
  local run_dir
  run_dir=$(config_get_run_dir "$PROJECT_DIR")
  [[ "$run_dir" == "${PROJECT_DIR}/.sipag.d" ]]
}

@test "config_get_run_dir: default path" {
  local run_dir
  run_dir=$(config_get_run_dir)
  [[ "$run_dir" == "./.sipag.d" ]]
}

@test "config_ensure_run_dir: creates workers and logs dirs" {
  local run_dir
  run_dir=$(config_ensure_run_dir "$PROJECT_DIR")
  [[ -d "${run_dir}/workers" ]]
  [[ -d "${run_dir}/logs" ]]
}

@test "config_ensure_run_dir: returns the run dir path" {
  local run_dir
  run_dir=$(config_ensure_run_dir "$PROJECT_DIR")
  [[ "$run_dir" == "${PROJECT_DIR}/.sipag.d" ]]
}
