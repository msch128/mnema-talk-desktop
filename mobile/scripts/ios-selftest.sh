#!/usr/bin/env bash
# Runs the self-test build of the iOS app in a booted simulator against the
# stand-in server (scripts/fake-instance.mjs) on the runner; the simulator
# shares the runner's loopback. The app asks for no camera or microphone.
#
#   ios-selftest.sh <app-bundle> <out-dir>
set -uo pipefail
app="$1"
out="$2"
bundle_id=com.mnema.talk.preview
mkdir -p "$out"

node scripts/fake-instance.mjs stand-in 8765 "$out/server.jsonl" dev &
server=$!
trap 'kill "$server" 2>/dev/null' EXIT

device="$(xcrun simctl list devices available -j | node -e '
  const all = JSON.parse(require("fs").readFileSync(0, "utf8")).devices
  const ios = Object.keys(all).filter((k) => k.includes("iOS")).sort().reverse()
  for (const runtime of ios) {
    const phone = all[runtime].find((d) => d.name.startsWith("iPhone"))
    if (phone) { console.log(phone.udid); break }
  }')"
[ -n "$device" ] || { echo "::error::no iPhone simulator available"; exit 1; }
echo "Simulator $device"
xcrun simctl boot "$device" || true
xcrun simctl bootstatus "$device" -b
xcrun simctl install "$device" "$app"
xcrun simctl launch --console-pty "$device" "$bundle_id" > "$out/console.txt" 2>&1 &

read_log="find \"\$(xcrun simctl get_app_container $device $bundle_id data)\" -name selftest.jsonl -exec cat {} \\;"
shoot="xcrun simctl io $device screenshot"
bash mobile/scripts/selftest-wait.sh "$out" "$read_log" "$shoot"
status=$?
echo "--- requests the stand-in server saw"
cat "$out/server.jsonl"
exit "$status"
