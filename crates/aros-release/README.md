# aros-release

`aros-release` is the repository-internal, fail-closed producer for native
`aros-tools` release archives. It consumes binaries that were built once for a
specific host, writes a normalized archive plus a machine-readable manifest and
SHA-256 sidecar, and verifies the result by reading it back.

The tool is deliberately not part of the public installation. Release CI uses
it so archive creation and validation have the same structured diagnostics and
opt-in logging contract as the shipped AROS command-line tools.
