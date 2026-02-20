#!/usr/bin/env bash
# sipag — Docker executor

# Format seconds as human-readable duration (e.g. "12m34s", "1h30m", "45s").
format_duration() {
	local seconds="$1"
	if [[ $seconds -lt 60 ]]; then
		printf '%ds' "$seconds"
	elif [[ $seconds -lt 3600 ]]; then
		printf '%dm%ds' "$((seconds / 60))" "$((seconds % 60))"
	else
		local h=$((seconds / 3600))
		local m=$((seconds % 3600 / 60))
		if [[ $m -eq 0 ]]; then
			printf '%dh' "$h"
		else
			printf '%dh%dm' "$h" "$m"
		fi
	fi
}

# Build the Claude prompt for a task.
# Arguments: title body [issue]
# issue: optional GitHub issue number (e.g. "142") to include in the draft PR body
executor_build_prompt() {
	local title="$1"
	local body="${2:-}"
	local issue="${3:-}"

	printf 'You are working on the repository at /work.\n'
	printf '\nYour task:\n'
	printf '%s\n' "$title"
	if [[ -n "$body" ]]; then
		printf '%s\n' "$body"
	fi
	printf '\nInstructions:\n'
	printf '%s\n' \
		'- Read and follow any CLAUDE.md or project-specific instructions in the repository' \
		'- Create a new branch with a descriptive name' \
		'- Before writing any code, open a draft pull request with this body:'
	printf '%s\n' \
		'    > This PR is being worked on by sipag. Commits will appear as work progresses.' \
		"    Task: ${title}"
	if [[ -n "$issue" ]]; then
		printf '%s\n' "    Issue: #${issue}"
	fi
	printf '%s\n' \
		'- The PR title should match the task title' \
		'- Commit after each logical unit of work (not just at the end)' \
		'- Push after each commit so GitHub reflects progress in real time' \
		'- Run any existing tests and make sure they pass' \
		'- When all work is complete, update the PR body with a summary of what changed and why' \
		'- When all work is complete, mark the pull request as ready for review'
}

