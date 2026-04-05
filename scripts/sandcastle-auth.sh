#!/usr/bin/env bash
set -euo pipefail

IMAGE="sandcastle:z"
VOLUME="sandcastle-claude-auth"
CONTAINER="sandcastle-login"

# Ensure auth volume exists
docker volume inspect "$VOLUME" &>/dev/null || docker volume create "$VOLUME"

# Check if already authenticated
STATUS=$(docker run --rm \
  -v "$VOLUME:/home/agent/.claude" \
  --entrypoint bash "$IMAGE" \
  -c "claude auth status 2>&1" || true)

if echo "$STATUS" | grep -qi "logged in\|authenticated\|active"; then
  echo "Already authenticated."
  echo "$STATUS"
  exit 0
fi

echo "Not authenticated. Running claude setup-token..."
echo "Follow the instructions below to authenticate."
echo ""

# Run setup-token interactively
docker run --rm -it \
  -v "$VOLUME:/home/agent/.claude" \
  --entrypoint bash "$IMAGE" \
  -c "claude setup-token"

echo ""
echo "Authentication complete. Credentials stored in Docker volume '$VOLUME'."
