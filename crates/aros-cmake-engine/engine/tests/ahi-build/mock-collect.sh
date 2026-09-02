#!/bin/sh
set -eu

printf '%s\n' "$*" >> "${0%/*}/../collector.log"

if [ "${1-}" != --ld ] || [ -z "${2-}" ]; then
    echo "mock collector requires --ld BACKEND" >&2
    exit 2
fi
backend=$2
shift 2
if [ "${1-}" != -- ]; then
    echo "mock collector requires -- before linker arguments" >&2
    exit 2
fi
shift
exec "$backend" "$@"
