#!/usr/bin/env bats
# sipag — unit tests for lib/worker.sh (label-gating behavior)

load ../helpers/test-helpers

setup() {
  setup_common

  # Isolated sipag dir so config and seen files don't touch the real ~/.sipag
  export SIPAG_DIR="${TEST_TMPDIR}/sipag"
  mkdir -p "$SIPAG_DIR"

  # Isolated log dir for PR state tracking
  export WORKER_LOG_DIR="${TEST_TMPDIR}/worker-logs"
  mkdir -p "$WORKER_LOG_DIR"

  # Clear env var so we test defaults from scratch
  unset SIPAG_WORK_LABEL

  source "${SIPAG_ROOT}/lib/worker.sh"

  # Override WORKER_LOG_DIR after sourcing (sourcing resets the default)
  WORKER_LOG_DIR="${TEST_TMPDIR}/worker-logs"
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

# --- PR iteration state tracking ---

@test "worker_pr_is_running returns false when PR is not running" {
  run worker_pr_is_running 42
  [[ "$status" -ne 0 ]]
}

@test "worker_pr_mark_running creates the running marker file" {
  worker_pr_mark_running 42
  [[ -f "${WORKER_LOG_DIR}/pr-42-running" ]]
}

@test "worker_pr_is_running returns true after marking running" {
  worker_pr_mark_running 99
  run worker_pr_is_running 99
  [[ "$status" -eq 0 ]]
}

@test "worker_pr_mark_done removes the running marker file" {
  worker_pr_mark_running 7
  worker_pr_mark_done 7
  [[ ! -f "${WORKER_LOG_DIR}/pr-7-running" ]]
}

@test "worker_pr_mark_done is idempotent when file does not exist" {
  run worker_pr_mark_done 999
  [[ "$status" -eq 0 ]]
}

@test "worker_pr_is_running tracks multiple PRs independently" {
  worker_pr_mark_running 1
  worker_pr_mark_running 2
  run worker_pr_is_running 1
  [[ "$status" -eq 0 ]]
  run worker_pr_is_running 2
  [[ "$status" -eq 0 ]]
  worker_pr_mark_done 1
  run worker_pr_is_running 1
  [[ "$status" -ne 0 ]]
  run worker_pr_is_running 2
  [[ "$status" -eq 0 ]]
}

# --- worker_find_prs_needing_iteration ---

@test "worker_find_prs_needing_iteration returns numbers from gh output" {
  # Mock gh to simulate output of `gh pr list --json ... -q '...'`
  # (the jq filtering is done inside gh; mock returns pre-filtered output)
  cat > "${TEST_TMPDIR}/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '5\n12\n'
EOF
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run worker_find_prs_needing_iteration "owner/repo"
  [[ "$status" -eq 0 ]]
  [[ "$output" == "5
12" ]]
}

@test "worker_find_prs_needing_iteration returns empty when no PRs need changes" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf ''
EOF
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run worker_find_prs_needing_iteration "owner/repo"
  [[ "$status" -eq 0 ]]
  [[ -z "$output" ]]
}

@test "worker_find_prs_needing_iteration sorts output numerically" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '20\n3\n11\n'
EOF
  chmod +x "${TEST_TMPDIR}/bin/gh"

  run worker_find_prs_needing_iteration "owner/repo"
  [[ "$output" == "3
11
20" ]]
}

# --- worker_find_prs_needing_iteration: jq filter logic ---
# These tests run the jq filter from worker_find_prs_needing_iteration directly
# against realistic PR JSON to verify the date-anchored detection logic.

# Extract the jq filter used by worker_find_prs_needing_iteration so tests
# can apply it to synthetic data without needing a live GitHub connection.
PR_ITER_JQ='
  .[] |
  (
      if (.commits | length) > 0
      then .commits[-1].committedDate
      else "1970-01-01T00:00:00Z"
      end
  ) as $last_push |
  select(
      ((.reviews // []) | map(select(.state == "CHANGES_REQUESTED" and .submittedAt > $last_push)) | length > 0) or
      ((.comments // []) | map(select(.createdAt > $last_push)) | length > 0)
  ) |
  .number
'

@test "jq filter: CHANGES_REQUESTED review after last commit triggers iteration" {
  local json='[{"number":10,
    "reviews":[{"state":"CHANGES_REQUESTED","submittedAt":"2024-01-02T00:00:00Z"}],
    "commits":[{"committedDate":"2024-01-01T00:00:00Z"}],
    "comments":[]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ "$result" == "10" ]]
}

@test "jq filter: CHANGES_REQUESTED review before last commit does not trigger iteration" {
  # Reviewer requested changes, worker pushed a fix — review is now stale
  local json='[{"number":11,
    "reviews":[{"state":"CHANGES_REQUESTED","submittedAt":"2024-01-01T00:00:00Z"}],
    "commits":[{"committedDate":"2024-01-02T00:00:00Z"}],
    "comments":[]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ -z "$result" ]]
}

@test "jq filter: comment after last commit triggers iteration" {
  local json='[{"number":12,
    "reviews":[],
    "commits":[{"committedDate":"2024-01-01T00:00:00Z"}],
    "comments":[{"createdAt":"2024-01-02T00:00:00Z","author":{"login":"reviewer"},"body":"please fix"}]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ "$result" == "12" ]]
}

@test "jq filter: comment before last commit does not trigger iteration" {
  # Comment was posted before the worker's last push — already addressed
  local json='[{"number":13,
    "reviews":[],
    "commits":[{"committedDate":"2024-01-02T00:00:00Z"}],
    "comments":[{"createdAt":"2024-01-01T00:00:00Z","author":{"login":"reviewer"},"body":"old comment"}]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ -z "$result" ]]
}

