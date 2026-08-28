# aros-fetch

`aros-fetch` is the native AROS-NG implementation of the upstream `%fetch`
contract. It downloads or copies declared payloads, verifies optional explicit
SHA-256 declarations, safely extracts supported archives, and applies declared
patches.

The tool preserves `scripts/fetch.sh`'s build-facing arguments while providing
stable `AFxxxx` diagnostics and opt-in local structured logging. It never
generates checksums, trusts a payload on first use, weakens TLS, or executes a
shell command assembled from build input.

AROS-NG CMake builds resolve the checkout-local release binary explicitly and
fail if it is missing. `scripts/fetch.sh` remains only as the upstream GNU Make
compatibility path. Offline mode permits verified cache hits and declared local
origins but cannot reach a network origin. A strict checksum policy covers
every archive candidate and every remotely sourced patch payload.

Diagnostics use `aros-tool-diagnostics-v1` and the stable `AFxxxx` family.
Logging is disabled by default; `--log-level`, `--log-format`, and `--log-file`
enable a deterministic `aros-fetch-log-v1` stream without timestamps, host
identity, environment snapshots, or raw invocations.

Use `aros-fetch --help` for the complete interface.
