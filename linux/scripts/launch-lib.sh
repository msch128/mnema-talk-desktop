# Shared by smoke.sh and refuse.sh (sourced): starts the command in a process
# group of its own with the app's debug log on, and stops the whole group.
# The log is removed when the script ends.
export MNEMA_DEBUG_PROTOCOL=1
log=''
pid=''

start_app() {
  log="$(mktemp)"
  trap 'rm -f "$log"' EXIT
  setsid "$@" >"$log" 2>&1 &
  pid=$!
}

stop_app() {
  kill -- "-$pid" 2>/dev/null
  sleep 1
  kill -9 -- "-$pid" 2>/dev/null
}
