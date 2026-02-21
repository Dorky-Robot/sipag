#!/usr/bin/env bash
# sipag — Docker worker for GitHub issues
#
# Polls a GitHub repo for open issues, spins up isolated Docker containers
# to work on them via Claude Code, creates PRs. Runs continuously until killed.
#
# This file is a thin loader. Implementation lives in lib/worker/:
#   config.sh  — defaults, worker_load_config(), worker_init(), worker_slugify()
#   dedup.sh   — state-file dedup (worker_is_completed/in_flight/failed,
#                worker_mark_state_done), worker_pr_is/mark_running/done
#   github.sh  — worker_has_pr/open_pr, find_prs_needing_iteration,
#                worker_reconcile, worker_transition_label, sipag_run_hook
#   merge.sh   — worker_auto_merge
#   docker.sh  — worker_run_issue, worker_run_pr_iteration
#   loop.sh    — worker_loop

# Resolve lib dir relative to this file so submodules load correctly
# regardless of the caller's working directory.
_SIPAG_WORKER_LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# shellcheck source=worker/config.sh
source "${_SIPAG_WORKER_LIB}/worker/config.sh"
# shellcheck source=worker/dedup.sh
source "${_SIPAG_WORKER_LIB}/worker/dedup.sh"
# shellcheck source=worker/github.sh
source "${_SIPAG_WORKER_LIB}/worker/github.sh"
# shellcheck source=worker/merge.sh
source "${_SIPAG_WORKER_LIB}/worker/merge.sh"
# shellcheck source=worker/docker.sh
source "${_SIPAG_WORKER_LIB}/worker/docker.sh"
# shellcheck source=worker/loop.sh
source "${_SIPAG_WORKER_LIB}/worker/loop.sh"
# shellcheck source=refresh-docs.sh
source "${_SIPAG_WORKER_LIB}/refresh-docs.sh"
