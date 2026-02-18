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

test:
	@echo "Running safety gate hook tests..."
	@echo '{"tool_name":"Read","tool_input":{"file_path":"/tmp/test.txt"}}' | SIPAG_SAFETY_MODE=strict CLAUDE_PROJECT_DIR=/tmp/project bash lib/hooks/safety-gate.sh | jq -e '.hookSpecificOutput.permissionDecision == "allow"' > /dev/null
	@echo '{"tool_name":"Bash","tool_input":{"command":"sudo rm -rf /"}}' | SIPAG_SAFETY_MODE=strict CLAUDE_PROJECT_DIR=/tmp/project bash lib/hooks/safety-gate.sh | jq -e '.hookSpecificOutput.permissionDecision == "deny"' > /dev/null
	@echo '{"tool_name":"Bash","tool_input":{"command":"git status"}}' | SIPAG_SAFETY_MODE=strict CLAUDE_PROJECT_DIR=/tmp/project bash lib/hooks/safety-gate.sh | jq -e '.hookSpecificOutput.permissionDecision == "allow"' > /dev/null
	@echo '{"tool_name":"Write","tool_input":{"file_path":"/etc/passwd"}}' | SIPAG_SAFETY_MODE=strict CLAUDE_PROJECT_DIR=/tmp/project bash lib/hooks/safety-gate.sh | jq -e '.hookSpecificOutput.permissionDecision == "deny"' > /dev/null
	@echo '{"tool_name":"Bash","tool_input":{"command":"npm install lodash"}}' | SIPAG_SAFETY_MODE=strict CLAUDE_PROJECT_DIR=/tmp/project bash lib/hooks/safety-gate.sh | jq -e '.hookSpecificOutput.permissionDecision == "allow"' > /dev/null
	@echo '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' | SIPAG_SAFETY_MODE=strict CLAUDE_PROJECT_DIR=/tmp/project bash lib/hooks/safety-gate.sh | jq -e '.hookSpecificOutput.permissionDecision == "deny"' > /dev/null
	@echo "All hook tests passed"

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

.PHONY: build-tui install install-bash uninstall lint fmt fmt-check typos test check dev review hooks
