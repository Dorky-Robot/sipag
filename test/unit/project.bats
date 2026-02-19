#!/usr/bin/env bats
# sipag — project registry tests

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
  setup_common
  source "${SIPAG_ROOT}/lib/core/log.sh"
  source "${SIPAG_ROOT}/lib/core/config.sh"
  source "${SIPAG_ROOT}/lib/core/project.sh"
}

teardown() {
  teardown_common
}

# --- _project_validate_slug ---

@test "_project_validate_slug: valid slugs accepted" {
  _project_validate_slug "my-app"
  _project_validate_slug "app123"
  _project_validate_slug "my_app.v2"
  _project_validate_slug "A-project"
}

@test "_project_validate_slug: empty slug rejected" {
  run _project_validate_slug ""
  [[ "$status" -ne 0 ]]
}

@test "_project_validate_slug: invalid chars rejected" {
  run _project_validate_slug "my app"
  [[ "$status" -ne 0 ]]

  run _project_validate_slug "my/app"
  [[ "$status" -ne 0 ]]

  run _project_validate_slug "-starts-with-dash"
  [[ "$status" -ne 0 ]]
}

# --- project_add ---

@test "project_add: creates project directory and config" {
  SIPAG_REPO="org/my-app"
  project_add "my-app" "--repo=org/my-app" "--source=github"

  local project_dir="${SIPAG_HOME}/projects/my-app"
  [[ -f "${project_dir}/config" ]]
  [[ -d "${project_dir}/workers" ]]
  [[ -d "${project_dir}/logs" ]]
  grep -q "SIPAG_REPO=org/my-app" "${project_dir}/config"
  grep -q "SIPAG_SOURCE=github" "${project_dir}/config"
}

@test "project_add: duplicate slug rejected" {
  create_project_config "my-app"
  run project_add "my-app" "--repo=org/my-app"
  [[ "$status" -ne 0 ]]
}

@test "project_add: github source without repo rejected" {
  SIPAG_REPO=""
  run project_add "my-app" "--source=github"
  [[ "$status" -ne 0 ]]
}

@test "project_add: adhoc source without repo accepted" {
  SIPAG_SOURCE="adhoc"
  project_add "my-tasks" "--source=adhoc"

  local project_dir="${SIPAG_HOME}/projects/my-tasks"
  [[ -f "${project_dir}/config" ]]
  grep -q "SIPAG_SOURCE=adhoc" "${project_dir}/config"
}

@test "project_add: custom options applied" {
  project_add "my-app" "--repo=org/my-app" "--concurrency=4" "--branch=develop" "--safety=yolo"

  local config="${SIPAG_HOME}/projects/my-app/config"
  grep -q "SIPAG_CONCURRENCY=4" "$config"
  grep -q "SIPAG_BASE_BRANCH=develop" "$config"
  grep -q "SIPAG_SAFETY_MODE=yolo" "$config"
}

# --- project_remove ---

@test "project_remove: removes project directory" {
  create_project_config "my-app"

  project_remove "my-app"

  [[ ! -d "${SIPAG_HOME}/projects/my-app" ]]
}

@test "project_remove: nonexistent project → error" {
  run project_remove "nonexistent"
  [[ "$status" -ne 0 ]]
}

@test "project_remove: active workers → error" {
  create_project_config "my-app"
  local project_dir="${SIPAG_HOME}/projects/my-app"

  # Create a fake active worker
  sleep 300 &
  local pid=$!
  echo "$pid" > "${project_dir}/workers/42.pid"

  run project_remove "my-app"
  [[ "$status" -ne 0 ]]
  [[ "$output" == *"active worker"* ]]

  # Project should still exist
  [[ -d "$project_dir" ]]

  kill "$pid" 2>/dev/null
}

# --- project_list ---

@test "project_list: no projects → shows message" {
  run project_list
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"No projects"* ]]
}

@test "project_list: shows registered projects" {
  create_project_config "app-one" "SIPAG_REPO=org/app-one" "SIPAG_SOURCE=github"
  create_project_config "app-two" "SIPAG_REPO=org/app-two" "SIPAG_SOURCE=adhoc"

  run project_list
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"app-one"* ]]
  [[ "$output" == *"app-two"* ]]
  [[ "$output" == *"github"* ]]
  [[ "$output" == *"adhoc"* ]]
}

# --- project_show ---

@test "project_show: displays project config" {
  create_project_config "my-app" "SIPAG_REPO=org/my-app"

  run project_show "my-app"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"Project: my-app"* ]]
  [[ "$output" == *"SIPAG_REPO=org/my-app"* ]]
}

@test "project_show: nonexistent project → error" {
  run project_show "nonexistent"
  [[ "$status" -ne 0 ]]
}
