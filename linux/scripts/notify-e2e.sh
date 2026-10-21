#!/usr/bin/env bash
# Runs test/e2e/notify.e2e.ts on a private session bus with a real notification
# daemon (dunst) on a virtual display: the app's toast must reach
# org.freedesktop.Notifications.Notify and using it must come back as a click.
# Needs dbus-run-session, xvfb-run, dunst, dbus-monitor and the app built
# (npm run build) or MNEMA_E2E_APP pointing at a packaged one.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${MNEMA_NOTIFY_E2E_INNER:-}" ]; then
  export MNEMA_NOTIFY_E2E_INNER=1
  exec dbus-run-session -- xvfb-run -a bash "$0" "$@"
fi

export MNEMA_E2E_NOTIFY=1
dunst >"${TMPDIR:-/tmp}/dunst.log" 2>&1 &
dunst_pid=$!
trap 'kill "$dunst_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 50); do
  if dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner string:org.freedesktop.Notifications | grep -q 'boolean true'; then
    ready=1
    break
  fi
  sleep 0.2
done
if [ -z "${ready:-}" ]; then
  echo "the notification daemon did not come up:"
  cat "${TMPDIR:-/tmp}/dunst.log"
  exit 1
fi
node --test --test-timeout=120000 test/e2e/notify.e2e.ts test/e2e/notify-page.e2e.ts
