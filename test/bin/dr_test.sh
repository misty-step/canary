#!/usr/bin/env bash
set -euo pipefail

# Compatibility entrypoint for the trusted pull-request CI control plane.
exec "$(cd "$(dirname "$0")" && pwd)/recovery_test.sh"