# Run a single task inside a Docker container.
# Arguments: task_file (full path, already moved to running/)
# Side effect: writes a .log file alongside the task file.
# Returns: docker exit code, or 1 on parse/lookup error.
executor_run_task() {
	local task_file="$1"
	local log_file="${task_file%.md}.log"

	# Parse task frontmatter
	if ! task_parse_file "$task_file"; then
		echo "Error: failed to parse task file: ${task_file}" >&2
		return 1
	fi

	# Look up repo URL
	local url
	if ! url=$(repo_url "$TASK_REPO"); then
		echo "Error: repo '${TASK_REPO}' not found in repos.conf" >&2
		return 1
	fi

	# Extract issue number from TASK_SOURCE (e.g. "github#142" -> "142")
	local issue_num=""
	if [[ "$TASK_SOURCE" =~ \#([0-9]+)$ ]]; then
		issue_num="${BASH_REMATCH[1]}"
	fi

	# Build Claude prompt
	local prompt
	prompt=$(executor_build_prompt "$TASK_TITLE" "$TASK_BODY" "$issue_num")

	# Resolve OAuth token from file if not already in environment
	local token_file="${SIPAG_TOKEN_FILE:-${HOME}/.sipag/token}"
	if [[ -z "${CLAUDE_CODE_OAUTH_TOKEN:-}" && -f "$token_file" ]]; then
		CLAUDE_CODE_OAUTH_TOKEN="$(cat "$token_file")"
		export CLAUDE_CODE_OAUTH_TOKEN
	fi

	local image="${SIPAG_IMAGE:-sipag-worker:latest}"
	local timeout_val="${SIPAG_TIMEOUT:-1800}"

	echo "==> Running: $(basename "$task_file" .md)"

	# Run Docker container; capture stdout+stderr to log file
	timeout "$timeout_val" docker run --rm \
		-e CLAUDE_CODE_OAUTH_TOKEN \
		-e GH_TOKEN \
		-e "REPO_URL=${url}" \
		-e "PROMPT=${prompt}" \
		"$image" \
		bash -c 'git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT"' \
		>"$log_file" 2>&1
}

# Core implementation for the sipag run command.
# Args: task_id repo_url description [issue] [background:0|1]
# Expects SIPAG_DIR to be set.
# Creates a tracking file in running/, runs docker, moves to done/ or failed/ on completion.
executor_run_impl() {
	local task_id="$1"
	local repo_url="$2"
	local description="$3"
	local issue="${4:-}"
	local background="${5:-0}"

	local running_dir="${SIPAG_DIR}/running"
	local done_dir="${SIPAG_DIR}/done"
	local failed_dir="${SIPAG_DIR}/failed"
	local tracking_file="${running_dir}/${task_id}.md"
	local log_file="${running_dir}/${task_id}.log"
	local container_name="sipag-${task_id}"

	# Capture start time for duration calculation
	local start_time start_epoch
	start_time="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
	start_epoch="$(date +%s)"

	# Write tracking file with run metadata
	{
		printf -- '---\n'
		printf 'repo: %s\n' "${repo_url}"
		[[ -n "${issue}" ]] && printf 'issue: %s\n' "${issue}"
		printf 'started: %s\n' "${start_time}"
		printf 'container: %s\n' "${container_name}"
		printf -- '---\n'
		printf '%s\n' "${description}"
	} >"${tracking_file}"

	# Build Claude prompt
	local prompt
	prompt=$(executor_build_prompt "${description}" "" "${issue}")

	# Resolve OAuth token from file if not already in environment
	local token_file="${SIPAG_TOKEN_FILE:-${HOME}/.sipag/token}"
	if [[ -z "${CLAUDE_CODE_OAUTH_TOKEN:-}" && -f "${token_file}" ]]; then
		CLAUDE_CODE_OAUTH_TOKEN="$(cat "${token_file}")"
		export CLAUDE_CODE_OAUTH_TOKEN
	fi

	local image="${SIPAG_IMAGE:-sipag-worker:latest}"
	local timeout_val="${SIPAG_TIMEOUT:-1800}"

	if [[ "${background}" -eq 1 ]]; then
		(
			if timeout "${timeout_val}" docker run --rm --name "${container_name}" \
				-e CLAUDE_CODE_OAUTH_TOKEN \
				-e GH_TOKEN \
				-e "REPO_URL=${repo_url}" \
				-e "PROMPT=${prompt}" \
				"${image}" \
				bash -c 'git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT"' \
				>"${log_file}" 2>&1; then
				if [[ -f "${tracking_file}" ]]; then
					local end_epoch; end_epoch="$(date +%s)"
					printf 'completed: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"${tracking_file}"
					printf 'duration: %s\n' "$(format_duration $((end_epoch - start_epoch)))" >>"${tracking_file}"
					mv "${tracking_file}" "${done_dir}/${task_id}.md"
				fi
				[[ -f "${log_file}" ]] && mv "${log_file}" "${done_dir}/${task_id}.log"
				echo "==> Done: ${task_id}"
				notify "success" "${description}"
			else
				if [[ -f "${tracking_file}" ]]; then
					local end_epoch; end_epoch="$(date +%s)"
					printf 'completed: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"${tracking_file}"
					printf 'duration: %s\n' "$(format_duration $((end_epoch - start_epoch)))" >>"${tracking_file}"
					mv "${tracking_file}" "${failed_dir}/${task_id}.md"
				fi
				[[ -f "${log_file}" ]] && mv "${log_file}" "${failed_dir}/${task_id}.log"
				echo "==> Failed: ${task_id}"
				notify "failure" "${description}"
			fi
		) &
		disown
	else
		if timeout "${timeout_val}" docker run --rm --name "${container_name}" \
			-e CLAUDE_CODE_OAUTH_TOKEN \
			-e GH_TOKEN \
			-e "REPO_URL=${repo_url}" \
			-e "PROMPT=${prompt}" \
			"${image}" \
			bash -c 'git clone "$REPO_URL" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
claude --print --dangerously-skip-permissions -p "$PROMPT"' \
			>"${log_file}" 2>&1; then
			local end_epoch; end_epoch="$(date +%s)"
			printf 'completed: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"${tracking_file}"
			printf 'duration: %s\n' "$(format_duration $((end_epoch - start_epoch)))" >>"${tracking_file}"
			mv "${tracking_file}" "${done_dir}/${task_id}.md"
			[[ -f "${log_file}" ]] && mv "${log_file}" "${done_dir}/${task_id}.log"
			echo "==> Done: ${task_id}"
			notify "success" "${description}"
		else
			local end_epoch; end_epoch="$(date +%s)"
			printf 'completed: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"${tracking_file}"
			printf 'duration: %s\n' "$(format_duration $((end_epoch - start_epoch)))" >>"${tracking_file}"
			mv "${tracking_file}" "${failed_dir}/${task_id}.md"
			[[ -f "${log_file}" ]] && mv "${log_file}" "${failed_dir}/${task_id}.log"
			echo "==> Failed: ${task_id}"
			notify "failure" "${description}"
		fi
	fi
}

# Worker loop: pick tasks from queue/, run in Docker, move to done/ or failed/.
# Loops until queue/ is empty. Uses executor_run_impl() internally.
executor_run() {
	local queue_dir="${SIPAG_DIR}/queue"
	local running_dir="${SIPAG_DIR}/running"
	local done_dir="${SIPAG_DIR}/done"
	local failed_dir="${SIPAG_DIR}/failed"
	local processed=0

	while true; do
		# Pick the first .md file from queue (sorted alphabetically by shell glob)
		local task_file=""
		local f
		for f in "${queue_dir}"/*.md; do
			[[ -f "$f" ]] && task_file="$f" && break
		done

		if [[ -z "$task_file" ]]; then
			if [[ $processed -eq 0 ]]; then
				echo "No tasks in queue — use 'sipag add' to queue a task"
			else
				echo "Queue empty — processed ${processed} task(s)"
			fi
			return 0
		fi

		local task_name
		task_name="$(basename "$task_file" .md)"

		# Parse task frontmatter to get repo and description
		if ! task_parse_file "$task_file"; then
			echo "Error: failed to parse task file: ${task_file}" >&2
			mv "$task_file" "${failed_dir}/${task_name}.md"
			echo "==> Failed: ${task_name}"
			notify "failure" "${task_name}"
			processed=$((processed + 1))
			continue
		fi

		# Look up repo URL
		local url
		if ! url=$(repo_url "$TASK_REPO"); then
			echo "Error: repo '${TASK_REPO}' not found in repos.conf" >&2
			mv "$task_file" "${failed_dir}/${task_name}.md"
			echo "==> Failed: ${task_name}"
			notify "failure" "${TASK_TITLE}"
			processed=$((processed + 1))
			continue
		fi

		# Move task to running/ — executor_run_impl will overwrite it with tracking metadata
		mv "$task_file" "${running_dir}/${task_name}.md"

		# Extract issue number from TASK_SOURCE (e.g. "github#142" -> "142")
		local issue_num=""
		if [[ "$TASK_SOURCE" =~ \#([0-9]+)$ ]]; then
			issue_num="${BASH_REMATCH[1]}"
		fi

		# Run the task in foreground via executor_run_impl
		executor_run_impl "${task_name}" "${url}" "${TASK_TITLE}" "${issue_num}" 0 || true

		processed=$((processed + 1))
	done
}
