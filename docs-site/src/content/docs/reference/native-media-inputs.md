---
title: Native media inputs
description: Explicit offline inputs for experimental source-selected external media preparation.
---

`aros image prepare` builds external media inputs, not the AROS kernel, BSP or
Developer volume. Its current provider is `esp-idf-bootloader-v1`. Preset,
chip, silicon interval, flash geometry and partition CSV come from the selected
source's inventoried native contract. There are no board-name defaults.

## Invocation

```sh
aros image prepare --preset esp32p4-d1001 \
  --source-root /work/AROS-P4 \
  --inputs /work/p4-media-inputs.json --inputs-sha256 REVIEWED_SHA256 \
  --work-parent /work/private-media-runs \
  --jobs 12 --timeout-seconds 900 --format json
```

The parent must already be a private directory, separate from every input.
Every attempt creates a fresh retained child. This command is always offline;
prepare and review source archives, patches, vendor tools and wheels separately.
It does not download, install host packages, use an ambient IDF environment,
replace an existing artifact or write a device. The deadline includes input
verification. Ctrl-C cancels supervised children and is checked between local
input checks. A single bounded filesystem/tree measurement is not interruptible;
cancel or timeout can therefore wait for that measurement to finish before
refusing the next phase. No producer starts after an expired preflight budget.

## Closed input document

All fields below are required; unknown fields and unsafe paths are rejected.
SHA-256 values are explicit 64-character digests, never inferred from a missing
pin. All paths are absolute and must not contain parent traversal.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` |
| `format` | `aros-native-media-inputs-v1` |
| `qualification` | `local-byte-lock-only` |
| `provider` | `esp-idf-bootloader-v1` |
| `target_profiles_sha256` | Exact raw source `aros-targets.toml` digest |
| `native_contract_sha256` | Exact raw native contract selected by that preset |
| `wheel_lock`, `idf_lock` | Objects with regular-file `path` and raw `sha256` |
| `interpreter`, `runtime_prefix` | Exact CPython executable and measured prefix |
| `wheel_cache` | Prepared directory containing every exact locked wheel |
| `idf_source`, `compiler_source`, `cmake_source`, `ninja_source` | Prepared archive receipt declarations below |
| `idf_root`, `compiler_root`, `cmake_root` | Exact extracted prefixes within their bound source trees |
| `cmake`, `ninja`, `git` | Explicit measured executables; no PATH discovery |
| `constraints` | Exact local Python constraints file bound by the IDF lock |

Selected CMake, Ninja and Git executables must be regular executable files, not
symlinks. Use their explicit canonical paths rather than package-manager aliases.

Each prepared source object has `destination`, `archive` (basename without
suffix), `suffix`, archive `sha256` and ordered `patches`. Each patch has `name`,
nullable relative `subdirectory`, `options` and `sha256`. Only the fetch parser's
closed patch options are accepted. These declarations must match an existing
`.aros-fetch` receipt and the live tree. They neither fetch nor authenticate an
archive origin. Vendor compiler/CMake/Ninja archives must be unpatched and match
the unique recommended host entries in the locked IDF `tools/tools.json`.

The wheel document is `aros-python-wheels-v1`: it binds host, CPython executable
and prefix digests, the pip bootstrap filename and exact wheel
name/version/filename/SHA-256/size entries. A fresh environment is built from
these bytes, without dependency resolution or network access. The
`aros-idf-bootloader-v1` document binds the IDF version, compiler prefix, source
receipts, IDF/compiler/CMake trees, Ninja/Git executables, requirements,
constraints and wheel-document digest. Both use schema version `1` and
`local-byte-lock-only` qualification. They are local consistency locks, not
release provenance. The typed schemas are maintained in
[`aros-toolchain`](https://github.com/metaneutrons/aros-tools/tree/main/crates/aros-toolchain/src).

A CPython prefix mismatch reports the expected and measured whole-tree digests.
Preserve the original lock and inspect or restore the runtime before preparing
new inputs; the command never updates a mismatched pin automatically.

## Native build configuration

An experimental source-native build contract may declare `make_include_bindings`:
a source-relative original include mapped to an inventoried source-relative
`.mk` fragment. A self-binding reads the inventoried original; a different
path selects a source-native projection. Both endpoints must match the
contract's hashes. The fragment is evaluated at the include's position; later source `make.opts` assignments
retain their order. It is not an ignored include or an ambient Make configuration.
Unmapped includes and unresolved values still refuse KOBJ consumption.

`generated_make_templates` reconstructs a generated Make include from a sealed
`.in` template, its literal `AC_CONFIG_FILES` declaration and explicit profile
substitutions. It never reads the generated file. Only plain assignments and
an optional leading `%common` marker are supported; quotes are preserved.
Duplicate fields, changed input hashes, target-selector overrides and overlapping
paths are rejected. The declaration check does not execute Autoconf or evaluate
its shell conditions; substitution values come from the source contract.

The P4 projection selects source-default KOBJ flags and disables function
instrumentation. Generated global `make.opts`/`make.defaults`, environment
overrides and Make-built outputs are not imported. This remains experimental;
configuration binding alone does not qualify the complete kernel or BSP.

Native SDK objects must have source-declared compile and staging rules.
The supported subset is a finite ordinary Make aggregate, local C/C++ sources,
`%rule_compile`/`%rule_compile_cxx`, and an exact object-copy pattern.
Declaration-time flags and generated-header prerequisites are retained.
CMake produces private objects; it does not adopt existing Make objects.
Unsupported recipes or conflicting output owners stop translation.

SDK file assets support finite source-declared copies under `Developer/bin`
and literal one-line manpage stubs under `Developer/man/man1`. Each copied
input must bind to one concrete file producer for the selected architecture.
Existing files and similarly named targets are not producer evidence.
Ambiguous producers, cycles, overlapping roots and symlink paths are rejected.
The engine also rejects executable output paths changed after registration.

Literal compile-only rules are separate from SDK object staging. A finite
ordinary Make aggregate may own local C/C++ inputs and explicit generated
object outputs. The native compiler command preserves the source's ordered
arguments, including repeated flag lanes; it does not inject CMake global
flags. Bound configuration supplies the source-owned flags and fresh native
Developer sysroot. Unknown flags, unmodelled commands and unsafe paths fail.

Archive, HIDD-compile and layered-header projections may appear in the graph
audit before their full producer contracts are implemented. These partial
records do not admit targets. Header recipes with unevaluated conditional
commands are not treated as simple copies.

GenMF text expansion, MetaMake owner selection and dependency-only Make
overlays are separate evidence components. They do not waive unsupported
rules or prove compile producers. Object/depfile identities and normal versus
order-only prerequisites must be bound before native graph admission.

Source archives accept `%mklib_q from=$^` or an explicit finite `from=`
expression that exactly matches the declared prerequisites. The output needs
one ordinary `#MM` owner and a verified archive command. Additional rules for
that output, unresolved target aliases and ambiguous paths are rejected.
Known one-stem target patterns must be provably disjoint from the archive;
the check preserves GNU Make's directory-stripping semantics.
Explicit expressions also require stable variable origins: ambient `?=`
assignments, later overrides and unproved GenMF or Make effects are refused.
Recognizing the archive recipe does not prove its object producers.

