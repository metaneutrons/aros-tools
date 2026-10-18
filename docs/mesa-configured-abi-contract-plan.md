# Configured Mesa compile and ABI contract

Tracking: [issue #353](https://github.com/metaneutrons/aros-tools/issues/353).
This work targets the release after 0.6.1. It does not change that release
candidate or close the separate provider/driver qualification in issue #254.

## Required outcome

Make-built and engine-built Mesa 26 modules for the same source configuration
must use equivalent compiled contracts and the same GalliumCoreAPI ABI string.
The issue's complete original scope remains required, including ARM/AArch64
equality tests and investigation of the supported Mesa 20 path. A matching hash
without a matching compiled contract is not acceptance.

## Measured input boundaries

The inspected source baseline is
`636f85ab8256983f9470ce92236abdbb303a2b62`; the initial Tools baseline is
`feb0d84183ed26d7adf5288b2ec571ab15cccd5f`.

- `Makefile.in` invokes `config/features` with the selected target compiler and
  sysroot. `config/features.in` tests individual `-fno-builtin-*` options;
  `config/compiler.cfg.in` exports the results. These are not host-compiler
  facts, and missing results cannot be treated as known-empty results.
- `configure.in` and `config/target.cfg.in` produce target ISA configuration.
  The selected CPU, floating-point ABI, endian and configuration options matter.
  The synthetic ISA strings in the prerequisite tests are not configured data.
- `workbench/libs/mesa/mesa.cfg` consumes eleven builtin selectors in source
  order, then constructs `MESA_COMPILEFLAGS` and `USER_CPPFLAGS`.
- The VC4 mmakefile appends its own flags and immediately assigns
  `GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)`. Its generator receives
  `$(strip $(GCA_ABI_FLAGS))`. Later assignments cannot change that frozen value.

The existing native contract scalar map is not a suitable transport: it rejects
whitespace and `=`, and its compiler/ISA validator is currently GNU/RISC-V
specific. Do not loosen that validation to smuggle ARM flag strings through it.
The developer transpiler path currently has no authoritative configured-value
handoff either.

## Implementation boundaries

1. Establish a typed configured-input handoff for the selected compiler and
   source configuration. Its producer must obtain actual configured/probed
   values, not infer success from an architecture name or compiler brand.
   Bind the result to the verified compiler identity and concrete invocation,
   target profile/settings, and reviewed source inputs. Reject stale or missing
   bindings before graph publication. The exact producer/API is still under
   design; this document does not authorize an unvalidated input escape hatch.
2. Evaluate reviewed source fragments with explicit known/empty input values.
   Preserve assignment timing, flag order, duplicates, definitions, undefinitions
   and compiler options. Unsupported expressions and unresolved conditions must
   reject the complete value; the legacy partial/deduplicating flag collector is
   not an ABI serializer.
3. Project this ordered contract into actual compilation and into the generator.
   Keep the existing own-compiled-flags hash until both paths can switch together.
   Do not set the Make-derived hash while retaining different static options.
4. Compare against GNU Make under the same measured configuration on ARM and
   AArch64, including the generated ABI hash, then investigate Mesa 20 using its
   own supported source/version contract. Preserve reviewed recipe fingerprints.

## Development and acceptance evidence

The prerequisite test `mesa_make_configuration_probe` has three passing tests:
all eleven known-empty/nonempty/missing selectors; immediate-assignment freezing
with ordered duplicate `-D`/`-U` inputs; and rejection of unknown conditions and
unsupported functions. These deterministic fixtures do not prove configured
compiler provenance, production serialization, actual compile flags or ABI parity.

Further focused positive/counter tests must cover changed compiler identity,
changed source/configuration, missing selectors, known-empty selectors, reordered
or duplicate options, stale snapshots, altered recipes and independent Make/hash
comparison. Relevant supported native integration follows the completed code;
do not launch expensive stale-base matrices for partial scaffolding.

No compiler/SDK diagnostic dispatch, publication, scope reduction or issue closure
is authorized by this plan. Actual implementation and qualification state belongs
in issue #353.
