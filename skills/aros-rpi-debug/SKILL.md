---
name: aros-rpi-debug
description: Diagnose and iterate on AROS Raspberry Pi 4/5 bring-up through the repository's `aros board` workflow. Use for boot, deploy, UART, symbol, JTAG, SWD, TFTP, or U-Boot USB-ECM issues; not for unrelated AROS builds or general Raspberry Pi OS setup.
---

# AROS Raspberry Pi Debug

Use the repository CLI as the single source of truth. Do not recreate a board-specific build, TFTP, serial, or GDB procedure in a prompt.

## Start safely

1. Identify the physical board and selected transport. For USB-ECM, first run the read-only `aros board scan`; pair a board by USB VID:PID, USB serial and the configured Pi-side gadget MAC, never by its transient `enX`/`enx…` name. If no local profile exists, `aros board init --board <name>` only prints a template; add `--apply` only to create a new configuration file and never to overwrite one. Run `aros board doctor` before a deployment or debugger action.
2. Read the active board profile and the bundle manifest. Treat USB device paths, USB identities, TFTP roots, probe endpoints, and power-control commands as local configuration, not repository target metadata.
3. Do not configure DHCP/TFTP services, unmount or write an SD card, reset/power-cycle a board, or alter firmware/EEPROM settings without the user's explicit authorization. An SD scan, image validation, unmount preview or write preview is non-mutating. `aros board sd unmount --device <scan-id> --apply` and `aros board sd write ... --confirm <token>` are two separately authorized state changes; permission for either one never implies permission for the other. Preserve the current UART log before a disruptive retry.
4. `aros board serve --board <name>` has no user-controlled `--bind`: it resolves the selected USB identity again at startup, verifies its concrete address belongs to the resulting current interface, and binds only there. On Linux, DHCP and every TFTP socket use `SO_BINDTODEVICE`; on macOS they use `IP_BOUND_IF`. Reject zero or multiple matches, a missing identity field, an unresolved interface, a missing atomically published deployment, and wildcard addresses (`0.0.0.0` and `::`); never fall back to another USB device, Wi-Fi, RJ45, or a wildcard listener. Use `--dry-run` to validate this plan without opening sockets.
5. The built-in DHCP server must answer only the configured Pi-side MAC and offer only its configured target address. The built-in TFTP server is read-only and must expose only the board's atomically published deployment directory, never an arbitrary root or build directory. Its listener, transfer and error sockets inherit the same mandatory OS-level interface bind.
6. `aros board sd image --board <name> --boot-bundle <dir> --output <new-artifact-dir>` validates only by default. Add `--apply` to create its atomic verified raw MBR/FAT32 artifact (`aros-pi-boot.img`, `boot/`, `manifest.json` and `SHA256SUMS`) below that new ordinary filesystem directory; it never opens or writes a physical disk. If a card is mounted, `aros board sd unmount` may only display disks with explicit whole/physical/removable/non-internal/writable/stable-identity evidence; pass the opaque ID once for preview and again with `--apply` for a separately authorized operation. Raw paths, mountpoints outside `/Volumes`, `/media`, `/run/media` or `/mnt`, unknown evidence and incomplete topology must fail closed. Revalidate the exact remaining topology before each normal per-volume unmount, bind it to the source-device identity, and never use force/lazy or a broad whole-disk command. `aros board sd scan --artifact <dir>` lists only explicitly verified whole/physical/removable/non-internal/writable/unmounted disks and prints an artifact-bound confirmation token for each. `aros board sd write --board <name> --artifact <dir> --device <scan-id>` is a preview without `--confirm`; raw paths are rejected, and a matching current token without `--dry-run` permits a write only after independently revalidating artifact, board/USB identity, disk fingerprint, capacity, stable identity, and every whole/physical/removable/non-internal/writable/unmounted predicate before open and under platform ownership before the first byte. Linux uses `O_EXCL` + `O_NOFOLLOW` and a post-open identity check. macOS must acquire an exclusive `DADiskClaim` RAII guard for the exact whole BSD disk before raw open, revalidate under the claim, keep it through write, sync and SHA-256 readback, close the raw descriptor before releasing the claim, and never fall back to an unclaimed raw open. Physical unplug remains a hard error boundary. Never auto-select, implicitly unmount or repartition, and never accept `--force`/`--yes`.

## Iteration loop

Use the narrowest applicable sequence:

```text
scan (USB-ECM only) → doctor → build → deploy --apply → serve --dry-run → serve → console capture → classify failure → symbols or hardware debugger
```

- Build the Pi artifact target rather than an arbitrary CMake product. Keep the debug ELF, map, ROM/PKG, DTB, and manifest together.
- Prefer the direct Raspberry Pi firmware/TFTP route when it is available. `uboot-usb-ecm` is an optional Pi 4 transport backend and must not silently replace the known-good native path.
- `aros board scan` is read-only candidate discovery on macOS and Linux. It reports a dynamic interface name only for diagnosis; it must not rewrite a board profile or silently choose a candidate. `aros board serve` accepts exactly one complete paired identity before it starts the restricted DHCP/TFTP service.
- The tool does not configure a host IP address. For USB-ECM, assign the private address to the selected current interface first; for native RJ45, put the intended physical port in `network.interface`. Both paths must configure an expected Pi MAC before DHCP may start.
- `aros board sd image` never builds or downloads U-Boot. It requires a versioned external boot bundle whose board, USB identity, complete payload and hashes match the selected profile. The remaining U-Boot source/patch/`arosboot` work is therefore an explicit external-input blocker, not a reason to fabricate an SD image.
- For an authorized SD write on Linux or macOS, use the exact sequence `sd scan --artifact` → `sd write` preview → copy the displayed token into `sd write --confirm`. Never reuse a token after a rebuild, card change, remount, capacity change or re-scan. On macOS, `--confirm` is not a claim bypass: any failed claim or under-claim revalidation must stop before the first write.
- If the intended card is mounted, treat `sd unmount` list → exact-ID preview → exact-ID `--apply` as an optional prior workflow with its own authorization. Re-run `sd scan --artifact` afterward; never carry an unmount ID forward as a write candidate or token.
- Classify a failure as firmware/transport, AROS bootstrap, core kernel, or loaded module before proposing a fix. Read [failure classification](references/failure-classification.md) for the required evidence.
- Generate symbol commands only when the log or debugger provides a PC/LR or exception address; a UART console is not a GDB remote stub. Ask for authorization before halting a board to obtain registers.

## Board boundaries

- Pi 4 uses GPIO UART for baseline logs and documented full JTAG for halt debugging. Do not assume a two-wire SWD probe is a Pi 4 CPU debugger.
- Pi 5 transport support does not imply an AROS BCM2712 port. Confirm the platform milestone before treating a Pi 5 boot as supported.
- Read [board profiles](references/board-profiles.md) before changing a board profile or choosing UART/JTAG/SWD wiring.

## Report

State the board, transport, build manifest, observed boot phase, captured log location, and the next least-disruptive action. Clearly separate verified observations from hypotheses.
