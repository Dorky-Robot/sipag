.PHONY: test test-unit test-integ lint fmt-check dev

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

dev: lint fmt-check test
