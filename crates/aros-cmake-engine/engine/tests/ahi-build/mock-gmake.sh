#!/bin/sh
set -eu

# The runner drives two invocations. `all` compiles the graph while configure's
# logical prefix is authoritative; `install` copies it into a private staging
# root through redirected destination variables. This fixture mocks the compiler,
# so there is nothing to compile: `all` only has to prove the closed environment
# and succeed. Delegating it to the real make would run the real AHI build under
# a mock compiler, which does not converge.
install=
prefix=
probe=
for arg in "$@"; do
    case "$arg" in
        install) install=1 ;;
        PREFIX=*) prefix=${arg#PREFIX=} ;;
        --version) probe=1 ;;
        gcc-include) probe=1 ;;
    esac
done

# Two invocations go to the real make. `--version` is a question about the tool
# rather than a build. `gcc-include` generates headers from the checked-in SFD
# descriptions with sfdc and needs no compiler, and the runner inspects the
# three headers it produces, so a stub would have to reproduce their content.
#
# `all` does not: with a mock compiler the real AHI build does not converge, and
# there is nothing to compile in a fixture whose products come from a manifest.
if [ -n "$probe" ]; then
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
if [ -z "$install" ]; then
    exit 0
fi

# Where the products go. The private install names it with a PREFIX assignment;
# without one this is the logical live prefix, which is what the pre-staging
# runner used.
destination=${prefix:-$AHI_INSTALL_PREFIX}

while IFS= read -r line; do
    set -- $line
    kind=$1
    path=$2
    output=$destination/$path
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
