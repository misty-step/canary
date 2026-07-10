#!/usr/bin/env bash

dr_default_host() {
  printf '%s\n' "${CANARY_SSH_HOST:-}"
}

dr_require_host() {
  local host="$1"

  if [ -z "$host" ]; then
    printf 'Missing Canary SSH host: pass --host or set CANARY_SSH_HOST\n' >&2
    exit 1
  fi

  case "$host" in
    -*|*[!A-Za-z0-9_.:@-]*)
      printf 'Invalid Canary SSH host: use an SSH alias, host, IP, or user@host without options\n' >&2
      exit 1
      ;;
  esac

  printf '%s\n' "$host"
}

dr_default_container() {
  printf '%s\n' "${CANARY_CONTAINER:-canary}"
}

dr_require_container() {
  local container="$1"

  case "$container" in
    ''|*[!A-Za-z0-9_.-]*|[.-]*)
      printf 'Invalid Canary container name: use letters, numbers, dot, underscore, or hyphen\n' >&2
      exit 1
      ;;
  esac

  printf '%s\n' "$container"
}

dr_default_db_path() {
  printf '%s\n' "/data/canary.db"
}

dr_require_db_path() {
  local db_path="$1"

  case "$db_path" in
    /*) ;;
    *)
      printf 'Invalid Canary database path: expected an absolute path\n' >&2
      exit 1
      ;;
  esac
  case "$db_path" in
    *[!A-Za-z0-9_./-]*)
      printf 'Invalid Canary database path: use letters, numbers, slash, dot, underscore, or hyphen\n' >&2
      exit 1
      ;;
  esac

  printf '%s\n' "$db_path"
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
  printf '%s\n' 'litestream status -config /etc/litestream.yml'
}

dr_restore_remote_script() {
  printf '%s' 'restore_path=$(mktemp /tmp/canary-restore.XXXXXX); rm -f "$restore_path"; cleanup() { rm -f "$restore_path"; }; trap cleanup EXIT; litestream restore -if-replica-exists -o "$restore_path" -config /etc/litestream.yml "$db_path"; if [ ! -s "$restore_path" ]; then echo "Litestream restore did not materialize a non-empty file at $restore_path" >&2; exit 1; fi; ls -lh "$restore_path"'
}

dr_restore_remote_command() {
  local db_path="$1"

  printf 'db_path=%s\n%s\n' "$db_path" "$(dr_restore_remote_script)"
}

dr_container_ssh() {
  local host="$1"
  local container="$2"
  local remote_command="$3"

  printf '%s\n' "$remote_command" \
    | ssh "$host" sudo docker exec -i "$container" sh -eu
}
