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
