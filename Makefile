.PHONY: test test-unit test-integ lint fmt-check dev tui build clippy cargo-test

test: test-unit test-integ

test-unit:
	bats test/unit/

test-integ:
	bats test/integration/

lint:
	shellcheck -x -S warning bin/sipag lib/*.sh .claude/hooks/safety-gate.sh

fmt-check:
	shfmt -d bin/sipag lib/*.sh .claude/hooks/safety-gate.sh

fmt:
	shfmt -w bin/sipag lib/*.sh .claude/hooks/safety-gate.sh

dev: lint fmt-check test

# ── Rust TUI ──────────────────────────────────────────────────────────────────

tui:
	cargo build --release --manifest-path tui/Cargo.toml

build:
	cargo build

clippy:
	cargo clippy -- -D warnings

cargo-test:
	cargo test
