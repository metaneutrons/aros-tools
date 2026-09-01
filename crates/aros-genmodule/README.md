# `aros-genmodule`

`aros-genmodule` parses AROS module `.conf` declarations and generates the SDK
headers, module sources, link-library sources, interfaces, and varargs glue
required by the CMake build. Generation is deterministic and writes a file
only when its bytes changed, avoiding timestamp-only rebuilds.

The library owns configuration interpretation and generated-source semantics.
Its binary is a thin argument/exit boundary; build scheduling and target
selection remain in CMake and `aros-cli`.

Run `aros-genmodule --help` for the current generated-output contract. Unit
tests cover configuration variants, architecture filtering, collision
handling, stale-output pruning, and generated ABI shapes.

A complete generator run is one durable publication transaction. The shared
publisher serializes writers, retains existing files as sibling inode backups,
records the complete intent in an fsynced journal, and recovers an interrupted
run before scanning the next one. `--output-inc` selects a stable writable build
root: `<build>/SDK/include` uses `<build>`, while `<build>/include` uses its
parent. Optional `--output-gen`, `--output-linklib`, and `--output-libbases`
paths must remain below that root and never change the journal/lock namespace.
Generated path components use a portable case-folded namespace; configuration
names cannot traverse that root and every explicit `conffile`/`confoverride`
source must canonicalize below the scan root.

The transaction is writer-serialised and crash-recoverable, but several file
renames cannot form a live snapshot for uncoordinated readers. During commit a
reader can observe an intermediate set. CMake/build-graph consumers must depend
on the complete genmodule action (or acquire the same persistent lock) before
opening generated files. Recovery is emitted as `publication.recovery` in logs
and included in the human success summary.

A zero-length `.aros-lock-*` file remains in the shared build root by design;
the stable inode closes lock-file replacement races and contains no build data.
