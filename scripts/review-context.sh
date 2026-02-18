#!/usr/bin/env bash
# review-context.sh — shared context-gathering functions for code review
# Sourced by review.sh and hooks. Sets global variables.
#
# Required inputs (must be set before calling gather_all_context):
#   CHANGED_FILES — newline-separated list of changed file paths
#
# Outputs (global variables):
#   FULL_CHANGED       — full content of every changed file
#   RELATED_FILES      — files that reference changed modules
#   PROJECT_INSTRUCTIONS — contents of .claude/CLAUDE.md

gather_full_files() {
  FULL_CHANGED=""
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    if [[ -f "$f" ]]; then
      FULL_CHANGED+="
=== FILE: $f ===
$(cat "$f")
"
    fi
  done <<<"$CHANGED_FILES"
}

gather_related_files() {
  RELATED_FILES=""
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    local base
    base=$(basename "$f" .sh)
    [[ "$base" = "_interface" ]] && continue
    # Find other .sh files that reference functions or source this file
    local refs
    refs=$(grep -rlw --include='*.sh' "$base" lib/ bin/ 2>/dev/null | grep -v "$f" | head -5 || true)
    while IFS= read -r ref; do
      if [[ -n "$ref" ]] && ! echo "$RELATED_FILES" | grep -Fq "$ref"; then
        RELATED_FILES+="
=== RELATED FILE: $ref (references $base) ===
$(cat "$ref")
"
      fi
    done <<<"$refs"
  done <<<"$CHANGED_FILES"
}

gather_conventions() {
  PROJECT_INSTRUCTIONS=""
  if [[ -f ".claude/CLAUDE.md" ]]; then
    PROJECT_INSTRUCTIONS=$(cat .claude/CLAUDE.md)
  fi
}

gather_all_context() {
  gather_full_files
  gather_related_files
  gather_conventions

  # Cap context to stay within Claude CLI prompt limits (~100KB).
  # The diff is always included in full; trim supplementary context first.
  MAX_CONTEXT_BYTES=100000
  _total=$(printf '%s%s%s' "$FULL_CHANGED" "$RELATED_FILES" "$PROJECT_INSTRUCTIONS" | wc -c)
  if [[ "$_total" -gt "$MAX_CONTEXT_BYTES" ]]; then
    echo "review-context: context too large (${_total} bytes), trimming supplementary files"
    RELATED_FILES="(trimmed — diff is large, review the diff directly)"
    FULL_CHANGED="(trimmed — diff is large, review the diff directly)"
  fi
}
