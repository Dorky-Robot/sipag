#!/usr/bin/env bash
# sipag — Docker executor

# Build the Claude prompt for a task.
# Arguments: title body
executor_build_prompt() {
	local title="$1"
	local body="${2:-}"

	printf 'You are working on the repository at /work.\n'
	printf '\nYour task:\n'
	printf '%s\n' "$title"
	if [[ -n "$body" ]]; then
		printf '%s\n' "$body"
	fi
	printf '\nInstructions:\n'
	printf '%s\n' \
		'- Create a new branch with a descriptive name' \
		'- Implement the changes' \
		'- Run any existing tests and make sure they pass' \
		'- Commit your changes with a clear commit message' \
		'- Push the branch and open a draft pull request early so progress is visible' \
		'- The PR title should match the task title' \
		'- The PR body should summarize what you changed and why' \
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

	# Build Claude prompt
	local prompt
	prompt=$(executor_build_prompt "$TASK_TITLE" "$TASK_BODY")

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

# Worker loop: pick tasks from queue/, run in Docker, move to done/ or failed/.
# Loops until queue/ is empty.
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
		local running_file="${running_dir}/${task_name}.md"
		local running_log="${running_dir}/${task_name}.log"

		# Move task to running/
		mv "$task_file" "$running_file"

		if executor_run_task "$running_file"; then
			# Success: move task and log to done/
			mv "$running_file" "${done_dir}/${task_name}.md"
			[[ -f "$running_log" ]] && mv "$running_log" "${done_dir}/${task_name}.log"
			echo "==> Done: ${task_name}"
		else
			local ec=$?
			# Failure: move task and log to failed/
			mv "$running_file" "${failed_dir}/${task_name}.md"
			[[ -f "$running_log" ]] && mv "$running_log" "${failed_dir}/${task_name}.log"
			echo "==> Failed (exit ${ec}): ${task_name}"
		fi

		processed=$((processed + 1))
	done
}
