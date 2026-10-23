#!/usr/bin/env bash
# Real optimized GLib regression in an owned throwaway Linux container.
set -euo pipefail
desktop_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
node "$desktop_root/scripts/checked-glib.mjs"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/mnema-glib-linux.XXXXXXXX")"
container_name="mnema-glib-regression-$(basename "$build_dir")"
cleanup() {
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  rm -rf "$build_dir"
}
trap cleanup EXIT
docker run --rm --name "$container_name" \
  --label mnema.fixture=glib-regression \
  --mount "type=bind,source=$desktop_root,target=/desktop,readonly" \
  --mount "type=volume,target=/build" \
  --env CARGO_TARGET_DIR=/build/target \
  rust:1.99.1-bookworm@sha256:48b7f435b8fc6378ce43b44f15e3a47fd8919b2825f835c39bd4bd3da50af5b8 \
  bash /desktop/scripts/test-glib-in-container.sh
