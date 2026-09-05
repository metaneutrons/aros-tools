# Moving the CMake engine out of the source tree

## Why

The engine is the set of CMake modules a transpiled target graph calls into. It
lived in the AROS source tree, but it does not describe a tree, it describes how
any tree is built. The transpiler emits calls into it, so the generated graph
and these modules are two halves of one contract, and that contract was split
across two repositories with a lock between them: a signature change needed two
synchronised commits.

Carrying the engine here also removes the reason the CMake product flow cannot
run against a pristine upstream checkout. A checkout that never heard of our
build system cannot be expected to hold it.

## What is done

**The engine is a crate.** `aros-cmake-engine` embeds all 160 files through a
build script that walks them into one sorted table and pins a SHA-256 over path,
length and content. `materialize()` places them in a build directory and makes
that directory hold exactly the engine, removing anything foreign, because a
module left by an older engine stays visible to `include()`. A stamp records the
digest so an unchanged directory is reused, and the stamp is verified against the
files it names.

**The API version has one source.** `engine/EngineVersion.cmake` declares it and
the build script reads it, so the Rust constant and the CMake declaration cannot
drift.

**The tree is an input.** `AROS_SOURCE_DIR` names the AROS checkout,
`AROS_CMAKE_ENGINE_DIR` names the engine, and `CMAKE_SOURCE_DIR` keeps only its
own meaning: the project being configured. It survives in the four places that
walk `BUILDSYSTEM_TARGETS`, which is CMake's directory structure rather than the
filesystem. Three root checks needed more than a rename, because they resolved an
input against the tree and then tested it against the project root; that is the
same directory today and would have rejected every tree input tomorrow.

The transpiler emitted the old name in 151 places. `SRCDIR` always meant the
tree, so they moved together.

**Proved, not assumed.** `cmake -S <engine> -B <build> -DAROS_SOURCE_DIR=<tree>`
configures from outside a checkout and writes 77,456 targets. The generated graph
holds 1,484 `AROS_SOURCE_DIR` references and no `CMAKE_SOURCE_DIR`, and the
source tree ends the run with no modified paths.

## The CLI configures from the engine

`aros build` no longer names a preset. It places the embedded engine in
`<build>/cmake-engine` and configures `-S <engine> -B <build>
-DAROS_SOURCE_DIR=<checkout>`, so nothing is written into the checkout.

The ten cache variables a preset carried are derived from the target profile,
because none is a free choice: the system name is fixed for a bare-metal target,
the processor is the architecture, the compilers are the LLVM path the target
graph assumes, and the bootloader follows the platform. `bootloader` became
profile data with that rule as its default. Build type and board model, the only
other things the debug presets varied, are `--debug` and the model the board
configuration already holds.

`--engine-dir` replaces the embedded engine and is honoured only when given.
An engine lying in the checkout is never preferred on its own. `aros info`
reports the digest, file count and API version in use, and the generated graph
opens with `aros_require_engine_api_version(N)` so a mismatch is named rather
than surfacing as an unknown function.

**Measured**: in an isolated AROS-NX worktree at
`9f15d22ee680247db490af1ca20a7994f7152509`, using the locked
`toolchain-v1-20260831-rc3` macOS/AArch64 `pc-x86_64` payload,
`aros build --preset pc-x86_64 --target kernel-exec --jobs 12 --clean`
configures 1,235 MetaMake files into 923 concrete targets and completes all
3,291 Ninja steps. `SYS/boot/pc/Libs/exec.library` links successfully. A second
non-clean invocation also completes successfully.

AROS-NX PR #28 removes the 159 files from that tree.

## Qualification and remaining boundary

**Historical fixture baseline.** All 35 engine tests passed against a source
checkout that still retained its old `cmake/` directory. That result did not
prove independence from every source-side engine resource; the complete
no-engine product run below exposed the remaining manifest dependency.

Sixteen places had been loading engine modules through the source tree, so they
were measuring the engine in the checkout while reporting on this one. They
passed only because AROS-NX still carries a copy. `AROS_TEST_ENGINE_DIR` now
names the engine beside the tests, and every driver passes it down with the tree
and the tool paths.

`FetchArchivePatchTest` generates its own CMakeLists and stubs BootstrapSDK
beside it, which worked while `CMAKE_SOURCE_DIR` named the fixture; it now puts
its own directory first in `CMAKE_MODULE_PATH` and declares itself as its
`AROS_SOURCE_DIR`, because the patch it tests lives beside that generated file.
`AhiBuildTest` runs on mock tools, so it receives only the engine location:
handing a mock fixture real executable paths makes it build for real.

Every child that test starts now has a three-minute bound. A wedged tool used to
hang the whole sweep, and a hang reports nothing while blocking everything behind
it. The slowest healthy run measured here is 22 seconds.

**`AhiBuildTest` found a real defect, and it was not where the symptom pointed.**
The test hung, so the first reading was a hang in the runner. Two things were
wrong, one in the test's own infrastructure and one in the runner.

