# `aros-collect`

`aros-collect` links AROS relocatable objects and materialises AROS symbol
sets and library-version requirements. The same executable also provides the
released `collect-aros` and `collect-aros32` compiler-driver aliases.

## One collection engine

Both invocation forms are parsed into one engine request and use the same
staging, ELF inspection, symbol-set and library-requirement discovery, linker
script generation, second link, cleanup, diagnostics, logging, and atomic
publication path. The front ends retain only their deliberate policy
differences:

| Behaviour | direct `aros-collect --ld` | `collect-aros[32]` alias |
| --- | --- | --- |
| first-link arguments | preserve the explicit CMake contract | add `-r` when absent |
| empty collection | publish the first pass immediately | retain the reference two-pass flow |
| collector-owned extras and library resupply | disabled | enabled from the explicit sysroot |
| undefined-symbol audit, AROS ABI byte, executable permissions | disabled | enabled |
| report and retained linker-script paths | accepted | not exposed |

Both paths stage beside the requested output. A failed first or second link
therefore leaves an existing good output untouched. Temporary files are
removed unless `COLLECT_AROS_DEBUG` is set; an explicitly requested retained
script is never treated as temporary.

## KOBJ symbol localization

After a collected partial link, the native engine can apply the source
macro's separate KOBJ localization step without Python or a shell pipeline:

```sh
aros-collect --localize-kobj member.o --nm /selected/target-nm \
  --objcopy /selected/target-objcopy
```

Both tools must be selected explicitly. This operation does not link objects
or infer their order. It reads plain `nm` output and reproduces the source's
field-three selection for `__*_LIST__`, `__*_END__` and `__aros_lib*` symbols,
in addition to its nine fixed library bases. `SysBase` is not in that fixed
set. Undefined two-field records and suppressed file symbols are not selected.
Truncated output, unsafe files and failed tools are rejected. Localization
uses a private adjacent copy; the original is replaced only after successful
processing and publication checks. Precommit failures leave the original
unchanged. A post-rename durability failure can leave the new object installed;
the error reports uncertain commit state and retains a recovery journal. This
mode excludes linker arguments,
`--ld`, `--report` and `--keep-script`.

The caller remains responsible for source-derived KOBJ object groups,
archives, linker flags and the selected tools' provenance. A localized object
alone is not evidence of a complete native core build.

## Compiler-driver tool family

The legacy `collect-aros` and `collect-aros32` names use the adjacent
`ld.lld` and `llvm-strip` executables when no tool manifest is present. A
prefixed driver name ending in `-collect-aros` requires an adjacent
`aros-collector-tools.json`; a present manifest also selects the tools for a
legacy name.

The manifest is a regular JSON file of at most 16 KiB. It rejects duplicate
and unknown fields. `invocation` must exactly match the running executable's
filename. `linker` and `strip` are safe single basenames for executable files
in the canonical directory containing that executable. The collector never
finds these tools through `PATH` or `COMPILER_PATH`; the manifest names are
path data, not shell commands.

For example, a GCC target can install the regular `collect-aros` alias beside
GNU `ld` and `strip` and place this file in that same directory:

```json
{
  "schema": "aros-collector-tools-v1",
  "family": "gnu",
  "invocation": "collect-aros",
  "linker": "ld",
  "strip": "strip",
  "emulation": "riscvelf_aros",
  "driver_emulation": "elf32lriscv"
}
```

`family` is `llvm` or `gnu`. GNU manifests require `emulation`; it may contain
only ASCII letters, digits, `_`, and `-`. An optional `driver_emulation` uses
the same character set and is valid only for GNU manifests. GCC may pass this
explicitly declared driver-side emulation to the collector; every occurrence
is removed and the configured linker `emulation` is inserted once for both
link passes. This is an exact manifest declaration, not a guessed target or
ISA-to-emulation mapping. The example values record the observed GCC
`-melf32lriscv` driver selection and GNU AROS `riscvelf_aros` linker selection.
Other user emulation selections fail before linking. Abbreviated GNU long
options, ambiguous operands, and malformed attached `-m` spellings fail
closed so a later option cannot override the configured emulation.

The `family` and tool basenames declare which adjacent siblings to invoke; they
do not prove a binary's version, provenance, or behavior. For each link, the
collector passes the manifest's linker emulation as a separate `-m VALUE`
argument. LLVM manifests may omit `emulation`; if one is supplied, it follows
the same explicit validation and forwarding rules. `driver_emulation` is
rejected for LLVM manifests.

A configured GNU driver follows GNU ld's standard output convention: when no
output option is supplied, the result is `a.out` in the invocation directory.
The collector inserts an explicit output option before linking, so both passes
still write adjacent temporary files and publish only a successful final ELF.
Explicit `-o`/`--output` selections take precedence; empty, missing or duplicate
output values fail before linking. Response files and the `--` operand boundary
retain their normal parsing rules. Direct mode, LLVM manifests and aliases
without a manifest still require an explicit output; tool names or host PATH
contents never enable the GNU default.

## Diagnostics

Human-readable diagnostics are the default. Machine consumers can request one
versioned JSON document on `stderr`:

```text
--diagnostic-format json
```

The document uses the shared `aros-tool-diagnostics-v1` schema. Warnings and a
possible terminal error are collected and rendered together exactly once per
invocation. `stdout` remains available for help, version, and intentional tool
output.

Collector codes are stable command-line API:

| Code | Meaning |
| --- | --- |
| `AC0001` | invalid collector invocation |
| `AC0002` | diagnostics or local logging failure |
| `AC0101` | required tool cannot be resolved |
| `AC0102` | invalid or incomplete AROS sysroot |
| `AC0201` | response-file expansion failure |
| `AC0301` | first relocatable link failure |
| `AC0302` | set-collection link failure |
| `AC0401` | linked-object inspection failure |
| `AC0501` | symbol-set or library-requirement collection issue |
| `AC0502` | collector-required sysroot input is missing |
| `AC0601` | undefined symbols remain after the final link |
| `AC0701` | AROS ELF ABI marking failure |
| `AC0702` | output stripping failure |
| `AC0801` | atomic output publication failure |
| `AC0901` | internal collector invariant failure |

Diagnostics may include typed context such as the tool, link mode, output,
argument index, exit code, signal, and log path. They deliberately exclude
timestamps, host names, and ambient environment snapshots.

## Local logging

Logging is opt-in and never sends telemetry. When enabled, a local file is
mandatory:

```text
--log-level info --log-format human --log-file build/collector.log
--log-level debug --log-format jsonl --log-file build/collector.jsonl
```

Supported levels are `off`, `error`, `warn`, `info`, `debug`, and `trace`.
The default is `off`; specifying only `--log-file` selects `info`. Log files
are opened in append mode. Strictly concurrent builds should select one file
per collector invocation and merge JSONL records afterwards. JSONL records use
the stable `aros-collect-log-v1` schema. Diagnostic events carry the same
stable code and stage as the corresponding human or JSON diagnostic.

Records do not automatically add a timestamp or machine identity. Paths that
are explicit parts of the invocation may still appear. Logs are observational
data and must not be included in byte-deterministic release archives.

Both the direct command and the compiler-driver aliases accept the same
settings. Namespaced spellings such as `--aros-log-file` are also accepted.
Environment equivalents are:

- `AROS_COLLECT_DIAGNOSTIC_FORMAT`
- `AROS_COLLECT_LOG_LEVEL`
- `AROS_COLLECT_LOG_FORMAT`
- `AROS_COLLECT_LOG_FILE`

Command-line settings take precedence. An explicit `--` ends collector-option
processing and preserves every following argument for the linker.
