#!/usr/bin/env bash
# sipag — GitHub Issues source plugin (uses `gh` CLI)

source_list_tasks() {
  local repo="$1" label="$2"

  gh issue list \
    --repo "$repo" \
    --label "$label" \
    --state open \
    --json number \
    --jq '.[].number' 2>/dev/null
}

source_claim_task() {
  local repo="$1" task_id="$2" wip_label="$3" ready_label="$4"

  gh issue edit "$task_id" \
    --repo "$repo" \
    --add-label "$wip_label" \
    --remove-label "$ready_label" 2>/dev/null
}

source_get_task() {
  local repo="$1" task_id="$2"

  local json
  json=$(gh issue view "$task_id" \
    --repo "$repo" \
    --json title,body,number,url 2>/dev/null) || return 1

  local title body number url
  title=$(echo "$json" | jq -r '.title')
  body=$(echo "$json" | jq -r '.body')
  number=$(echo "$json" | jq -r '.number')
  url=$(echo "$json" | jq -r '.url')

  echo "TASK_TITLE=${title}"
  echo "TASK_BODY=${body}"
  echo "TASK_NUMBER=${number}"
  echo "TASK_URL=${url}"
}

source_complete_task() {
  local repo="$1" task_id="$2" done_label="$3" wip_label="$4" pr_url="$5"

  gh issue edit "$task_id" \
    --repo "$repo" \
    --add-label "$done_label" \
    --remove-label "$wip_label" 2>/dev/null

  if [[ -n "$pr_url" ]]; then
    source_comment "$repo" "$task_id" "PR opened: ${pr_url}"
  fi

  gh issue close "$task_id" --repo "$repo" 2>/dev/null
}

source_fail_task() {
  local repo="$1" task_id="$2" ready_label="$3" wip_label="$4" error_msg="$5"

  gh issue edit "$task_id" \
    --repo "$repo" \
    --add-label "$ready_label" \
    --remove-label "$wip_label" 2>/dev/null

  if [[ -n "$error_msg" ]]; then
    source_comment "$repo" "$task_id" "sipag failed: ${error_msg}"
  fi
}

source_comment() {
  local repo="$1" task_id="$2" message="$3"

  gh issue comment "$task_id" \
    --repo "$repo" \
    --body "$message" 2>/dev/null
}
