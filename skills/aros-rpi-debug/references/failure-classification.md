# Failure classification

Use the UART capture and the deployed manifest to identify the earliest failed phase. Do not skip straight to a hardware debugger when the failure can be classified from the log.

| Phase | Evidence to collect | First action |
| --- | --- | --- |
| Firmware / transport | EEPROM or U-Boot output, selected file path, TFTP transfer result | Validate board profile, boot bundle manifest, power and transport; do not rebuild blindly. |
| AROS bootstrap | Bootstrap banner, FDT parsing, exception or load-address output | Preserve the complete UART log including the preceding lines. Record whether the FDT line is prefixed by U-Boot or by the AROS bootstrap, then compare the deployed manifest's DTB/image/BSP hashes with the local build before a reset. |
| Core kernel | Kernel startup banner, panic/bug output, first scheduler transition | Use the core debug ELF and actual load address to symbolize the PC. |
| Loaded module | PKG member name and its reported virtual address | Add that module's debug ELF at the captured address before debugging further. |

Do not symbolize an FDT stop solely from a text marker: a bootstrap ELF/map becomes actionable only after a PC, LR, or exception address is available. If that requires a JTAG halt or register dump, explain that it changes board state and obtain authorization first.

If a deployment may have been incomplete, compare the manifest hashes in the
atomically published board deployment (the only directory `aros pi serve`
exports) with the local build output before resetting the board. A reset, an
SD rewrite, or a power cycle is a separate user-authorized action.
