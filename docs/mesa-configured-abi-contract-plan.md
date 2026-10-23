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

Measured undefinedness is different from a defined-empty global variable.
The closed input must explicitly retain any such observation required by the
source scope. Preflight checks its GNU Make origin; the consumer preserves
undefined history so a later source `?=` still applies. This does not authorize
turning arbitrary missing inputs into empty values. Optional generated includes
also need their producer and lifecycle bound; unknown active includes still
reject. The local Source correction removes five obsolete unversioned Mesa
entrypoint imports and conditions. The original versioned generator does not
produce that path or variable, and no active Mesa section-GC link uses the
anchors. Common `MESA_COMPILEOPTIONS` is explicitly empty, preserving clean-build
flags rather than adding unconditional function-section options. GenMF/GNU Make
regressions cover stale-file independence and restored-old-rule counters; they
do not qualify complete Mesa 20/26 builds. Only the five affected whole-file
Mesa 26 capability fingerprints change after review; legacy Mesa 20 and SSE4.1
recipe/context admission remains unchanged pending its separate investigation.

The local unreleased transport additionally requires `llvm_configuration` with
separate `OPT_LLVM` and `TARGET_LLVM_VER` states: undefined, defined-empty or a
canonical bounded version. There is no default or migration of old local JSON.
This transport is absent from the current main tree; version 1 has not been
published as a supported persistent interface. Producer measurements retain
definedness and framed values, then compare the complete normalized Make map
with the validated transport before retaining its receipt. Only the private
source-evaluation context receives measured LLVM values; a missing frontend
provider selection must not enable a target-LLVM build globally.

The two reviewed LLVM source wrappers use one exact shell-printf expression to
format their version. The local evaluator interprets only that literal idiom,
in positional active scope, without executing a shell. Canonical decimal byte
components avoid printf octal/signed conversion differences. Recipe/define
forms, duplicate or changed expressions, custom recipe prefixes and unresolved
inputs reject. Literal Make error assertions are accepted only when their
branch is proved inactive; active/unknown assertions and opaque effects refuse.
Actual producer and complete graph execution still require fresh evidence.

Configured option projection materializes only the trusted deferred
`${AROS_BUILD_DIR}` token after positional Make evaluation. The selected
canonical CMake build root is used, never the private configure-owner root.
Other residual dollar expressions refuse; source-data escaped dollars refuse
before the shared collector can erase their provenance. The generator's
literal-option safety check remains unchanged. Order and duplicates survive;
the separately evaluated GCA string is not rewritten. This uses the existing
engine layout mapping (`SYS/Developer`, `build/gen`), not the legacy Make
layout (`bin/<platform>-<cpu>/AROS/Developer`). Raw path-byte equality between
those layouts is not asserted or evidence of ABI/hash parity.

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

### Productive graph boundary

Configured projection must be atomic across every selected Mesa compile target.
Derive source-positioned C/C++ options before publishing any graph; failure of
one declaration must not retain a partial map. Keep capability fingerprints and
static metadata for recipe/shape validation, but do not emit the static flags in
addition to the configured vectors. The scalar fallback map alone cannot select
this path.

The source's literal `Makefile.sources` existence guard must observe the selected
fetched tree, not derive file absence from a Mesa version. Bind both presence
and absence to the exact normalized path, existing parent and selected ports
root, and revalidate before graph publication. A malformed or inaccessible
inventory must not silently select the absent-file fallback.

Configured targets need a distinct CMake directory scope: target options cannot
remove inherited global or build-type compiler flags. Clear those flags only
in the Mesa directory; ordinary parent targets remain unchanged. Retain structural
SDK/genmodule quote paths independently of the source-ordered options. The GCA
helper and its generated objects must use the same directory scope. The winsys
handwritten compile rule has a separate, deliberately repeated option vector;
do not copy a deduplicated target option list into it.

GCA selection requires every provider/consumer to have complete configured
language metadata and the exact frozen string. Missing or mixed metadata must
fail, not fall back to the legacy static hash. Ordinary assembler declarations
use `AFLAGS`, unlike the handwritten GCA `.S` rule's `compile_q`. Until their
separate measured assembler projection exists, a mixed declaration must reject
the new graph transaction. That rejection is unfinished work, not removal of
assembler support from the acceptance scope.

### Generated module and per-source recipe boundary

Make language semantics belong in reusable transpiler constructs, not in copied
Mesa compiler recipes. The Mesa adapter selects and binds original source/template
inputs; it must not substitute guessed owner-specific variable values or rebuild
the command's option sequence. Recursive definitions, positional expressions,
immediate target variables and rule-generating statements share the generic Make
layers. Unsupported language semantics must reject rather than silently retaining
an older definition or dropping a side effect.

Expand rule-generating statements at their original program sites and retain an
original source-line mapping for every output line. Consumers must resolve original
assignments against that position, not the final globals of the expanded file.
Generated target-local variables must participate in the same ordered overlays as
later ordinary target assignments. Rule-data-only eval must not admit a hidden
second parse-time global effect or a Make-wide control target. This bounded mode
must feed a common positioned rule collector and finite explicit/static-pattern
instantiator. Preserve normal prerequisite repetition, order-only prerequisites,
inline and empty recipes, and header-time expression positions. Recipe evaluation
receives only independently known automatic values; time-dependent facts must not
be guessed. List-valued automatic variables retain their distinct duplicate and
path-part semantics. Syntax/recipe primitives must not depend on Mesa owner names,
languages or copied compiler commands.

Literal recipe AND/OR lists retain every operand and operator in a flat,
left-to-right sequence with equal `&&`/`||` precedence; pipelines bind more
tightly. Structural program/file bindings must account for all operands,
including those a shell might skip, without inferring statuses or selecting an
execution winner. This is syntax and input binding, not shell execution
authority. Dynamic loop variables and command substitutions still require
their own independently bound inputs and semantics before a native recipe can
be admitted.

This does not eliminate the remaining
requirements to bind actual source lookup, inheritance and marker/writefiles
recipes. Original-template replay against synthetic inputs is not evidence that
the productive graph uses these primitives. Compose ordinary declarations for
the same concrete target before deriving automatic variables: the recipe-bearing
declaration's prerequisite lists precede the remaining original-order lists.
Retain duplicate normal inputs and independent double-colon rules. Never silently
drop supplemental recipe-less prerequisite rules. Multiple ordinary recipes must
either follow an explicitly modeled override contract or refuse; selecting the
first recipe is not acceptable. The GCA immediate
assignment retains its own original position; it is not replaced by the compile
recipe's later global scope.

Selective directory-search declarations must be positioned events, not inferred
from a Mesa version or copied basename recipe. Expand each declaration in its
original scope; preserve repeated-pattern order, exact-pattern clearing and
clear-all effects. Search the literal prerequisite before selective entries and
the independently evaluated global VPATH. A missing candidate may advance the
search; an unsafe/unreadable first candidate must refuse rather than be skipped.
Freeze the general search list from the final global frame before target
traversal, as GNU Make does. A target-specific or inherited `VPATH` can differ
in recipe expressions without changing that frozen filesystem search list.
Do not rebuild search paths from each selected target's variable context.
The first existing candidate must match the independently selected source, and
its resolved path must feed prerequisite/automatic-variable evaluation. A pure
candidate catalogue or retained lookup test alone does not establish that final
compiler-path handoff. Shell-based drive-prefix normalization remains a separate
pure-data interpretation boundary, never permission to execute arbitrary shell.
The generic expression layer may explicitly opt into a bounded echo/substitution
pipeline model: parse data and a restricted basic regular expression rather than
matching Mesa-owned command strings or invoking the host's shell/sed. Literal
regex punctuation, capture order, slash escaping, replacement backreferences and
output/work limits require counter coverage. Ordinary shell functions remain
unsupported by default. The option does not replace the separate wildcard/filesystem
policy; sealed-data evaluation must still explicitly disable filesystem functions.

One target-wide C/C++ vector is not the complete source contract. The original
`tools/genmodule writemakefile` command supplies an included module Makefile.
For the versioned Mesa GL module its inputs are the base
`workbench/libs/gl/gl.conf` **and** the `workbench/libs/mesa/mesa3dgl.conf`
override; the override alone is not equivalent. This is one module, not a
universal generated include for every Mesa-related target.
The generated fragment owns start/end and normal/relative client source lists,
relative-library order, and distinct module and client preprocessor additions.
Use the existing genmodule manifest model and compare it against the original
producer rather than introducing another independent configuration parser.
Such comparison establishes model behavior, not producer authentication.

Key a generated fragment by its exact declaration owner, module name, module
type, base configuration and optional override, not by module name alone.
The Source library and HIDD declarations named `gallium` are different
producers with different configuration files and outputs. EGL, Gallium,
llvmpipe, VC4 and selected V3D module scaffolds need their own records. The
standalone GL normal/relative client archive marker declarations are a
separate consumer path and must not disappear from coverage merely because
they do not use the runtime module source-origin registry. A `rellib`
dependency does not itself enable generation of relative client stubs.

Derive selection through the existing source declaration parser and the
complete selected target context before invoking the original producer; do
not introduce a second declaration/config grammar or infer selection from
generated directory names. The narrow measured compiler-value wrapper is not
a complete module inventory. A probe of one declared module does not prove
that it belongs to every selected profile. Require all selected producer and
consumer records, with no missing, duplicate or cross-owner substitutions,
before per-source activation. The current local singleton handoff is an
unfinished implementation boundary, not an exception to this requirement.

