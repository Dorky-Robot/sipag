PREFIX ?= /usr/local
SHAREDIR = $(PREFIX)/share/sipag

build-tui:
	cd tui && cargo build --release

install: build-tui
	@mkdir -p $(SHAREDIR)/bin
	@mkdir -p $(SHAREDIR)/lib/core
	@mkdir -p $(SHAREDIR)/lib/sources
	@cp bin/sipag $(SHAREDIR)/bin/sipag
	@chmod +x $(SHAREDIR)/bin/sipag
	@cp lib/core/*.sh $(SHAREDIR)/lib/core/
	@cp lib/sources/*.sh $(SHAREDIR)/lib/sources/
	@mkdir -p $(PREFIX)/bin
	@ln -sf $(SHAREDIR)/bin/sipag $(PREFIX)/bin/sipag
	@cp tui/target/release/sipag-tui $(PREFIX)/bin/sipag-tui
	@echo "sipag installed to $(PREFIX)/bin/sipag"

install-bash:
	@mkdir -p $(SHAREDIR)/bin
	@mkdir -p $(SHAREDIR)/lib/core
	@mkdir -p $(SHAREDIR)/lib/sources
	@cp bin/sipag $(SHAREDIR)/bin/sipag
	@chmod +x $(SHAREDIR)/bin/sipag
	@cp lib/core/*.sh $(SHAREDIR)/lib/core/
	@cp lib/sources/*.sh $(SHAREDIR)/lib/sources/
	@mkdir -p $(PREFIX)/bin
	@ln -sf $(SHAREDIR)/bin/sipag $(PREFIX)/bin/sipag
	@echo "sipag installed to $(PREFIX)/bin/sipag (no TUI)"

uninstall:
	@rm -f $(PREFIX)/bin/sipag
	@rm -f $(PREFIX)/bin/sipag-tui
	@rm -rf $(SHAREDIR)
	@echo "sipag uninstalled"

.PHONY: build-tui install install-bash uninstall