@test "jq filter: PR with no commits uses epoch as baseline so any feedback triggers iteration" {
  local json='[{"number":14,
    "reviews":[{"state":"CHANGES_REQUESTED","submittedAt":"2024-01-01T00:00:00Z"}],
    "commits":[],
    "comments":[]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ "$result" == "14" ]]
}

@test "jq filter: APPROVED review does not trigger iteration" {
  local json='[{"number":15,
    "reviews":[{"state":"APPROVED","submittedAt":"2024-01-02T00:00:00Z"}],
    "commits":[{"committedDate":"2024-01-01T00:00:00Z"}],
    "comments":[]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ -z "$result" ]]
}

@test "jq filter: only the latest commit date is used as baseline" {
  # Second commit is after the CHANGES_REQUESTED review — worker already addressed it
  local json='[{"number":16,
    "reviews":[{"state":"CHANGES_REQUESTED","submittedAt":"2024-01-02T00:00:00Z"}],
    "commits":[
      {"committedDate":"2024-01-01T00:00:00Z"},
      {"committedDate":"2024-01-03T00:00:00Z"}
    ],
    "comments":[]}]'
  result=$(echo "$json" | jq -r "$PR_ITER_JQ")
  [[ -z "$result" ]]
}

# --- worker_slugify ---

@test "worker_slugify lowercases title" {
  result=$(worker_slugify "Hello World")
  [[ "$result" == "hello-world" ]]
}

@test "worker_slugify replaces spaces and punctuation with dashes" {
  result=$(worker_slugify "Fix: auth bug!")
  [[ "$result" == "fix-auth-bug" ]]
}

@test "worker_slugify squeezes consecutive dashes" {
  result=$(worker_slugify "foo  --  bar")
  [[ "$result" == "foo-bar" ]]
}

@test "worker_slugify strips leading and trailing dashes" {
  result=$(worker_slugify "  leading and trailing  ")
  [[ "$result" == "leading-and-trailing" ]]
}

@test "worker_slugify truncates to 50 chars" {
  long="this is a very long issue title that exceeds fifty characters easily"
  result=$(worker_slugify "$long")
  [[ "${#result}" -le 50 ]]
}

@test "worker_slugify preserves alphanumeric characters" {
  result=$(worker_slugify "Add OAuth2 support v3")
  [[ "$result" == "add-oauth2-support-v3" ]]
}

# --- worker_unsee ---

@test "worker_unsee removes an issue from the seen file" {
  WORKER_SEEN_FILE="${SIPAG_DIR}/seen"
  printf '10\n20\n30\n' > "$WORKER_SEEN_FILE"
  worker_unsee 20
  run grep -cx '20' "$WORKER_SEEN_FILE"
  [[ "$output" == "0" ]]
}

