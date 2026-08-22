# Raspberry Pi debug and transport assets

This directory is the checked-in, machine-independent asset and contract part
of the AROS Raspberry Pi bring-up workflow. It deliberately contains no
downloaded firmware, host-network configuration, or device-specific serial
paths. The executable behaviour lives in the repository's `aros pi` CLI.

`aros pi` consumes the board-profile and artifact conventions in this
directory.  The directory remains a reviewed provisioning reference, not a
replacement for the normal AROS build: it deliberately does not download
firmware, configure the host network, or touch a physical board.

## Scope and phases

The first supported board profile is the Raspberry Pi 4 (BCM2711).  It has
two transport choices:

| Transport | Firmware path | Purpose | Status |
| --- | --- | --- | --- |
| `native-tftp` | Raspberry Pi EEPROM firmware over the built-in RJ45 port | Reference and recovery path | Restricted host DHCP/TFTP service implemented; needs its ordinary Pi boot inputs |
| `uboot-usb-ecm` | SD card firmware starts U-Boot; U-Boot exposes USB-C CDC-ECM and fetches over TFTP | Optional cable-constrained Pi 4 lab path | Restricted host service implemented; bootable SD bootstrap still waits for external U-Boot inputs |

`native-tftp` is the reference because the Raspberry Pi boot firmware already
understands the Pi boot partition and does not introduce a second bootloader.
The Pi EEPROM's network boot path uses the built-in Ethernet controller; it
does **not** use the USB-C gadget port.

`uboot-usb-ecm` is intentionally a transport adapter, not a new AROS boot
format.  A fixed SD card holds only Pi firmware, U-Boot and its boot script.
Every AROS rebuild is published by the Mac's TFTP service and loaded after a
board reset.

Raspberry Pi 5 is a separate future phase.  Its BCM2712 platform support,
boot hand-off and debugger wiring must be validated independently; the
Pi-4-specific U-Boot profile in this directory must not be presented as Pi 5
support.

## Directory layout

```text
tools/rpi-debug/
  README.md                  this contract
  firmware.lock.toml         pinned Pi firmware source revision and required files
  config.txt.in              SD boot-partition template for the Pi 4 U-Boot profile
  boards.example.toml        non-secret examples for a local board config
  uboot/
    README.md                Pi 4 USB-ECM bootstrap contract
    boot-bundle.toml.in      external, versioned SD boot-bundle manifest template
    rpi4-aros-usbdev_defconfig
                              U-Boot configuration delta, not a complete defconfig
    boot.cmd.in              generated boot-script source template
    patches/README.md        required U-Boot patch series, before patches exist
```

The actual local board configuration belongs outside the repository, normally
at `~/.config/aros/boards.toml` (or `$XDG_CONFIG_HOME/aros/boards.toml`).
It contains host paths, IP addresses, USB identities, serial-device names and
debugger selection. Copy and adapt `boards.example.toml`; do not commit the
result.

`aros pi init --board <local-name>` prints an intentionally incomplete
USB-ECM template without writing anything. Add `--apply` to create a *new*
configuration file at the selected `--config` path (or the normal default).
It creates parent directories when needed but refuses to overwrite or merge an
existing file, so comments and local choices cannot be lost. Replace every
`REPLACE_ME` value before using the profile.

The board's `target` / `preset` selects its CMake configuration. Its
`toolchain_preset` independently selects the locked cross-toolchain profile;
the Pi 4 debug preset deliberately uses `rpi-aarch64`. This keeps
board-specific CMake settings separate from the portable, audited toolchain
identity.

## Boot contracts

### Native Ethernet TFTP

```text
Mac TFTP server -- RJ45 --> Pi EEPROM firmware --> AROS boot bundle
```

The published bundle contains the firmware-selected Pi 4 DTB, the raw AROS
bootstrap image, the AROS BSP package and a manifest. CMake creates it under
`build/rpi4-aarch64-debug/boot/raspi` by way of `rpi-artifacts`; `aros pi
deploy --board <name> --apply` atomically publishes a completed bundle into
the configured `<tftp_root>/<tftp_prefix>` directory. `aros pi serve` exposes
only that verified, AROS-managed deployment directory—never its parent TFTP
root and never the build directory directly.

