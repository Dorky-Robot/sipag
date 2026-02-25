.PHONY: build install uninstall test lint fmt fmt-check dev clean machete install-hooks review review-security review-architecture review-correctness

SHARE_DIR ?= $(HOME)/.sipag/share
BIN_DIR   ?= $(HOME)/.cargo/bin

build:
	cargo build --release

install:
	@# Rust binaries — sipag is the sole entry point, sipag-tui for the TUI
	cargo install --path sipag
	cargo install --path tui
	cargo install --path sipag-worker
	@# Prompts — kept in share/ for reference
	@mkdir -p "$(SHARE_DIR)/lib/prompts"
	@install -m 644 lib/prompts/*.md "$(SHARE_DIR)/lib/prompts/"
	@echo ""
	@echo "sipag installed. Try: sipag version"

uninstall:
	@rm -f "$(BIN_DIR)/sipag"
	@rm -rf "$(SHARE_DIR)"
	@cargo uninstall sipag 2>/dev/null || true
	@cargo uninstall sipag-tui 2>/dev/null || true
	@echo "sipag uninstalled."

# ── Testing ───────────────────────────────────────────────────────────────────
test:
	cargo test --workspace

# ── Code quality ──────────────────────────────────────────────────────────────
lint:
	cargo clippy --workspace --all-targets -- -D warnings

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
# Install agents first with `sipag init`, then use in a Claude Code session.
review:
	@echo "sipag review agents (install with: sipag init):"
	@echo ""
	@echo "  make review-security      STRIDE + OWASP + Docker security"
	@echo "  make review-architecture  Crate boundaries + bash modules + config"
	@echo "  make review-correctness   Lifecycle edges + races + API error handling"
	@echo ""
	@echo "In a Claude Code session, ask:"
	@echo "  Review PR #<N> using the security reviewer"
	@echo "  Review PR #<N> using the architecture reviewer"
	@echo "  Review PR #<N> using the correctness reviewer"

review-security:
	@echo "In a Claude Code session with agents installed (sipag init), ask:"
	@echo "  Review the latest changes using the security reviewer"

review-architecture:
	@echo "In a Claude Code session with agents installed (sipag init), ask:"
	@echo "  Review the latest changes using the architecture reviewer"

review-correctness:
	@echo "In a Claude Code session with agents installed (sipag init), ask:"
	@echo "  Review the latest changes using the correctness reviewer"
