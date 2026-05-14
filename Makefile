IMAGE := sandcastle:zigzag
AUTH_VOLUME := sandcastle-claude-auth
CARGO ?= cargo
ZIGZAG_MANIFEST := zigzag/Cargo.toml
ZIGZAG_PACKAGE := zigzag-cli
ZIGZAG_BIN := zigzag
ZIGZAG_CLI_PATH := zigzag/crates/zigzag-cli
INSTALL_ROOT ?= $(HOME)/.local
INSTALL_BIN_DIR := $(INSTALL_ROOT)/bin
INSTALL_BIN := $(INSTALL_BIN_DIR)/$(ZIGZAG_BIN)

.DEFAULT_GOAL := help

.PHONY: help run build install clean docker login auth-status sandcastle

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Targets:"
	@echo "  run           Run zigzag via cargo (pass args with ARGS='...')"
	@echo "  build         Build zigzag binary"
	@echo "  install       Install zigzag binary to $(INSTALL_BIN)"
	@echo "  clean         Clean Rust build artifacts"
	@echo "  sandcastle    Build Docker image and run Sandcastle"
	@echo "  login         Authenticate Claude Code in container (Claude Max/Pro)"
	@echo "  auth-status   Check authentication status"
	@echo "  docker        Build Docker image only"

run:
	$(CARGO) run --manifest-path $(ZIGZAG_MANIFEST) --package $(ZIGZAG_PACKAGE) --bin $(ZIGZAG_BIN) -- $(ARGS)

build:
	$(CARGO) build --manifest-path $(ZIGZAG_MANIFEST) --package $(ZIGZAG_PACKAGE) --bin $(ZIGZAG_BIN)

install:
	$(CARGO) build --manifest-path $(ZIGZAG_MANIFEST) --package $(ZIGZAG_PACKAGE) --bin $(ZIGZAG_BIN) --release
	mkdir -p "$(INSTALL_BIN_DIR)"
	install -m 755 "zigzag/target/release/$(ZIGZAG_BIN)" "$(INSTALL_BIN)"

clean:
	$(CARGO) clean --manifest-path $(ZIGZAG_MANIFEST)

docker:
	docker build -t $(IMAGE) .sandcastle/

login: docker
	@docker volume inspect $(AUTH_VOLUME) &>/dev/null || docker volume create $(AUTH_VOLUME)
	docker run --rm -it \
		-v $(AUTH_VOLUME):/home/agent/.claude \
		--entrypoint bash $(IMAGE)

auth-status: docker
	@docker volume inspect $(AUTH_VOLUME) &>/dev/null || docker volume create $(AUTH_VOLUME)
	@docker run --rm \
		-v $(AUTH_VOLUME):/home/agent/.claude \
		--entrypoint bash $(IMAGE) \
		-c "claude auth status"

sandcastle: docker
	@docker volume inspect $(AUTH_VOLUME) &>/dev/null || docker volume create $(AUTH_VOLUME)
	npm run sandcastle
