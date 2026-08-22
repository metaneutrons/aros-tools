# Required U-Boot patch series

No U-Boot source patch is checked in yet.  This file records the smallest
reviewable series that must exist before `rpi4-aros-usbdev_defconfig` and
`boot.cmd.in` are turned into an automated bootstrap builder.

## Patch order and scope

1. **`0001-rpi4-aros-usb-ecm-config.patch`**
   Add a complete `rpi4-aros-usbdev_defconfig` derived from the exact pinned
   upstream `rpi_4_defconfig`; it must select DWC2 gadget support and
   CDC-ECM, while retaining normal network/TFTP command support.
2. **`0002-rpi4-dwc2-device-mode.patch`**
   Make the Pi 4 USB-C DWC2 controller enter and remain in peripheral mode
   early enough for CDC-ECM to register.  This must be board-specific and
   must not change the USB host-controller setup used by normal AROS booting.
3. **`0003-arosboot-aarch64-handoff.patch`**
   Add a narrowly scoped `arosboot` command.  It must validate supplied
   ranges, avoid overlap with U-Boot and its FDT, pass the Pi 4 FDT through
   AArch64 `x0`, expose the AROS BSP package at the agreed physical address,
   clean/disable caches as required, and transfer control to the raw AROS
   bootstrap entry.
4. **`0004-rpi4-arosboot-tests.patch`**
   Add sandbox/unit coverage where possible plus a documented hardware smoke
   test: cold boot, CDC-ECM enumeration, multi-megabyte TFTP transfer, AROS
   serial banner, and repeat reset.

## Review requirements

- Pin the U-Boot upstream tag or immutable commit in
  `../../firmware.lock.toml` before adding the first patch.
- Record the exact `git format-patch` base and patch hashes in a `series`
  file.  Never apply an unpinned patch onto a moving U-Boot branch.
- Do not use an unallocated or borrowed USB vendor/product ID in a released
  build.  Development identity choices must be explicit in the full
  configuration.
- Preserve physical UART diagnostics.  A CDC-ECM failure before network
  registration must remain observable without USB networking.
- Keep Pi 5 out of this series.  It needs its own BCM2712 boot and debugger
  validation.