The inventory export pass uses `aros-transpiler
--mesa-genmodule-inventory-only` with a validated source native selection and
explicit target selectors. It shares normal discovery, parsing, dependency
closure and final input revalidation, but publishes only the selected JSON
declaration data at `--output`, not a CMake graph or preparation sidecars.
Runtime module owners, synthetic module-builder normal/relative archive
endpoints and concrete standalone LinkLib marker consumers are separate graph
selection paths. In particular, the synthetic archive set must not filter
concrete LinkLib owners which the selected graph has already retained. A
client-only module selection still requires that module's original config even
when its runtime is absent. Preserve any prior output on selection/input failure.
This export is not a preflight receipt, original-producer authentication or
full compiler-coverage proof; its data and source selection must subsequently
be bound to the retained original-generator execution.

This declaration-only pass must also work before fetched Mesa sources are
materialized. Retain the normal parser's pending declaration identities with
their base/override configuration paths, prune them with the same selected
endpoint closure, and carry runtime and client lanes separately. Never promote
an identity into a compilable target or invent its source list. Concrete
declarations take precedence over pending metadata for the same owner; marker
source tokens still require concrete declarations. Selected fetched wildcards
must retain their actual fetch owner. The ordinary full export continues to
reject missing compilation producers. A bounded shared reader binds the JSON
selection to the caller's exact profile, contract kind and contract digest;
this validates data, not source authenticity or successful producer execution.

Inventory scope is explicit and caller-bound. `contract-roots` reuses the
selected Build/Consumer/SDK closure. `full-target` mirrors ordinary unscoped
target discovery and parsing, while separately binding a source SDK profile's
configuration contract and exact selectors; it does not import that contract's
SDK roots or native-only parsing context. The CLI requests the latter with
`--mesa-genmodule-full-inventory-profile` alongside the inventory-only flag.
The two scopes cannot substitute for one another in the shared reader. Full
inventory refuses unowned fetched wildcard/direct inputs and late capability
errors before publishing data. It still publishes no compilation graph and
does not qualify the ordinary graph's complete build behavior.

The source-profile API derives the expected scope/profile/kind/contract binding
and the complete helper argument vector from its already validated Source
preset. The compiler-profile alias is not the Source preset. Caller-owned
source/output/ports/build paths and the measured helper executable remain
separate execution inputs, to be bound by the retained parent.

The generated data must reach the source-recipe evaluator with its exact
declaration, both configuration identities, source producer identity and output
bytes bound to the retained execution. A data file or a matching caller digest
alone is insufficient. Do not invent an empty generated include, infer a source
lane from a path prefix, or use the module definitions for client stubs. In the
inspected Mesa declaration both lanes contain the ordered relative-library base
definitions, but only the module lane contains its own `NOLIBBASE` definition.

Select each C/C++ recipe by its expanded GenMF provenance and exact source
membership. Evaluate immediate assignments at their source position, including
the target-specific extra and safety additions; evaluating all rules against
the final variable map loses earlier captures when one owner is reused. GenMF
argument substitution is not GNU Make evaluation. Preserve each rule's source
name, quote context, compiler/sysroot role, language options and repeated atoms.
Require complete, unique coverage before replacing target-wide options with
per-source options or changing the compiled-contract/GCA equivalence claim.
The ordinary-target and empty-sysroot cases must remain unchanged.

The local test-only positioned-import bridge preserves generated-Make include order and
origin. Its internal API selects outputs by owner from the validated collection,
preventing callers from pairing another same-prefix owner's Makefile with a
producer. A separate positional Source evaluator resolves both outer Source
and inner core-frame owner/type/module/base/override arguments before comparison;
explicit-empty overrides refuse, matching the declaration parser. Ten focused
tests and a data replay of all six module
owners from the retained ARM selection pass. The replay uses explicitly selected
existing fetched inventory for Source-scope evaluation, without modifying the
completed preflight owner. Marker-owned `writefiles` producers have no module
include and remain a separate recipe boundary. The bridge is not productively
called. The single-invocation replay is not whole-file Make evaluation or
per-source compiler/GCA qualification; do not activate it on these tests alone.

The configured handoff must retain the complete owner program, including
statements after the selected declaration. Reuse the common, byte-checked
recursive include reconstruction for physical origins and the existing
continuation joiner; do not implement a second include parser. Synthetic setup
lines have no physical location. GenMF accepts an explicit verified origin map
for an assembly, preserves template-body locations, and maps source invocation
stacks without changing emitted text. Map storage and every copied coordinate
share the expansion work budget. In-place formatting/guard projections preserve
line count, including trailing blank lines. Complete-program GenMF expansion is
not complete Make interpretation: composition with generated imports, rule
evaluation and productive per-source compilation remains required.

Source-ordered generated imports resolve identity expressions against the
already-emitted program, including earlier imports but excluding current and
future assignments. Branch selection is independently evaluated in that same
prefix: proven-false imports remain in their original conditional, unknown
selection refuses before producer lookup. The legacy unconditional adapter is
test-only. The cumulative prefix bytes offered to resolver callbacks are
bounded; callers must additionally interpret or refuse other includes/effects.
The execution target (`TARGET`) is distinct from measured configure settings:
bind it explicitly to the selected MetaMake endpoint and do not reuse a
projection under another endpoint. Retain unjoined source byte ranges through
continuation joining so rule origins can still reference every contributing
GenMF/import span. Imported data alone does not evaluate top-level `eval` global
assignments; these require synchronous source-order evaluation before subsequent
immediate assignments, never a copied helper value or a guessed undefined name.

The explicit program-expression mode delivers each first-expanded `eval` body
synchronously to a second-stage interpreter. Later references in the same
expression or foreach iteration must observe applied changes. Keep ordinary
expression and rule-data-only modes strict. All second parses in a program use
one shared expression budget for scanned/emitted bytes, work/list items, total
effects and active recursion depth. A callback error unwinds depth but does not
undo already staged mutations: the program driver must commit only when the
entire enclosing evaluation succeeds. Statement parsing/storage needs separate
bounds. Reuse the collector's assignment/flavor/uncertainty application rather
than duplicating it. A second parse of a simple assignment freezes literal
dollars; recursive assignments defer them, and undefine needs a tombstone that
cannot fall back to an old configured value. A reached single-character variable
requires explicit known state, not an assumed empty result. These expression
primitives alone do not satisfy the complete-source development gate.

The mutable scalar program stages each public operation and publishes variable
history and source events only on complete success. Nested eval mutations are
visible within that stage; errors discard the whole stage, not already consumed
resource budgets. Reuse the shared assignment binder with explicit transactional
semantics while preserving the legacy collector's conservative guards. Preserve
RHS whitespace and the append flavor selected before RHS effects, including a
nested replacement of the same variable. Definedness is independent of directory
lookup: only a measured definition or proved undefined binding can authorize a
default/append assignment. Normalize and bound the effective configured selectors
before admitting state, and reject contradictory execution-coordinate bindings.
Unique execution ordinals must remain separate from physical source positions.
This scalar layer does not interpret conditions, definitions, includes, rules or
recipes. Compose those layers in source order before claiming complete-program
interpretation; a passing scalar suite is not the complete-source gate.

The source-order file reader must share that staged state across condition
operands, recursive definitions, include text and synchronous eval second parses.
Only independently supplied include inputs may be consumed; proved absence is
distinct from resolver failure, and an empty expanded filename list is a no-op.
Keep both unique execution order and original input/physical-line identity.
Freeze rule headers and simple target-variable values while reading, but retain
recursive target variables and recipe bytes as deferred data. Recipe inheritance,
source lookup and later execution need their own integration rather than being
inferred from a retained rule event. Refuse unmodeled Make-wide control targets.
Expand source rule targets exactly once before parsing or expanding their tail.
A proved empty target list creates only an ignored-rule event: no prerequisite,
target-assignment RHS, static-pattern tail, inline/following recipe or default
goal is admitted. Unknown target state still refuses. Rule/empty-recipe state
must not cross an include/eval parser boundary or attach to an earlier rule.
Measure `.RECIPEPREFIX` separately from ordinary initial-default values, retaining
the legacy v1 dot-variable exclusion. The parser-control v1 packet has exact
file/default membership and the shared bounded raw-value codec. Missing or
nondefault selected prefix values refuse; no tab-prefix assumption substitutes
for a measured control. Native positive/counter cases must cover these boundaries.
Plain `.PHONY` declarations require distinct source-positioned marker events,
not ordinary prerequisite edges or default-goal candidates. Freeze names at the
declaration, preserving order, repetition and literal percent signs; repeated
markers accumulate rather than overwrite. Do not attach a marker's following
recipe to an earlier rule. Unsupported special-marker forms and marking other
control targets refuse. Before admitting a complete build graph, its consumer
must apply markers across the whole file, including to earlier target declarations,
and honor always-out-of-date/no-file/no-implicit-search semantics; merely
retaining marker data is not scheduling or execution qualification.
Plain `.SUFFIXES` declarations likewise require distinct source-positioned
events. Empty expanded prerequisites reset the list; nonempty declarations
append every suffix in order, including duplicates and literal special-target
words. An unknown initial list remains unknown until a reset, never a guessed
platform default. Preserve declaration data separately from final suffix-state
composition, with explicit name/byte/work bounds. Refuse unsupported double-colon,
static-pattern, assignment, order-only and recipe-bearing forms. Suffix markers
are not ordinary rules or default goals. Full graph admission still requires
source-positioned suffix-rule/default-goal interpretation and native initial
state binding; final list data alone is not an implicit-rule search engine.
Native comparisons must verify exact final state and an order-reversal counter
that changes selected suffix-rule behavior without building target artifacts.
Retain `.DEFAULT` declarations separately from ordinary rules/default-goal
candidates, with frozen own prerequisites and deferred, positioned recipes.
Their prerequisites belong to `.DEFAULT` itself, not every unmatched target.
Preserve single/double-colon, absent/empty inline recipe and bare reset forms;
mixed colon modes and unsupported special-header forms must refuse. Graph
consumers must resolve explicit/implicit rules before applying fallback, preserve
prerequisite-only declarations, and distinguish a bare reset from an empty recipe.
Retained declaration data alone does not select or execute a fallback recipe.
Recipe continuations preserve backslash/newline bytes and remove only the next
recipe prefix; ordinary lines use Make's backslash parity and whitespace folding.
Definition bodies use ordinary line folding even when they contain tabbed data.
The existing legacy collector remains conservative until deliberately migrated.

