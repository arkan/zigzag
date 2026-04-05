#!/usr/bin/env bash
# Patches @ai-hero/sandcastle for macOS Docker Desktop compatibility:
# 1. Ignore chown errors (VirtioFS doesn't allow chown on bind mounts)
# 2. Mount persistent auth volume (sandcastle-claude-auth) into every container
set -euo pipefail

DIST="node_modules/@ai-hero/sandcastle/dist"

# Patch 1: Ignore chown errors
sed -i.bak 's/export const chownInContainer = (containerName, owner, path) => Effect.asVoid(dockerExec(\[/export const chownInContainer = (containerName, owner, path) => Effect.asVoid(dockerExec([/' "$DIST/DockerLifecycle.js"
# Replace the chown function to catch errors
cat > /tmp/sandcastle-patch-chown.py << 'PYTHON'
import re, sys
path = sys.argv[1]
with open(path, 'r') as f:
    content = f.read()

old = '''export const chownInContainer = (containerName, owner, path) => Effect.asVoid(dockerExec([
    "exec",
    "-u",
    "root",
    containerName,
    "chown",
    "-R",
    owner,
    path,
]));'''

new = '''export const chownInContainer = (containerName, owner, path) => Effect.asVoid(dockerExec([
    "exec",
    "-u",
    "root",
    containerName,
    "chown",
    "-R",
    "--silent",
    owner,
    path,
])).pipe(Effect.catchAll(() => Effect.void));'''

content = content.replace(old, new)
with open(path, 'w') as f:
    f.write(content)
PYTHON
python3 /tmp/sandcastle-patch-chown.py "$DIST/DockerLifecycle.js"

# Patch 2: Mount auth volume in SandboxFactory.js
python3 -c "
import sys
path = '$DIST/SandboxFactory.js'
with open(path, 'r') as f:
    content = f.read()
content = content.replace(
    'volumeMounts,\n        workdir: SANDBOX_WORKSPACE_DIR,',
    'volumeMounts: [...volumeMounts, \"sandcastle-claude-auth:/home/agent/.claude\"],\n        workdir: SANDBOX_WORKSPACE_DIR,',
    1
)
with open(path, 'w') as f:
    f.write(content)
"

# Patch 3: Mount auth volume in createSandbox.js
python3 -c "
import sys
path = '$DIST/createSandbox.js'
with open(path, 'r') as f:
    content = f.read()
content = content.replace(
    'volumeMounts,\n            workdir: SANDBOX_WORKSPACE_DIR,',
    'volumeMounts: [...volumeMounts, \"sandcastle-claude-auth:/home/agent/.claude\"],\n            workdir: SANDBOX_WORKSPACE_DIR,',
    1
)
with open(path, 'w') as f:
    f.write(content)
"

echo "Sandcastle patched successfully."
