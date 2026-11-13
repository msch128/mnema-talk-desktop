#!/usr/bin/env bash
# Starts the given command (the AppImage, or an installed executable) and waits
# until it has served the instance selector and opened its window. Run it under
# a display, for example `xvfb-run -a bash scripts/smoke.sh ./Mnema.AppImage`;
# it turns the app's debug log on itself (the app then logs what it serves) and
# needs xwininfo (x11-utils). With SMOKE_FD_MIN set, the started program must
# also hold an open-file soft limit of at least that many (Chromium raises its
# own; started under `ulimit -Sn 256` this shows that it does).
set -u
. "$(dirname "$0")/launch-lib.sh"
[ "$#" -ge 1 ] || { echo 'usage: smoke.sh <command> [args...]' >&2; exit 2; }
start_app "$@"
ready=0
fds=''
for _ in $(seq 1 120); do
  sleep 0.5
  kill -0 "$pid" 2>/dev/null || break
  if grep -qF 'selector mnema://localhost/ -> 200' "$log" && xwininfo -root -tree 2>/dev/null | grep -qF 'Mnema Desktop DEV'; then
    ready=1
    fds="$(awk '/^Max open files/ {print $4}' "/proc/$pid/limits" 2>/dev/null)"
    break
  fi
done
stop_app
if [ "$ready" = 1 ] && [ -n "${SMOKE_FD_MIN:-}" ] && ! [ "${fds:-0}" -ge "$SMOKE_FD_MIN" ] 2>/dev/null; then
  echo "::error::$1 holds an open-file soft limit of ${fds:-unknown}, below $SMOKE_FD_MIN"
  exit 1
fi
if [ "$ready" = 1 ]; then
  echo "ok: $1 served the instance selector and opened its window"
  exit 0
fi
cat "$log"
echo "::error::$1 did not show the instance selector"
exit 1
