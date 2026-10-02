#!/bin/sh
# Immutable executable for private-install tests. The body belongs to the
# individual fixture selected by make's existing -C argument, not ambient env.
set -eu
if [ "$#" -lt 2 ] || [ "$1" != '-C' ]; then
    exit 64
fi
case "$2" in
    /*) ;;
    *) exit 64 ;;
esac
exec /bin/sh "$2/make-body.sh" "$@"
