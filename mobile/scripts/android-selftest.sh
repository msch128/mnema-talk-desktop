#!/usr/bin/env bash
# Runs the self-test build of the Android app in a booted emulator against the
# stand-in server (scripts/fake-instance.mjs) on the runner, which the
# emulator reaches as 127.0.0.1 through `adb reverse`. No camera, microphone
# or audio is involved: the emulator is started without them.
#
#   android-selftest.sh <apk> <out-dir>
set -uo pipefail
apk="$1"
out="$2"
package=com.mnema.talk.preview
mkdir -p "$out"

node scripts/fake-instance.mjs stand-in 8765 "$out/server.jsonl" dev &
server=$!
trap 'kill "$server" 2>/dev/null' EXIT

adb wait-for-device
adb reverse tcp:8765 tcp:8765
adb install -r "$apk"
adb logcat -c
adb logcat > "$out/logcat.txt" 2>&1 &
logcat=$!
adb shell am start -W -n "$package/.MainActivity"

read_log="adb shell run-as $package sh -c 'cat \$(find . -name selftest.jsonl 2>/dev/null | head -n 1)'"
shoot="adb exec-out screencap -p >"
bash mobile/scripts/selftest-wait.sh "$out" "$read_log" "$shoot"
status=$?
kill "$logcat" 2>/dev/null
echo "--- requests the stand-in server saw"
cat "$out/server.jsonl"
exit "$status"
