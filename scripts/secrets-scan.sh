#!/usr/bin/env bash
# secrets-scan.sh — scan commits being pushed for potential secrets
#
# Called by the pre-push hook.  When run as a git hook, stdin carries:
#   <local-ref> <local-sha1> <remote-ref> <remote-sha1>
# When run standalone (e.g. make secrets-scan), it scans HEAD against the
# upstream tracking branch, or all tracked files if no upstream is set.
#
# Exit codes:
#   0  — clean, nothing found
#   1  — at least one secret pattern matched (push blocked)

set -euo pipefail

# ── Patterns ─────────────────────────────────────────────────────────────────
# Each entry is an extended-regex fragment searched with grep -E.
PATTERNS=(
    # Anthropic / OpenAI API keys
    'sk-ant-[A-Za-z0-9_-]{40,}'
    'sk-[A-Za-z0-9]{20,}'

    # GitHub tokens
    'ghp_[A-Za-z0-9]{36,}'
    'gho_[A-Za-z0-9]{36,}'
    'ghs_[A-Za-z0-9]{36,}'
    'github_pat_[A-Za-z0-9_]{82,}'

    # AWS credentials
    'AKIA[0-9A-Z]{16}'
    'ASIA[0-9A-Z]{16}'

    # Generic private keys (PEM headers)
    'BEGIN (RSA|EC|DSA|OPENSSH) PRIVATE KEY'

    # Slack tokens
    'xox[baprs]-[A-Za-z0-9-]{10,}'

    # Generic high-entropy patterns that look like secrets
    # (variable name followed by quoted high-entropy value)
    '(password|passwd|secret|api_key|auth_token|access_token)\s*[:=]\s*["\047][A-Za-z0-9+/]{16,}["\047]'
)

# ── Files to exclude from scanning ───────────────────────────────────────────
EXCLUDE_PATHS=(
    'test/'
    '*.bats'
    '*.md'
    '*.jpg'
    '*.png'
    '*.gif'
    'scripts/secrets-scan.sh'   # this file contains the patterns themselves
)

# ── Build the grep exclude args ───────────────────────────────────────────────
build_excludes() {
    local args=()
    for path in "${EXCLUDE_PATHS[@]}"; do
        args+=(--exclude="$path" --exclude-dir="${path%/}")
    done
    printf '%s\n' "${args[@]}"
}

# ── Scan a block of diff/file content ────────────────────────────────────────
scan_content() {
    local label="$1"   # descriptive label for messages
    local content="$2"
    local found=0

    for pattern in "${PATTERNS[@]}"; do
        if echo "$content" | grep -qE "$pattern" 2>/dev/null; then
            echo "  MATCH  pattern: $pattern" >&2
            found=1
        fi
    done

    if [[ $found -eq 1 ]]; then
        echo "SECRETS DETECTED in $label" >&2
        return 1
    fi
    return 0
}

# ── Collect ranges to scan (from pre-push stdin or fallback) ─────────────────
collect_ranges() {
    local ranges=()

    if [[ -t 0 ]]; then
        # Running standalone — compare HEAD against upstream or initial commit
        local upstream
        upstream=$(git rev-parse --abbrev-ref '@{upstream}' 2>/dev/null || true)
        if [[ -n "$upstream" ]]; then
            ranges+=("${upstream}..HEAD")
        else
            # No upstream: scan all commits reachable from HEAD
            local root
            root=$(git rev-list --max-parents=0 HEAD 2>/dev/null || true)
            if [[ -n "$root" ]]; then
                ranges+=("${root}..HEAD")
            fi
        fi
    else
        # Running as pre-push hook — read ref lines from stdin
        while IFS=' ' read -r _local_ref local_sha _remote_ref remote_sha; do
            # Skip deletions and zero-SHA refs
            [[ "$local_sha" =~ ^0+$ ]] && continue
            if [[ "$remote_sha" =~ ^0+$ ]]; then
                # New branch: scan all commits not yet on any remote
                local base
                base=$(git merge-base HEAD "$(git rev-parse --abbrev-ref 'HEAD')" 2>/dev/null \
                    || git rev-list --max-parents=0 HEAD 2>/dev/null || true)
                [[ -n "$base" ]] && ranges+=("${base}..${local_sha}")
            else
                ranges+=("${remote_sha}..${local_sha}")
            fi
        done
    fi

    printf '%s\n' "${ranges[@]}"
}

# ── Main ─────────────────────────────────────────────────────────────────────
main() {
    local failed=0

    mapfile -t ranges < <(collect_ranges)

    if [[ ${#ranges[@]} -eq 0 ]]; then
        echo "secrets-scan: nothing to scan (no commits in range)" >&2
        exit 0
    fi

    for range in "${ranges[@]}"; do
        local diff_content
        diff_content=$(git diff "$range" -- 2>/dev/null || true)

        if [[ -z "$diff_content" ]]; then
            continue
        fi

        if ! scan_content "commits $range" "$diff_content"; then
            failed=1
        fi
    done

    if [[ $failed -eq 1 ]]; then
        echo "" >&2
        echo "Push blocked: potential secrets detected." >&2
        echo "Review the matches above. If they are false positives, update" >&2
        echo "EXCLUDE_PATHS in scripts/secrets-scan.sh or use git-crypt / a" >&2
        echo "secrets manager to handle sensitive values." >&2
        exit 1
    fi

    echo "secrets-scan: clean"
    exit 0
}

main "$@"
