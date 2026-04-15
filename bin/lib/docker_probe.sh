docker_available() {
  local pid
  local ticks=0
  local timeout_ticks="${CANARY_DOCKER_PROBE_TIMEOUT_TICKS:-30}"

  docker version >/dev/null 2>&1 &
  pid=$!

  while kill -0 "$pid" >/dev/null 2>&1; do
    if (( ticks >= timeout_ticks )); then
      kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" >/dev/null 2>&1 || true
      return 1
    fi

    sleep 0.1
    ((ticks += 1))
  done

  wait "$pid" >/dev/null 2>&1
}
