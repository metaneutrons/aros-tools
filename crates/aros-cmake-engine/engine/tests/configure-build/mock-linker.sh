#!/bin/sh

# The WirelessManager fixture must prove that its exact MUI archive is built
# before the private link step.  This linker stand-in rejects a missing archive
# and creates the requested relocatable product without host ABI assumptions.
output=""
archive=""
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
        shift
        output=$1
    else
        case "$1" in
            *.a) archive=$1 ;;
        esac
    fi
    shift
done

if [ -z "$output" ]; then
    echo "mock linker requires an output" >&2
    exit 87
fi
if [ "$(basename "$output")" = "wpa_supplicant" ] &&
   { [ -z "$archive" ] || [ ! -f "$archive" ]; }; then
    echo "mock linker requires an existing MUI archive for WirelessManager" >&2
    exit 87
fi
mkdir -p "$(dirname "$output")" || exit 1
: > "$output"
