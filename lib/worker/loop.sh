#!/usr/bin/env bash
# sipag — worker main polling loop
#
# Orchestrates the polling cycle: reconcile merged PRs, dispatch new issue
# workers, dispatch PR iteration workers. Calls into github.sh, dedup.sh,
# and docker.sh; does no I/O itself beyond logging and sleeping.
#
# Depends on globals set by config.sh:
#   WORKER_WORK_LABEL, WORKER_BATCH_SIZE, WORKER_POLL_INTERVAL,
#   WORKER_LOG_DIR, WORKER_ONCE

# shellcheck disable=SC2154  # Globals defined in config.sh, sourced by worker.sh

# Main polling loop
# Accepts one or more repos in "owner/repo" format.
# When multiple repos are given, each polling cycle iterates through all of them.
worker_loop() {
    local -a repos=("$@")

    echo "sipag work"
    if [[ ${#repos[@]} -eq 1 ]]; then
        echo "Repo: ${repos[0]}"
    else
        echo "Repos (${#repos[@]}): ${repos[*]}"
    fi
    echo "Label: ${WORKER_WORK_LABEL:-<all>}"
    echo "Batch size: ${WORKER_BATCH_SIZE}"
    echo "Poll interval: ${WORKER_POLL_INTERVAL}s"
    echo "Logs: ${WORKER_LOG_DIR}/"
    echo "Started: $(date)"
    echo ""

    # Recover orphaned containers from a previous worker process crash.
    # Runs once before the first poll cycle so state files and labels are
    # consistent before we start dispatching new work.
    worker_recover

    while true; do
        # Check drain signal before picking up new work
        if [[ -f "${SIPAG_DIR}/drain" ]]; then
            echo "[$(date +%H:%M:%S)] Drain signal detected. Finishing in-flight work, not picking up new issues."
            break
        fi

        # Finalize any containers that exited since last cycle.
        # This catches containers adopted by worker_recover() or left over
        # from a killed worker — no background subshells needed.
        worker_finalize_exited

        local found_work=0
        local repo
        for repo in "${repos[@]}"; do
            # Update per-repo globals (slug for log/state file naming)
            # shellcheck disable=SC2034  # Used by docker.sh (sourced, not seen by shellcheck)
            WORKER_REPO_SLUG="${repo//\//--}"

            # Reconcile: close issues that already have merged PRs
            worker_reconcile "$repo"

            # Auto-merge clean sipag PRs (prevents conflict cascades)
            worker_auto_merge "$repo"

            # Fetch open issues with the work label
            local -a label_args=()
            [[ -n "$WORKER_WORK_LABEL" ]] && label_args=(--label "$WORKER_WORK_LABEL")
            local -a all_issues=()
            mapfile -t all_issues < <(gh issue list --repo "$repo" --state open "${label_args[@]}" --json number -q '.[].number' | sort -n)

            local -a new_issues=()
            local issue
            for issue in "${all_issues[@]}"; do
                # 1. State file says done → skip (already completed successfully)
                if worker_is_completed "$repo" "$issue"; then
                    continue
                fi

                # 2. State file says running → skip (container may still be alive)
                if worker_is_in_flight "$repo" "$issue"; then
                    continue
                fi

                # 3. State file says failed + issue still labeled approved → re-dispatch
                if worker_is_failed "$repo" "$issue"; then
                    echo "[$(date +%H:%M:%S)] Re-queuing #${issue} (previous worker failed, issue still approved)"
                    new_issues+=("$issue")
                    continue
                fi

                # 4. No state file → check for existing open or merged PR
                if worker_has_pr "$repo" "$issue"; then
                    # PR exists from another session; record as done so we skip next cycle
                    echo "[$(date +%H:%M:%S)] Skipping #${issue} (existing PR found, recording state)"
                    worker_mark_state_done "$repo" "$issue"
                    continue
                fi

                # No state file, no PR → dispatch new worker
                new_issues+=("$issue")
            done

            # Find open PRs with review feedback requesting changes
            local -a prs_to_iterate=()
            local -a prs_needing_changes=()
            mapfile -t prs_needing_changes < <(worker_find_prs_needing_iteration "$repo")
            local pr_num
            for pr_num in "${prs_needing_changes[@]}"; do
                if worker_pr_is_running "$pr_num"; then
                    echo "[$(date +%H:%M:%S)] Skipping PR #${pr_num} iteration (already in progress)"
                    continue
                fi
                prs_to_iterate+=("$pr_num")
            done

            if [[ ${#new_issues[@]} -eq 0 && ${#prs_to_iterate[@]} -eq 0 ]]; then
                local total_open open_prs
                total_open=$(gh issue list --repo "$repo" --state open --limit 500 --json number --jq 'length' 2>/dev/null || echo "?")
                open_prs=$(gh pr list --repo "$repo" --state open --json number --jq 'length' 2>/dev/null || echo "?")
                echo "[$(date +%H:%M:%S)] [${repo}] ${#all_issues[@]} approved, ${total_open} open total, ${open_prs} PRs open. No work."
                continue
            fi

            found_work=1

            # PR iterations take priority over new issues: fix what is already in flight first.
            if [[ ${#prs_to_iterate[@]} -gt 0 ]]; then
                echo "[$(date +%H:%M:%S)] Found ${#prs_to_iterate[@]} PRs needing iteration: ${prs_to_iterate[*]}"

                local i
                for ((i = 0; i < ${#prs_to_iterate[@]}; i += WORKER_BATCH_SIZE)); do
                    local iter_batch=("${prs_to_iterate[@]:i:WORKER_BATCH_SIZE}")
                    echo "--- PR iteration batch: ${iter_batch[*]} ---"

                    local pids=()
                    for pr_num in "${iter_batch[@]}"; do
                        worker_run_pr_iteration "$repo" "$pr_num" &
                        pids+=($!)
                    done

                    for pid in "${pids[@]}"; do
                        wait "$pid" 2>/dev/null || true
                    done

                    echo "--- PR iteration batch complete ---"
                    echo ""
                done
            fi

            # Process new issues in batches (after PR iterations)
            if [[ ${#new_issues[@]} -gt 0 ]]; then
                echo "[$(date +%H:%M:%S)] Found ${#new_issues[@]} new issues: ${new_issues[*]}"

                local i
                for ((i = 0; i < ${#new_issues[@]}; i += WORKER_BATCH_SIZE)); do
                    local batch=("${new_issues[@]:i:WORKER_BATCH_SIZE}")
                    echo "--- Issue batch: ${batch[*]} ---"

                    local pids=()
                    for issue in "${batch[@]}"; do
                        worker_run_issue "$repo" "$issue" &
                        pids+=($!)
                    done

                    for pid in "${pids[@]}"; do
                        wait "$pid" 2>/dev/null || true
                    done

                    echo "--- Batch complete ---"
                    echo ""
                done
            fi

            echo "[$(date +%H:%M:%S)] [${repo}] Cycle done. Open PRs:"
            gh pr list --repo "$repo" --state open --json number,title \
                -q '.[] | "  #\(.number): \(.title)"'
            echo ""
        done

        if [[ "${WORKER_ONCE}" -eq 1 ]]; then
            if [[ $found_work -eq 0 ]]; then
                echo "[$(date +%H:%M:%S)] --once: no work found — exiting."
            else
                echo "[$(date +%H:%M:%S)] --once: cycle complete, exiting."
            fi
            break
        fi
        echo "[$(date +%H:%M:%S)] Next poll in ${WORKER_POLL_INTERVAL}s..."
        sleep "$WORKER_POLL_INTERVAL"
    done
}
