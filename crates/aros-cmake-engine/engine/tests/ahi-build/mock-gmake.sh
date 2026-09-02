#!/bin/sh
set -eu

install=
for arg in "$@"; do
    if [ "$arg" = install ]; then install=1; fi
done
if [ -z "$install" ]; then
    if [ -x /opt/homebrew/bin/gmake ]; then
        exec /opt/homebrew/bin/gmake "$@"
    fi
    exec /usr/bin/make "$@"
fi

test -n "$AHI_MODE"
test -n "$AHI_INSTALL_PREFIX"
test -n "$AHI_PRODUCT_MANIFEST"
# The runner must not let host compiler/package lookup or shell-startup state
# cross the closed configure/Make boundary.
test -z "${CDPATH-}"
test -z "${ENV-}"
test -z "${BASH_ENV-}"
test -z "${CPATH-}"
test -z "${C_INCLUDE_PATH-}"
test -z "${CPLUS_INCLUDE_PATH-}"
test -z "${LIBRARY_PATH-}"
test -z "${SDKROOT-}"
test -z "${PKG_CONFIG_PATH-}"
test -z "${PKG_CONFIG_LIBDIR-}"
test -z "${PKG_CONFIG_SYSROOT_DIR-}"
test -z "${CPP-}"
test -z "${AHI_BUILDHANDLER-}"
test -z "${CPU-}"
test -z "${ASCPPFLAGS-}"
test -z "${ARFLAGS-}"
test -z "${CFLAG_RESIDENT-}"
test -z "${LDFLAG_RESIDENT-}"
test -z "${STRIPFLAGS-}"
test -z "${INSTALL_PROGRAM-}"
test -z "${INSTALL_DATA-}"
test -z "${INSTALL_SCRIPT-}"
test -z "${DISTDIR-}"
while IFS= read -r line; do
    set -- $line
    kind=$1
    path=$2
    output=$AHI_INSTALL_PREFIX/$path
    /bin/mkdir -p "$(/usr/bin/dirname "$output")"
    if [ "$kind" = elf ]; then
        case "$AHI_MODE" in
            arm) /usr/bin/printf '\177ELF\001\001\001\000\000\000\000\000\000\000\000\000\001\000\050\000' > "$output" ;;
            aarch64) /usr/bin/printf '\177ELF\002\001\001\000\000\000\000\000\000\000\000\000\001\000\267\000' > "$output" ;;
            *) /usr/bin/printf '\177ELF\002\001\001\000\000\000\000\000\000\000\000\000\001\000\076\000' > "$output" ;;
        esac
    else
        /usr/bin/printf 'closed fixture %s\n' "$path" > "$output"
    fi
done < "$AHI_PRODUCT_MANIFEST"