Captured Make inputs retain exact original bytes, read identities and content
digests. The parser, not the filesystem snapshot, owns comment syntax. A
non-UTF-8 byte may be ignored only inside a proved ordinary Make comment, never
by guessed file encoding or lossy conversion of semantic content. Recipe text,
inline recipes, escaped comment markers and definition bodies remain semantic
data and require exact supported decoding. Raw source ranges and resource
accounting must still cover the original file, including ignored comment bytes;
the legacy text-read APIs remain strict UTF-8. Native positive and invalid-data
counter-probes must demonstrate this distinction before using the byte path for
the real configured source. This does not relax root identity or transactional
publication checks.

GNU function arguments are partitioned before expansion. For fixed-maximum
signatures only the first `max - 1` top-level commas separate arguments; later
commas are part of the final argument, including for a one-argument function.
Never split expanded values again. Variadic functions retain their separate
argument lists; minimum counts, balanced references and resource ceilings still
apply. Test this grammar against GNU with literal/nested commas and invalid
arity, without target-specific flag substitutions.

The initial namespace must be independently measured, not inferred from an
assignment found only in a false source branch. A complete, bounded namespace
observation may prove absence; a partial configured-value map cannot. Bind that
observation to the same source/configure/compiler owner, preserve defined-empty
versus undefined, and never import ambient environment values or serialize them
as a shortcut. Unknown reached values must continue to refuse until their actual
input is available. Genmodule import selection must ultimately use this same
source-order state, not a parallel prefix-only approximation.

The generic namespace transport contains names and exact GNU origins only.
Presence does not provide a raw value or flavor. Absence queries are restricted
to the bounded literal source-name subset (ASCII alphanumeric, underscore and
hyphen, including leading digits), not C identifiers. Automatic variables,
computed names and unsupported syntax remain outside this absence proof.
Unresolved present or unsupported names must
not fall through to another directory's variable configuration. Explicitly
bound execution-overlay values, subsequent source writes and source undefines
remain distinct. Unknown conditional writes take precedence over initial
absence. Configuration v2 explicitly carries the observation; v1 cannot gain
that proof. Both local input loading and portable evidence readback must compare
the observation with the retained final Make stdout and its stage digest.
These data checks do not authenticate execution. The producer must capture the
namespace after the same configuration/compiler-feature includes, before probe
helper assignments, with an iterator-collision counter and no value expansion.
Preserve GNU's built-in variable namespace; disabling implicit rules must not
disable default variables. When the adapter removes the global wrapper import
and starts from its measured post-import state, refuse owner statements before
that import until their actual import position is modeled. Comments are inert;
assignments, definitions and conditional prefixes are not silently reordered.
Reached, initially defined values still require their own source-bound binding
and flavor/assignment-time semantics. A namespace entry cannot authorize a
directory fallback, a guessed path, or freezing later USER contributions.

The generic variable-program constructor accepts independently supplied initial
values with explicit GNU expansion flavor and exact observed origin. Simple
values are already-expanded data: embedded dollars must not execute or expand
again. Recursive values retain their expression and observe later source
contributions. Constructor-only installation precedes all source events; failed
file/include interpretation must not publish partial changes to those values.
Supported initial origins include ordinary file/default, recursive environment
bindings and command-line bindings with their measured flavor. File assignment
replaces ordinary environment values; command-line replacement and undefine
are ignored, but GNU immediate RHS expansion and effects still occur before
stronger-origin discard. Environment-override, explicit override and automatic
priority/lifetime semantics remain unsupported, never ordinary file values.
Conflicting execution overlays,
coordinates, closed selectors, duplicates and resource-limit violations refuse.
These data semantics are not a producer. Actual source/configuration ownership,
dependencies and assignment-time meaning still require independent evidence
before any reached global value can enter productive replay. In particular,
the real whole-owner GENINCDIR refusal is not resolved by this constructor.

The native preflight's generic raw-value transport retains ordinary file/default
bindings after configuration and compiler-feature imports, before probe helpers.
GNU `value` supplies raw text without executing deferred expressions; `flavor`
distinguishes simple and recursive data. Preserve spaces, dollars and multiline
definitions with bounded, unambiguous encoding. Exclude environment origins and
special dot variables. Require the exact eligible namespace membership and
collision-free private helpers, not first-match selection or missing-as-empty.
Configuration v3 explicitly carries this packet; v1/v2 cannot acquire value proof
by assertion. Both local loading and portable readback compare it with the exact
retained configured-values stdout and its original stage digest.

A global configure snapshot belongs only to its original Make context. Replacing
CURDIR, TARGET or execution roots does not update other already-expanded simple
values. Native configuration must therefore be observed in each selected owner's
actual pre-import execution context, or completely replayed there with proven
dependencies and assignment order. Bind such scope data to the original selected
owner, explicit execution inputs and source/configuration/compiler identities;
reject owner swaps or changed inputs. Repeated native observations must start
fresh Make processes rather than accumulate append state. Do not filter out
coordinate entries and silently transplant the remaining simple values. Keep
global snapshots data-only until that owner-context proof exists; productive
path projection and compile/GCA parity remain separate acceptance requirements.

The inspected native MetaMake boundary is a shell command, not an existing
tokenized argv contract. `tools/MetaMake/project.c` selects the source or build
cwd according to whether the makefile is generated, enters its relative owner
directory, and supplies per-call CURDIR/TARGET plus project TOP/SRCDIR. Its
`execute` function substitutes project variables into the configured command and
uses `popen` on its POSIX path; variable lookup falls back to the inherited environment. The actual
`mmake.config.in` command also supplies MKARGS. MetaMake consumes its own options,
forwards other explicit dash options and does not rewrite inherited MAKEFLAGS.
Preserve that selected command/cwd/environment and its generic substitution and
quoting semantics before claiming an effective child argv. Do not manufacture a
fixed architecture-variable environment map or treat root-prefix replay as this
native transport. The existing collection refusal remains until this boundary
is modeled and independently verified.

The generic scope packet binds the selected owner, source/compiler/configuration
digests, working directory and exact literal command-line input set. Preserve
input origin and flavor, reject additional command-line variables outside that
set, and distinguish a missing value field from an explicitly empty value.
Collection input v7/v8 adds an observation stage per selected producer; local
loading and portable consistency readback must reject exchanged owner packets
even after all surrounding stage, invocation and document digests are rebound.
These are consistency checks, not producer authentication or native execution
qualification.

The native initial state includes source-defined environment transport. The
root Makefile invokes MetaMake through `CALL`, whose `env` assignments are
defined in `config/make.cfg`; leaf Make receives them from the environment,
while TOP/SRCDIR/CURDIR/TARGET are command-line assignments. A copied root
Makefile prefix changes origins and cannot prove equivalent leaf semantics.
Current development scope production therefore refuses the not-yet-modeled
CALL interface. Wire the generic literal environment-prefix, explicit exports
and initial-origin primitives into source-bound transport and the actual
pre-import boundary before native replay; do not substitute
a fixed architecture-variable list or promote synthetic scope tests to native
owner qualification.
CALL alone is not the whole inherited environment: the root Makefile also
declares exports, and the configuration exports its admitted PATH state.
Retain the source-declared export mechanism generically while preserving the
sealed host-tool policy; do not inherit arbitrary ambient operator values.

The generic program binds the build root and actual process cwd independently.
TOP assertions must match the independently selected build root, not the leaf
directory. Native generated and source-directory Make invocations may use
different cwd roots. Preserve relocatable TOP in ordinary expressions; pure
`abspath` may materialize it only from the explicit build-root binding. Relative
path evaluation must use the explicit cwd, with no ambient directory discovery.
Changing either bound path invalidates later evaluation and statements before
publication. Synthetic GNU Make comparisons must cover both cwd layouts and a
TOP-equals-cwd counter. These path semantics do not establish the native export
handoff or qualify whole-owner replay.
Materialization and cwd-relative joining must enforce byte bounds before
allocating their replacement/joined strings, including for direct expression
context callers. Split GNU Make words after materializing bound variable values;
whitespace in the selected roots is not implicit shell quoting.

The native launch boundary must also account for source-declared changes after
the root configuration include, such as an optional later Makefile include.
Capturing the restricted configuration prefix alone cannot certify the parent
environment at its MetaMake invocation. Either retain that actual launch context
under the sealed host policy or prove a complete positional replay. Do not
invoke ordinary source build prerequisites merely to obtain an observation.

