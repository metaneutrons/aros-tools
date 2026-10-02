# Portable board registry

`registry-v1.toml` records stable model IDs and reviewed defaults for the four
current Pi 3/4/5 and Milk-V Titan contracts. The board CLI and template
generator resolve model IDs from this registry embedded in the tools build;
they do not discover an external registry. A profile name such as `lab-rpi4`
is a local alias, distinct from the stable model ID `rpi4`. Serial devices,
network settings and operator paths remain in `boards.toml`.

The parser checks the complete closed document, rejects duplicate IDs and
unsupported backend/transport capabilities, and binds its exact raw-byte
SHA-256. Entries contain safe source-target/compiler references and relative
artifact paths, never executable commands or host devices. Model and transport
iteration is sorted by stable ID; TOML ordering cannot select a default.

The schema is closed over known fields and implemented capability families,
not over exactly four model names. The embedded catalog currently contains
four model IDs. A descriptor does not qualify hardware or media, or supply an
unpublished compiler. The catalog includes the existing Titan profile even
though its RISC-V compiler input and physical acceptance remain pending under
BM5. No ESP32-P4 model or support is advertised.
