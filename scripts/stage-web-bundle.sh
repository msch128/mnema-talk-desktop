#!/usr/bin/env bash
# Stages the signed web bundle that the host embeds: the one attached to the
# newest published mnema-talk release (no draft, no pre-release) that has one
# (doc/desktop-trusted-client.md). The host verifies it with the key pinned in
# its source, so a bundle from anywhere else is useless. Needs GH_TOKEN.
# RELEASE_TAG is set for a release build, which must not go out without one.
# WEB_BUNDLE_TAG pins the mnema-talk release to take it from, to rebuild an
# older tag with the bundle it shipped with. BUNDLE_DIR (default
# experimental/app/bundle, where the Tauri host embeds it) says where to put
# builtin.zip; the Linux client stages its own copy in linux/bundle.
# A release is published before the server's release workflow has attached its
# bundle, so the newest one can have no assets yet: that one is waited for
# (WEB_BUNDLE_WAIT seconds, default 900, in WEB_BUNDLE_STEP steps, default 30),
# then the next older release with a bundle is taken. The host verifies the
# bundle either way. The chosen release and the SHA-256 of the bundle go to the step summary.
set -euo pipefail
dir="${BUNDLE_DIR:-experimental/app/bundle}"
repo=msch128/mnema-talk
mkdir -p "$dir"
found=""
wait_max="${WEB_BUNDLE_WAIT:-900}"
step="${WEB_BUNDLE_STEP:-30}"
if [ -n "${WEB_BUNDLE_TAG:-}" ]; then
  candidates="$WEB_BUNDLE_TAG"
else
  candidates="$(gh release list --repo "$repo" --limit 15 --exclude-drafts --exclude-pre-releases --json tagName --jq '.[].tagName')"
fi
first=1
for tag in $candidates; do
  waited=0
  while :; do
    rm -f "$dir"/mnema-web-*.zip
    # A release without a bundle is skipped; any other failure (auth, network) is an error.
    if err="$(gh release download "$tag" --repo "$repo" --pattern 'mnema-web-*.zip' --dir "$dir" --clobber 2>&1)"; then
      found="$tag"
      break 2
    fi
    case "$err" in
      *"no assets match"*) break ;;
      *"no assets to download"*)
        # Only the newest release can still be mid-publish.
        if [ "$first" = 1 ] && [ -z "${WEB_BUNDLE_TAG:-}" ] && [ "$waited" -lt "$wait_max" ]; then
          echo "$repo $tag has no assets yet; waiting for its web bundle ($waited/${wait_max}s)"
          sleep "$step"
          waited=$((waited + step))
          continue
        fi
        echo "::warning::$repo $tag has no assets; trying the next older release"
        break ;;
      *) echo "::error::could not download the web bundle of $repo $tag: $err"; exit 1 ;;
    esac
  done
  first=0
done
if [ -z "$found" ]; then
  if [ -n "${RELEASE_TAG:-}" ]; then echo "::error::no signed web bundle in the recent mnema-talk releases"; exit 1; fi
  echo "::warning::no mnema-talk release carries a signed web bundle yet; this build has none built in"
  exit 0
fi
mv "$dir"/mnema-web-*.zip "$dir/builtin.zip"
if command -v sha256sum >/dev/null; then sum="$(sha256sum < "$dir/builtin.zip" | cut -d' ' -f1)"; else sum="$(shasum -a 256 < "$dir/builtin.zip" | cut -d' ' -f1)"; fi
echo "Built-in web bundle from mnema-talk $found (sha256 $sum)" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
