# aros-macos-disk-claim

Small fail-closed RAII wrapper around macOS Disk Arbitration's
`DADiskClaim`. It accepts only an exact whole BSD disk name (`diskN`) or its
buffered device path (`/dev/diskN`), never unmounts anything, and keeps the
claim alive until the guard is dropped.

The crate is an internal implementation detail of the `aros board sd` workflow.
It must not be used as a substitute for the workflow's removable-device,
identity, capacity, complete descendant mount-topology, and raw-device identity
rechecks. The whole disk's own `VolumePath` absence is only an extra check, not
proof that every child partition is unmounted. Those checks must run again while
the claim is held.

Disk Arbitration has no claim-cancellation API. If acquisition exceeds the
caller's timeout, the caller returns but the dedicated worker retains its
session, disk, run loop, and synchronized callback state. The completion
callback owns a separate raw `Arc` strong reference: a missing callback can at
worst leak that allocation, never access freed worker memory. The worker keeps
pumping until completion; a late successful claim is immediately unclaimed.
The worker is joined after normal cleanup and detached only if this bounded
failure cleanup itself times out, so callback memory is never freed early.