Complete source-program replay is separate from engine-coordinate projection.
Its initial state comes from the native pre-file namespace and measured values,
not the final configured map. Do not seed architecture selectors or inject
engine TOP/SRCDIR/CURDIR aliases into this interpreter: source assignments and
environment/command-line priority apply to those names normally. The existing
projection constructors retain their protected aliases. The new interpreter
produces data only; its presence does not admit a graph or replace producer
binding, include-closure validation or the still-required inherited-environment
transport. The current file/default raw packet does not carry environment data;
that missing producer/transport boundary must remain explicit.
The shared native observer can run as a pre-first-file `--eval` fragment,
retaining the existing namespace and raw file/default packets without seeding
them from a later config include. Its fixed private helpers must be absent
before any packet is emitted and must not survive the observation. Preserve
the selected Make flags: `-r` and `-rR` have observably different default values.
Environment and command-line raw values are not emitted by the default observer; an
importer must bind them separately to the exact explicit invocation and observed
origin/flavor, or reject a reference requiring an unmeasured value. A successful
synthetic observer/import round trip is not native Source owner qualification.
An additive opt-in command-line packet may capture raw values only after its
explicit complete expected name set matches the native command-line namespace.
Use the shared bounded value codec and measured flavor; do not implement another
GNU argument/assignment parser or a table of AROS names. Unsupported literal-name
syntax and helper collisions refuse before raw output. Capture via `value`, not
evaluation: embedded Make expressions remain inert data. Keep captured output
private and redact diagnostics. This packet neither captures environment values
nor authenticates an invocation. Immediate argument RHS and startup options can
act before any observer; a separately admitted closed argv is mandatory before
launch. Pre-file eval and first-input prelude are distinct instrumented phases,
not interchangeable evidence of an observer-free native context.
Native environment-origin values require a separate explicit opt-in packet,
using the same bounded raw-value codec and complete expected-name check. Keep
the whole stdout private, including on later failure. Obtain membership from an
independently bound observation at the same phase/context, not an ambient
environment read or a fixed table of engine-generated names. Preserve native
simple/recursive flavor and raw bytes; environment-override state refuses before
raw output. Original process environment membership remains separate because a
CLI override can hide its origin and inherited export history.
The source interpreter's aggregate reader must parse namespace, default, CLI
and environment packets from the same selected stdout, validate each complete
membership against that namespace, and install them through the existing initial
state path. Bind original process names together with measured environment-origin
names, including native-created bindings, without guessing their values. This
composition is data consistency only: exact executable/argv/env/cwd/files/phase
and successful execution receipt remain independent producer obligations.
The observation is exact only for its instrumented invocation: GNU retains
`--eval` through an internal dependency in MAKEFLAGS. Do not remove or normalize
that dependency to claim an observer-free context. Until the complete native
administrative state is bound, a reached reference to that unmeasured value
must refuse transactionally. Make may also manufacture environment-origin
values absent from the parent's explicit map; preserve their measured flavor,
including simple flavor, rather than guessing that all environment values are
recursive. Up-front completeness of the inherited native environment remains
unproven; unused unmeasured names do not authorize importing guessed values.
Before native startup, admit the exact ordered argument sequence through a
closed literal grammar, separately from shell-word transport. Preserve repeated
flags, file operands, goals and assignments; derive the expected CLI name set
without computing final values or emulating GNU assignment precedence. Refuse
unmodeled options, expressions, immediate/shell assignment forms, GNU's `-`
stdin shorthand and
nonempty preload/option controls. Check each raw control assignment before a
map could hide an earlier occurrence; check the exact resulting process
environment separately. Explicit files and old-file operands retain their read
identities; device/descriptor aliases require separate input binding. Admission
does not prove file ownership, no-remake closure, selected
cwd or source-effect safety. Dry-run is not isolation: native counter-probes must
show that rejected startup expressions really would perform a controlled effect
without the gate. A finite grammar is a conservative refusal boundary, not a
second GNU option parser or authorization to execute arbitrary source.
For the actual root's MAKEFLAGS-dependent selection, prefer the target-free
first-input prelude rather than `--eval`. A normal MAKEFILE_LIST assignment
removes the observer's list contribution without overriding command-line
priority. This snapshot is before the selected source, not before all Make
inputs: retain that distinct phase in the future producer/receipt binding.
Native comparison must prove the first selected file and later includes see
the same list, origins/flavors and flags as an observer-free invocation.
The prelude must not define targets or change the default goal. Supply explicit
`--old-file` protection for the observer and every independently admitted read
input using every exact Make read identity (including relative spellings), not
just an equivalent canonical pathname. GNU may run Makefile-remake recipes even
in dry-run mode; an absolute alias does not protect a relative input identity.
Reaching this
boundary still requires a complete, revalidated input closure. It is not
permission to execute an actual Source query against an incomplete inventory.
Preloaded MAKEFILES inputs are not covered by the empty-list transition: they
would be erased from the selected root's history. Validate the exact cleared
process environment and command-variable map before launching Make. Nonempty
MAKEFILES or inherited MAKEFLAGS/GNUMAKEFLAGS/MFLAGS currently refuse until
their option/source effects are explicitly modeled; never clear them silently.
This limitation remains part of the open native parent/leaf handoff, not a waiver
of inherited-context acceptance. The first-input prelude must precede all other
explicit Make files, and native argv ownership remains independently required.
In this source mode, WILDCARD is a normal source-defined function, not the
legacy adapter's specially recognized helper. Ordinary native wildcard uses
the explicitly bound process cwd, not a guessed source directory. The new
whole-program API refuses ambient filesystem enumeration. The captured-file
adapter can now supply bounded membership/directory queries over independently
selected and revalidated roots. Without that explicit provider, reached
filesystem expressions still refuse; no missing inventory becomes empty data.
The wildcard provider preserves per-pattern order and duplicates, immediate
versus recursive assignment timing and actual cwd spellings. Use bounded
component matching rather than unbounded glob backtracking. Parsing, class
construction and matching share one work budget across the complete read or
later captured evaluation; byte membership in a class has constant-time cost.
Native C-locale comparisons establish the implemented byte-pattern behavior,
not an implicit proof of every locale or all filesystem functions. Unknown
aliases must refuse rather than become false absence on case-folding hosts.
Directory queries do not claim that Make read the corresponding files.
Directory lookup must use an index or charge every scanned tree entry; a small
returned list is not a bound on repeatedly traversing unrelated subtrees.
Symbolic-link traversal, unsupported locale classes and missing intermediate
directory queries currently refuse explicitly. Complete actual native input
closure, inherited invocation and remaining reached filesystem/shell functions
remain required integration work, not acceptance exclusions.
Captured `realpath` uses the same independently bound roots/cwd and query
budgets, without ambient canonicalization or additional Make-read claims.
The native host `PATH_MAX` applies to the original token, before normalization;
unknown host limit bindings refuse rather than guess. Literal commas and dollar
bytes remain path data, not extra arguments or executable Make references.
Word lists use GNU's C-locale whitespace bytes rather than Unicode White_Space;
NBSP and EM SPACE filename bytes are not separators in that bound locale.
GNU normalizes terminal separators/dots even on a regular file; that function
behavior must not weaken the separate actual file-read suffix checks. Existing
regular files/directories resolve to selected absolute names; safe missing final
entries disappear while aliases, symbolic traversal, missing intermediate
components and changed selections refuse. These conservative refusals are not
permission to omit required source behavior. Preserve immediate versus later
recursive evaluation and reject stale selected trees. The actual Source
first-input/root-tools condition must remain ordinary interpreted source, not
a special variable, pathname or architecture lookup in the transpiler. A native
fixture using that exact guard can establish this conditional distinction but
does not qualify a complete actual Source invocation or owner closure.
Engine deferred-token recognition and Source/build/Ports token materialization
also belong only to projection mode. In source-program mode, even names shared
with engine placeholders are ordinary measured Make variables in both `$()`
and `${}` forms. Proved absence expands empty; literal escaped-dollar data in
a pure echo must remain literal and must not become a context-derived path.
The pure source text evaluator accepts a bounded literal echo subset, optionally
followed by the existing sed subset. Single-quoted words are inert data; no
host shell, environment lookup, command substitution or general shell syntax
is admitted. This is generic syntax handling, not recognition of an AROS path,
architecture or named recipe.

Retain GNU Make's input lifecycle in the same transactional program. The first
real file initializes MAKEFILE_LIST at file/simple priority, replacing ordinary
environment and pre-file eval values but not a stronger command-line binding.
Each actually read input then appends its identity in order; repeated inputs
remain repeated. Later source assignment flavor and undefinition affect the
next append. Missing optional inputs and eval text alone add no filename. Mark
implicit input updates separately from source statements, preserving their
execution ordinals without inventing source lines. Bound identities, depth and
storage, and reject an identity containing the evaluator's reserved escape
marker rather than silently changing its data. Input failures must roll back
the list, first-file state and ordinary variable state together.

Bind interpreted inputs through independently selected complete roots, retaining
both the requested include token and the actual native read identity. Relative
includes resolve against the process cwd, then the independently measured native
include-directory order, not the including file's parent. Missing relative
includes cannot be classified as absent without a bound search context. Native
option strings are not substitutes for that context: GNU normalizes, for example,
`-I./directory` to `directory` in its actual include search list. Preserve actual
read spellings in input events, MAKEFILE_LIST and old-file controls.

The data interpreter stages state until all captured trees pass both initial and
final identity/content checks. Final checking also runs after an interpretation
error; a caught read refusal poisons the transaction rather than permitting a
manifest to be published. Exact repeated reads remain ordered. Reject unsafe
links, directory-only file suffixes, unknown aliases and out-of-selection paths;
do not erase their native lookup semantics with lexical normalization. Root
selection must reject overlaps and aliases without conflating distinct existing
case-sensitive directories. These utilities do not authenticate the root digest,
native argv, observation phase or completeness of an arbitrary caller's program.
An explicit empty include search is a caller claim requiring native proof, not
a new default for the real Source launcher. Native success status alone cannot
prove a file was read: old-file controls can suppress a missing-root error.
Before a productive query, bind and revalidate the actual selected closure and
invocation, then compare the observed input sequence and required values.

