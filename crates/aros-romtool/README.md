# `aros-romtool`

`aros-romtool` assembles and validates AROS ROM package layouts used by the
generated build graph. It owns package ordering, alignment, offsets, capacity
checks, and the final image bytes. It does not choose a target profile or
schedule a build.

Input or layout failures are fatal; the tool never publishes a knowingly
partial image. Run `aros-romtool --help` for the current command and package
contract.

Package creation is no-clobber by default:

```console
aros-romtool pkg create --output kickstart.pkg exec.library dos.library
```

Replacing an existing package is always an explicit compare-and-swap. Measure
the exact existing file, then supply that digest; a concurrent change makes the
command fail without overwriting it:

```console
aros-romtool pkg create \
  --output kickstart.pkg \
  --replace-if-sha256 "$EXPECTED_SHA256" \
  exec.library dos.library
```

Extraction requires a new destination directory (`--directory`). The complete
flat tree is staged and synced before one no-clobber directory rename exposes
it, so readers see either no destination or every member.

If the directory rename succeeds but the parent-directory `fsync` cannot be
proven, the command reports a publication failure with an uncertain commit
state and leaves the complete extraction in place. It never "rolls back" by
emptying that visible directory. A later run safely cleans an owned recovery
marker; interrupted pre-rename stages are removed only when their marker,
directory identity, portable member set, and per-member digests validate.
Recovery is logged as `publication.recovery` when publication continues.

Mutating publication currently requires Unix `openat`/`O_NOFOLLOW`, exclusive
rename, advisory locks, and directory `fsync`. Creation and extraction fail
closed on Windows until an equivalent native handle-relative, write-through
implementation exists; the tool does not claim durability from a file flush
alone.
