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

# Parse a task file with YAML frontmatter.
# Sets: TASK_REPO, TASK_PRIORITY (default: medium), TASK_SOURCE, TASK_ADDED,
#       TASK_TITLE (first non-empty line after frontmatter), TASK_BODY (remaining lines).
# Returns 1 if the file is not found.
task_parse_file() {
	local file="$1"

	if [[ ! -f "$file" ]]; then
		return 1
	fi

	# shellcheck disable=SC2034
	TASK_REPO=""
	# shellcheck disable=SC2034
	TASK_PRIORITY="medium"
	# shellcheck disable=SC2034
	TASK_SOURCE=""
	# shellcheck disable=SC2034
	TASK_ADDED=""
	# shellcheck disable=SC2034
	TASK_TITLE=""
	# shellcheck disable=SC2034
	TASK_BODY=""

	# Read all lines into an array for indexed access
	local lines=()
	while IFS= read -r line || [[ -n "$line" ]]; do
		lines+=("$line")
	done <"$file"

	local n=${#lines[@]}
	local i=0

	# Check for opening frontmatter delimiter
	if [[ $n -gt 0 && "${lines[0]}" == "---" ]]; then
		i=1
		# Parse frontmatter key: value pairs until closing ---
		while [[ $i -lt $n ]]; do
			if [[ "${lines[$i]}" == "---" ]]; then
				i=$((i + 1))
				break
			fi
			if [[ "${lines[$i]}" =~ ^([a-zA-Z_]+):\ *(.*)$ ]]; then
				local key="${BASH_REMATCH[1]}"
				local value="${BASH_REMATCH[2]}"
				case "$key" in
				repo) TASK_REPO="$value" ;;
				priority) TASK_PRIORITY="$value" ;;
				source) TASK_SOURCE="$value" ;;
				added) TASK_ADDED="$value" ;;
				esac
			fi
			i=$((i + 1))
		done
	fi

	# Find title: first non-empty line after frontmatter
	while [[ $i -lt $n ]]; do
		if [[ -n "${lines[$i]}" ]]; then
			# shellcheck disable=SC2034
			TASK_TITLE="${lines[$i]}"
			i=$((i + 1))
			break
		fi
		i=$((i + 1))
	done

	# Find last non-empty line index to trim trailing blank lines from body
	local body_end=$i
	local j=$((n - 1))
	while [[ $j -ge $i ]]; do
		if [[ -n "${lines[$j]}" ]]; then
			body_end=$((j + 1))
			break
		fi
		j=$((j - 1))
	done

	# Skip leading blank lines in body
	while [[ $i -lt $body_end && -z "${lines[$i]}" ]]; do
		i=$((i + 1))
	done

	# Build TASK_BODY from remaining lines
	local first=1
	while [[ $i -lt $body_end ]]; do
		if [[ $first -eq 0 ]]; then
			TASK_BODY+=$'\n'
		fi
		TASK_BODY+="${lines[$i]}"
		first=0
		i=$((i + 1))
	done

	return 0
}

# Convert text to a URL-safe slug (lowercase, hyphens only, no special chars).
task_slugify() {
	local text="$1"
	printf '%s' "$text" \
		| tr '[:upper:]' '[:lower:]' \
		| tr -cs 'a-z0-9' '-' \
		| sed 's/^-*//;s/-*$//'
}

# Generate the next sequential filename for a task in a queue directory.
# Pattern: NNN-slugified-title.md where NNN is zero-padded (e.g. 001, 042).
task_next_filename() {
	local queue_dir="$1"
	local title="$2"
	local slug
	slug=$(task_slugify "$title")

	local max_num=0
	if [[ -d "$queue_dir" ]]; then
		for f in "${queue_dir}"/*.md; do
			[[ -f "$f" ]] || continue
			local base="${f##*/}"
			if [[ "$base" =~ ^([0-9]+)- ]]; then
				local num=$((10#${BASH_REMATCH[1]}))
				if [[ $num -gt $max_num ]]; then
					max_num=$num
				fi
			fi
		done
	fi

	local next_num=$((max_num + 1))
	printf '%03d-%s.md\n' "$next_num" "$slug"
}

# Write a task file with YAML frontmatter.
# Arguments: file title repo [priority] [source]
task_write_file() {
	local file="$1"
	local title="$2"
	local repo="$3"
	local priority="${4:-medium}"
	local source="${5:-}"
	local added
	added=$(date -u +%Y-%m-%dT%H:%M:%SZ)

	{
		printf -- '---\n'
		printf 'repo: %s\n' "$repo"
		printf 'priority: %s\n' "$priority"
		if [[ -n "$source" ]]; then
			printf 'source: %s\n' "$source"
		fi
		printf 'added: %s\n' "$added"
		printf -- '---\n'
		printf '%s\n' "$title"
	} >"$file"
}