The generic variable program records explicit export/unexport directives in
source/include/eval order. Export names are selected at read time; recursive
export values expand at invocation time, while simple values stay frozen.
Both directives create a proved-undefined name as a simple empty file binding,
as GNU Make does; later undefine destroys its export property. Conditional
uncertainty, unproved definedness, side-effectful invocation-time expansion,
implicit export-all and unsupported export syntax refuse transactionally.
Explicit export data is not a claim to contain the whole inherited environment.
Source-controlled export projection must also preserve measured automatic
inheritance: environment-origin variables retain export status through later file
replacement, while command-line-only names follow GNU's identifier eligibility.
A command-line override can retain an earlier imported flag even for another
name shape. The final origin cannot establish that history; bind complete original
environment-name membership independently or refuse the ambiguous selection.
Unchanged environment-origin bytes are exported literally, not recursively
expanded as a Make expression; file-origin replacement uses its current source
flavor. Keep undefined, unexported, empty, inherited and unresolved states distinct.
This projection does not synthesize engine-owned MAKELEVEL/MAKEFLAGS/SHELL
transformations or authenticate the resulting child context. Opaque inherited
process keys must not be filtered as shell-assignment names. A downstream bounded
command adapter may still refuse a source projection larger than its channel.
Invocation-time source export expansion must use the same explicitly selected,
descriptor-bound filesystem capabilities as captured expression evaluation.
Recursive wildcard/realpath values remain deferred; both projected data and
variable-state changes publish only after complete selected-tree revalidation.
The returned read evidence does not authenticate a native child environment.
Recipe expression and recursive source-export evaluation must share one transient,
explicitly bound automatic-variable frame and one captured-input transaction.
Do not install automatic values as global file or environment assignments. Missing
timestamp/archive/stem data must refuse, not become guessed empty strings. This
primitive does not select a recipe or discharge target-variable inheritance;
state-changing expression effects remain refused until their timing is modeled.
The literal `env` prefix parser preserves shell word quoting as inert data,
refuses expansion/options/control syntax/duplicate assignments and enforces
size/count limits. Combining explicit exports and prefix assignments must use
the actual source context, preserve prefix precedence, and then apply the fresh
child's environment/command-line priorities. Synthetic GNU parent/child oracles
are development evidence, not native Source owner or complete SDK qualification.

An expanded literal child command must retain program/argv separately from its
`env` assignments. The generic bounded command-word adapter preserves quoted
empty arguments and `NAME=value` words after the program as argv, not
environment assignments. Within an env prefix, repeated assignments use native
last-write precedence. Applying them requires an explicit inherited map; never
discover the operator's ambient environment. Leading shell assignments,
expansion, operators, env options and unmodeled shell-command effects refuse.
Quoted control words also refuse when quote provenance would be necessary to
distinguish them; after env, the utility operand is not a shell builtin.
Diagnostic formatting must not disclose parsed assignments or argument data.
This is inert transport interpretation, not PATH/host-env-utility authentication,
Make-option parsing or native launch/receipt authority. All those bindings,
automatic MAKEFLAGS/MAKELEVEL, preloaded context and the complete parent export
environment remain required before lifting the collection CALL guard. Compare
values and origins separately: replacing environment inputs with file assignments
can preserve every value while changing the native contract.

The private preflight runner must retain the exact selected program spelling,
arguments, cwd and complete resolved environment used to construct its command
alongside successful stage digests. Do not reconstruct literal process data from
normalized environment hashes. A child scope inherits that actual parent map and
checks its expected observation phase before writing scope inputs. Raw context
is not portable evidence or deserializable execution authority; retain the closed
receipt digest format until a separately bound Root/MetaMake protocol replaces it.
Retain the successful configured context in the completed live producer as well;
do not discard it at receipt publication and later recreate it from request
policy. A later native observation that changes the sealed configure tree must
invalidate that completed result, not silently extend its old receipt.

Before a native Root observation, account for GNU Make's Makefile-remake phase:
even a missing optional include can reach `.DEFAULT` and start unrelated builds.
A separately bound parse-only observation may use Root as its first input and
an explicit aborting observer second, before automatic remaking. Any subsequent
old-file fence must bind reached inputs and absent optional includes, preserve
read spellings, and record its changed MAKEFLAGS/phase. Such a capture is not the
ordinary SDK target's execution context. A helper interpreter's visible
environment alone also does not prove another executable's initial environment
or close that interpreter's runtime inputs.

Native MetaMake capture must also bind its own scan and side effects before
substituting a capture-only make launcher. A target can have several real owners
and recursively selected dependencies. Missing global-variable files and stale
Makefile.src outputs can launch generators before that launcher; scanning writes
mmake.cache in the build root. Isolate that writable root, verify the selected
owner/dependency/regeneration closure, and do not enable dependency/failure logs
in an original source directory. Changing only the executable does not neutralize
unquoted target, filename or template shell syntax. A private capture experiment
must retain the effective original argument template, bind names and cwd, and
never start GNU Make. It is evidence about the native launch boundary, not
equivalence of a changed private project configuration with the full production
project or permission to remove the still-open collection guard.

The opt-in `metamake_launch_capture` fixture separates pre-generation from native
owner launch. It selects the original Mesa GenMF input/template closure and
launch template by measured identity, then uses a fresh private source/build
mirror, an absolute capture-only interpreter and an empty executable search path.
Before native execution, the completely bound template must parse as one literal
capture command. A missing-launcher counter must fail because native default
Make cannot be found, with no capture; `-n` alone is not isolation. Source and
already-generated owner layouts, inherited target controls, forwarded flags and
duplicate real owners must be checked separately. This fixture does not establish
native preregeneration, complete interpreter/runtime input closure, production
export/environment equivalence or permission to execute Make on the source tree.
Native launcher declarations must also match their definitions: including their
shared header in the defining translation unit makes declaration drift visible
to the C compiler rather than only to a later child launch.

The subsequent target-local capture primitive records immediate `:=`/`::=`
values separately for each evaluated object target and original joined line.
All target-local immediate variables enter its internal overlay, including
unselected dependencies that shadow globals; only reporting is output-filtered.
Unknown selection, unsupported flavors, patterns and resource-limit violations
refuse. The existing Make scanner supplies inert define-body boundaries.
Eight focused tests and the fingerprinted original `rule_compile_multi` replay
with synthetic globals pass. Independent GNU Make 4.4.1 observations confirm
target-local RHS timing and repeated-group freezing. This is not complete
Source recipe evaluation: C++ `eval` expansion, target-variable inheritance,
full source membership and productive compile/GCA activation remain open.

## Development and acceptance evidence

### Runtime preflight insertion

The existing native compatibility `UpstreamConfigure` phase occurs after
`CmakeConsumer`; its outputs therefore cannot supply pre-transpilation facts.
The measured Mesa configuration must come from a separate bounded preflight
before the first engine configure, in both CLI and native compatibility paths.
The fixed six-phase compatibility proof remains required and in its original
order; a preflight is neither an SDK owner nor a substitute for any phase.

The implementation must reuse the validated source/compiler selections and
closed host environment, run with compiler construction disabled, and bind the
concrete configure target/options, source closure, compiler bytes, environment
and generated configuration to its result. A board name is not a configure
target, and a previously failed SDK execution is not a reusable producer.
Frontend configuration needs an explicit source-owned configure selection where
no validated producer profile already supplies one. The same validated result
must reach every transpiler pass. A data file and a caller-supplied matching
digest alone are not evidence that this preflight ran.

The child data channel now has an explicit `bound` mode and an `off` mode
which removes cached input selection. It binds the exact configuration and
receipt bytes, checks source and complete private-output stability, and is
rechecked for both engine graph passes. Its sidecar deliberately claims only
data consistency: it does not establish full target/compiler selection or
activate compile/ABI projection. Producer and consumer now share
`MesaSourceSnapshot`: the historical mode measures the complete tree; local
input version 2 carries `LocalSourceIdentity` and omits only its selected
`build/{preset}` subtree after checking Git/index, source bytes and generated
namespace safety. Sibling builds, dirty/untracked/ignored inputs outside that
subtree and Git-selection drift remain measured. Local receipt and child binding
use v2, while historical v1 omits the new field. Mixed modes reject. Generated
paths within source must stay in the exact selected subtree. The existing native
portable reader still rejects local v2; this does not expand native qualification.

The native execution owner must run and retain the completed preflight before
constructing `CmakeConsumer`, then revalidate it after that phase and before
aggregate publication. The later LLVM SDK link stage is too late to supply
these facts. Select whether preflight is required from the independently
verified source profile, not from receipt presence. Match the full source
profile, platform, ABI/MMU, configure selection and measured compiler binding.
The aggregate, retained inventory and portable readback must explicitly carry
the required evidence together; version their new claim without changing the
meaning or byte shape of historical non-Mesa receipts. Missing required proof,
changed inputs/outputs and forged or unexpected proof must reject. Passing the
data channel alone must never upgrade an older execution to this new claim.

The current local native-executor slice starts the selected Mesa 26 preflight
before CMake, retains its private completion across that phase and revalidates
it before/after the local aggregate receipt. The local v7 aggregate joins the
pre-execution selection, v4 input observation and fixed twelve-member byte proof;
the corresponding portable measurement is v5. The old observation API rejects
configured inputs unless its caller independently selects the Mesa policy.
Package-payload inventory and full compiler content-CAS use distinct digests;
the former binds the verified package and the latter binds the Mesa selection.
The opaque byte proof also binds the complete expectations, including original
paths and profile-document identity. Rootless readback with the original test
roots removed and resealed stage-log corruption are covered by synthetic tests.
These checks establish semantic-file and log consistency, not authenticated
execution; they cannot recreate the private execution owner. Historical v6
remains the nonconfigured SDK contract. New release-evidence selection v3 and
SDK-generation selection v2 carry a separately selected source-policy document;
the collectors bind it to the independent SDK source/preset/compiler/contract
before choosing the observation policy. Policy origin and runtime authentication
remain the protected caller's responsibility. Producer orchestration must select
these new formats and retain/authenticate that policy before configured lane
qualification. `SourceMesaPreflightProfile` now loads the ordinary frontend's
explicit `mesa_configure_target` from its sealed SDK contract, binds the exact
source profile document and contract digest, and retains the canonical source
root. This stays separate from producer-owned `Profile`. Source mode compares
the complete selected SDK descriptor with the measured driver binding, not
only its triple. This is metadata binding, not proof that every ABI fact was
observed from actual configured output. Toolchain resolution now retains a
declared verified compiler identity; old unversioned legacy inputs remain
explicitly without that identity. The ordinary CLI now has a local retained
owner before its first CMake configure. A separately sealed source-owned host
Python lock reuses verified package extraction/runtime preparation without
inventing a producer SourceLock or accepting global site packages. The same
closed host policy reaches configure/build; managed compiler-cache overlays
add only their selected values, not inherited process variables. Local-source
readers additionally receive a measured Git role for Git/index/submodule
revalidation. Native qualification keeps its unchanged exact host-role set.
The retained configure/features preflight always prohibits fetching. Actual
CMake/build children reuse that validated closed host closure but receive the
explicitly selected build fetch policy. The sole network-policy overlay must
not mutate the retained preflight identity or reimport ambient variables.

