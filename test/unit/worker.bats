#!/usr/bin/env bats
# sipag — unit tests for lib/worker.sh (label-gating behavior)

load ../helpers/test-helpers

setup() {
  setup_common

  # Isolated sipag dir so config and seen files don't touch the real ~/.sipag
  export SIPAG_DIR="${TEST_TMPDIR}/sipag"
  mkdir -p "$SIPAG_DIR"

  # Clear env var so we test defaults from scratch
  unset SIPAG_WORK_LABEL

  source "${SIPAG_ROOT}/lib/worker.sh"
}

teardown() {
  teardown_common
}

# --- default label ---

@test "default work_label is 'approved'" {
  [[ "$WORKER_WORK_LABEL" == "approved" ]]
}

@test "SIPAG_WORK_LABEL env var sets initial work_label before config load" {
  # Re-source with the env var set to verify module-level assignment
  SIPAG_WORK_LABEL="custom-env-label" source "${SIPAG_ROOT}/lib/worker.sh"
  [[ "$WORKER_WORK_LABEL" == "custom-env-label" ]]
}

# --- worker_load_config: work_label ---

@test "worker_load_config reads work_label from config file" {
  echo "work_label=greenlit" > "${SIPAG_DIR}/config"
  worker_load_config
  [[ "$WORKER_WORK_LABEL" == "greenlit" ]]
}

@test "worker_load_config: empty work_label in config disables label filter" {
  echo "work_label=" > "${SIPAG_DIR}/config"
  worker_load_config
  [[ -z "$WORKER_WORK_LABEL" ]]
}

@test "worker_load_config: missing config file returns 0 and leaves defaults" {
  # SIPAG_DIR exists but config file does not
  run worker_load_config
  [[ "$status" -eq 0 ]]
  [[ "$WORKER_WORK_LABEL" == "approved" ]]
}

@test "worker_load_config reads all config keys including work_label" {
  cat > "${SIPAG_DIR}/config" <<'EOF'
batch_size=8
image=my-image:v2
timeout=3600
poll_interval=60
work_label=triaged
EOF
  worker_load_config
  [[ "$WORKER_WORK_LABEL" == "triaged" ]]
  [[ "$WORKER_BATCH_SIZE" == "8" ]]
  [[ "$WORKER_IMAGE" == "my-image:v2" ]]
  [[ "$WORKER_TIMEOUT" == "3600" ]]
  [[ "$WORKER_POLL_INTERVAL" == "60" ]]
}

@test "worker_load_config ignores comment lines" {
  cat > "${SIPAG_DIR}/config" <<'EOF'
# This is a comment
work_label=labeled
# batch_size=99
EOF
  worker_load_config
  [[ "$WORKER_WORK_LABEL" == "labeled" ]]
  [[ "$WORKER_BATCH_SIZE" == "4" ]]  # default unchanged
}

@test "worker_load_config trims whitespace from keys and values" {
  printf "  work_label  =  spaced-label  \n" > "${SIPAG_DIR}/config"
  worker_load_config
  [[ "$WORKER_WORK_LABEL" == "spaced-label" ]]
}

@test "worker_load_config: config overrides SIPAG_WORK_LABEL env var default" {
  SIPAG_WORK_LABEL="env-label" source "${SIPAG_ROOT}/lib/worker.sh"
  [[ "$WORKER_WORK_LABEL" == "env-label" ]]

  echo "work_label=config-label" > "${SIPAG_DIR}/config"
  worker_load_config
  [[ "$WORKER_WORK_LABEL" == "config-label" ]]
}
