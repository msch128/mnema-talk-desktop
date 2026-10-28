#!/usr/bin/env bash
# Runs the advisory policy of deny.toml (Windows and macOS target graph, see
# doc/security-review-1.2.md) against every Rust lockfile of the repository.
set -euo pipefail
cd "$(dirname "$0")/.."
status=0
while IFS= read -r lock; do
  dir="$(dirname "$lock")"
  echo "== cargo deny: $dir"
  cargo deny --manifest-path "$dir/Cargo.toml" check advisories || status=1
done < <(find . -name Cargo.lock -not -path '*/target/*' -not -path '*/node_modules/*' | sort)
exit "$status"
