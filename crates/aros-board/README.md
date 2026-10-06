# aros-board

`aros-board` owns local physical-board profiles and the safety-critical board
lab engines used by `aros board`. It deliberately contains no command-line
parser and does not depend on `aros-cli`.

The checked-in `aros-targets.toml` remains the source of truth for reproducible
build targets. The local `boards.toml` identifies concrete hardware instances,
their transport, stable device identity, and host-local paths.

The SD image engine also accepts a closed, model-independent `MediaImagePlan`
for MBR/FAT32 composition. It stages a raw image, remeasures source files,
reads back the filesystem and publishes only to a new ordinary directory.
This internal backend does not select or write a block device. The public
`aros image` commands and ISO backend are separate follow-up work; no boot
qualification follows from creating an image.
The internal `verify_fat32_media_artifact` entry point reads back a published
image without the original source tree: it requires the exact three-file
artifact inventory, checks SHA256SUMS, parses the closed manifest, and verifies
the MBR, FAT32 filesystem and every declared payload file. These metadata are
self-consistency evidence, not an authenticated source or boot attestation.