The source module check explicitly invokes the selected interpreter rather than
making a shell launcher the interpreter of another script's shebang. A bounded
real ARM owner run has passed with positive/counter tests, but subsequent full
CLI graph preparation exposed archive-cache and missing source checksum inputs.
These results are not complete current CMake/Mesa execution. Full configured
target/ABI matching and simultaneous compile/hash activation, followed by all
affected real workflows, remain required before M1 acceptance.

Capture the configured global C/C++ baseline as well as the named USER and ISA
components before switching compilation. The actual AArch64 source oracle
selects `--target=aarch64-unknown-aros -fno-pic -fno-pie -mcmodel=large`, not a
guessed architecture flag bundle. Read configured paths/selectors from the
same Make context, and bind optional configuration files or their measured
absence. Never invoke the ordinary Makefile or depend on a dry-run to prevent
recursive builds. The preflight receipt must be included in execution/readback
identity before claiming SDK qualification. Production wiring is still open.

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

### Implemented primitives, not production qualification

`aros-common::mesa_configuration` provides a bounded typed transport with all
eleven required builtin outcomes and ordered ISA atoms. Parsing compares every
field against the caller's expected source/configuration/invocation and compiler
identity, including version and compiler tree. It does not authenticate a
producer or prove that configure ran. The measured producer returns a private
completed-result type with source/compiler/environment and output revalidation;
runtime consumers must retain that binding rather than accepting a standalone
JSON claim. The runtime handoff remains to be implemented. Other source-evaluated C/CXX feature options
are now carried through a closed, required set of twenty-one configured compiler
variables. Every variable is required even when empty; missing/extra/duplicate
keys and unsafe atoms reject. Eleven separately required baseline entries carry
the configured C++ ISA, CONFIG/TARGET C/C++/CPP values, debug/optimization options
and LLVM diagnostic baseline. They preserve atom order and explicit empties;
missing, extra or duplicate entries reject. These are configuration-time
measurements: TARGET_CFLAGS/CXXFLAGS/CPPFLAGS must not replace the source's
recursive expressions after local USER flags are assigned. The final projection
must evaluate those expressions at the declaration position, preserving the
configured baseline and source-local contributions. The complete compile
contract must not be switched before that projection and its receipt are bound.
The full recipe scan added the four configure-owned unused-variable,
unused-value, unused-function and misleading-indentation warning outcomes to
the earlier seventeen-entry development transport. Older local receipts remain
historical evidence, not current-input qualification; do not fill missing values
with guessed empty defaults.
`MESA_COMPILEOPTIONS` is also an explicitly required measured bundle, not an
implicit empty fallback. The actual configured ARM Make oracle found this
variable undefined/empty, and its command-line counter produced the expected
changed C/C++ option sequence. The transport retains that distinction between
a measured empty value and a missing input.

The ordered source evaluator accepts separate declaration and GCA assignment
contexts. It preserves the four named ISA/CPP/C/C++ component vectors and the
assignment-time stripped GCA string. It does not equate these components with
the complete Make compiler command or replace production Mesa behavior yet.
The opt-in `mesa_configured_source_oracle` test compares these components and
the GCA assignment against retained GNU Make measurements from the actual
configured ARM source, with exact source/compiler-configuration hashes. It
also checks the changed option-bundle counter. This is ARM source-evaluator
parity for those named values, not an AArch64 result, a complete compiler
invocation comparison, generated ABI-hash equality or SDK qualification.

The strict transport/evaluator boundary has focused tests; the previous scalar
native contract validator is unchanged. Missing, duplicate or extra fields,
unsafe atoms, stale identities and absent selectors reject rather than default.

An actual CMake/compiler counter-probe demonstrated an additional projection
hazard: ordinary target options de-duplicate `-DORDER=1 -UORDER -DORDER=1` into
`-DORDER=1 -UORDER`, changing the compiled program's configuration. The new
`OrderedCompileContract.cmake` primitive groups one complete argument sequence
before CMake de-duplication. Its fixture compiles C and C++ sources, inspects the
actual emitted command sequences, verifies quoting and language isolation, and
rejects unsafe arguments and overlapping contract calls. The raw negative build
is isolated to its own target. This helper is not yet called by production Mesa
and has not replaced its honest own-compiled-flags ABI hash.

No compiler/SDK diagnostic dispatch, publication, scope reduction or issue closure
is authorized by this plan. Actual implementation and qualification state belongs
in issue #353.

### Target-specific source options

Source object-specific options must not be discarded as incidental Make syntax.
The draw-LLVM C target set uses `MC_EXTRA_CPPFLAGS` to change structure layouts.
A whole-target C projection is valid only when the source rule covers exactly
every C object and dependency output of the selected declaration. Partial,
duplicate or additional outputs require a finer-grained model, not propagation
to unrelated objects. C++ and the VC4/GCA lanes remain separate.

The bounded literal-immediate rule model binds the selected declaration, its
effective source/object paths, source-vector order and the reviewed
`compile_q`, `rule_compile_multi` and `build_linklib` template blocks. Recipe
text and deferred definitions cannot supply executable declarations or owners.
Unresolved conditional selection, target-local RHS expressions, later rules,
explicit LTO settings and unsupported callsite overrides must refuse until
their exact execution semantics are modeled. Positive full-set and negative
partial-set GNU Make measurements supplement the source projection tests.
This slice does not establish the complete command baseline, safety flags,
configured LTO behavior, real Mesa objects or generated ABI-hash parity.

Generator-local `PRIVATE_SCRIPT` and `PRIVATE_XML` values are not global
compiler-option inputs. Their bounded source-scope handling requires the exact
reviewed Mesa26 core recipe/configuration and the existing unique Python output
owner in the productive graph. Only the literal single-output immediate binding
form is admitted; global/conditional/compile-option consumption still refuses.
RHS side-effect and escaped-dollar checks remain active. This does not add a
second generator inventory or execute arbitrary Make recipes: the existing
capability retains ownership of its 25 products and the core consumer. Dormant
legacy rules in the same sealed source do not create additional scheduled jobs.

### Native call handoff: explicit data versions

Configuration-input/receipt versions 9 (complete source) and 10 (selected local
source) distinguish native call packets from the earlier per-owner initial
scope observations. The input document retains its existing collection shape;
the receipt additionally binds the native context, control and packet bytes.
Versions 1–8 cannot acquire these fields or native exit statuses. Explicit null
is not absence. A portable data proof accepts complete-source version 9 only;
it does not turn a local version 10 snapshot into release qualification.

The fixed stage protocol is the four configure/feature stages, Root query,
host MetaMake build, Root launch, generator inventory/build, then one generator
output stage per selected owner. Query/build/launch retain explicit statuses
2/0/0. This layout is a protocol, not a table of compiler options. Calls and
owner associations are separate: multiple owners may share a call, every
selected owner must be represented exactly once, and every call must have its
explicit Source input coordinate (not necessarily a genmodule association).
Source owner semantics remain the generic native Source reader's responsibility.

The genmodule producer roster is only one subset of the native calls, not the
universe of compile owners. Ordinary link-library declarations must not acquire
fabricated genmodule producers. A required, dense `source_calls` roster accounts
for every call separately: each row names its generated Source input and slot,
checked against the retained explicit input and actual working directory.
Repeated calls to the same generated input remain distinct slots. Genmodule
owner associations retain their independent exact producer coverage.

Complete call accounting and compile-metadata extraction are separate scopes.
Native MetaMake skips missing dependency names, but rejects an unknown requested
root. Exact coverage of its existing physical owner calls is therefore not a
closed dependency-graph claim. A data-only direct capture may retain unresolved
dependency names in its independent Source metadata; it must not delete them,
turn them into known targets, or treat them as produced headers/archives. All
requested roots and required compile-owner pairs must exist. The strict closed
dependency check remains required wherever producer completeness is claimed;
capture consistency alone cannot satisfy a build or SDK acceptance gate.
Keep the full independently selected native owner/goal roster, including header
and SDK dependencies. Before reading compiler recipes, match each required
Mesa graph declaration to its exact original Source input and actual observed
goal in that roster. Missing or ambiguous matches refuse the entire configured
selection; do not normalize `-quick` goals or guess an owner from a basename.
Different required goals may share a captured child when that child actually
contains both goals. Each selected child's entire input program and original
assignment positions remain mandatory. Sibling child programs do not supply
one another's Make variable state. Interpreting every unrelated sibling is not
a prerequisite for deriving one compiler recipe, but focused derivation never
establishes complete native execution, dependency builds or SDK qualification.

The test-only `configured_native_source::read` adapter connects these retained
slots to the generic native Source reader. It is not a production API or an
active graph handoff. It checks the retained source/build coordinates and actual
source content identity. The captured input selection must also contain the
exact prepared-build root and digest retained with the call, so a freshly
captured but foreign preparation cannot replace it. The input transaction
revalidates live roots before and after reading. The adapter then checks the
generated input and goals
against the caller-selected native owner projection. The result is the generic
variable program, positioned file events and verified reads. Source formulas,
immediate assignments, target-specific freezes and automatic-variable frames
remain Source/interpreter semantics, not a Mesa formula reconstruction table.
This proves neither that all required calls were captured nor that a generated
Makefile was produced from its corresponding Source input; the owning live
producer must still bind both facts.

