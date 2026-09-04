#!/bin/sh

# Minimal deterministic compiler/archiver fixture.  The host ADFlib lane is
# compiled for real; target lanes use this so their closed CMake contracts can
# be exercised on any CI host without requiring an AROS cross toolchain.
case "$1" in
    q*)
        shift
        output=$1
        ;;
    *)
        output=""
        while [ "$#" -gt 0 ]; do
            if [ "$1" = "-o" ]; then
                shift
                output=$1
                break
            fi
            shift
        done
        ;;
esac

if [ -n "$output" ]; then
    mkdir -p "$(dirname "$output")" || exit 1
    : > "$output" || exit 1
fi
