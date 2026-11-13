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

saw() { grep -q "\"step\":\"$1\"" "$out/selftest.jsonl" 2>/dev/null; }
poll() {
  for _ in $(seq 1 "$2"); do
    bash -c "$read_log" > "$out/selftest.jsonl" 2>/dev/null || true
    if saw "$1" || saw failed; then return 0; fi
    sleep 2
  done
  return 1
}

if poll picker 90; then
  sleep 3
  bash -c "$shoot $out/picker.png" || true
fi
poll proxied 120
sleep 5
bash -c "$shoot $out/server.png" || true
bash -c "$read_log" > "$out/selftest.jsonl" 2>/dev/null || true
echo "--- selftest.jsonl"
cat "$out/selftest.jsonl"
if saw proxied && grep -q '"step":"tls","detail":"ok' "$out/selftest.jsonl"; then
  echo "The app showed the picker, opened the server and reached it through the proxy; TLS verified."
  exit 0
fi
echo "::error::the self-test did not finish; see selftest.jsonl and the screenshots"
exit 1
