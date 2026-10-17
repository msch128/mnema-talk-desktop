#!/usr/bin/env bash
# Stages the signed web bundle that the host embeds: the one attached to the
# newest mnema-talk release that has one (doc/desktop-trusted-client.md). The
# host verifies it with the key pinned in its source, so a bundle from anywhere
# else is useless. Needs GH_TOKEN. RELEASE_TAG is set for a release build,
# which must not go out without one.
set -euo pipefail
mkdir -p experimental/app/bundle
found=""
for tag in $(gh release list --repo msch128/mnema-talk --limit 15 --json tagName --jq '.[].tagName'); do
  if gh release download "$tag" --repo msch128/mnema-talk --pattern 'mnema-web-*.zip' --dir experimental/app/bundle --clobber 2>/dev/null; then
    found="$tag"
    break
  fi
done
if [ -z "$found" ]; then
  if [ -n "${RELEASE_TAG:-}" ]; then echo "::error::no signed web bundle in the recent mnema-talk releases"; exit 1; fi
  echo "::warning::no mnema-talk release carries a signed web bundle yet; this build has none built in"
  exit 0
fi
mv experimental/app/bundle/mnema-web-*.zip experimental/app/bundle/builtin.zip
echo "Built-in web bundle from mnema-talk $found" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
