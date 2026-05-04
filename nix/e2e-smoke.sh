#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${NIXCLIP_SMOKE_IN_DBUS:-}" && -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]] \
  && command -v dbus-run-session >/dev/null 2>&1; then
  export NIXCLIP_SMOKE_IN_DBUS=1
  dbus_args=(dbus-run-session)
  if [[ -n "${DBUS_SESSION_BUS_CONFIG_FILE:-}" ]]; then
    dbus_args+=(--config-file="$DBUS_SESSION_BUS_CONFIG_FILE")
  fi
  exec "${dbus_args[@]}" -- bash "$0" "$@"
fi

workdir="$(mktemp -d "${TMPDIR:-/tmp}/nixclip-smoke.XXXXXX")"
logdir="$workdir/logs"
mkdir -p "$logdir" "$workdir/home" "$workdir/config" "$workdir/data" "$workdir/cache" "$workdir/runtime"
chmod 700 "$workdir/runtime"

export HOME="$workdir/home"
export XDG_CONFIG_HOME="$workdir/config"
export XDG_DATA_HOME="$workdir/data"
export XDG_CACHE_HOME="$workdir/cache"
export XDG_RUNTIME_DIR="$workdir/runtime"
export XDG_SESSION_TYPE=wayland
export GDK_BACKEND=wayland
export NO_AT_BRIDGE=1
export NIXCLIP_DISABLE_PORTAL_HOTKEYS=1
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=pixman
export RUST_LOG="${RUST_LOG:-nixclipd=debug,nixclip=debug,nixclip_core=debug,info}"

compositor_pid=""
daemon_pid=""
wayland_socket=""

