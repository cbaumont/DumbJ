#!/usr/bin/env bash
# Build and test the project.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "$0")" && pwd)"
JOBS=${JOBS:-4}

log() {
  printf '%s\n' "$*" >&2
}

if [[ ! -x "$ROOT/gradlew" ]]; then
  log "gradlew is missing"
  exit 1
fi

for module in core app; do
  log "building ${module} with $JOBS jobs"
  "$ROOT/gradlew" --no-daemon ":${module}:build" | tee "build-$module.log"
done

cat <<EOF
done at $(date +%H:%M)
EOF
