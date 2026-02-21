.PHONY: build install test lint fmt fmt-check dev clean machete install-hooks

build:
	cargo build --release

install:
	cargo install --path sipag

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

# ── Rust (no-op until Rust migration lands) ───────────────────────────────────
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
