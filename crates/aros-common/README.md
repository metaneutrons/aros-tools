# `aros-common`

`aros-common` is the dependency leaf for AROS host tools. It owns stable,
reusable contracts that must behave identically across components:

- versioned diagnostics and deterministic human/JSON rendering;
- opt-in local human/JSONL logging;
- fail-closed parsing of `aros-targets.toml`;
- typed SHA-256 parsing and streaming hashing;
- architecture, ELF, source-text, toolchain-lock, and manifest types;
- bounded, deadline-aware process execution with cooperative cancellation and
  process-group cleanup;
- broken-pipe-safe shared standard-output macros and deferred output errors;
- portable output-name and source-root containment validation;
- durable, journalled file-set publication and atomic no-clobber tree/file
  publication with descriptor-relative no-follow traversal;
- recursively durable, single-rename publication of caller-prepared trees,
  including no-follow symlink handling and portable/case-folded name checks;
- stable publication receipts, recovery outcomes, and failure classes for
  component-specific diagnostics and logs.

It deliberately contains no command-line workflow policy. Each executable
chooses its own diagnostic codes, hints, stages, and logging schema through a
small component adapter. New shared code belongs here only when at least two
components require identical semantics and can preserve their own error
boundary while using it.

`CancellationToken` is operation-local, shared between clones and one-way.
`run_output_with_control` starts no child after observed pre-spawn cancellation;
otherwise it returns the reaped status and bounded output with separate
`cancelled` and `timed_out` flags. An already observed exit wins over a late
request. Frontends own signal handlers; the library installs none. Existing
process entry points retain their behavior and report `cancelled: false`.

On Unix, captured stdin/stdout/stderr use nonblocking pipe workers. Once child
cleanup finishes, workers have one second to finish I/O and close their handles.
An escaped descendant holding or flooding a pipe therefore produces an explicit
cleanup error instead of an unbounded join. A cancellation remains an
`Interrupted` failure when capture cleanup also fails. The runner does not
claim to kill descendants that deliberately leave its process group, enforce a
network sandbox or interrupt an operating-system call stuck inside the kernel.
Process deadlines cover spawn and child waiting; pipe cleanup has that separate
bounded allowance. The same pipe-worker guarantee is not implemented on Windows.

`TargetProfile::load_from_file` and `TargetProfile::load_config` treat their
path as authoritative. Missing files, invalid TOML, and empty target arrays are
errors; bootstrap defaults must never silently replace repository state.

Mutating publication currently has a deliberately narrower platform contract:
it is enabled on Unix hosts, where `openat`, `O_NOFOLLOW`, inode identity,
exclusive rename, advisory locks, and directory `fsync` provide the required
semantics. It fails closed on Windows rather than pretending that `rename` and
file flush alone make parent-directory entries durable. Windows enablement
requires a native handle-relative, write-through implementation with the same
CAS and recovery guarantees.

The publisher keeps a zero-length, deterministically named `.aros-lock-*` file
in each transaction namespace. Its inode must remain stable for advisory-lock
serialization, so a successful run does not delete it. It contains no user or
build data and must not be removed while a writer may be active.

`DurableFileSet` records intent before creating auxiliary files, binds original,
staged and installed regular files to identity plus SHA-256, and removes only
objects whose ownership/CAS proof still matches. A prepared journal is rolled
back on the next lock acquisition; a committed journal completes cleanup. This
provides writer serialization and deterministic crash recovery, not a live
multi-file snapshot for readers. Readers must be scheduled after the writer or
participate in the same lock protocol.

For single-file and single-tree rename operations, a failure before rename
leaves the destination absent/unchanged. Once rename succeeds, the destination
is the committed object: a later parent-`fsync` failure is reported as
`CommitStateUncertain` and the complete destination is deliberately retained.
Callers must inspect that state and must never interpret the error as permission
to delete the destination.
