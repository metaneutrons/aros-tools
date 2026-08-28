# Pi 4 U-Boot USB-ECM bootstrap

This directory describes an optional Raspberry Pi 4 bootstrap in which a
stable SD card starts U-Boot and U-Boot turns the USB-C port into a CDC-ECM
network device for a Mac host.  It is intentionally **not** a general AROS
bootloader replacement and is not a Raspberry Pi 5 profile.

## What will live on the SD card

```text
FAT boot partition
  config.txt
  start4.elf
  fixup4.dat
  bcm2711-rpi-4-b.dtb
  u-boot.bin
  boot.scr                  generated only after the AROS hand-off is implemented
```

`config.txt` is rendered from `../config.txt.in`; `u-boot.bin` must be built
from a pinned U-Boot revision plus a reviewed patch series.  The SD card is
not rewritten for normal AROS development builds.

The external input directory passed to `aros board sd image --board <name>
--boot-bundle <dir> --output <new-artifact-dir>` is versioned by
`boot-bundle.toml`. Start with `boot-bundle.toml.in`, replace every
board/USB identity value and every file checksum, and put the six regular
payload files beside it. The command validates by default; add `--apply` only
to create the atomic raw MBR/FAT32 artifact in the new output directory. The
format-1 Pi 4 USB-ECM contract requires these exact role-to-FAT-name mappings:

| Role | FAT boot-partition destination |
| --- | --- |
| `config` | `config.txt` |
| `firmware-start` | `start4.elf` |
| `firmware-fixup` | `fixup4.dat` |
| `device-tree` | `bcm2711-rpi-4-b.dtb` |
| `u-boot` | `u-boot.bin` |
| `boot-script` | `boot.scr` |

All sources are relative to the external bundle directory, must be regular
files rather than symlinks, and must have an exact lowercase SHA-256 in the
manifest. The manifest repeats the local board name, model, transport and
complete USB-ECM identity; a mismatch is a hard error. No tooling downloads
or synthesizes any of these inputs.

After `sd image --apply` has created an artifact, use this reviewable
two-phase write sequence:

```text
aros board sd unmount
aros board sd unmount --device <scan-id>
aros board sd unmount --device <scan-id> --apply
aros board sd scan --artifact <artifact-dir>
aros board sd write --board <name> --artifact <artifact-dir> --device <scan-id>
aros board sd write --board <name> --artifact <artifact-dir> --device <scan-id> --confirm <token>
```

The three optional `unmount` forms list, preview, and then explicitly unmount
one mounted removable whole disk. They accept only the opaque current scan ID,
never a raw path. They accept mountpoints only below the standard removable
roots and re-scan the complete remaining topology before normally unmounting
each exact source volume; there is no broad whole-disk, force or lazy command.
That authorization is not write authorization. The following `sd scan` prints
only safe, entirely unmounted whole physical removable disks and a token bound
to the selected artifact and current disk. The first `sd write` is a
non-writing preview and prints the required token again. Only the final command
may write on Linux or macOS. It independently revalidates the token, board,
artifact and the same whole, physical, removable, non-internal, writable,
entirely unmounted disk before opening it, then flushes and SHA-256 hashes the
readback. Linux uses an exclusive/no-follow raw open and a post-open identity
check. macOS first acquires a Disk Arbitration `DADiskClaim` RAII guard for the
exact whole BSD disk, revalidates the target under that claim, and has no
unclaimed raw-device fallback. The raw file descriptor closes before the claim
is released. A physical unplug remains a hard I/O/readback error. The CLI
never auto-selects, implicitly unmounts or repartitions a card and has no
`--force`/`--yes` bypass.

## Configuration basis

`rpi4-aros-usbdev_defconfig` is a **configuration delta** against upstream
`rpi_4_defconfig`, not a stand-alone U-Boot defconfig.  Upstream already
enables the DWC2 USB gadget foundation for the Pi 4, but USB Ethernet is not
part of the base Pi-4 profile.  The delta selects CDC-ECM because macOS
supports it without an RNDIS driver.

Before this becomes buildable, the bootstrap implementation must pin an
upstream U-Boot tag or commit in `../firmware.lock.toml`, copy/derive a full
defconfig reproducibly, and attach the reviewed patches listed in
`patches/README.md`.  Do not treat the delta as `make
rpi4-aros-usbdev_defconfig` yet.

## Remaining external bootstrap inputs

The host-side AROS service and the SD-image input contract do **not** replace
these deliberately external inputs:

1. a pinned U-Boot source commit and a complete reproducible Pi 4 defconfig;
2. a reviewed patch series that establishes DWC2 device mode, the selected
   USB VID:PID, USB serial, Pi-side CDC-ECM MAC, and the `arosboot` command;
3. the resulting `u-boot.bin` built from exactly those sources; and
4. a generated `boot.scr` whose AROS TFTP requests are bare deployment-root
   filenames and whose load addresses originate from a reviewed AROS bundle
   manifest.

Until all four are supplied to the external `boot-bundle.toml` with matching
hashes, the tool reports missing or mismatched inputs. It must never emit a
plausible, unbootable U-Boot SD card.

## Required proof points

1. U-Boot starts from the Pi 4 SD card and produces 115200-bit/s physical UART
   output after repeated cold boots.
2. The Mac sees a stable CDC-ECM network interface after each power cycle.
3. U-Boot transfers a multi-megabyte blob from the Mac TFTP server repeatedly.
4. The U-Boot-specific AROS hand-off receives the exact Pi 4 FDT, raw AROS
   bootstrap image and AROS BSP package, while physical serial output remains
   usable.
5. Native RJ45 TFTP remains a documented recovery path.

The proof points are deliberately ordered.  Do not debug an AROS kernel crash
until the transport and hand-off have passed independently.

## Boot-script template

`boot.cmd.in` is input for a future manifest-aware renderer. It contains
placeholders for the server, *bare* deployment-root filenames and verified
load addresses, and names the required `arosboot` U-Boot command. It must not
be converted to `boot.scr` until that command and its associated patch are
available. The generated script must request only the bare filenames because
`aros board serve` exposes the atomically published board deployment as its TFTP
root, not the parent `tftp_root`.

The template does not use Linux `booti`: the existing AROS Pi bootstrap is a
raw image plus an AROS package, not a Linux `Image` with a Linux initrd.

## Debugging boundary

USB-C CDC-ECM is a post-U-Boot network transport.  It does not replace:

- Pi 4 physical UART on GPIO 14/15 for early logs;
- Pi 4 full JTAG on GPIO 22--27 for hardware debugging; or
- a deliberate reset/power-control procedure when early code wedges.

Do not enable Linux-specific gadget overlays in the Pi boot `config.txt`.
U-Boot owns DWC2 peripheral setup in this profile.
