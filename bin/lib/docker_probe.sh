docker_probe_timeout_ticks() {
  local timeout_seconds="${CANARY_DOCKER_PROBE_TIMEOUT_SECONDS:-}"
  local timeout_ticks="${CANARY_DOCKER_PROBE_TIMEOUT_TICKS:-}"

  if [[ -n "$timeout_seconds" ]]; then
    case "$timeout_seconds" in
      ''|*[!0-9]*)
        ;;
      *)
        if (( timeout_seconds > 0 )); then
          printf '%s\n' $((timeout_seconds * 10))
          return 0
        fi
        ;;
    esac
  fi

  if [[ -n "$timeout_ticks" ]]; then
    case "$timeout_ticks" in
      ''|*[!0-9]*)
        ;;
      *)
        if (( timeout_ticks > 0 )); then
          printf '%s\n' "$timeout_ticks"
          return 0
        fi
        ;;
    esac
  fi

  printf '30\n'
}

docker_available() {
  local pid
  local ticks=0
  local status=0
  local timeout_ticks

  DOCKER_PROBE_STATUS="failed"
  timeout_ticks="$(docker_probe_timeout_ticks)"

  docker version >/dev/null 2>&1 &
  pid=$!

  while kill -0 "$pid" >/dev/null 2>&1; do
    if (( ticks >= timeout_ticks )); then
      kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" >/dev/null 2>&1 || true
      DOCKER_PROBE_STATUS="timed_out"
      return 1
    fi

    sleep 0.1
    ((ticks += 1))
  done

  set +e
  wait "$pid" >/dev/null 2>&1
  status=$?
  set -e

  if (( status == 0 )); then
    DOCKER_PROBE_STATUS="ready"
    return 0
  fi

  if (( status == 127 )); then
    DOCKER_PROBE_STATUS="unavailable"
    return 1
  fi

  if command -v docker >/dev/null 2>&1; then
    DOCKER_PROBE_STATUS="failed"
  else
    DOCKER_PROBE_STATUS="unavailable"
  fi

  return 1
}

colima_ssh_config_path() {
  printf '%s\n' "${HOME}/.colima/ssh_config"
}

colima_ready() {
  DOCKER_RUNTIME_ERROR_REASON=""

  if ! colima version >/dev/null 2>&1; then
    DOCKER_RUNTIME_ERROR_REASON="colima_missing"
    return 1
  fi

  if [[ ! -f "$(colima_ssh_config_path)" ]]; then
    DOCKER_RUNTIME_ERROR_REASON="colima_ssh_config_missing"
    return 1
  fi

  if ! colima status >/dev/null 2>&1; then
    DOCKER_RUNTIME_ERROR_REASON="colima_not_running"
    return 1
  fi

  return 0
}

docker_runtime_auto_backend() {
  DOCKER_RUNTIME_BACKEND=""
  DOCKER_RUNTIME_ERROR_REASON=""
  DOCKER_RUNTIME_FALLBACK_REASON=""

  if docker_available; then
    DOCKER_RUNTIME_BACKEND="direct"
    return 0
  fi

  case "${DOCKER_PROBE_STATUS:-failed}" in
    unavailable)
      DOCKER_RUNTIME_FALLBACK_REASON="docker_unavailable"
      ;;
    timed_out)
      DOCKER_RUNTIME_FALLBACK_REASON="docker_timed_out"
      ;;
    *)
      DOCKER_RUNTIME_FALLBACK_REASON="docker_failed"
      ;;
  esac

  if colima_ready; then
    DOCKER_RUNTIME_BACKEND="colima-ssh"
    return 0
  fi

  return 1
}

docker_runtime_auto_error_message() {
  case "${DOCKER_RUNTIME_ERROR_REASON:-}" in
    colima_missing)
      printf "Repo-local Dagger could not use the active Docker client and no Colima fallback is installed. Start your Docker runtime or install Colima and run 'colima start --runtime docker'.\n"
      ;;
    colima_ssh_config_missing)
      printf "Repo-local Dagger could not use the active Docker client and the Colima fallback is not ready. Missing %s. Start your Docker runtime or start Colima with 'colima start --runtime docker'.\n" "$(colima_ssh_config_path)"
      ;;
    colima_not_running)
      printf "Repo-local Dagger could not use the active Docker client and the Colima fallback is not running. Start your Docker runtime or start Colima with 'colima start --runtime docker'.\n"
      ;;
    *)
      printf "Repo-local Dagger could not detect a working Docker runtime.\n"
      ;;
  esac
}

docker_runtime_auto_note() {
  if [[ "${DOCKER_RUNTIME_BACKEND:-}" != "colima-ssh" ]]; then
    return 0
  fi

  case "${DOCKER_RUNTIME_FALLBACK_REASON:-}" in
    docker_unavailable)
      printf '==> tooling: Docker was unavailable; using Colima over SSH for repo-local Dagger.\n'
      ;;
    docker_timed_out)
      printf '==> tooling: direct Docker probe timed out; using Colima over SSH for repo-local Dagger.\n'
      ;;
    *)
      printf '==> tooling: direct Docker access failed; using Colima over SSH for repo-local Dagger.\n'
      ;;
  esac
}

docker_runtime_bootstrap_note() {
  if docker_runtime_auto_backend; then
    return 0
  fi

  case "${DOCKER_RUNTIME_ERROR_REASON:-}" in
    colima_missing)
      printf '==> tooling: macOS local validation needs a working Docker runtime. Start Docker Desktop or install/start Colima before running ./bin/validate.\n'
      ;;
    *)
      printf '==> tooling: no working Docker runtime detected. If you use Colima, start it before running local validation: colima start --runtime docker\n'
      ;;
  esac
}
