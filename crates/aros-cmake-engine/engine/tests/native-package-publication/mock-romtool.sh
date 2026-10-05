#!/bin/sh
set -eu

if [ "$#" -lt 2 ] || [ "$1" != "pkg" ]; then
    exit 2
fi
shift

case "$1" in
    create)
        shift
        if [ "$#" -lt 4 ] || [ "$1" != "--basename" ] || [ "$2" != "-o" ]; then
            exit 2
        fi
        output=$3
        shift 3
        if [ "$#" -lt 1 ]; then
            exit 2
        fi
        printf '12345' > "$output"
        ;;
    list)
        if [ "$#" -ne 2 ] || [ ! -f "$2" ] || [ "$(wc -c < "$2")" -ne 5 ]; then
            exit 3
        fi
        ;;
    *)
        exit 2
        ;;
esac
