#!/usr/bin/env bash
# sipag — source plugin interface
#
# Each source plugin must implement the following functions.
# All functions receive the repo identifier as their first argument.
#
# source_list_tasks <repo> <label>
#   Print one task ID per line for tasks matching <label>.
#   Exit 0 on success, non-zero on failure.
#
# source_claim_task <repo> <task_id> <wip_label> <ready_label>
#   Mark a task as in-progress (add wip_label, remove ready_label).
#   Exit 0 on success, non-zero on failure.
#
# source_get_task <repo> <task_id>
#   Print task details as KEY=VALUE lines:
#     TASK_TITLE=...
#     TASK_BODY=...
#     TASK_NUMBER=...
#     TASK_URL=...
#   Exit 0 on success, non-zero on failure.
#
# source_complete_task <repo> <task_id> <done_label> <wip_label> <pr_url>
#   Mark a task as completed (add done_label, remove wip_label, post PR link).
#   Exit 0 on success, non-zero on failure.
#
# source_fail_task <repo> <task_id> <ready_label> <wip_label> <error_msg>
#   Return a task to backlog (add ready_label, remove wip_label, post error).
#   Exit 0 on success, non-zero on failure.
#
# source_comment <repo> <task_id> <message>
#   Post a comment on the task.
#   Exit 0 on success, non-zero on failure.
