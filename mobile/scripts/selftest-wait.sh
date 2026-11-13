#!/usr/bin/env bash
# Waits for the self-test of the phone app (mobile/src/selftest.rs) and takes
# the screenshots. The platform scripts pass two commands: one that prints the
# app's selftest.jsonl (empty while there is none) and one that writes a PNG
# screenshot to the path it is given.
#
#   selftest-wait.sh <out-dir> <read-command> <screenshot-command>
#
# Exits 0 when the page of the stand-in server answered through the proxy,
# 1 when the app reported a failure or ran out of time. The result is in
# <out-dir>/selftest.jsonl, the screenshots next to it.
set -uo pipefail
out="$1"
read_log="$2"
shoot="$3"
mkdir -p "$out"

# True when the log has a line for that step (and, given a second argument,
# whose detail starts with it).
saw() {
  node -e '
    const [file, step, detail = ""] = process.argv.slice(1)
    let lines = []
    try { lines = require("fs").readFileSync(file, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) } catch {}
    process.exit(lines.some((l) => l.step === step && String(l.detail).startsWith(detail)) ? 0 : 1)
  ' "$out/selftest.jsonl" "$@"
}
poll() {
  for _ in $(seq 1 "$2"); do
    bash -c "$read_log" > "$out/selftest.jsonl" 2>/dev/null || true
    if saw "$1" || saw failed; then return 0; fi
    sleep 2
  done
  return 1
}

if poll picker 90; then
  sleep 2
  bash -c "$shoot $out/picker.png" || true
  # What the app had done when the picker was taken: no "opened" line yet.
  bash -c "$read_log" > "$out/picker.jsonl" 2>/dev/null || true
fi
poll proxied 120
sleep 5
bash -c "$shoot $out/server.png" || true
bash -c "$read_log" > "$out/selftest.jsonl" 2>/dev/null || true
echo "--- selftest.jsonl"
cat "$out/selftest.jsonl"
if saw proxied && saw tls ok && saw identity selector && saw identity instance; then
  echo "The app showed the picker, opened the server and reached it through the proxy; TLS verified."
  exit 0
fi
echo "::error::the self-test did not finish; see selftest.jsonl and the screenshots"
exit 1