Both readers check bounded, digest-selected context/control/packet data and
the configured identities. The filesystem reader also checks the prepared
tree's actual content digest. The rootless reader can check only retained data
consistency, not the physical tree, process execution, complete preparation or
authenticated ownership. Raw observations remain private and are not diagnostics.

`MakeVariableProgram::evaluate_many_captured_target_recipes` evaluates the
selected deferred recipes and Source exports for multiple target/automatic
frames in one staged program and one captured filesystem transaction. Frames
remain isolated, aggregate work/storage limits apply, and any later failure
prevents publication of the entire batch. This lets C, C++ and ABI-generator
recipes use the same Source state without inventing language-specific flag
formulas or accepting independently queried partial results. A GNU Make
dry-run oracle covers immediate target freezes, later recursive changes,
duplicate ordered flags and the frozen ABI-generator assignment together.

The real Source `compile_q` template wraps its command in an `if` condition
with redirections and nested diagnostic branches. The read-only shell-data
evaluator remains unchanged in authority: it cannot execute compiler commands.
A separate syntax-only literal recipe reader now retains every command and
if branch, literal argv, and ordered file/descriptor redirections. Dynamic
parameters, assignments, pathname expansion and unsupported control
syntax refuse even in unselected branches. The shared lexer enables
redirections only in this new syntax mode; data evaluation still refuses them.

`MakeRecipeProjection::parse_literal_shell` separates explicit Make @/-/+
prefixes from that inert shell AST. This is one-logical-recipe data, not
`.ONESHELL` semantics, inferred recursive-Make behavior or launch authority.
The shared `original_shell_text` operation retains the exact body after the
existing leading whitespace/prefix scan, without shell parsing; it grants no
execution authority. On 2026-10-22 Fabian
authorized controlled original-recipe execution **only as a Mesa 26 exception**.
Admission must bind each recipe to its independently selected Mesa 26 Source
owner and runtime, not merely a requested version, filename or transitive build
dependency. Mesa 20 and unrelated recipes retain the static policy. The exception
does not claim advance enumeration of every possible inner shell command/input;
all identity, environment, process-budget, generated-input/effect ownership and
readback obligations remain. A closed environment is not filesystem confinement.
The current raw GCA adapter requires exactly its selected single goal; membership
in a multi-goal observation plus a matching first-parent chain is insufficient.
The literal reader and single-command recorder stay unchanged, and native graph
projection still refuses until its owning execution transaction is implemented
and verified. This decision does not authorize publication or a new diagnostic.
The real Source template is GenMF-expanded in a fixture and interpreted by the
generic Make reader. Its resulting AST matches a GNU Make dry run. A second
oracle runs the original Source wrapper with a private argv recorder instead
of a compiler; exact NUL-separated arguments match the projected command,
including repeated ordered flags and quoted spaces/embedded quotes. No target
compiler or object generation is involved. Template bytes are checked unchanged.

The retained child Source now derives an explicit-rule recipe plan from the
original observed goals and its original captured input snapshot. It retains
source positions, recipe-provider prerequisite order, first-parent frames,
automatic variables, and all deferred recipe expressions. Implicit declarations
and non-rule policies remain obligations, not silently satisfied rules. A new
snapshot cannot replace changed original preparation after interpretation.

`DeclaredChildRecipeProjection::with_original_recipe` reads an explicitly indexed
expanded expression together with its retained event origin and matched child
coordinates under that same original snapshot transaction. A missing index,
callback failure or input drift prevents the returned value from being released.
It neither parses nor executes the shell body. This is data consistency, not
proof of generated-Makefile production, Mesa 26 ownership admission or runtime
permission. The callback must remain data-only; before/after verification does
not prevent or roll back a callback's external side effects. Existing static
recipe binding and the native graph refusal remain unchanged.

For an entry-program recipe, `with_source_generated_recipe` additionally compares
the actual generated program from the held child snapshot with the raw-byte
digest of that owner's original deterministic Source/GenMF expansion. The owner
projection retains those digests when it derives its metadata graph, not from a
caller-provided generated file or receipt. Source is revalidated before and
after the callback. Included recipe programs are refused until separately bound;
entry-program agreement must not bless an unrelated include. This establishes
program-byte consistency, not proof that a native GenMF process ran, complete
include/variable-frame equivalence, a Mesa 26 exception scope or permission to
execute. Raw program digests remain distinct from tree-CAS and payload digests.
For `.src` inputs they hash the expansion re-encoded in GenMF's native
ISO-8859-15 file-mode representation. `ExpandedText::native_bytes` is the strict
inverse of the existing input decoder: no replacement bytes or UTF-8 fallback.
Direct, non-GenMF inputs retain their original raw byte digest. Candidate native
files are never normalized before comparison. Text/provenance offsets still
refer to the internal Unicode text, not to native output bytes. Synthetic
Source/template probes do not qualify actual native GenMF output.

The Mesa 26 exception has a separate private-construction binding for the exact
VC4 GCA owner, selected GCA goal and its direct archive prerequisite. Deeper
dependencies do not inherit admission. Original body, exports, shell controls
and reserved selectors are evaluated together in the concrete target frame;
unknown policy selectors must remain undefined, not merely empty. Both original
child interpretation and later binding require the native default directory
context, rather than importing a separate transpiler directory table. Command
preparation reuses the existing selected-shell/environment checks and pinned
preflight invocation context; it establishes no new clock or execution owner.

Output comparison uses that binding's captured build root and all retained
Source, recipe-evaluation and frame file-read groups. Other root bindings and
measurement domains remain unchanged; retained inputs cannot be declared mutable
outputs. Reads retain direct/include operation kind and original spelling, so
replay checks include search and optional absence, not just previously selected
file bytes. This does not prove directory-query equivalence, producer lineage,
ordering or authority to write selected paths, and does not refresh the original
snapshot. The consuming live phase must establish those obligations before any
GCA launch; subsequent recipe evaluation cannot reuse this comparison as proof
that earlier wildcard/query results remain valid.

An archive update may replace its output inode rather than modify it in place.
The output comparison therefore distinguishes in-place-only files from explicitly
replaceable existing regular outputs. Replacement retains filesystem device and
mode; deletion, creation under a replacement selection, symbolic links, directory
replacement and changes to unselected entries refuse. Retained Make inputs cannot
be selected as replaceable outputs, even when replacement bytes are identical.
Only ancestor directory timestamps/size may vary for namespace effects. The old
in-place comparison remains strict. This is still a snapshot comparison, not
producer authority, filesystem confinement or rollback; root permissions and
any ownership guarantee need separate live checks.

The consuming original-owner path now constructs one recipe binding from the
owner's own observed child slot inside the existing data-only Source batch.
Only after its postcheck may it pass the pinned invocation to the existing
bounded runner with the original start time and cancellation. Direct normal and
order-only prerequisites must be independently captured regular files, disjoint
from outputs; expanded phony markers refuse even if a same-named file exists.
This explicit captured-input observation is not Make timestamp scheduling or
proof that prerequisite producers ran. A later recipe line requires consuming
the immediately preceding successful outcome of that same frozen rule;
unsupported ignore-error/recursive controls refuse rather than skipping work.
The entire original output root, not just the child build subtree, is checked
against selected effects; separate private logs are checked against the captured
streams. There is no producer/preflight/SDK conversion from this result.
Actual GCA generator/list/member handoff and execution remain required.

The selected first-recipe process path is exercised through the full collected
owner using synthetic Source, a fixture MetaMake and pinned actual GNU Make
sampling. Success, nonzero child, unselected-write and prelaunch cancellation
checks do not establish upstream MetaMake or GCA execution. The live preparation
environment, including its deterministic locale, is retained rather than rebuilt
from request variables alone. Source-unexported variables must not be reintroduced
by extending that environment; the measured Source-export map replaces it.
Selected tool reports and search path remain independently checked. There is no
public data-binding-to-Command bypass; preparation stays inside the consuming
owner path.

The controlled execution path adds a separately measured serial recipe environment
to the generic Source-export projection. The optional engine packet preserves
GNU's opaque CLI-propagation scalar rather than reconstructing its escaping;
legacy packet memberships do not acquire automatic variables. Missing relevant
engine data refuses execution. The projection binds the original typed invocation,
increments measured MAKELEVEL, preserves effective flags and GNU's independent
incoming/exported SHELL semantics. Source engine-state mutations, dry-run and
parallel/jobserver startup refuse. Complete-map GNU differential tests cover this
bounded subset at direct-exec and shell process boundaries; this is not general
native environment equivalence or productive graph admission. Staged per-line
producer ownership and actual GCA execution remain required.

Same-rule continuation retains the original collection snapshot rather than
rebasing it. Each next step revalidates the exact preceding output snapshot and
all held prior evidence directories, stage invocations and evidence trees. The
original owner, start/deadline and cancellation remain in force. A namespaced
frozen-plan digest binds expanded rule target/header/prerequisites and automatic
scope, every original recipe-event origin and all line expressions/exports/serial
environments/SHELL/flags. Reprojection must match that plan before the exact next
line may launch; jumps, replay and changing child/rule refuse. Every output
transition is independently checked from its immediate predecessor. There is no
public reconstruction or resume-from-files API. A late failure consumes the live
state without promising rollback. This does not authenticate prerequisite
producers, switch rules, discharge generated-list membership or yield a completed
GCA/preflight/SDK receipt. Those remain distinct integration requirements.

The separate dependent-rule transition consumes the completed preceding rule's
live state. It retains the original child and unchanged Mesa26 admission policy;
it does not broaden the exception to deeper or foreign dependency recipes. A
frozen declared-graph identity binds rule/header provenance and first-parent
scope as well as expanded recipes. Every prerequisite must be either an
untouched original captured input or a regular output retained from a fully
successful producer rule. At least one completed producer input is required for
the dependent transition. Declared effect footprints conservatively invalidate
older output lineage on any overlap, even when bytes happen to remain equal.
Only the final successful line may record a written regular declared target.
Reached recipe-less, non-phony aliases with exactly that one normal prerequisite
may describe additional outputs; each alias must still be a real regular file
within the producer's successful write scope. File presence or an undeclared
side file never creates lineage. Competing producers and repeated completed rules
refuse. This is controlled local execution lineage, not artifact authentication,
timestamp scheduling, a static admission for transitive dependencies, or proof
that the real GCA private-member list and complete affected SDK flows work.

