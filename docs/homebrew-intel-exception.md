# Temporary macOS Intel Homebrew PR exception

**HB-2026-09-05 — active, incomplete qualification, not a release waiver.**
Approved by Fabian on 2026-09-05; expiry explicitly set to **2026-10-05,
00:00 UTC**. Owner: Fabian (`@metaneutrons`).

## Scope and enforcement

Only the macOS Intel **Homebrew installation job in pull-request qualification**
is paused. Native Intel compilation, archive verification and workspace tests
still run. Both Linux architectures and Apple-silicon Homebrew still run.
The shared Homebrew tap and other projects are unchanged.

| Qualification context | Required Homebrew hosts |
| --- | --- |
| Pull request within the exception window | Three; macOS Intel explicitly unqualified |
| Tag push, including a prerelease | All four, including genuine Intel |
| Manual dispatch, whether branch or tag | All four, including genuine Intel |
| Pull request at or after expiry | Policy failure before building; explicit review required |

The [reviewed policy](../scripts/release/homebrew-qualification.json) is the
single source of the matrix and exception dates. The
[selector](../scripts/release/homebrew-matrix.py) uses the actual event, ref and
current UTC date, not the source commit's reproducibility epoch. The allowed
window starts on 2026-09-05 and ends exclusively at the expiry above. The
exception has a maximum thirty-day duration; it cannot silently renew itself.
Setting `pr_exception` to `null` restores all four PR hosts.

Each affected run writes an explicit **temporarily UNQUALIFIED** notice and a
Homebrew scope summary. A successful three-host PR run is not a successful
four-host package qualification. All native archives are still built; omitting
one installation test must not be described as full Intel Homebrew support.

Release configuration, channel preflight and publication each require both
`homebrew_coverage == 'four-hosts'` and a successful Homebrew matrix result.
Missing coverage, an exception, failed/skipped/cancelled installation and
invalid policy data do not grant publication. There is no `continue-on-error`,
replacement ARM runner, dependency pin, API-path protection bypass or alternate
Homebrew source mode. A failed dependency post-install remains a hard error.

## Why the exception exists

The genuine Intel check in
[run 33987257343, debug job 101367689886](https://github.com/metaneutrons/aros-tools/actions/runs/33987257343/job/101367689886)
failed inside Homebrew **6.0.22**, commit
`08e85c4e42f5d8f1ea17c36cb59cf61c2ccb26c3`. OpenSSL 3.6.4 compiled and its
dylibs passed codesign verification. Homebrew then rejected its own cached
API-source formula in `FormulaInstaller#post_install_formula_path`, before the
dependency's post-install hook began. The overall installer exited nonzero;
`AP7322` correctly rejected it. AROS's installed-byte checks did not run.

Automatic source selection creates a source-backed formula, but
`build_from_source?` records only **explicit** source requests. Both existing
early returns therefore miss the automatic case; reloading the API-cache path
trips the normal path guard. See the
[selector and its guards](https://github.com/Homebrew/brew/blob/08e85c4e42f5d8f1ea17c36cb59cf61c2ccb26c3/Library/Homebrew/formula_installer.rb#L1423).
[Homebrew PR #20992](https://github.com/Homebrew/brew/pull/20992) fixed the
explicit-request case, not this automatic transition.

Independent isolated checks on 2026-09-05 reproduced the same failure on
Apple silicon with the real `ca-certificates` API formula, both on the exact
runner commit and inspected upstream main
`d55434689b260b5dd7e97dbfd6f559126a213099`. The diagnostic `--cc=clang` option
selected a source install without marking an explicit source request; it is
not an installation recommendation. The main installer/API/path/environment
test set had 220 examples, with only the newly added automatic-source
regression failing. No Homebrew production code was changed or fix submitted
by this investigation. Intel's missing OpenSSL bottle exposes the defect;
the defect itself is not architecture-specific.

An earlier green `macos-x86_64` label in run `33985724773` used an ARM runner
and is **not** evidence. The current native-host verifier prevents that
substitution. Repeated identical failures prompted this bounded PR exception
to avoid spending runner capacity on the known failure during development.

## Removal checklist

- [ ] Identify and review the actual upstream correction; record its exact
  Homebrew revision and regression evidence. A newer version number alone is
  not evidence, nor is a temporary bottle avoiding the defective code path.
- [ ] Run a deliberate manual `Release qualification` at the exact candidate
  commit. Manual runs retain all four hosts even while the exception is active.
- [ ] On genuine `macos-15-intel`, require successful dependency post-install,
  native host/prefix checks, exact staged/installed bytes for all eight tools,
  formula tests and every version check. Record the run and job IDs.
- [ ] Set `pr_exception` to `null`; keep the closed four-host inventory and
  regression tests. Run PR qualification with the restored Intel lane.
- [ ] Record the removal commit/date and successful four-host run here and in
  [initial release closure](initial-release-closure.md). Preserve this history.
- [ ] Before release, still pass the complete tag and App-authenticated
  four-host tap qualification. PR acceptance is not that evidence.

At expiry, `AP7331` fails PR qualification with this document as the recovery
pointer. Do not extend the date automatically or suppress that error. Any new
deadline or different workaround needs an explicit maintainer decision and
reviewed policy/documentation changes.

## Regression commands

```sh
python3 scripts/release/test-homebrew-matrix.py
python3 scripts/release/test-homebrew-app.py
bash scripts/release/check-actions-policy.sh
bash scripts/release/test-release-policy.sh
actionlint
```

These offline tests exercise real selector exit codes, absence of outputs on
rejection, exact date boundaries, manual/tag exclusion from the waiver, unsafe
or ambiguous policy files, restoration with `null`, and the actual publication
conditions with positive and negative inputs. They run in the normal workspace
quality gate. They do not install Homebrew or substitute for a live tag run.
