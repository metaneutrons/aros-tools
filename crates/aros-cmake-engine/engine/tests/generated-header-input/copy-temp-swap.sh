#!/bin/sh
set -eu
: "${COPY_TEST_REAL_CMAKE:?}"
: "${COPY_TEST_SWAP:?}"
"$COPY_TEST_REAL_CMAKE" "$@"
test "$1" = -E
test "$2" = copy
case "$COPY_TEST_SWAP" in
    staging)
        rm -f "$4"
        ln -s "$3" "$4"
        ;;
    input)
        rm -f "$3"
        ln -s "$4" "$3"
        ;;
    *) exit 2 ;;
esac
