#!/usr/bin/env bats
# sipag — unit tests for lib/start.sh

load ../helpers/test-helpers
load ../helpers/mock-commands

setup() {
    setup_common
    # shellcheck source=../../lib/start.sh
    source "${SIPAG_ROOT}/lib/start.sh"
    unset SIPAG_SKIP_PERMISSIONS 2>/dev/null || true
    unset SIPAG_MODEL            2>/dev/null || true
}

teardown() {
    teardown_common
}

# ---------------------------------------------------------------------------
# start_run — argument validation
# ---------------------------------------------------------------------------

@test "start_run: no args prints usage and returns 1" {
    run start_run
    [ "$status" -eq 1 ]
    [[ "$output" == *"Usage:"* ]]
    [[ "$output" == *"Available modes:"* ]]
}

@test "start_run: mode only (no repo) prints error and returns 1" {
    run start_run "triage" ""
    [ "$status" -eq 1 ]
    [[ "$output" == *"repo is required"* ]]
}

@test "start_run: unknown mode prints error and returns 1" {
    create_mock "gh" 0 "[]"
    run start_run "unknown" "owner/repo"
    [ "$status" -eq 1 ]
    [[ "$output" == *"Unknown mode: unknown"* ]]
    [[ "$output" == *"Available modes:"* ]]
}

# ---------------------------------------------------------------------------
# start_run — valid modes invoke claude
# ---------------------------------------------------------------------------

@test "start_run: triage mode calls claude" {
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local count
    count="$(mock_call_count "claude")"
    [ "$count" -eq 1 ]
}

@test "start_run: refinement mode calls claude" {
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "refinement" "owner/repo"

    local count
    count="$(mock_call_count "claude")"
    [ "$count" -eq 1 ]
}

@test "start_run: review mode calls claude" {
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "review" "owner/repo"

    local count
    count="$(mock_call_count "claude")"
    [ "$count" -eq 1 ]
}

# ---------------------------------------------------------------------------
# start_run — claude flags
# ---------------------------------------------------------------------------

@test "start_run: passes --system-prompt to claude" {
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local calls
    calls="$(get_mock_calls "claude")"
    [[ "$calls" == *"--system-prompt"* ]]
}

@test "start_run: SIPAG_SKIP_PERMISSIONS=1 (default) adds --dangerously-skip-permissions" {
    export SIPAG_SKIP_PERMISSIONS=1
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local calls
    calls="$(get_mock_calls "claude")"
    [[ "$calls" == *"--dangerously-skip-permissions"* ]]
}

@test "start_run: default SIPAG_SKIP_PERMISSIONS adds --dangerously-skip-permissions" {
    unset SIPAG_SKIP_PERMISSIONS
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local calls
    calls="$(get_mock_calls "claude")"
    [[ "$calls" == *"--dangerously-skip-permissions"* ]]
}

@test "start_run: SIPAG_SKIP_PERMISSIONS=0 omits --dangerously-skip-permissions" {
    export SIPAG_SKIP_PERMISSIONS=0
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local calls
    calls="$(get_mock_calls "claude")"
    [[ "$calls" != *"--dangerously-skip-permissions"* ]]
}

@test "start_run: SIPAG_MODEL sets --model flag" {
    export SIPAG_MODEL="claude-opus-4-5"
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local calls
    calls="$(get_mock_calls "claude")"
    [[ "$calls" == *"--model"* ]]
    [[ "$calls" == *"claude-opus-4-5"* ]]
}

@test "start_run: unset SIPAG_MODEL omits --model flag" {
    unset SIPAG_MODEL
    create_mock "gh" 0 "[]"
    create_mock "claude" 0 ""

    run start_run "triage" "owner/repo"

    local calls
    calls="$(get_mock_calls "claude")"
    [[ "$calls" != *"--model"* ]]
}

# ---------------------------------------------------------------------------
# Prompt content — repo name appears in each mode's system prompt
# ---------------------------------------------------------------------------

@test "start_build_triage_prompt: includes repo name" {
    create_mock "gh" 0 "[]"

    local prompt
    prompt=$(start_build_triage_prompt "myorg/myrepo")

    [[ "$prompt" == *"myorg/myrepo"* ]]
}

@test "start_build_refinement_prompt: includes repo name" {
    create_mock "gh" 0 "[]"

    local prompt
    prompt=$(start_build_refinement_prompt "myorg/myrepo")

    [[ "$prompt" == *"myorg/myrepo"* ]]
}

@test "start_build_review_prompt: includes repo name" {
    create_mock "gh" 0 "[]"

    local prompt
    prompt=$(start_build_review_prompt "myorg/myrepo")

    [[ "$prompt" == *"myorg/myrepo"* ]]
}

# ---------------------------------------------------------------------------
# Gather helpers — gh is called with the repo argument
# ---------------------------------------------------------------------------

@test "start_gather_triage: calls gh with repo" {
    create_mock "gh" 0 "[]"

    start_gather_triage "myorg/myrepo"

    local calls
    calls="$(get_mock_calls "gh")"
    [[ "$calls" == *"myorg/myrepo"* ]]
}

@test "start_gather_refinement: calls gh with repo" {
    create_mock "gh" 0 "[]"

    start_gather_refinement "myorg/myrepo"

    local calls
    calls="$(get_mock_calls "gh")"
    [[ "$calls" == *"myorg/myrepo"* ]]
}

@test "start_gather_review: calls gh with repo" {
    create_mock "gh" 0 "[]"

    start_gather_review "myorg/myrepo"

    local calls
    calls="$(get_mock_calls "gh")"
    [[ "$calls" == *"myorg/myrepo"* ]]
}
