#!/usr/bin/env bash
# Starts the given command and checks that it refuses to run: it prints the
# message (a regular expression, first argument) and serves nothing. The
# command may keep waiting for a click on its dialog; it is stopped here.
# Run it under a display, for example `xvfb-run -a bash scripts/refuse.sh 'sandbox is turned off' ./app --no-sandbox`.
set -u
. "$(dirname "$0")/launch-lib.sh"
[ "$#" -ge 2 ] || { echo 'usage: refuse.sh <message regex> <command> [args...]' >&2; exit 2; }
message="$1"
shift
start_app "$@"
seen=0
for _ in $(seq 1 60); do
  sleep 0.5
  if grep -Eq "$message" "$log"; then seen=1; break; fi
  kill -0 "$pid" 2>/dev/null || break
done
sleep 2
served=0
grep -qF 'mnema://localhost/' "$log" && served=1
stop_app
if [ "$seen" = 1 ] && [ "$served" = 0 ]; then
  echo "ok: $1 refused to run"
  exit 0
fi
cat "$log"
echo "::error::$1 did not refuse as expected (message seen: $seen, served pages: $served)"
exit 1
