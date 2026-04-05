IMAGE := sandcastle:z
AUTH_VOLUME := sandcastle-claude-auth

.DEFAULT_GOAL := help

.PHONY: help docker login auth-status sandcastle

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Targets:"
	@echo "  sandcastle    Build Docker image and run Sandcastle"
	@echo "  login         Authenticate Claude Code in container (Claude Max/Pro)"
	@echo "  auth-status   Check authentication status"
	@echo "  docker        Build Docker image only"

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
