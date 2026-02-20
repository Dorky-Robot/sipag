.PHONY: test test-unit test-integ lint fmt-check check fmt dev

test: test-unit test-integ

test-unit:
	bats test/unit/

test-integ:
	bats test/integration/

lint:
	shellcheck -x -S warning bin/sipag lib/*.sh extras/safety-gate.sh

fmt-check:
	shfmt -d bin/sipag lib/*.sh extras/safety-gate.sh

fmt:
	shfmt -w bin/sipag lib/*.sh extras/safety-gate.sh

check: lint fmt-check

dev: lint fmt-check test
