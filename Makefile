.PHONY: build install uninstall test lint fmt fmt-check dev clean machete install-hooks review review-security review-architecture review-correctness

SHARE_DIR ?= $(HOME)/.sipag/share
BIN_DIR   ?= $(HOME)/.cargo/bin

build:
	cargo build --release

install:
	@# Rust binaries — sipag is the sole entry point, sipag-tui for the TUI
	cargo install --path sipag
	cargo install --path tui
	@# Bash scripts — kept in share/ for Docker containers (setup, doctor, etc.)
	@mkdir -p "$(SHARE_DIR)/lib/prompts" "$(SHARE_DIR)/lib/container"
	@install -m 644 lib/*.sh "$(SHARE_DIR)/lib/"
	@install -m 644 lib/container/*.sh "$(SHARE_DIR)/lib/container/"
	@install -m 644 lib/prompts/*.md "$(SHARE_DIR)/lib/prompts/"
	@echo ""
	@echo "sipag installed. Try: sipag status"

uninstall:
	@rm -f "$(BIN_DIR)/sipag"
	@rm -rf "$(SHARE_DIR)"
	@cargo uninstall sipag 2>/dev/null || true
	@cargo uninstall sipag-tui 2>/dev/null || true
	@echo "sipag uninstalled."

# ── Testing ───────────────────────────────────────────────────────────────────
test:
	cargo test

# ── Code quality ──────────────────────────────────────────────────────────────
lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

# ── Rust ──────────────────────────────────────────────────────────────────────
machete:
	@if command -v cargo-machete >/dev/null 2>&1; then \
		cargo machete; \
	else \
		echo "cargo-machete not installed — skipping (install: cargo install cargo-machete)"; \
	fi

# ── Developer workflow ────────────────────────────────────────────────────────
# Full local validation cycle: format → lint → test
dev: fmt lint test

clean:
	cargo clean

# ── Hook installation ─────────────────────────────────────────────────────────
# Run once after cloning to activate pre-commit and pre-push quality gates.
install-hooks:
	git config core.hooksPath .husky
	chmod +x .husky/pre-commit .husky/pre-push
	@echo "Git hooks installed (core.hooksPath → .husky)"
	@echo "  pre-commit: gitleaks, typos, cargo deny, fmt, clippy, shellcheck"
	@echo "  pre-push:   cargo test, cargo machete, gitleaks"

# ── Review agents ─────────────────────────────────────────────────────────────
# Invoke specialized Claude Code review agents from .claude/agents/.
# Run these in a `sipag start` session or directly with `claude --print`.
review:
	@echo "sipag review agents:"
	@echo ""
	@echo "  make review-security      STRIDE + OWASP + Docker security"
	@echo "  make review-architecture  Crate boundaries + bash modules + config"
	@echo "  make review-correctness   Lifecycle edges + races + API error handling"
	@echo ""
	@echo "To review a PR in a sipag start session:"
	@echo "  Review PR #<N> using the security reviewer"
	@echo "  Review PR #<N> using the architecture reviewer"
	@echo "  Review PR #<N> using the correctness reviewer"
	@echo ""
	@echo "Or invoke directly (pipe a diff):"
	@echo "  gh pr diff <N> | claude --print -p 'Review this diff' --agent security-reviewer"

review-security:
	@echo "Running security reviewer..."
	@echo "Provide a PR number or diff to review:"
	@echo "  gh pr diff <N> | claude --print --agent security-reviewer -p 'Review this diff for security issues'"

review-architecture:
	@echo "Running architecture reviewer..."
	@echo "Provide a PR number or diff to review:"
	@echo "  gh pr diff <N> | claude --print --agent architecture-reviewer -p 'Review this diff for architecture issues'"

review-correctness:
	@echo "Running correctness reviewer..."
	@echo "Provide a PR number or diff to review:"
	@echo "  gh pr diff <N> | claude --print --agent correctness-reviewer -p 'Review this diff for correctness issues'"
