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
roles and FAT destinations must be unique, relative and traversal-free. The
external bundle must still supply regular files with measured hashes.

The internal `MediaBuildReceipt` v1 contract records target, model, transport,
and the roles, relative paths, sizes and SHA-256 digests of source-dependent
inputs. It distinguishes future CMake output from measured legacy-v1 bundle
inputs; a legacy bundle cannot prove its own build origin. Verification rejects
unsafe paths, symlinks, missing required roles and altered bytes. An adapter
can derive this receipt from an already validated `boot-bundle.toml` v1
without changing that public bundle format.
No CMake producer or general `aros image` command emits or consumes the new
receipt yet; neither profile selection nor receipt validation is boot evidence.

The internal external-input lock contract records exact file sizes, SHA-256,
public HTTPS origins, source-revision labels and declared license IDs. Its raw
bytes must match a reviewed lock ID and digest before any file is used. This
contract does not fetch firmware or make a license or boot-qualification claim.
The current Pi 4 reference firmware lock still lacks per-file hashes and is
not accepted as a complete automated media input lock.
