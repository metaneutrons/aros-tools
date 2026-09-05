#!/usr/bin/env bash
set -euo pipefail
script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
exec python3 "$script_root/test-governance-policy.py"