### Current core-link boundary

The modern CMake module graph does not yet generate the three legacy KOBJ
inputs that the Pi core linker requires.  Until that CMake port exists, the
Pi 4 artifact target deliberately requires a local `core_kobj_dir` containing
the legacy-generated AArch64 `kernel_resource.o`, `exec_library.o` and
`task_resource.o`. Generate them with the legacy Pi build's `make
kernel-raspi-aarch64`; its directory is normally
`<legacy-build>/bin/raspi-aarch64/gen/kobjs`. `aros pi doctor` validates them
and `aros pi build` passes the directory explicitly to CMake. It must not
substitute the modern runtime relocatable module ELFs: doing so produces
unresolved loader glue and is not a bootable core.

### U-Boot USB-C CDC-ECM

```text
fixed SD card
  Pi 4 firmware --> U-Boot --> DWC2 peripheral / CDC-ECM --> Mac TFTP server
                                                  |
                                                  +--> AROS-specific hand-off
```

The Raspberry Pi USB-C port is only the data transport in this mode.  It does
not provide hardware UART, JTAG or SWD.  Keep physical UART connected for all
first boots; U-Boot cannot help diagnose a failure before it starts.

The final hand-off is deliberately not modelled as Linux `booti`: AROS uses a
raw bootstrap image and an AROS package.  A reviewed U-Boot `arosboot` hand-off
must load the image, BSP package and Pi DTB at verified addresses, preserve
their sizes, and enter AROS with the expected FDT argument.  Until that exists,
`uboot/boot.cmd.in` remains a source template rather than a provisionable
`boot.scr`.

## USB-ECM discovery and pairing

The network name assigned to a USB gadget is transient: it might be `en7` on
macOS and `enx…` or another name on Linux. It must be reported for diagnostics,
but it is not a board identity and must not be saved as one. A physical Pi is
instead paired by all of these values:

```text
USB vendor ID + product ID + USB serial + expected CDC-ECM target MAC
```

VID:PID identifies the configured U-Boot gadget profile; the USB serial
distinguishes two boards using that profile; the expected target MAC pins the
Pi-side CDC-ECM Ethernet function. The U-Boot gadget configuration must supply
both the serial descriptor and the target MAC consistently. A device with no
serial is visible for diagnosis, but must not be paired automatically.

`aros pi scan` is the read-only discovery step. It finds CDC-ECM candidates and
prints their current interface name, USB VID:PID, USB serial when available,
host-interface MAC, current IPv4 addresses and whether CDC-ECM was confirmed.
It neither writes `boards.toml`, changes addresses, nor opens DHCP or TFTP
sockets. The user selects a unique candidate and copies its stable descriptor
values into the local profile's `[boards.<name>.usb_ecm.identity]` table.

The scanner uses the native device hierarchy on both supported developer host
platforms:

| Host | Discovery route | Dynamic result |
| --- | --- | --- |
| macOS | USB device descriptors through IOKit/IORegistry, correlated with the registered USB network interface | BSD interface such as `en7` |
| Linux | `/sys/class/net/<interface>/device` and its USB-device ancestors, with CDC-ECM class/driver validation | interface such as `enx…` |

`aros pi serve --board <name>` repeats discovery at service start. For
`uboot-usb-ecm`, it accepts exactly one CDC-ECM match for USB VID, PID and
serial, resolves that candidate's current interface, and verifies that
`usb_ecm.host_address` is assigned to it. It then binds DHCP and TFTP only to
that concrete IPv4 address; there is intentionally no `--bind` override.
Zero matches, multiple matches, an incomplete identity, a missing deployment,
an address on a different interface, wildcard addresses, or a non-IPv4
address stop the command before a socket is opened. There is no fallback to
the first USB adapter, Wi-Fi, RJ45, or a wildcard listener.

The address check is supplemented by a real operating-system interface bind:
on Linux, DHCP and **every** TFTP socket (listener, transfer and error socket)
use `SO_BINDTODEVICE`; on macOS they use `IP_BOUND_IF`. Thus a selected
USB-ECM or explicitly named RJ45 interface is not merely a source-address
preference. A socket cannot silently egress through Wi-Fi, another Ethernet
port or another USB adapter that happens to carry the same address.

The expected Pi-side MAC cannot be learned from the USB descriptor scan. It is
checked by the restricted DHCP service instead: only DHCP DISCOVER/REQUEST
packets whose Ethernet client MAC exactly matches
`usb_ecm.identity.expected_target_mac` receive the one configured
`target_address` lease. All other clients receive no DHCP response. TFTP is
read-only and serves only the atomically published deployment described above.

For `native-tftp`, the profile must name `network.interface` explicitly and
must supply `network.expected_target_mac`; `serve` verifies that
`network.server_address` belongs to that named interface and applies the same
one-MAC/one-lease restriction. The command does not configure either host
interface; configure the private address first, then use
`aros pi serve --board <name> --dry-run` to prove the full selection and
deployment plan without opening DHCP or TFTP sockets.

`serve` is intentionally separate from `deploy`: deployment publishes a
finished bundle, while service startup is an explicit foreground lab action.

## SD-card provisioning

Image creation, disk discovery and writing are deliberately separate:

```text
aros pi sd image --board <name> --boot-bundle <dir> --output <new-artifact-dir>
aros pi sd image --board <name> --boot-bundle <dir> --output <new-artifact-dir> --apply
aros pi sd unmount
aros pi sd unmount --device <scan-id>
aros pi sd unmount --device <scan-id> --apply
aros pi sd scan
aros pi sd scan --artifact <artifact-dir>
aros pi sd write --board <name> --artifact <artifact-dir> --device <scan-id>
aros pi sd write --board <name> --artifact <artifact-dir> --device <scan-id> --confirm <token>
```

Without `--apply`, `sd image` is a read-only validation run: it checks the
versioned external `boot-bundle.toml`, all regular input files and their
SHA-256 values, the selected board/model/transport/USB-ECM identity, and the
declared MBR/FAT32 layout. It writes no image. `--apply` creates a new,
previously absent output directory atomically and produces:

- `aros-pi-boot.img` — the verified raw MBR/FAT32 image;
- `boot/` — the verified boot-partition payload;
- `manifest.json` — deterministic artifact metadata; and
- `SHA256SUMS` — checksums for the artifact members.

The image command writes only below the chosen ordinary filesystem output
directory; it never opens, selects or writes a physical disk.
`uboot/boot-bundle.toml.in` records the exact format-1 fields, required roles
and FAT destinations. Do not treat `sd image` as an U-Boot builder.

`aros pi sd unmount` is a separate operation for preparing an already mounted
card. With no `--device` it performs a read-only scan and lists only mounted
whole physical disks for which the operating system explicitly reports every
required removable, non-internal, writable and stable-identity property. A
missing or ambiguous property hides the device. Passing its opaque `scan-id`
without `--apply` is still only a preview; only the exact same ID plus
`--apply` may unmount it. Raw `/dev/...` paths are rejected. The command
re-scans immediately before the operation and before each volume, accepts
mountpoints only below `/Volumes`, `/media`, `/run/media` or `/mnt`, and
refuses incomplete or changed descendant topology. It unmounts only the exact
source volumes already bound to the candidate: Linux verifies the current
topmost mount's device identity, while macOS addresses the verified descendant
device node. It never uses force/lazy unmount, a broad whole-disk command,
eject, repartitioning or a shell command.

`aros pi sd scan` is read-only. It lists only candidates that the platform
explicitly reports as whole, physical, removable, non-internal, writable and
completely unmounted disks with stable identity; it never chooses one.
Unknown or missing safety evidence excludes the disk. With `--artifact
<artifact-dir>` it additionally verifies the generated image artifact and
prints a distinct confirmation token for each currently safe candidate. The
token is bound to the artifact manifest hash, raw-image hash and size, the
candidate's identity/fingerprint, and its capacity.

`aros pi sd write` requires the opaque `scan-id` from that current scan.
Raw `/dev/...` paths are rejected at argument parsing.
Without `--confirm` it is a preview: it validates the selected board and
artifact, shows the exact token, and writes nothing. `--dry-run` also prevents
a write. A matching `--confirm <token>` without `--dry-run` authorizes a
physical write on Linux or macOS. Immediately before any claim or raw-device
open, the CLI re-reads and hashes the artifact, re-scans the candidate,
verifies the selected board and USB identity, requires the same whole,
physical, removable, non-internal, writable and completely unmounted disk with
stable identity and sufficient capacity, and checks the token again. This is
an independent writer invariant rather than trust in an earlier display: the
writer re-establishes every removable-device predicate before opening the
target and again under its platform ownership immediately before the first
byte.

On Linux, the writer opens the whole raw device with `O_EXCL` and `O_NOFOLLOW`
and performs a post-open identity recheck. On macOS, it first acquires an
exclusive Disk Arbitration `DADiskClaim` RAII guard for the exact whole BSD
disk. A failed claim stops before raw open; there is no unclaimed raw-device
fallback. While the claim is held, the writer revalidates disk identity,
fingerprint, capacity and unmounted state, opens the raw device, and performs
the post-open identity check. Both writers flush the completed write and
SHA-256 read back the image bytes. On macOS the raw file descriptor is closed
before the RAII guard releases the claim. Disk Arbitration cannot prevent a
physical unplug; removal or any resulting I/O/readback failure remains a hard
error boundary.

The writer never automatically chooses a disk, unmounts it, repartitions it,
or accepts a blanket `--force`/`--yes` bypass. Partitions, internal/system
disks, mounted targets, stale `scan-id` values, changed artifacts and
mismatched tokens are rejected.

The image generator is usable once a complete external U-Boot boot bundle is
provided. This repository still does not contain a pinned U-Boot
source/build/patch pipeline or the required `arosboot` hand-off. Until those
external inputs are supplied and manifest-verified, `sd image` for
`uboot-usb-ecm` fails with the missing-input list rather than emitting a
plausible but unbootable image. A future explicit `native-firmware` mode can
instead package a verified direct-firmware Pi boot bundle.

## Safe operating rules

- Do not let tooling silently download or update Pi firmware or U-Boot.  The
  source revision and required file set are recorded in `firmware.lock.toml`.
- Do not make `aros pi deploy` configure DHCP, TFTP or a power relay.  Those
  are explicit lab-owner actions.
- `aros pi scan`, `aros pi serve --dry-run`, `aros pi sd scan`, `aros pi sd
  unmount` without `--apply`, `aros pi sd image` without `--apply`, and `aros
  pi sd write` without `--confirm` are non-mutating. Starting `aros pi serve`,
  applying an unmount, or confirming an SD write are separate explicit
  lab-owner actions; each must fail closed against its identity, address,
  artifact and disk checks. Unmount authorization never authorizes a later
  write. On macOS a confirmed write must hold its `DADiskClaim` from before raw
  open until after the raw file descriptor closes; it must never fall back to
  an unclaimed raw-device write.
- Treat TFTP as an isolated development-network service.  It has no
  authentication or transport integrity.
- Do not use loose GPIO wiring as a normal power source.  USB-C gadget data
  and sufficient board power need a stable, appropriate setup.
- Keep native Ethernet TFTP and physical serial available as recovery paths
  even when testing USB-ECM.

## CLI boundary

The CLI owns local orchestration; CMake owns build artifacts:

```text
CMake              rpi-artifacts / debug ELF / maps / boot bundle
aros pi init       prints, or with --apply creates, a new local board config
aros pi scan       read-only macOS/Linux USB CDC-ECM candidate discovery
aros pi doctor     validates local board configuration and prerequisites
aros pi deploy     atomically publishes an already-built board deployment
aros pi serve      explicit foreground restricted DHCP + read-only TFTP service
aros pi sd image   validates, then with --apply creates, an SD image artifact
aros pi sd unmount list/preview by default; explicitly unmount bound removable volumes
aros pi sd scan    read-only safe removable-disk discovery and token display
aros pi sd write   preview by default; token-gated physical write on Linux/macOS
aros pi console    opens an external physical UART terminal
```

Both transports must expose the same `aros pi deploy --board <name>` contract.
Only the backend changes.  This keeps a later Pi 5 implementation from being
coupled to the Pi 4 USB-ECM experiment.