@test "worker_unsee leaves other entries intact" {
  WORKER_SEEN_FILE="${SIPAG_DIR}/seen"
  printf '10\n20\n30\n' > "$WORKER_SEEN_FILE"
  worker_unsee 20
  run grep -cx '10' "$WORKER_SEEN_FILE"
  [[ "$output" == "1" ]]
  run grep -cx '30' "$WORKER_SEEN_FILE"
  [[ "$output" == "1" ]]
}

@test "worker_unsee is idempotent when issue is not in seen file" {
  WORKER_SEEN_FILE="${SIPAG_DIR}/seen"
  printf '10\n30\n' > "$WORKER_SEEN_FILE"
  run worker_unsee 99
  [[ "$status" -eq 0 ]]
}

@test "worker_unsee is safe when seen file does not exist" {
  WORKER_SEEN_FILE="${SIPAG_DIR}/no-such-seen"
  run worker_unsee 42
  [[ "$status" -eq 0 ]]
}

@test "worker_unsee + worker_is_seen: issue is not seen after unsee" {
  WORKER_SEEN_FILE="${SIPAG_DIR}/seen"
  printf '7\n' > "$WORKER_SEEN_FILE"
  worker_unsee 7
  run worker_is_seen 7
  [[ "$status" -ne 0 ]]
}

# --- worker_has_open_pr ---

@test "worker_has_open_pr returns true when open PR body references issue" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo '[{"number":42,"body":"Closes #5"}]'
EOF
  chmod +x "${TEST_TMPDIR}/bin/gh"
  run worker_has_open_pr "owner/repo" 5
  [[ "$status" -eq 0 ]]
}

@test "worker_has_open_pr returns false when no open PR exists" {
  cat > "${TEST_TMPDIR}/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo '[]'
EOF
  chmod +x "${TEST_TMPDIR}/bin/gh"
  run worker_has_open_pr "owner/repo" 5
  [[ "$status" -ne 0 ]]
}

# --- sipag_run_hook ---

@test "sipag_run_hook: silently skips missing hook" {
  run sipag_run_hook "on-nonexistent-hook"
  [[ "$status" -eq 0 ]]
}

@test "sipag_run_hook: silently skips non-executable hook" {
  mkdir -p "${SIPAG_DIR}/hooks"
  echo "#!/usr/bin/env bash" > "${SIPAG_DIR}/hooks/on-worker-started"
  # intentionally NOT chmod +x
  run sipag_run_hook "on-worker-started"
  [[ "$status" -eq 0 ]]
}

@test "sipag_run_hook: runs executable hook" {
  mkdir -p "${SIPAG_DIR}/hooks"
  local marker="${TEST_TMPDIR}/hook-ran"
  cat > "${SIPAG_DIR}/hooks/on-worker-completed" <<HOOK
#!/usr/bin/env bash
touch "${marker}"
HOOK
  chmod +x "${SIPAG_DIR}/hooks/on-worker-completed"
  sipag_run_hook "on-worker-completed"
  # wait for background hook (up to 2s)
  local i=0
  while [[ ! -f "$marker" && $i -lt 20 ]]; do
    sleep 0.1
    i=$(( i + 1 ))
  done
  [[ -f "$marker" ]]
}

@test "sipag_run_hook: hook inherits exported env vars" {
  mkdir -p "${SIPAG_DIR}/hooks"
  local output_file="${TEST_TMPDIR}/hook-env"
  cat > "${SIPAG_DIR}/hooks/on-worker-started" <<HOOK
#!/usr/bin/env bash
echo "\${SIPAG_EVENT}" > "${output_file}"
HOOK
  chmod +x "${SIPAG_DIR}/hooks/on-worker-started"
  export SIPAG_EVENT="worker.started"
  sipag_run_hook "on-worker-started"
  # wait for background hook (up to 2s)
  local i=0
  while [[ ! -f "$output_file" && $i -lt 20 ]]; do
    sleep 0.1
    i=$(( i + 1 ))
  done
  grep -q "worker.started" "$output_file"
}
