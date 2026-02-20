#!/usr/bin/env bash
# sipag — Docker executor

# --- Time / duration utilities ---

# Convert seconds to a human-readable duration string.
# e.g. 30 → "30s", 90 → "1m30s", 3661 → "1h1m"
_secs_to_dur() {
	local secs="$1"
	if [[ $secs -lt 60 ]]; then
		echo "${secs}s"
	elif [[ $secs -lt 3600 ]]; then
		echo "$((secs / 60))m$((secs % 60))s"
	else
		echo "$((secs / 3600))h$((secs % 3600 / 60))m"
	fi
}

# Parse an ISO 8601 UTC timestamp to epoch seconds.
_ts_to_epoch() {
	local ts="$1"
	if [[ "$(uname)" == "Darwin" ]]; then
		date -j -f "%Y-%m-%dT%H:%M:%SZ" "$ts" +%s 2>/dev/null || echo "0"
	else
		date -d "$ts" +%s 2>/dev/null || echo "0"
	fi
}

# Compute the difference in seconds between two ISO 8601 timestamps.
# Returns 0 if the result would be negative (clock skew / bad data).
_ts_diff_secs() {
	local start="$1"
	local end="$2"
	local se ee diff
	se=$(_ts_to_epoch "$start")
	ee=$(_ts_to_epoch "$end")
	diff=$((ee - se))
	[[ $diff -lt 0 ]] && diff=0
	echo "$diff"
}

# Parse a human-readable duration string (as produced by _secs_to_dur) back to seconds.
# Handles: "30s", "5m30s", "1m0s", "2h30m"
_dur_to_secs() {
	local dur="$1"
	local secs=0
	if [[ "$dur" =~ ^([0-9]+)h([0-9]+)m$ ]]; then
		secs=$(( BASH_REMATCH[1] * 3600 + BASH_REMATCH[2] * 60 ))
	elif [[ "$dur" =~ ^([0-9]+)m([0-9]+)s$ ]]; then
		secs=$(( BASH_REMATCH[1] * 60 + BASH_REMATCH[2] ))
	elif [[ "$dur" =~ ^([0-9]+)m$ ]]; then
		secs=$(( BASH_REMATCH[1] * 60 ))
	elif [[ "$dur" =~ ^([0-9]+)s$ ]]; then
		secs="${BASH_REMATCH[1]}"
	fi
	echo "$secs"
}

# Update a task tracking file in-place to add completed: and duration: fields
# to the YAML frontmatter (inserted just before the closing ---).
# Args: file completed_timestamp duration_string
_tracking_file_complete() {
	local file="$1"
	local completed_ts="$2"
	local duration="$3"
	local tmp
	tmp="$(mktemp)"
	awk -v comp="$completed_ts" -v dur="$duration" '
		NR==1 { print; in_front=1; next }
		in_front && /^---$/ {
			print "completed: " comp
			print "duration: " dur
			in_front=0
		}
		{ print }
	' "$file" >"$tmp"
	mv "$tmp" "$file"
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

	# Write tracking file with run metadata
	{
		printf -- '---\n'
		printf 'repo: %s\n' "${repo_url}"
		[[ -n "${issue}" ]] && printf 'issue: %s\n' "${issue}"
		printf 'started: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
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
					_cts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
					_sts="$(grep -m1 '^started:' "${tracking_file}" | cut -d' ' -f2-)"
					_dur="$(_secs_to_dur "$(_ts_diff_secs "${_sts}" "${_cts}")")"
					_tracking_file_complete "${tracking_file}" "${_cts}" "${_dur}"
					mv "${tracking_file}" "${done_dir}/${task_id}.md"
				fi
				[[ -f "${log_file}" ]] && mv "${log_file}" "${done_dir}/${task_id}.log"
				echo "==> Done: ${task_id}"
			else
				if [[ -f "${tracking_file}" ]]; then
					_cts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
					_sts="$(grep -m1 '^started:' "${tracking_file}" | cut -d' ' -f2-)"
					_dur="$(_secs_to_dur "$(_ts_diff_secs "${_sts}" "${_cts}")")"
					_tracking_file_complete "${tracking_file}" "${_cts}" "${_dur}"
					mv "${tracking_file}" "${failed_dir}/${task_id}.md"
				fi
				[[ -f "${log_file}" ]] && mv "${log_file}" "${failed_dir}/${task_id}.log"
				echo "==> Failed: ${task_id}"
			fi
		) &
		disown
	else
		local completed_ts started_ts dur
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
			completed_ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
			started_ts="$(grep -m1 '^started:' "${tracking_file}" | cut -d' ' -f2-)"
			dur="$(_secs_to_dur "$(_ts_diff_secs "${started_ts}" "${completed_ts}")")"
			_tracking_file_complete "${tracking_file}" "${completed_ts}" "${dur}"
			mv "${tracking_file}" "${done_dir}/${task_id}.md"
			[[ -f "${log_file}" ]] && mv "${log_file}" "${done_dir}/${task_id}.log"
			echo "==> Done: ${task_id}"
		else
			completed_ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
			started_ts="$(grep -m1 '^started:' "${tracking_file}" | cut -d' ' -f2-)"
			dur="$(_secs_to_dur "$(_ts_diff_secs "${started_ts}" "${completed_ts}")")"
			_tracking_file_complete "${tracking_file}" "${completed_ts}" "${dur}"
			mv "${tracking_file}" "${failed_dir}/${task_id}.md"
			[[ -f "${log_file}" ]] && mv "${log_file}" "${failed_dir}/${task_id}.log"
			echo "==> Failed: ${task_id}"
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
			processed=$((processed + 1))
			continue
		fi

		# Look up repo URL
		local url
		if ! url=$(repo_url "$TASK_REPO"); then
			echo "Error: repo '${TASK_REPO}' not found in repos.conf" >&2
			mv "$task_file" "${failed_dir}/${task_name}.md"
			echo "==> Failed: ${task_name}"
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
