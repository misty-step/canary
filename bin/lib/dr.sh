#!/usr/bin/env bash

dr_default_app() {
  printf '%s\n' "${FLY_APP:-canary-obs}"
}

dr_default_db_path() {
  printf '%s\n' "/data/canary.db"
}

dr_require_command() {
  local command_name="$1"

  if command -v "$command_name" >/dev/null 2>&1; then
    return 0
  fi

  printf 'Missing required command: %s\n' "$command_name" >&2
  exit 1
}

dr_status_remote_command() {
  printf '%s' "sh -eu -c 'litestream status -config /etc/litestream.yml'"
}

dr_restore_remote_script() {
  printf '%s' 'restore_path=$(mktemp /tmp/canary-restore.XXXXXX); rm -f "$restore_path"; cleanup() { rm -f "$restore_path"; }; trap cleanup EXIT; litestream restore -if-replica-exists -o "$restore_path" -config /etc/litestream.yml "$1"; if [ ! -s "$restore_path" ]; then echo "Litestream restore did not materialize a non-empty file at $restore_path" >&2; exit 1; fi; ls -lh "$restore_path"'
}

dr_restore_remote_command() {
  local db_path="$1"

  printf "sh -eu -c '%s' sh %s" \
    "$(dr_restore_remote_script)" \
    "$(printf '%q' "$db_path")"
}

dr_fly_ssh() {
  local app="$1"
  local remote_command="$2"

  exec flyctl ssh console --app "$app" -C "$remote_command"
}
