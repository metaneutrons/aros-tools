# Media profiles

These files are reviewed, portable boot-filesystem layout contracts. They are
not target presets or local board configurations. `aros-targets.toml` selects
the compiler target; `boards.toml` supplies a local board identity and guarded
device settings. A media profile binds a stable model and transport to required
file roles and destinations.

The current registry contains only the existing Pi 4 U-Boot USB-ECM and
Milk-V Titan UEFI bundle layouts. It does not yet describe native Pi SD media
or the PC ISO, and it does not imply that either board has passed a physical
boot qualification. The SD bundle validator loads these contracts from the
binary's reviewed source revision; changing a profile requires a new tools
build and tests.

Parser fixtures under `crates/aros-common/tests/fixtures/media/` exercise PC
BIOS ISO and native Pi target identities, including `pc-x86_64`. They are not
part of the runtime registry and are not media-build or boot claims.

Each TOML document has a closed `format_version = 1` schema. It may declare
identities and file destinations, never commands or host devices. Required
roles and media destinations must be unique, relative and traversal-free. The
external bundle must still supply regular files with measured hashes.
The `layout` section is format-specific: MBR/FAT32 profiles declare a
sector-aligned partition and FAT label; BIOS ISO profiles declare a volume ID
and the required El Torito boot-image role. The existing Pi 4 and Titan
profiles declare a reviewed 64 MiB/2048-LBA geometry for future portable
composition. Legacy v1 bundles may still declare other valid geometries;
their existing `aros board sd image` behavior is unchanged. The PC/Pi schema
fixtures are not runtime profiles.

The internal `MediaBuildReceipt` v1 contract records target, model, transport,
and the roles, relative paths, sizes and SHA-256 digests of source-dependent
inputs. It distinguishes future CMake output from measured legacy-v1 bundle
inputs; a legacy bundle cannot prove its own build origin. Verification rejects
unsafe paths, symlinks, missing required roles and altered bytes. An adapter
can derive this receipt from an already validated `boot-bundle.toml` v1
without changing that public bundle format.
The Titan CMake staging target emits and verifies a measured build receipt.
No general `aros image` command consumes it yet; neither profile selection nor
receipt validation is boot evidence.

The internal external-input lock contract records exact file sizes, SHA-256,
credential-free HTTPS DNS origins, source-revision labels and declared license
IDs. Its raw bytes must match a reviewed lock ID and digest before any file is
used. This contract does not fetch firmware or make a license or
boot-qualification claim.
Profiles can pin an external lock by exact ID and raw-byte SHA-256 and bind a
required file role to one locked file ID. Every referenced lock must be pinned,
every pin must be used, and the supplied lock set must match exactly. A CMake
receipt then reports only build-produced roles; it cannot claim an external
file as its output. The legacy-v1 receipt still measures all bundled roles,
but does not independently establish their origin. These bindings are an
internal contract slice, not a registered native-SD profile.
The read-only `MediaImagePlan` resolver checks the complete selected role set,
build receipt, pinned lock bytes and actual regular input files before any
image output exists. Its file paths remain point-in-time observations. The
internal MBR/FAT32 backend remeasures during copying, checks filesystem
capacity, reads back the image, and publishes without replacing an existing
directory. Its independent artifact verifier checks the exact file inventory,
checksums, MBR, FAT32 tree and payload hashes without the source tree. This is
self-consistency evidence, not authenticated origin or a boot test. ISO and
the public `aros image` command remain unimplemented.
The current Pi 4 reference firmware lock still lacks per-file hashes and is
not accepted as a complete automated media input lock.
