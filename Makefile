PREFIX ?= /usr/local
SHAREDIR = $(PREFIX)/share/sipag

build-tui:
	cd tui && cargo build --release

install: build-tui
	@mkdir -p $(SHAREDIR)/bin
	@mkdir -p $(SHAREDIR)/lib/core
	@mkdir -p $(SHAREDIR)/lib/sources
	@mkdir -p $(SHAREDIR)/lib/hooks
	@cp bin/sipag $(SHAREDIR)/bin/sipag
	@chmod +x $(SHAREDIR)/bin/sipag
	@cp lib/core/*.sh $(SHAREDIR)/lib/core/
	@cp lib/sources/*.sh $(SHAREDIR)/lib/sources/
	@cp lib/hooks/*.sh $(SHAREDIR)/lib/hooks/
	@chmod +x $(SHAREDIR)/lib/hooks/*.sh
	@mkdir -p $(PREFIX)/bin
	@ln -sf $(SHAREDIR)/bin/sipag $(PREFIX)/bin/sipag
	@cp tui/target/release/sipag-tui $(PREFIX)/bin/sipag-tui
	@echo "sipag installed to $(PREFIX)/bin/sipag"

install-bash:
	@mkdir -p $(SHAREDIR)/bin
	@mkdir -p $(SHAREDIR)/lib/core
	@mkdir -p $(SHAREDIR)/lib/sources
	@mkdir -p $(SHAREDIR)/lib/hooks
	@cp bin/sipag $(SHAREDIR)/bin/sipag
	@chmod +x $(SHAREDIR)/bin/sipag
	@cp lib/core/*.sh $(SHAREDIR)/lib/core/
	@cp lib/sources/*.sh $(SHAREDIR)/lib/sources/
	@cp lib/hooks/*.sh $(SHAREDIR)/lib/hooks/
	@chmod +x $(SHAREDIR)/lib/hooks/*.sh
	@mkdir -p $(PREFIX)/bin
	@ln -sf $(SHAREDIR)/bin/sipag $(PREFIX)/bin/sipag
	@echo "sipag installed to $(PREFIX)/bin/sipag (no TUI)"

uninstall:
	@rm -f $(PREFIX)/bin/sipag
	@rm -f $(PREFIX)/bin/sipag-tui
	@rm -rf $(SHAREDIR)
	@echo "sipag uninstalled"

SH_FILES = $(shell find bin/ lib/ scripts/ -name '*.sh' -o -name 'sipag' 2>/dev/null)

lint:
	@echo "Running shellcheck..."
	@shellcheck -x --severity=warning $(SH_FILES)
	@echo "shellcheck passed"

fmt:
	shfmt -w -i 2 -ci $(SH_FILES)

fmt-check:
	@echo "Checking formatting..."
	@shfmt -d -i 2 -ci $(SH_FILES)
	@echo "formatting OK"

typos:
	typos --format brief

check-bats:
	@command -v bats >/dev/null 2>&1 || { echo "bats not found. Install: brew install bats-core"; exit 1; }

test: check-bats
	bats test/unit/ test/integration/

test-unit: check-bats
	bats test/unit/

test-integ: check-bats
	bats test/integration/

test-parallel: check-bats
	@NCPUS=$$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4); \
	HALF=$$(( NCPUS / 2 )); [ "$$HALF" -lt 1 ] && HALF=1; \
	echo "Running tests on $$NCPUS cores ($$HALF per suite)..."; \
	ULOG=$$(mktemp); ILOG=$$(mktemp); \
	bats --jobs "$$HALF" test/unit/ > "$$ULOG" 2>&1 & UPID=$$!; \
	bats --jobs "$$HALF" test/integration/ > "$$ILOG" 2>&1 & IPID=$$!; \
	URC=0; IRC=0; wait "$$UPID" || URC=$$?; wait "$$IPID" || IRC=$$?; \
	[ "$$URC" -ne 0 ] && echo "Unit FAILED:" && cat "$$ULOG"; \
	[ "$$IRC" -ne 0 ] && echo "Integration FAILED:" && cat "$$ILOG"; \
	[ "$$URC" -eq 0 ] && echo "Unit tests passed"; \
	[ "$$IRC" -eq 0 ] && echo "Integration tests passed"; \
	rm -f "$$ULOG" "$$ILOG"; [ "$$URC" -eq 0 ] && [ "$$IRC" -eq 0 ]

check: typos lint fmt-check

dev: lint fmt-check test

review:
	@TMPDIR_REVIEW=$$(mktemp -d); \
	trap 'rm -rf "$$TMPDIR_REVIEW"' EXIT; \
	git diff > "$$TMPDIR_REVIEW/diff.txt"; \
	git diff --name-only > "$$TMPDIR_REVIEW/files.txt"; \
	scripts/review.sh --hook manual \
	  --diff-file "$$TMPDIR_REVIEW/diff.txt" \
	  --files-file "$$TMPDIR_REVIEW/files.txt"

hooks:
	@echo "Installing git hooks..."
	@git config core.hooksPath .husky
	@echo "Git hooks installed (using .husky/)"

.PHONY: build-tui install install-bash uninstall lint fmt fmt-check typos check-bats test test-unit test-integ test-parallel check dev review hooks
