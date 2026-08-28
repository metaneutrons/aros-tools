# GNU Make compatibility boundary

The normative language reference for MetaMake expression evaluation is the
[GNU Make manual](https://www.gnu.org/software/make/manual/make.html), in
particular its [function reference](https://www.gnu.org/software/make/manual/html_node/Functions.html),
[text functions](https://www.gnu.org/software/make/manual/html_node/Text-Functions.html),
[wildcard semantics](https://www.gnu.org/software/make/manual/html_node/Wildcard-Function.html),
[substitution references](https://www.gnu.org/software/make/manual/html_node/Substitution-Refs.html)
and [`call`](https://www.gnu.org/software/make/manual/html_node/Call-Function.html).

`aros-transpiler` implements the complete deterministic expression vocabulary
used by the current AROS MetaMake declarations:

- nested `$(...)` and `${...}` variable references, computed names, recursive
  and simply-expanded assignments, append/default assignments, and suffix or
  `%` substitution references;
- `addprefix`, `addsuffix`, `subst`, `patsubst`, `strip`, `findstring`,
  `filter`, `filter-out`, and `sort`;
- `word`, `wordlist`, `words`, `firstword`, `lastword`, `join`, `dir`,
  `notdir`, `suffix`, and `basename`;
- lazy `if`, `or`, and `and`, temporary `foreach` bindings, general user
  `call`, and the AROS `call WILDCARD` helper;
- deterministic, sorted `wildcard` evaluation against source and materialised
  Port trees.

The pure-function corpus is tested differentially against a locally available
GNU Make. A missing GNU Make executable skips only that oracle test; fixed Rust
expectations still run everywhere.

This is deliberately not a shell or GNU Make interpreter. `shell`, `eval`,
`file`, and `guile` can execute host commands, mutate the parser, or introduce
ambient inputs. They remain unsupported in build declarations and must produce
an explicit `MakeExprError` with the expression and owning declaration. Known
reproducible metadata forms such as AROS build dates are translated by their
own audited capability rather than executing their original shell fragment.

Hand-written generated-header recipes cross a separate, equally strict
boundary. Exact `$<` to `$@` copies, anchored literal substitutions, and
literal token-substitution pipelines are translated into declarative CMake
products. Arbitrary recipe commands are never replayed through a shell. A
recipe that drifts outside those proved shapes remains an explicit unmodelled
rule until the transpiler gains a reviewed capability and regression test.

Exact Python recipes form another declarative lane. The transpiler resolves
`$@`, `$<`, `$^`, `$(dir $@)`, an optional literal `cd ... &&`, direct output
arguments, and `> $@` standard-output capture. It emits an argument-vector
custom command without a shell, attaches any `%fetch` target that materialises
the script or its inputs, registers every output before source discovery, and
binds the output owner to its source or aggregate consumers. Standard-output
generation is written to a temporary file and atomically installed only after
the script succeeds. Recognisable rules for dependency versions that are not
reachable from the selected build graph remain inactive; merely finding a
recipe is not permission to fetch or execute it.

An unknown function, unresolved variable, unsafe conditional, recursive cycle,
deferred Port wildcard, malformed expression, or unsupported automatic
reference is an error. It must never become an empty list or a partial target.
When upstream adds a new construct, the correct response is to extend this
documented vocabulary and its GNU Make differential corpus, or fail with an
update-required diagnostic.