dump_logs() {
  for log in "$logdir"/*.log "$logdir"/*.err "$logdir"/*.json "$logdir"/*.jsonl; do
    [[ -e "$log" ]] || continue
    echo "===== $log =====" >&2
    sed -n '1,220p' "$log" >&2 || true
  done
}

cleanup() {
  status=$?
  set +e
  if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" >/dev/null 2>&1; then
    kill "$daemon_pid" >/dev/null 2>&1
    for _ in {1..20}; do
      kill -0 "$daemon_pid" >/dev/null 2>&1 || break
      sleep 0.1
    done
    kill -KILL "$daemon_pid" >/dev/null 2>&1
    wait "$daemon_pid" >/dev/null 2>&1
  fi
  if [[ -n "$compositor_pid" ]] && kill -0 "$compositor_pid" >/dev/null 2>&1; then
    kill "$compositor_pid" >/dev/null 2>&1
    for _ in {1..20}; do
      kill -0 "$compositor_pid" >/dev/null 2>&1 || break
      sleep 0.1
    done
    kill -KILL "$compositor_pid" >/dev/null 2>&1
    wait "$compositor_pid" >/dev/null 2>&1
  fi
  if [[ "$status" -ne 0 ]]; then
    dump_logs
  fi
  rm -rf "$workdir"
  exit "$status"
}
trap cleanup EXIT

wait_for() {
  local label="$1"
  local timeout_secs="$2"
  shift 2

  local deadline=$((SECONDS + timeout_secs))
  until "$@"; do
    if (( SECONDS >= deadline )); then
      echo "Timed out waiting for $label" >&2
      return 1
    fi
    sleep 0.2
  done
}

cat >"$workdir/keybinding-command.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail

mode="\${1:?}"
shift

printf '%s\n' "\$mode" >>"$logdir/keybinding-events.log"

ui_status=0
timeout 5s nixclip-ui "\$@" >"$logdir/nixclip-ui-keybinding-\$mode.log" 2>&1 || ui_status=\$?
case "\$ui_status" in
  0|124)
    ;;
  *)
    printf '%s\n' "\$ui_status" >"$logdir/keybinding-\$mode.status"
    exit "\$ui_status"
    ;;
esac

if grep -Ei 'cannot open display|failed to initialize gtk|thread .* panicked|panic' \
  "$logdir/nixclip-ui-keybinding-\$mode.log" >/dev/null; then
  printf 'startup failure\n' >"$logdir/keybinding-\$mode.status"
  exit 1
fi

printf '%s\n' "\$ui_status" >"$logdir/keybinding-\$mode.status"
touch "$logdir/keybinding-\$mode.done"
EOF
chmod +x "$workdir/keybinding-command.sh"

cat >"$workdir/sway.conf" <<'EOF'
xwayland disable
set $mod Mod4
bindsym $mod+v exec __KEYBINDING_COMMAND__ formatted
bindsym $mod+Shift+v exec __KEYBINDING_COMMAND__ plain --plain
exec sleep 300
EOF
sed -i "s#__KEYBINDING_COMMAND__#$workdir/keybinding-command.sh#g" "$workdir/sway.conf"

sway -c "$workdir/sway.conf" -d >"$logdir/sway.log" 2>&1 &
compositor_pid=$!

wait_for "Wayland socket" 10 bash -c '
  for socket in "$1"/wayland-*; do
    if [[ -S "$socket" ]]; then
      printf "%s" "$socket" >"$2"
      exit 0
    fi
  done
  exit 1
' _ "$XDG_RUNTIME_DIR" "$logdir/wayland-socket"
wayland_socket="$(cat "$logdir/wayland-socket")"
export WAYLAND_DISPLAY="$(basename "$wayland_socket")"

wayland-info >"$logdir/wayland-info.log" 2>"$logdir/wayland-info.err"
if ! grep -Eq 'zwlr_data_control_manager_v1|ext_data_control_manager_v1' "$logdir/wayland-info.log"; then
  echo "Headless compositor does not expose a data-control protocol" >&2
  exit 1
fi

# Attach a virtual keyboard once so wl-clipboard sees an input-capable seat.
wtype a >"$logdir/wtype.log" 2>&1

printf '%s' "nixclip clipboard preflight" | wl-copy --type text/plain \
  >"$logdir/wl-copy-preflight.log" 2>"$logdir/wl-copy-preflight.err"
preflight="$(wl-paste --no-newline 2>"$logdir/wl-paste-preflight.err")"
if [[ "$preflight" != "nixclip clipboard preflight" ]]; then
  echo "Wayland clipboard preflight failed" >&2
  exit 1
fi

nixclipd --verbose >"$logdir/nixclipd.log" 2>&1 &
daemon_pid=$!

wait_for "nixclipd IPC socket" 10 test -S "$XDG_RUNTIME_DIR/nixclip.sock"
wait_for "wl-paste watcher startup" 10 grep -F "starting wl-paste clipboard watcher" "$logdir/nixclipd.log"
if grep -F "wl-paste --watch exited" "$logdir/nixclipd.log" >/dev/null; then
  echo "wl-paste watcher exited before clipboard capture" >&2
  exit 1
fi

needle="nixclip smoke text $$ $(date +%s)"
printf '%s' "$needle" | wl-copy --type text/plain \
  >"$logdir/wl-copy-needle.log" 2>"$logdir/wl-copy-needle.err"

search_json="$logdir/search.jsonl"
search_err="$logdir/search.err"
wait_for "clipboard capture" 20 bash -c '
  nixclip --json search "$1" --limit 5 >"$2" 2>"$3" &&
    grep -F "$1" "$2" >/dev/null
' _ "$needle" "$search_json" "$search_err"

entry_id="$(
  jq -r --arg needle "$needle" 'select(.preview == $needle) | .id' "$search_json" | head -n1
)"
if [[ -z "$entry_id" || "$entry_id" == "null" ]]; then
  echo "Captured entry was not present in nixclip search output" >&2
  exit 1
fi

nixclip --json show "$entry_id" >"$logdir/show.json" 2>"$logdir/show.err"
jq -e --arg needle "$needle" \
  '.preview == $needle and .content_class == "text" and (.id | type == "number")' \
  "$logdir/show.json" >/dev/null

printf '%s' "replacement clipboard text" | wl-copy --type text/plain
nixclip paste "$entry_id" --plain >"$logdir/paste.log" 2>&1
wait_for "clipboard restore" 10 bash -c '
  [[ "$(wl-paste --no-newline)" == "$1" ]]
' _ "$needle"

wtype -M logo -P v -p v -m logo >"$logdir/wtype-super-v.log" 2>&1
wait_for "Super+V keybinding" 10 test -e "$logdir/keybinding-formatted.done"

wtype -M logo -M shift -P v -p v -m shift -m logo \
  >"$logdir/wtype-super-shift-v.log" 2>&1
wait_for "Super+Shift+V keybinding" 10 test -e "$logdir/keybinding-plain.done"

grep -Fx formatted "$logdir/keybinding-events.log" >/dev/null
grep -Fx plain "$logdir/keybinding-events.log" >/dev/null

echo "nixclip smoke test passed"