The mock make knew only `install`. The runner had been changed to compile with
`make all` while configure's logical prefix is authoritative and then install
into a private staging root, and the mock delegated everything that was not
`install` to the real make -- which ran the real AHI build under a mock
compiler and did not converge. That was the hang. `--version` and
`gcc-include` still go to the real make, the first because the engine probes
for GNU make and the second because it generates the three headers the runner
inspects from checked-in SFD descriptions and needs no compiler.

With the hang gone the actual defect surfaced in seconds.
`measure_live_set` classified the installed product set as wholly absent or
wholly present and rejected anything between as "partial; refusing a mixed
replacement". A deleted product is the ordinary reason a build runs again, so
that refusal left the only repair as deleting the rest by hand. The snapshot now
records absence per product. Publication never distinguished the two cases: the
snapshot exists to be measured again before and after the commit, so a
concurrent writer cannot slip in, and recording absence keeps that comparison
exact. The test that pinned the old behaviour is replaced by the guarantee that
matters -- a partial set is repaired, and a failed install still leaves an
incomplete set untouched.

`AhiBuildTest` passes in 22 seconds, matching the pre-move measurement.

**The local Darwin/arm64 sweep is 35 of 35.** `GrubBuildTest` builds the real PC,
EFI64 and EFI32 host-tool lanes. It downloads grub-2.12 from the canonical GNU
origin with the GNU mirror redirector as a fallback; both origins share the
same required SHA-256 identity. The canonical exact-source workspace gate now
discovers every `*Test.cmake` file and executes every host-compatible fixture.
The GRUB test is reported as an explicit host-qualified omission elsewhere, so
the suite remains a visible CI contract rather than a manual list.

**Built-in profiles are complete.** The four current target profiles and the
host LLVM declaration are embedded in `aros-common`, including explicit
MetaMake selectors. An absent checkout file selects them without writing into
the tree. An existing file remains an authoritative complete override and a
malformed or unreadable override fails closed. Both `aros info` and isolated
`source sync` qualification cover the pristine-checkout path.

**The nine AF0501 fetch failures are closed at their owner.** Configure-time
source-inventory fetching formerly wrote `.<archive>-fetched` inside the source
tree whose contents are protected by the `aros-fetch` receipt. The first Ninja
invocation therefore correctly saw an external mutation, attempted a safe
replacement, and then correctly refused to clobber the existing source. The
engine now stores completion state below `CMakeFiles/aros-fetch`, outside the
payload. It removes only its exact obsolete in-payload marker when upgrading an
old build tree; the fetcher's receipt and no-clobber rules remain unchanged.
`FetchArchivePatchTest` covers clean fetch, legacy migration, patch refresh and
final Ninja no-op, while `SourceInventoryReconfigureTest` covers the
configure-time path.

## Product manifests belong to the selected engine

AROS-NX PR #28's fresh matrix
[33995179765](https://github.com/metaneutrons/AROS-NX/actions/runs/33995179765)
passed the relocated companion-header test on all six Linux lanes, then failed
at AHI configuration after the old source-side engine was removed. Both native
Linux hosts reproduced `AHI: audited source or manifest is unavailable`.

The remaining AHI and GRUB product manifests were still addressed through
`<source>/cmake/manifests`; the AHI runner also required that obsolete identity.
The selected CMake modules and GRUB runners now resolve their own bundled
manifests. The AHI contract names `AHI_ENGINE_ROOT` separately from
`AHI_SOURCE_ROOT`, with exact mode-specific identity, digest and ownership
checks. A missing engine manifest does not fall back to a source copy. Older
generated AHI contracts must be regenerated with the matching tools suite.

AHI, GRUB host-build and GRUB ISO fixtures stage only their real required
upstream inputs into isolated source trees without `cmake/`. The suite also
rejects missing manifests, symlinked files and symlinked manifest directories;
Rust tests reject old source-side, wrong-engine and wrong-mode substitutions.
`ArosToolsTest` now resolves its module from the selected engine as well.

The local macOS ARM64 exact-source gate passed all locked Rust tests and all
35 engine fixtures, without host-qualified omissions, against
`f3cfc243a84065166a46da28b0a5b22bbd0f8869`. The locked workspace release build,
quality gate and documentation gate also passed. These local contract tests
do not substitute for the real cross-host product matrix.

The corrected engine and runner must pass the tools repository's normal
qualification before AROS-NX #28 updates its tools pin. Its fresh complete
four-host/three-profile product matrix remains a merge gate; the cancelled
matrix above is failure evidence, not acceptance.

A complete translated product from an entirely pristine
upstream checkout still needs the source-side compatibility changes and an
explicit released-toolchain selection currently carried by AROS-NX. Component
tools, checkout lifecycle and target-graph validation remain usable without
copying tools-owned files into upstream AROS.
