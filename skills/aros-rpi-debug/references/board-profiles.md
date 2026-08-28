# Board profile boundaries

Board profiles describe a specific lab device: transport, TFTP destination, UART adapter, debugger endpoint, and optional explicitly authorized power control. They are local and must not be added to portable target metadata.

`target` / `preset` names the CMake preset, while `toolchain_preset` names the
locked AROS cross-toolchain profile. For the Pi 4 debug CMake preset, set
`toolchain_preset = "rpi-aarch64"`; do not invent a separate lock-profile name.

For USB-ECM, a profile must use a stable USB identity rather than a host network
name: USB vendor ID, product ID, USB serial, and the expected Pi-side
CDC-ECM MAC. macOS (`enX`) and Linux (`enx…` or another name) assign the host
interface name dynamically. `aros board scan` reports that current name together
with the USB descriptors, but scanning is read-only and never pairs a board by
itself.

The supported profile shape is:

```toml
[boards.<local-name>.usb_ecm]
host_address = "192.168.77.1"
target_address = "192.168.77.2"
subnet_mask = "255.255.255.0"

[boards.<local-name>.usb_ecm.identity]
vendor_id = 0x1d6b
product_id = 0x0104
serial = "aros-rpi4-lab-01"
expected_target_mac = "02:aa:00:00:00:01"
```

`aros board serve --board <local-name>` repeats the USB lookup and requires
exactly one VID:PID+serial match. It verifies `host_address` is currently
assigned to that matched interface before binding. The expected target MAC is
checked at DHCP packet time: the service gives the one `target_address` lease
only to that MAC, and ignores all other clients. TFTP is read-only and serves
only the atomically published deployment at `<tftp_root>/<tftp_prefix>`.
The CLI additionally makes the selection an OS-level socket constraint:
Linux uses `SO_BINDTODEVICE` and macOS uses `IP_BOUND_IF` for DHCP and every
TFTP socket, including per-transfer/error sockets.

An explicit non-USB Ethernet port instead needs this local profile shape:

```toml
[boards.<local-name>.network]
interface = "REPLACE_ME"
server_address = "192.168.77.1"
target_address = "192.168.77.2"
subnet_mask = "255.255.255.0"
expected_target_mac = "02:aa:00:00:00:02"
```

`serve` verifies that `server_address` is on exactly that named interface; it
never guesses an RJ45 port. In both transport modes the CLI owns no host
network configuration, has no `--bind` escape hatch, and rejects wildcard or
wrong-interface addresses. Use `aros board serve --board <name> --dry-run` to
validate the complete plan before opening any sockets.

## SD boot-bundle boundary

`aros board sd image --board <name> --boot-bundle <dir> --output <new-dir>`
requires a versioned external `boot-bundle.toml` whose board/model/transport
and, for USB-ECM, complete identity match the selected profile. The Pi 4
`uboot-usb-ecm` format additionally needs hash-pinned `config.txt`,
`start4.elf`, `fixup4.dat`, `bcm2711-rpi-4-b.dtb`, `u-boot.bin` and
`boot.scr` at their exact FAT destinations. The command validates only by
default; `--apply` produces a filesystem artifact, never a physical disk
write. The U-Boot source, patch series and `arosboot` hand-off remain external
inputs; their absence must stop image creation rather than be guessed.

`aros board sd scan --artifact <dir>` is the only way to obtain a candidate's
current opaque `scan-id` and artifact-bound confirmation token. A later
`aros board sd write --board <name> --artifact <dir> --device <scan-id>` is only
a preview until that exact token is passed with `--confirm`. The actual writer
revalidates the artifact and local board identity, re-scans the same whole,
physical, removable, non-internal and writable disk with stable identity, and
rejects it if it is mounted, changed or too small. Missing or unknown safety
evidence excludes the disk from both display and writing. It never auto-selects,
implicitly unmounts or repartitions storage and has no force bypass. Linux
uses an exclusive/no-follow raw-device open plus a post-open identity recheck.
macOS first acquires an exclusive Disk Arbitration `DADiskClaim` RAII guard
for the exact whole BSD disk, revalidates every candidate predicate, identity,
fingerprint, capacity and unmounted state while the claim is held, and never
falls back to an unclaimed raw open. The guard spans raw open, write, sync and
SHA-256 readback; the raw descriptor closes before claim release. Physical
unplug remains an I/O/readback error boundary rather than something the claim
can prevent.

For an already mounted card, `aros board sd unmount` lists only mounted whole
physical removable non-internal writable disks with complete descendant
topology. `--device <scan-id>` is a preview; only the same opaque ID plus
`--apply` can perform a fresh-revalidated unmount. Raw paths, mountpoints
outside `/Volumes`, `/media`, `/run/media` or `/mnt`, and unknown safety
evidence are rejected. Before every normal per-volume operation the remaining
topology and source-device identity are checked again; no broad whole-disk,
force/lazy unmount or eject is available. This is a separate authorization;
afterward obtain a new writer candidate and token with `sd scan --artifact`.

## Raspberry Pi 4

- Use a 3.3 V UART on GPIO14/15 as the first-line console.
- The documented CPU debug interface is full JTAG on GPIO22–27. Plan a JTAG-capable adapter and preserve UART logging independently.
- `native-tftp` is the reference transport. `uboot-usb-ecm` is allowed only after a provisioned SD/U-Boot chain has passed the transport smoke test.

## Raspberry Pi 5

- The three-pin debug connector can be switched between its UART role and CPU SWD; preserve an independent UART route when halt debugging is enabled.
- Keep Pi 5 profiles disabled until the AROS BCM2712 platform support is implemented and validated. A compatible TFTP or USB transport alone is insufficient.
