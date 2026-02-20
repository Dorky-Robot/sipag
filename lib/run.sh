#!/usr/bin/env bash
# sipag — Claude CLI wrapper

# Run a prompt through the Claude CLI and print the response to stdout.
# Arguments: prompt
# Environment: SIPAG_MODEL — Claude model to use (default: claude-opus-4-5)
run_claude() {
	local prompt="$1"
	local model="${SIPAG_MODEL:-claude-opus-4-5}"

	claude --model "$model" --print "$prompt"
}
