---
title: Physical boards
description: Match a local profile to real hardware, validate boot inputs, and preview deployment before writing.
---

A board profile identifies one physical device. Its **name** is your local
label; its **model**, **backend** and **transport** determine the supported
operations. A profile or successful image build is not proof of a UART boot.

## Implemented models and transports

| Model | Backend | Supported transport | Required core inputs |
| --- | --- | --- | --- |
| `rpi3` (Pi 3B+ DTB contract) | `raspberry-pi` | `native-tftp` | ARM KOBJ triplet and model DTB |
| `rpi4` | `raspberry-pi` | `native-tftp`, `uboot-usb-ecm` | AArch64 KOBJ triplet and model DTB |
| `rpi5` | `raspberry-pi` | `native-tftp` | AArch64 KOBJ triplet and model DTB |
| `milk-v-titan` | `opensbi-uefi` | `uefi-esp` | RISC-V legacy core objects |

The validator rejects USB-ECM on Pi 3/5 and rejects a Pi transport for the
OpenSBI/UEFI backend. Debug transport and power-control fields are metadata;
the CLI does not automate JTAG, SWD or power equipment.

Source: [board schema and validation](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-board/src/config.rs).

The [portable board registry](https://github.com/metaneutrons/aros-tools/blob/main/profiles/boards/README.md)
contains the four stable model IDs used by the CLI and template generator.
Those IDs resolve from the registry embedded in the tools build; the CLI does
not discover an external registry. A local profile name is an alias and stays
separate from the model ID. No ESP32-P4 model or support is advertised.
The `--model` help and generated Bash, Zsh and Fish completions use the same
catalog as the parser.

### Experimental ESP32-P4 integration

Native P4 development is tracked in [RV3](https://github.com/metaneutrons/aros-tools/issues/322).
Local checks cover source-bound contracts, compiler isolation without the old
Make SDK, ESP image inspection and byte-identical partition-table generation.
The internal planner checks source-declared partition types and separate BSP /
Developer erase slots. `aros image prepare` prepares a hash-locked
Python environment and builds a fresh bootloader with controlled vendor tools.
Archive/patch receipts bind prepared source bytes. Vendor tool archives are
checked against the locked IDF manifest and their selected executable trees.
IDF uses a private verified
working copy so its Git index refresh cannot mutate those inputs. Independent
image checks pass; local receipts are not release provenance.

Native SDK header generation follows the selected source rules, including
host-C generators and GENMODULE recipes. GUI selection belongs to the source profile;
there is no ESP32-specific MUI switch in Rust. Required headers may still be
generated for nongraphical builds.
Bounded source-text recipes also preserve ordered header edits and generated
cross-configuration scripts. Both FreeType products pass local byte comparisons
against the source's GNU sed rules; this is not a complete native P4 build.
Finite SDK file copies preserve the exact source file list and bytes. The
Codesets FD copy passes local staging, missing-output repair and rebuild probes.
Developer-library copies use only the configured library root and regular local
inputs. Source Make defaults and finite file lists are retained; private CMake
recipes bind the input hashes. These bounded capabilities do not qualify the
complete P4 SDK.
Full resource macros also select public GENMODULE ABI production; runtime-only
macros do not. A full declaration can retain its source-bound header generation
when its runtime compiler capability is unsupported. That projection creates
no runtime module or client archive; selecting either still fails. The native
graph remains incomplete.
Finite static-pattern header copies preserve nested paths and their single
source-declared include destination. Directory lists follow the recipe's Make
variable state; local include-root redirection is rejected.
Named host-header aggregates retain all declared outputs and only explicit
include mirrors. Incremental builds recheck their paths and sealed recipes;
an unchanged valid build does not rewrite the headers.
Handwritten SFDC header recipes retain their source-declared mode, target,
input hash and SDK output. The private host compiler and actual CIA headers
match direct reference generation. Input drift, unsafe paths and a successful
process without a new output are rejected. This is header evidence, not CIA
runtime or complete P4 qualification.
Optional MetaMake selector edges must be declared in the source contract and
proven in their hash-bound recipe. Only absent providers are omitted and
recorded. Literal dependencies, unknown selectors and rejected real producers
still stop native selection.
The source-owned MetaMake policy can also select the supported `module`,
`linklib` and `set-archincludes` template-hook families. Each omission requires
the sealed macro definition and exact source callsite, not a target-name
allowlist. The base `module` family includes the reviewed module, archive and
program callers; it does not admit their quick or other suffix-specific hooks.
Physical architecture objects and include-flag files retain their
own producers and prerequisites; a missing source implementation is not an
optional hook. Unsupported declarations retain source-line diagnostics.
Continued `#MM` declarations use the same grammar for parsing and source
verification; their required prerequisites are preserved.
Architecture effects use the profile's explicit Make configuration and replay
with the same captured values. CLI selectors must match the source profile;
a declared Mesa version cannot be overridden. Unknown feature switches remain
errors, not implicit defaults. A verified physical architecture producer can have an
optional generated variant hook; the producer and its required edges remain.
KOBJ integration preserves the source macro form and separates partial linking
from GNU symbol localization. Source groups retain Make's architecture-object
ordering, including fetched-source proxies, and copy the module's compilation
state. The CMake partial-link helper matches a separate real RV32 link and GNU
localization recipe byte-for-byte. Declaration-scoped extra objects, libraries
and linker flags retain ordered metadata; unknown inputs cannot be consumed as
empty lists. The native core consumes registered collected/localized KOBJ
intermediates, not raw module objects. Its source binding also retains the macro
libraries, global partial-link values and instrumentation selector. Missing
configuration proof and unsupported instrumentation stop configuration. Global
configuration and a fresh P4 core remain unqualified. These are host-only
contracts, not a complete P4 build.
An empty module `uselibs` list can leave the literal prefix `linklibs-` in
GenMF metadata. Native selection normalizes this only with an exact sealed
module-template definition, an empty resolved argument and matching callsite
provenance for every claim. Handwritten collisions, independent required
edges, declared endpoints and nonempty or unresolved lists remain required.
Imported metadata records provenance per dependency: another recipe declaring
a different prerequisite on the same target does not become its origin.
Classic MetaMake's general tolerance of missing targets is not reproduced.
`uselibs` names library interfaces, not necessarily Make targets. A generated
prerequisite can bind to a different archive target only when its sealed
callsite, typed consumer and unique source-owned `%build_linklib` agree.
Handwritten dependencies, name collisions and ambiguous providers still fail.

The source contract's optional `make_variables` object supplies literal
configuration defaults. An explicit empty value is known; an omitted variable
remains unknown. Local Make assignments can replace these defaults. Unknown
conditional writes still prevent publishing a partial native graph.
Uninstantiated GNU `define` bodies cannot supply live assignments. An opaque
definition also prevents reuse of an older value or configuration default.
Architecture declarations use bounded Make expressions and source-bound
include scopes at their declaration positions. They preserve ordered include
paths and `-D` definitions; filesystem enumeration is not allowed in this
proof. A configured value with an unknown Make assignment flavor cannot
justify a later append.

A source-defined C-to-assembly header pipeline requires exact compiler-role,
sysroot, flags and include metadata. Its generated file is not producer proof.
Unsupported recipes remain diagnostics; native selection does not replace
them with an empty target or borrow a bootstrap output. The current P4 source
profile is not yet sufficient to qualify that complete pipeline.

Local compiler admission uses byte-verified metadata, not release provenance.
CLI and CMake admission are tested with a real RV32 compiler; they do not
qualify a P4 payload. SDK-text generation binds its source recipe and preserves
first-match substitutions and line endings; changed recipes stop the build.

Local native builds record source, compiler, engine, executable and configure
input hashes before execution. Dirty edits and initialized submodule files are
included. Changed inputs or cached Ninja rules stop reuse; unstamped historical
trees are refused. This stamp is not a successful-build or media receipt.

A native source contract may explicitly declare a disabled MetaMake metadata
owner. Its comment and dependency must be in the same hash-verified recipe.
The translator reports absent metadata without enabling the commented rule;
active producers and other missing dependencies retain their normal checks.

The device-free external preparation uses an explicit native source preset,
closed local input document, positive jobs/deadline and fresh private work root.
See [native media inputs](/aros-tools/reference/native-media-inputs/) for its
command and byte-lock contract. It is not exposed as a physical P4 board model.

Still open: fresh native kernel/BSP/Developer builds, reviewed external-input
provenance, complete flash-artifact composition and the full public workflow. Older Make outputs,
container checks and host-only tests do not qualify device readiness or flashing.

## Create and diagnose a profile

For a **Pi 4 USB-ECM** profile, preview the generated template:

```sh
aros board init --profile rpi4-usb --model rpi4 --transport uboot-usb-ecm
```

Then create a new config file explicitly:

```sh
aros board init --profile rpi4-usb --model rpi4 --transport uboot-usb-ecm --apply
aros board scan
```

:::note[Model and transport are explicit]
`--profile` is only your local label. `--model` is required and selects the
hardware contract; it is never inferred from the label. `--transport` is
optional only where the model has one conservative default: Pi 3, Pi 4 and Pi
5 default to `native-tftp`; Milk-V Titan defaults to `uefi-esp`. Pi 4 USB-ECM
must be selected explicitly, as shown above.
:::

The generated build defaults come from the embedded reviewed registry. The
example file at
[support/rpi-debug/boards.example.toml](https://github.com/metaneutrons/aros-tools/blob/main/support/rpi-debug/boards.example.toml)
is a legacy local-profile example; it is not discovered as a model registry.
Network addresses and MAC values in generated or example profiles are
placeholders. Adapt them to the local network and the actual device identity;
do not infer network or device values from a model ID. Unsupported
model/transport pairs fail before a file is written. For example, Pi 3 and Pi
5 cannot select USB-ECM, and Titan cannot select a Pi TFTP path.

Edit the printed file (normally `~/.config/aros/boards.toml`) with the real
paths, interfaces, serial device and device identity. Use a matching target
preset declared by your selected AROS checkout. The built-in four toolchain
profiles are not the example registry's board-specific debug presets.

For Pi 4 USB-ECM, copy the stable USB descriptor values from `board scan`;
do not persist a guessed dynamic interface name. Then, inside the AROS tree:

```sh
aros board doctor --profile rpi4-usb
```

## Build, deploy and serve

With the profile's exact DTB and legacy core objects present:

```sh
aros board build --profile rpi4-usb
aros board deploy --profile rpi4-usb
aros board deploy --profile rpi4-usb --apply
aros board serve --profile rpi4-usb --dry-run
aros board serve --profile rpi4-usb
```

Deploy previews by default; `--apply` stages the bundle to the configured
TFTP destination. The configured `tftp_prefix` must resolve through real
directories below `tftp_root`; symbolic links in that path are rejected.
Serve binds restricted DHCP and read-only TFTP to the validated interface and
board identity. The host address must already be configured. These network
commands are not the Milk-V UEFI-ESP boot path.

Open a serial console separately:

```sh
aros board console --profile rpi4-usb --dry-run
aros board console --profile rpi4-usb --program picocom
```

Supported terminal programs are `picocom`, `screen` and `minicom`.
Auto mode searches in that order. Save and interpret the resulting UART
evidence using your board's bring-up procedure.

## SD-card safety sequence

Image creation needs an external `boot-bundle.toml` plus its hash-declared
firmware and artifact inputs. A build directory alone is not a boot bundle.
Use the prepared bundle for the exact model/transport. The reviewed Pi 4
U-Boot USB-ECM and Milk-V Titan UEFI file layouts are defined in the
[media-profile registry](https://github.com/metaneutrons/aros-tools/tree/main/profiles/media);
the local board name does not select a different layout. Native Pi SD and PC
ISO profiles are not yet available through this SD command.

```sh
aros board sd image --profile rpi4-usb --boot-bundle /verified/bundle --output /new/artifact
aros board sd image --profile rpi4-usb --boot-bundle /verified/bundle --output /new/artifact --apply
aros board sd scan --artifact /new/artifact
```

If the intended removable disk is mounted, inspect and explicitly unmount it:

```sh
aros board sd unmount
aros board sd unmount --device SCAN_ID --apply
```

Run `sd scan --artifact` again after unmounting. Substitute its exact scan
ID and confirmation token in the write sequence:

```sh
aros board sd write --profile rpi4-usb --artifact /new/artifact --device SCAN_ID --dry-run
aros board sd write --profile rpi4-usb --artifact /new/artifact --device SCAN_ID --confirm TOKEN
```

The final command writes the selected medium. Candidates must be whole,
removable, writable and unmounted. Raw device paths are rejected; the token
binds the measured image and device identity. The writer performs read-back
verification. SD image creation uses the implemented MBR/FAT32 format;
it is not a general disk-partitioning frontend.

## Evidence still required

For a physical-boot claim, record the model, firmware, source commit,
cross-toolchain identity, legacy core inputs, artifact digests and UART log.
Do not infer boot support from an available profile or compiler.

The detailed [Raspberry Pi lab guide](https://github.com/metaneutrons/aros-tools/blob/main/support/rpi-debug/README.md)
covers the prepared firmware and external-debugger workflow.
