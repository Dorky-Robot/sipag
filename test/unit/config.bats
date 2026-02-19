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

# --- Legacy config_load ---

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

@test "config_load: missing SIPAG_REPO still loads (repo not required for all sources)" {
  cat > "${PROJECT_DIR}/.sipag" <<'EOF'
SIPAG_SOURCE=adhoc
SIPAG_REPO=
EOF
  config_load "$PROJECT_DIR"
  [[ "$SIPAG_SOURCE" == "adhoc" ]]
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

# --- Legacy run dir helpers ---

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

# --- New: config_get_home ---

@test "config_get_home: respects SIPAG_HOME env var" {
  export SIPAG_HOME="/tmp/test-sipag-home"
  local home
  home=$(config_get_home)
  [[ "$home" == "/tmp/test-sipag-home" ]]
}

@test "config_get_home: defaults to ~/.sipag" {
  unset SIPAG_HOME
  local home
  home=$(config_get_home)
  [[ "$home" == "${HOME}/.sipag" ]]
}

# --- New: config_get_project_dir ---

@test "config_get_project_dir: returns correct path" {
  local dir
  dir=$(config_get_project_dir "my-app")
  [[ "$dir" == "${SIPAG_HOME}/projects/my-app" ]]
}

# --- New: config_ensure_project_dir ---

@test "config_ensure_project_dir: creates workers and logs subdirs" {
  local dir
  dir=$(config_ensure_project_dir "my-app")
  [[ -d "${dir}/workers" ]]
  [[ -d "${dir}/logs" ]]
}

# --- New: config_list_projects ---

@test "config_list_projects: returns nothing when no projects" {
  local result
  result=$(config_list_projects)
  [[ -z "$result" ]]
}

@test "config_list_projects: lists projects with config files" {
  create_project_config "app-one" "SIPAG_REPO=org/app-one"
  create_project_config "app-two" "SIPAG_REPO=org/app-two"

  local result
  result=$(config_list_projects)
  [[ "$result" == *"app-one"* ]]
  [[ "$result" == *"app-two"* ]]
}

@test "config_list_projects: skips dirs without config" {
  mkdir -p "${SIPAG_HOME}/projects/broken"
  create_project_config "valid" "SIPAG_REPO=org/valid"

  local result
  result=$(config_list_projects)
  [[ "$result" == *"valid"* ]]
  [[ "$result" != *"broken"* ]]
}

# --- New: config_load_project ---

@test "config_load_project: loads project config vars" {
  create_project_config "my-app" "SIPAG_REPO=myorg/my-app" "SIPAG_CONCURRENCY=4"
  config_load_project "my-app"

  [[ "$SIPAG_REPO" == "myorg/my-app" ]]
  [[ "$SIPAG_CONCURRENCY" == "4" ]]
}

@test "config_load_project: missing project → exit 1" {
  run config_load_project "nonexistent"
  [[ "$status" -ne 0 ]]
}

# --- New: config_save_project ---

@test "config_save_project: writes config file" {
  SIPAG_SOURCE="github"
  SIPAG_REPO="org/repo"
  config_save_project "my-app"

  local config_file="${SIPAG_HOME}/projects/my-app/config"
  [[ -f "$config_file" ]]
  grep -q "SIPAG_REPO=org/repo" "$config_file"
  grep -q "SIPAG_SOURCE=github" "$config_file"
}

# --- New: clone URL derivation ---

@test "_config_validate: derives clone URL from repo" {
  SIPAG_REPO="myorg/myrepo"
  SIPAG_CLONE_URL=""
  _config_validate
  [[ "$SIPAG_CLONE_URL" == "https://github.com/myorg/myrepo.git" ]]
}

@test "_config_validate: explicit clone URL preserved" {
  SIPAG_REPO="myorg/myrepo"
  SIPAG_CLONE_URL="git@github.com:myorg/myrepo.git"
  _config_validate
  [[ "$SIPAG_CLONE_URL" == "git@github.com:myorg/myrepo.git" ]]
}
