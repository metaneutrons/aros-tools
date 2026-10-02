# Portable board registry

`registry-v1.toml` records stable model IDs and reviewed defaults for the current
Pi 3/4/5 and Titan contracts. It is embedded in the tools build and parsed by
`aros_common::board_registry`. Local aliases, serial devices, network settings
and operator paths remain in `boards.toml`.

The parser checks the complete closed document, rejects duplicate IDs and
unsupported backend/transport capabilities, and binds its exact raw-byte
SHA-256. Entries contain safe source-target/compiler references and relative
artifact paths, never executable commands or host devices. Model and transport
iteration is sorted by stable ID; TOML ordering cannot select a default.

This is the registry foundation for [RV1](https://github.com/metaneutrons/aros-tools/issues/320),
not the completed consumer migration. The existing board CLI and templates
still use their previous model enums in this slice. Tests compare registry
defaults with those current consumers; a later slice must switch consumers and
remove the duplicate model lists before RV1 can close.

No ESP32-P4 model is advertised yet. Adding a descriptor does not implement a
new transport or qualify hardware, native media or an unpublished compiler.
The registry records the existing Titan profile even though its RISC-V compiler
input and physical acceptance remain pending under BM5.
