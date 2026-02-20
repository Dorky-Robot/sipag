#!/usr/bin/env bash
# sipag — task file parser
#
# Reads a markdown checklist file. Finds pending tasks, marks them done,
# lists status, and appends new tasks.

# Find the first unchecked task (- [ ]).
# Sets: TASK_LINE (line number), TASK_TITLE (text), TASK_BODY (indented lines after).
# Returns 1 if no pending tasks found.
task_parse_next() {
	local file="$1"

	if [[ ! -f "$file" ]]; then
		return 1
	fi

	# shellcheck disable=SC2034
	TASK_LINE=""
	# shellcheck disable=SC2034
	TASK_TITLE=""
	# shellcheck disable=SC2034
	TASK_BODY=""

	local line_num=0
	local found=0

	while IFS= read -r line || [[ -n "$line" ]]; do
		line_num=$((line_num + 1))

		if [[ $found -eq 0 ]]; then
			# Look for first unchecked item
			if [[ "$line" =~ ^-\ \[\ \]\ (.+)$ ]]; then
				# shellcheck disable=SC2034
				TASK_LINE=$line_num
				# shellcheck disable=SC2034
				TASK_TITLE="${BASH_REMATCH[1]}"
				found=1
			fi
		else
			# Collect indented body lines (2+ spaces)
			if [[ "$line" =~ ^[[:space:]]{2,} ]]; then
				if [[ -n "$TASK_BODY" ]]; then
					TASK_BODY+=$'\n'
				fi
				# Strip leading whitespace
				TASK_BODY+="${line#"${line%%[![:space:]]*}"}"
			else
				# Non-indented line — stop collecting body
				break
			fi
		fi
	done <"$file"

	if [[ $found -eq 0 ]]; then
		return 1
	fi

	return 0
}

# Mark a task as done at the given line number.
# Portable sed in-place for macOS and Linux.
task_mark_done() {
	local file="$1"
	local line_number="$2"

	if [[ "$(uname)" == "Darwin" ]]; then
		sed -i '' "${line_number}s/- \[ \]/- [x]/" "$file"
	else
		sed -i "${line_number}s/- \[ \]/- [x]/" "$file"
	fi
}

# Print all tasks with status and summary counts.
task_list() {
	local file="$1"

	if [[ ! -f "$file" ]]; then
		echo "No task file: $file"
		return 1
	fi

	local done=0
	local pending=0

	while IFS= read -r line; do
		if [[ "$line" =~ ^-\ \[x\]\ (.+)$ ]]; then
			echo "  [x] ${BASH_REMATCH[1]}"
			done=$((done + 1))
		elif [[ "$line" =~ ^-\ \[\ \]\ (.+)$ ]]; then
			echo "  [ ] ${BASH_REMATCH[1]}"
			pending=$((pending + 1))
		fi
	done <"$file"

	local total=$((done + pending))
	echo ""
	echo "${done}/${total} done"
}

# Create the sipag directory structure (idempotent).
# Uses SIPAG_DIR env var if no argument is given (default: ~/.sipag).
# Prints a confirmation line for each directory created, then a summary.
sipag_init_dirs() {
	local dir="${1:-${SIPAG_DIR:-${HOME}/.sipag}}"
	local created=0

	for subdir in queue running done failed; do
		if [[ ! -d "${dir}/${subdir}" ]]; then
			mkdir -p "${dir}/${subdir}"
			echo "Created: ${dir}/${subdir}"
			created=1
		fi
	done

	if [[ $created -eq 0 ]]; then
		echo "Already initialized: ${dir}"
	else
		echo "Initialized: ${dir}"
	fi
}

# Append a new pending task. Creates the file if missing.
task_add() {
	local file="$1"
	local text="$2"

	if [[ ! -f "$file" ]]; then
		echo "- [ ] ${text}" >"$file"
	else
		echo "- [ ] ${text}" >>"$file"
	fi
}
