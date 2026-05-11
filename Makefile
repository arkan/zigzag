IMAGE := sandcastle:z
AUTH_VOLUME := sandcastle-claude-auth
CARGO ?= cargo
Z_MANIFEST := z/Cargo.toml
Z_PACKAGE := z-cli
Z_BIN := z
Z_CLI_PATH := z/crates/z-cli
INSTALL_ROOT ?= $(HOME)/.local
INSTALL_BIN_DIR := $(INSTALL_ROOT)/bin
INSTALL_BIN := $(INSTALL_BIN_DIR)/$(Z_BIN)

.DEFAULT_GOAL := help

.PHONY: help run build install clean docker login auth-status sandcastle

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Targets:"
	@echo "  run           Run z via cargo (pass args with ARGS='...')"
	@echo "  build         Build z binary"
	@echo "  install       Install z binary to $(INSTALL_BIN)"
	@echo "  clean         Clean Rust build artifacts"
	@echo "  sandcastle    Build Docker image and run Sandcastle"
	@echo "  login         Authenticate Claude Code in container (Claude Max/Pro)"
	@echo "  auth-status   Check authentication status"
	@echo "  docker        Build Docker image only"

run:
	$(CARGO) run --manifest-path $(Z_MANIFEST) --package $(Z_PACKAGE) --bin $(Z_BIN) -- $(ARGS)

build:
	$(CARGO) build --manifest-path $(Z_MANIFEST) --package $(Z_PACKAGE) --bin $(Z_BIN)

install:
	$(CARGO) build --manifest-path $(Z_MANIFEST) --package $(Z_PACKAGE) --bin $(Z_BIN) --release
	mkdir -p "$(INSTALL_BIN_DIR)"
	install -m 755 "z/target/release/$(Z_BIN)" "$(INSTALL_BIN)"

clean:
	$(CARGO) clean --manifest-path $(Z_MANIFEST)

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
