# Explicit supply-chain lock for the one archive downloaded directly by the
# AROS-NX CMake build instead of an upstream MetaMake %fetch declaration.
# Builder and runner both include this file so the mandatory integrity value
# has exactly one maintenance location.
set(_AROS_GRUB2_VERSION "2.12")
set(_AROS_GRUB2_SOURCE_URL "https://ftpmirror.gnu.org/grub/grub-2.12.tar.xz")
set(_AROS_GRUB2_ARCHIVE_SHA256
    "f3c97391f7c4eaa677a78e090c7e97e6dc47b16f655f04683ebd37bef7fe0faa")
