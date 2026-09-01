#!/usr/bin/env bash

set -euo pipefail

toolchain_file=${1:-rust-toolchain.toml}
if [[ ! -f "$toolchain_file" || -L "$toolchain_file" ]]; then
    printf '::error::AP7050 Rust toolchain contract is missing or unsafe: %s\n' \
        "$toolchain_file" >&2
    exit 1
fi

python3 - "$toolchain_file" <<'PY'
import re
import sys
import tomllib
from pathlib import Path


def fail(message: str) -> None:
    print(f'::error::AP7050 {message}', file=sys.stderr)
    raise SystemExit(1)


try:
    document = tomllib.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
    contract = document['toolchain']
except (KeyError, OSError, tomllib.TOMLDecodeError) as error:
    fail(f'cannot read Rust toolchain contract: {error}')

channel = contract.get('channel')
if not isinstance(channel, str) or re.fullmatch(r'(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)', channel) is None:
    fail('toolchain.channel must be one exact stable X.Y.Z release')
if contract.get('profile') != 'minimal':
    fail('toolchain.profile must be minimal')
components = contract.get('components')
if not isinstance(components, list) or any(not isinstance(item, str) for item in components):
    fail('toolchain.components must be a string array')
if len(components) != len(set(components)) or not {'clippy', 'rustfmt'} <= set(components):
    fail('toolchain.components must uniquely include clippy and rustfmt')

print(channel)
PY
