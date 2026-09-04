#!/bin/sh
set -eu

out=
target=x86_64
previous=
for arg in "$@"; do
    if [ "$previous" = output ]; then
        out=$arg
        previous=
        continue
    fi
    case "$arg" in
        -o) previous=output ;;
        --target=arm-*|--target=arm-none-eabi) target=arm ;;
        --target=aarch64-*) target=aarch64 ;;
        --target=x86_64-*) target=x86_64 ;;
    esac
done
extra=
if [ -z "$out" ]; then
    for arg in "$@"; do
        case "$arg" in
            *.c|*.s) out=a.out; extra=conftest ;;
        esac
    done
fi
if [ -n "$out" ]; then
    /bin/mkdir -p "$(/usr/bin/dirname "$out")"
    case "$target" in
        arm) /usr/bin/printf '\177ELF\001\001\001\000\000\000\000\000\000\000\000\000\001\000\050\000' > "$out" ;;
        aarch64) /usr/bin/printf '\177ELF\002\001\001\000\000\000\000\000\000\000\000\000\001\000\267\000' > "$out" ;;
        *) /usr/bin/printf '\177ELF\002\001\001\000\000\000\000\000\000\000\000\000\001\000\076\000' > "$out" ;;
    esac
    if [ -n "$extra" ]; then /bin/cp "$out" "$extra"; fi
fi