Generated path-list admission is a separate data operation on that same live
owner. The list must be a declared completed output newly created by its
producer. Members must be regular files newly observed during that same producer's transition,
below an independently selected captured directory. An allowed effect scope
alone does not prove creation; preexisting files and later overlapping foreign
effects refuse. Current output, original inputs, retained evidence and original
lifetime are checked around the no-follow captured read. Preserve list order
and duplicate entries, with explicit byte/record/storage bounds; accept only
normalized absolute literal shell-word paths, not substitutions, globbing or
whitespace-dependent names. An empty list is valid. No member count or private
object naming formula is assumed. This admission neither runs the archive loop
nor widens the raw-recipe scope; actual generator/static producer execution and
the complete real GCA consumer/provider chain remain required.
This is before/after transition evidence, not proof of causal process authorship
against concurrent writers: the existing runner provides no filesystem isolation.
Source rules must explicitly declare generated files they consume, including
lists read inside shell recipes. A graph-only adapter must not invent such
dependency edges by recognizing command strings. The VC4 GCA archive therefore
declares its private-member list alongside its existing generated inputs; the
ordinary Source alias then makes the stamp relationship reachable. This does
not admit deeper producer execution or prove any archive was actually built.
Reachability and common Source-file ownership alone do not select additional
raw-shell producers. Any intrinsic Mesa26 GCA producer admission must separately
bind its explicit Source-owned producer selection, expanded header/recipe origins,
cooutput alias relation and consuming archive frame. Other prerequisite producers
keep the existing static policy. Do not infer an exception from command names,
path basenames or an arbitrary transitive visit, or remove the default depth
refusal to make the generator run.

The optional Source policy `mesa26_original_producers` selects a bounded tuple
of original owner input, exact goal, producer, cooutput and consumer expressions.
Expand these expressions in the original selected target frame; require one
unique reached triple, distinct normalized build outputs, a recipe-less alias
with one normal producer prerequisite, and the direct consumer's normal alias
edge. Every contributing ordinary rule declaration and every recipe line must
originate in the exact Source-expanded owner; inspecting only the effective
recipe-bearing header would miss merged prerequisite declarations. Retain the
expanded role selection in the frozen runtime plan so later frame changes cannot
silently substitute another alias between recipe lines. This is an execution
scope consistency binding, not artifact authentication. Absent explicit policy,
the existing depth refusal remains; no transitive prerequisite is admitted by
this tuple unless it is the selected intrinsic producer itself. Real input
archives, ordered execution and generated-data readback remain separate gates.

### Original owner environment before genmodule completion

The production genmodule collection must receive the original captured child
invocation and its independently selected Source coordinates while the same
preparation owner and deadline are still live. A configuration-root prefix
cannot stand in for MetaMake's `CALL` environment: equal value strings do not
preserve environment/file/command-line origin or `?=` and override semantics.
Do not lift the existing refusal merely because a literal environment prefix
can be parsed. Reuse the retained invocation, Source child reader and bounded
execution infrastructure rather than adding another environment reconstruction
or an unscoped completion mode.

An inventory helper's selected producer tuples must join unique original
observed Source coordinates and goals in the complete retained call roster.
Derive the Source input from those matched coordinates, not a conventional
filename. Bind parsed inventory bytes to the retained evidence snapshot, helper
and held Source selection; revalidate them with the same owner and deadline.
This association alone neither proves the tuple from the parsed Source program
nor produces the generated module Makefile or admits the native graph.

The scanner's evidence directory retains its exact JSON/stdout/stderr set and
the persistent empty publication lock derived by the scanner's shared publisher
for that JSON output. Validate the lock through a no-follow regular-file lookup,
require a single link, and bind it in the complete evidence tree. Do not delete
the inode or admit arbitrary lock-shaped names, journals, or other extra files.

The existing native sampler stops before the first selected Source input. Its
`FirstInputPrelude` record is not a post-parse effective-owner-scope sample.
Any handoff must state and verify the actual measurement phase, bind argv/argv0,
cwd, full environment, Source/Make inputs and the selected generator owner, and
retain the original lifetime and output checks. A parsed Source projection is
not itself a GNU Make observation. Required counterprobes include environment
versus file-origin defaults, overrides, changed invocation or Source mapping,
and an attempted post-source interpretation of a prelude-only record. The
complete ordinary-CLI Mesa build must exercise this handoff; fixture scope
success alone is insufficient.

Generic captured program sets bind exact invocation spellings to independently
selected root and executable digests. Aliases sharing executable bytes retain
their distinct argv[0] paths. Every command in every branch remains visible;
unknown commands are not dropped. The Source plan binds its whole recipe batch
while both original Source/preparation and selected program roots remain stable.
Explicit argument slots may bind auxiliary program roles, but neither basename
matching nor option-name inference chooses those slots. This data consistency
boundary does not authenticate a producer or close interpreter dependencies.

The literal shell AST also retains simple-command pipelines, including every
stage's argv and ordered redirections. Program bindings retain a stage position
for every command. This permits Source archive recipes to pass through the same
reader without an archive-command special case. Pipe status, concurrent process
execution, stdin contents, and tools that reinterpret stdin remain separate
runtime obligations. The read-only shell-data evaluator still refuses pipes;
syntax retention does not grant execution authority.

Explicit artifact requests now select byte spans in non-command argv words or
complete file-redirection targets. No compiler option spelling chooses a field.
The plan-owned transaction descriptor-reads each selected input as a regular
file, including non-executable scripts and binary inputs. Outputs must be absent
or regular files under the original matched build root, whose selected content
must also match the original snapshot. A different captured Source root cannot
be substituted as the output scope. All outputs require real
captured parent directories; symlinks and missing intermediate directories are
not new output permissions. Terminal directory-only spellings refuse. Exact
argv, branch and pipeline positions remain unchanged. Duplicate/overlapping
field selections refuse, while repeated output paths across commands remain
ordered data, not a unique-producer assertion. Programs and file fields bind
inside one original Source/preparation transaction, without replacing snapshots
or rehashing compiler roots per object. Recipe tracing also refuses a changed
build coordinate after the original child match. The owning adapter must still
bind the configured preparation authority and qualify scheduling,
write policy, all required fields and producer ownership.

Next, the native graph must consume these source-positioned recipe plans,
program bindings and artifact fields for per-object compilation and the GCA
consumer together. Production field selection/closure, script/interpreter
closure, complete rule semantics and the live producer/phase/input closure remain
open. Stripping the
wrapper or recognizing a copied compiler-option formula would not establish
equivalent compilation. Original Source recipes, not reconstructed option
formulas, must supply the complete compiler argv.

The actual preflight producer now transports its independently measured compiler
root and exact C/C++ invocation identities in optional receipt data. It uses the
selected driver binding rather than deriving a prefix from pathname layout.
Physical input loading checks the closed compiler-role selection against the
configuration and captures the program set around final input-root validation;
two whole-prefix scans protect the transaction, followed by an input-root drift
check. Subsequent input revalidation
also checks this external compiler root. Portable receipt reading validates only
the metadata relationship against both independently selected root path and tree
digest, and never opens a local compiler prefix. Historical
receipts without this field retain no program binding and gain no native graph
admission. This closes compiler identity transport, not host interpreter/tool
closure, live native preparation, graph execution, or compile/GCA equivalence.
The outer Mesa selection also compares its retained compiler root with the
independent LLVM SDK selection or checked byte proof during owner, observation,
retained, and portable readback. Rewriting both a Mesa receipt and its outer
selection therefore cannot replace that independent compiler coordinate.

The new data must not enter the old copied-formula projection. Productive native
graph projection therefore refuses until the generic captured Source program
is connected to compilation and GCA hashing together. The live producer,
complete input closure and actual affected SDK workflows are still required;
reader tests alone do not establish Mesa compilation or ABI parity.

The immediate live-producer gap is concrete: ordinary preflight emits only
versions 3/4/7/8. Native v9 input reading and capture launchers exist, but the
producer does not yet supervise the complete original host MetaMake build
and collection launch. A separate first-phase API now consumes the original
prepared configuration and observes the Source Root query. Its caller selects
literal variable names and goals, not copied CPU or compiler-option formulas.
It retains the exact fatal-stop marker, requested values, measured input bytes
and private evidence directory before the include-remake boundary. Original
Source/prepared-input digests bind the reported Make input list; that list is
not an independent claim of execution completeness or authentication. Source
parse-time expressions still execute: selected Source trust and exclusive
ownership remain caller preconditions, not sandbox guarantees. The opaque
observation preserves the original preparation and deadline but emits no
completed receipt, graph input or native admission. Later native phases must
consume that same state rather than reconstruct it from serialized data.

The ordinary stage runner still accepts only exit zero. The shared process
observer can retain an explicitly selected
exact exit outcome under the original owner deadline, including nonzero query
outcomes. It rejects different codes, signal termination, timeout, cancellation,
and post-child input changes while retaining bounded streams. This does not
validate the Root query's protocol: observing exit two alone is not a successful
Root query, and no native phase is admitted by that observation. The caller must
bind the selected query, its fatal-stop marker and complete measured inputs
before the include-remake boundary. A failed ordinary phase must not be
reclassified as an expected query outcome. The native producer must retain actual completed call
packets and bind their generated Source/owner coordinates and prepared tree,
rather than manufacture those records from the reader fixtures. Native local
version 10 remains a local-source format, not portable release proof.