With a sealed MetaMake invocation policy, native selection preserves reachable
GenMF virtual aliases and their exact source dependencies. Aliases are metadata
routes, not executable producers. Existing real providers remain mandatory.
An already-known native aggregate cannot hide additional source virtual edges:
they are unioned as prerequisites, including uncontracted required edges.
The source-owned `optional_meta_dependencies` contract is the explicit authority
for optional architecture hooks: each `Selector` record must match its captured
recipe, original selector expression and bound native edge. A selector spelling
alone grants no omission. Absent selector leaves below a contracted virtual
route can be omitted only when every declaration agrees and no independent
required path reaches that route. Literal collisions, native prerequisites,
known providers and rejected capabilities cannot borrow another edge's proof.
The audit and invocation sidecar record `source_meta_semantics`: imported
aliases, verified contracts and exact omitted hook edges.

Without that sealed proof, active dependencies remain required. A commented-out
owner never establishes an optional edge; `DisabledOwner` declarations cannot
enable it or waive its consumers. Missing required endpoints fail export and
source preparation with the declaring recipe and a `fix upstream` hint.
Classic MetaMake's tolerance of unknown targets is not an absence proof.
Unsupported translation capabilities are reported separately and do not, by
themselves, imply an upstream source defect.
Rejected rules need an exact source consumer chain to establish a MetaMake
owner. Unmodeled includes (including optional generated `.d` files) and unknown
conditional consumers keep ownership unresolved; a path alone is not proof.

Native owner projection and parsing consume the same recipe inputs:
`mmakefile.src` takes precedence over a generated `mmakefile`; direct fragments
remain inputs. Before admission, the tool rechecks recipe discovery and captured
bytes against the source-owned ignore policy. A diagnostic graph audit retains
source failures and sibling evidence but publishes no build graph or inventory.
When a sealed MetaMake policy proves the complete invocation, unowned parser
capability failures from uninvoked recipes are recorded separately. Selection
uses all actual parser origins, not architecture names or displayed paths.
The protected inputs include both the classic source-owner traversal and every
required native producer, including implicit link dependencies. Unbound native
producer origins prevent all capability exclusions. Every failure from a
selected recipe remains checked. Missing source endpoints,
unknown origins and global failures cannot be waived by this scope proof.
The `*.native-invocation.json` sidecar records successful export/preparation scope;
excluded capabilities remain unsupported, including unsealed `.d` includes.

Directory-only producers support finite `%mkdirs_q` recipes and explicit
`%rule_makedirs dirs=... setuptarget=...` declarations. They create directories
under the configured generated, include or Developer-library roots; they do
not execute source-provided shell commands. Unresolved conditions, root
overrides and paths crossing symlinks are rejected.

Named handwritten `GENMODULE writefiles` stamps use the same manifest-backed
writer as normal and relative client archives. CMake tracks concrete generated
sources and private headers; a missing output reruns the writer. Configuration
paths must remain regular and source-confined, without symlink components.
Shell additions, unresolved conditions and conflicting selected owners refuse
translation. This capability does not import Make-generated stubs or qualify a
complete native payload.

Source contracts may also declare `host_file_generators`: an inventoried C
utility, its matching Make recipes, a generated-file owner and exact raw
inputs. Each input has a versioned HTTPS URL, SHA-256 and size. The CLI prepares
the verified input cache; offline builds require those bytes to be present.
The transpiler rejects declarations that do not match the source recipes.
CMake compiles the host utility and generates the file locally, without
running Make or fetching inputs. This is experimental build support, not a
qualification of the complete kernel, BSP or Developer volume.

## Result and limitations

The JSON result and retained `preparation.receipt.json` bind the input document,
source selector, media geometry, isolated Python receipt, bootloader receipt and
verified bootloader/table bytes. The private IDF execution copy preserves the
prepared source; logs and failed attempts remain available for diagnosis.
Native AROS output roles are not invented: `complete_flash_plan` is `false`.

Wrong chip/revision/capacity, altered checksums, mismatched source inventories,
overlapping slots or changed receipts fail. Verification does not prove secure
boot, a complete P4 native build, physical boot or permission to flash. Full
native core/BSP/Developer integration remains experimental.
