# aros-board

`aros-board` owns local physical-board profiles and the safety-critical board
lab engines used by `aros board`. It deliberately contains no command-line
parser and does not depend on `aros-cli`.

The checked-in `aros-targets.toml` remains the source of truth for reproducible
build targets. The local `boards.toml` identifies concrete hardware instances,
their transport, stable device identity, and host-local paths.
