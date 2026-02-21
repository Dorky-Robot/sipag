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
worker_loop() {
    local repo="$1"

    echo "sipag work"
    echo "Repo: ${repo}"
    echo "Label: ${WORKER_WORK_LABEL:-<all>}"
    echo "Batch size: ${WORKER_BATCH_SIZE}"
    echo "Poll interval: ${WORKER_POLL_INTERVAL}s"
    echo "Logs: ${WORKER_LOG_DIR}/"
    echo "Started: $(date)"
    echo ""

    while true; do
        # Check drain signal before picking up new work
        if [[ -f "${SIPAG_DIR}/drain" ]]; then
            echo "[$(date +%H:%M:%S)] Drain signal detected. Finishing in-flight work, not picking up new issues."
            break
        fi

        # Reconcile: close issues that already have merged PRs
        worker_reconcile "$repo"

        # Auto-merge clean sipag PRs (prevents conflict cascades)
        worker_auto_merge "$repo"

        # Fetch open issues with the work label
        local -a label_args=()
        [[ -n "$WORKER_WORK_LABEL" ]] && label_args=(--label "$WORKER_WORK_LABEL")
        mapfile -t all_issues < <(gh issue list --repo "$repo" --state open "${label_args[@]}" --json number -q '.[].number' | sort -n)

        local new_issues=()
        for issue in "${all_issues[@]}"; do
            if worker_is_seen "$issue"; then
                # Seen issues: skip only while an open PR is still in progress.
                # If re-labeled approved after a closed/failed PR, re-queue.
                if worker_has_open_pr "$repo" "$issue"; then
                    continue
                fi
                echo "[$(date +%H:%M:%S)] Re-queuing #${issue} (re-approved, no open PR)"
                worker_unsee "$issue"
            elif worker_has_pr "$repo" "$issue"; then
                # Never seen but already has an open or merged PR (e.g. from another session)
                echo "[$(date +%H:%M:%S)] Skipping #${issue} (already has a PR)"
                worker_mark_seen "$issue"
                continue
            fi
            new_issues+=("$issue")
        done

        # Find open PRs with review feedback requesting changes
        local prs_to_iterate=()
        mapfile -t prs_needing_changes < <(worker_find_prs_needing_iteration "$repo")
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
            if [[ "${WORKER_ONCE}" -eq 1 ]]; then
                echo "[$(date +%H:%M:%S)] --once: ${#all_issues[@]} approved, ${total_open} open total, ${open_prs} PRs open. No work found — exiting."
                break
            fi
            echo "[$(date +%H:%M:%S)] ${#all_issues[@]} approved, ${total_open} open total, ${open_prs} PRs open. Next poll in ${WORKER_POLL_INTERVAL}s..."
            sleep "$WORKER_POLL_INTERVAL"
            continue
        fi

        # PR iterations take priority over new issues: fix what is already in flight first.
        if [[ ${#prs_to_iterate[@]} -gt 0 ]]; then
            echo "[$(date +%H:%M:%S)] Found ${#prs_to_iterate[@]} PRs needing iteration: ${prs_to_iterate[*]}"

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

            for ((i = 0; i < ${#new_issues[@]}; i += WORKER_BATCH_SIZE)); do
                local batch=("${new_issues[@]:i:WORKER_BATCH_SIZE}")
                echo "--- Issue batch: ${batch[*]} ---"

                for issue in "${batch[@]}"; do
                    worker_mark_seen "$issue"
                done

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

        echo "[$(date +%H:%M:%S)] Cycle done. Open PRs:"
        gh pr list --repo "$repo" --state open --json number,title \
            -q '.[] | "  #\(.number): \(.title)"'
        echo ""
        if [[ "${WORKER_ONCE}" -eq 1 ]]; then
            echo "[$(date +%H:%M:%S)] --once: cycle complete, exiting."
            break
        fi
        echo "[$(date +%H:%M:%S)] Next poll in ${WORKER_POLL_INTERVAL}s..."
        sleep "$WORKER_POLL_INTERVAL"
    done
}
