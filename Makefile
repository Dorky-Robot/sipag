.PHONY: build install test lint fmt fmt-check dev clean

build:
	cargo build --release

install:
	cargo install --path sipag

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

dev: lint fmt-check test

clean:
	cargo clean
