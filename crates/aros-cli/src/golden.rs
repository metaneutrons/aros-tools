//! A baseline of the transpiler's output, and a comparison against it.
//!
//! The decomposition of `aros-transpiler` rests on one claim: that moving code
//! changed no generated CMake. Until now that claim was checked by hand -- run
//! the transpiler, note a sha256, run it again after the change, compare the
//! two by eye -- which is both unrepeatable and silent about *what* differs.
//!
//! Three things make this harness rather than a pinned digest:
//!
//!   * **The output is not pinned in the repository.** A digest of a live
//!     output would be stale after every deliberate transpiler change, and a
//!     test that is nearly always red is not a gate (the same coupling
//!     OPEN-POINTS 7 and 46 record for the fingerprints). The baseline is
//!     captured on demand, into the ignored build tree, and belongs to the
//!     refactor that captured it.
//!   * **The run is replayed, not re-derived.** CMake writes the argv it used
//!     next to its output, because the scoped arguments (`--family`,
//!     `--variant`, `--cpu32`, `--use-mmu`, `--float-abi`) are derived during
//!     configuration. They change the output: the unscoped run of this tree
//!     produces 873 concrete targets and no configure, GRUB2, AHI or Python
//!     groups, while the pc-x86_64 run produces 901 and all four. One golden
//!     file therefore cannot cover every preset, which is what point 13
//!     assumed.
//!   * **Capture proves determinism before it trusts itself.** It runs the
//!     transpiler twice into separate directories and refuses to store a
//!     baseline if the two runs differ, because a baseline from a
//!     non-deterministic producer would report noise as regression forever.
//!
//! Output enumeration is by construction rather than by list: each run writes
//! into an empty directory, so whatever is in it afterwards is what the
//! transpiler wrote. That matters because `generated_targets.*` in a build tree
//! is a mixed family -- `binary-object-gaps.txt`, `arch-override-gaps.txt`,
//! `kickstart-sets.txt` and others are written by CMake, not by the transpiler.

use miette::{Context, IntoDiagnostic, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The name CMake writes its recorded argv under, next to the output it
/// produced.
const INVOCATION_SUFFIX: &str = "generated_targets.cmake.invocation";

/// One file in a captured or freshly produced run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Product {
    pub name: String,
    pub bytes: usize,
    pub lines: usize,
    pub sha256: String,
}

/// How one file compares between a baseline and a fresh run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added(Product),
    Removed(Product),
    /// Same name, different content. `first_differing_line` is 1-based, or
    /// `None` when the files share a prefix and one simply ends earlier.
    Changed {
        baseline: Product,
        fresh: Product,
        first_differing_line: Option<usize>,
    },
}

impl Change {
    fn name(&self) -> &str {
        match self {
            Self::Added(product) | Self::Removed(product) => &product.name,
            Self::Changed { fresh, .. } => &fresh.name,
        }
    }
}

/// The verdict of one comparison.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    pub identical: usize,
    pub changes: Vec<Change>,
}

impl Comparison {
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.changes.is_empty()
    }
}

/// How much longer the fresh file is, in whichever unit is passed.
///
/// Reported as a signed number because that is the first thing worth knowing
/// about a changed file: same length and different content is a different kind
/// of change from a few added lines.
fn delta(fresh: usize, baseline: usize) -> i64 {
    i64::try_from(fresh).unwrap_or(i64::MAX) - i64::try_from(baseline).unwrap_or(i64::MAX)
}

/// Digest, size and line count of one file.
///
/// The line count is carried because it is what makes a diff readable at a
/// glance: "+3 lines" is a different kind of change from "same length,
/// different content", and the second is the one worth looking at closely
/// during a refactor.
fn describe(directory: &Path, name: &str) -> Result<Product> {
    let path = directory.join(name);
    let bytes = fs::read(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading {}", path.display()))?;
    Ok(Product {
        name: name.to_owned(),
        bytes: bytes.len(),
        // A byte loop over a few megabytes, once per file per run, next to a
        // sha256 of the same bytes. A faster counter would need a dependency
        // for no measurable gain here.
        #[allow(clippy::naive_bytecount)]
        lines: bytes.iter().filter(|byte| **byte == b'\n').count(),
        sha256: aros_common::sha256_bytes(&bytes).to_string(),
    })
}

/// Every file directly in `directory`, which for a run directory is exactly
/// what the transpiler wrote there.
fn products(directory: &Path) -> Result<BTreeMap<String, Product>> {
    let mut found = BTreeMap::new();
    let entries = fs::read_dir(directory)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading {}", directory.display()))?;
    for entry in entries {
        let entry = entry.into_diagnostic()?;
        if !entry.file_type().into_diagnostic()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == MANIFEST_NAME {
            continue;
        }
        found.insert(name.clone(), describe(directory, &name)?);
    }
    Ok(found)
}

/// The first line number at which two files differ, 1-based.
fn first_differing_line(left: &Path, right: &Path) -> Option<usize> {
    let left = fs::read_to_string(left).unwrap_or_default();
    let right = fs::read_to_string(right).unwrap_or_default();
    left.lines()
        .zip(right.lines())
        .position(|(left_line, right_line)| left_line != right_line)
        .map(|index| index + 1)
}

const MANIFEST_NAME: &str = "manifest.txt";

/// The argv CMake used, one argument per line, with `--output` redirected at
/// `output`.
///
/// Replacing the output is the whole point: the recorded run wrote into the
/// build tree, and a comparison run must write somewhere empty without
/// disturbing it.
fn replayed_arguments(invocation: &Path, output: &Path) -> Result<Vec<String>> {
    let recorded = fs::read_to_string(invocation)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading {}", invocation.display()))?;
    // One argument per line, empty lines included: `--variant ""` and
    // `--float-abi ""` are real values, and dropping the empty line would
    // shift every following argument onto the wrong flag.
    let mut arguments: Vec<String> = recorded.lines().map(str::to_owned).collect();
    if arguments.is_empty() {
        miette::bail!(
            "{} is empty; reconfigure the preset so CMake records its argv",
            invocation.display()
        );
    }
    let Some(index) = arguments.iter().position(|argument| argument == "--output") else {
        miette::bail!(
            "{} does not carry --output; it was not written by this build system",
            invocation.display()
        );
    };
    let Some(value) = arguments.get_mut(index + 1) else {
        miette::bail!("{} ends after --output", invocation.display());
    };
    *value = output.to_string_lossy().into_owned();
    Ok(arguments)
}

/// Runs the transpiler once into an empty directory and returns what it wrote.
fn run_into(transpiler: &Path, invocation: &Path, directory: &Path) -> Result<()> {
    fs::create_dir_all(directory).into_diagnostic()?;
    let output = directory.join("generated_targets.cmake");
    let arguments = replayed_arguments(invocation, &output)?;
    crate::observability::run_quiet_command(
        Command::new(transpiler)
            .args(&arguments)
            .stdout(std::process::Stdio::null()),
        &format!(
            "{} while replaying {}",
            transpiler.display(),
            invocation.display()
        ),
    )?;
    if !output.is_file() {
        miette::bail!("{} produced no {}", transpiler.display(), output.display());
    }
    Ok(())
}

/// Compares two directories of products.
pub fn compare(baseline_dir: &Path, fresh_dir: &Path) -> Result<Comparison> {
    let baseline = products(baseline_dir)?;
    let fresh = products(fresh_dir)?;
    let names: BTreeSet<&String> = baseline.keys().chain(fresh.keys()).collect();
    let mut comparison = Comparison::default();
    for name in names {
        match (baseline.get(name), fresh.get(name)) {
            (Some(old), Some(new)) if old.sha256 == new.sha256 => comparison.identical += 1,
            (Some(old), Some(new)) => comparison.changes.push(Change::Changed {
                baseline: old.clone(),
                fresh: new.clone(),
                first_differing_line: first_differing_line(
                    &baseline_dir.join(name),
                    &fresh_dir.join(name),
                ),
            }),
            (None, Some(new)) => comparison.changes.push(Change::Added(new.clone())),
            (Some(old), None) => comparison.changes.push(Change::Removed(old.clone())),
            (None, None) => unreachable!("name came from one of the two maps"),
        }
    }
    comparison
        .changes
        .sort_by(|left, right| left.name().cmp(right.name()));
    Ok(comparison)
}

/// The report a comparison prints.
///
/// A removed or added report file is as much a change as an edited one: the
/// transpiler deletes a report when it has nothing left to say, so a report
/// that disappears means the finding behind it disappeared too.
pub fn render(comparison: &Comparison) -> String {
    let mut text = String::new();
    for change in &comparison.changes {
        match change {
            Change::Added(product) => {
                let _ = writeln!(
                    text,
                    "  added    {} ({} bytes, {} lines)",
                    product.name, product.bytes, product.lines
                );
            }
            Change::Removed(product) => {
                let _ = writeln!(
                    text,
                    "  removed  {} (was {} bytes, {} lines)",
                    product.name, product.bytes, product.lines
                );
            }
            Change::Changed {
                baseline,
                fresh,
                first_differing_line,
            } => {
                let bytes = delta(fresh.bytes, baseline.bytes);
                let lines = delta(fresh.lines, baseline.lines);
                let at = first_differing_line.map_or_else(
                    || "one is a prefix of the other".to_owned(),
                    |line| format!("first differs at line {line}"),
                );
                let _ = writeln!(
                    text,
                    "  changed  {} ({bytes:+} bytes, {lines:+} lines, {at})",
                    fresh.name
                );
            }
        }
    }
    let _ = writeln!(
        text,
        "  {} identical, {} changed",
        comparison.identical,
        comparison.changes.len()
    );
    text
}

/// A build directory that carries a recorded invocation.
pub struct Subject {
    pub name: String,
    pub invocation: PathBuf,
    /// The output the recorded run itself wrote, used to check the record
    /// against the build tree it came from.
    pub build_output: PathBuf,
}

impl Subject {
    fn new(build_root: &Path, name: String) -> Self {
        let directory = build_root.join(&name);
        Self {
            invocation: directory.join(INVOCATION_SUFFIX),
            build_output: directory.join("generated_targets.cmake"),
            name,
        }
    }
}

/// What a capture produced.
pub struct Capture {
    pub products: usize,
    pub destination: PathBuf,
    /// Whether the replayed run reproduced the build tree's own output, or
    /// `None` when that output is absent.
    ///
    /// This is what checks the recorded argv end to end. A false does not
    /// invalidate the baseline -- a build tree that predates a source change is
    /// simply stale -- but it does mean the two are not describing the same
    /// tree, and that is worth knowing before trusting a comparison.
    pub reproduces_build_tree: Option<bool>,
}

/// Build directories to work on: the named ones, or every one that has a
/// recorded invocation.
///
/// Discovered rather than listed, so a fourth preset is covered without
/// editing this.
pub fn subjects(build_root: &Path, presets: &[String]) -> Result<Vec<Subject>> {
    let mut found = Vec::new();
    if presets.is_empty() {
        let entries = fs::read_dir(build_root)
            .into_diagnostic()
            .wrap_err_with(|| format!("reading {}", build_root.display()))?;
        let mut names: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.into_diagnostic()?;
            if !entry.file_type().into_diagnostic()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if build_root.join(&name).join(INVOCATION_SUFFIX).is_file() {
                names.push(name);
            }
        }
        names.sort();
        for name in names {
            found.push(Subject::new(build_root, name));
        }
        if found.is_empty() {
            miette::bail!(
                "no build directory under {} carries {INVOCATION_SUFFIX}; \
                 configure a preset first",
                build_root.display()
            );
        }
        return Ok(found);
    }
    for preset in presets {
        let subject = Subject::new(build_root, preset.clone());
        if !subject.invocation.is_file() {
            miette::bail!(
                "{} has no recorded invocation; configure that preset first",
                subject.invocation.display()
            );
        }
        found.push(subject);
    }
    Ok(found)
}

fn write_manifest(directory: &Path) -> Result<()> {
    let mut text = String::new();
    for product in products(directory)?.values() {
        let _ = writeln!(
            text,
            "{}  {:>9}  {:>7}  {}",
            product.sha256, product.bytes, product.lines, product.name
        );
    }
    fs::write(directory.join(MANIFEST_NAME), text)
        .into_diagnostic()
        .wrap_err("writing the baseline manifest")
}

/// Replaces `destination` with `source`.
fn replace_directory(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_dir_all(destination)
            .into_diagnostic()
            .wrap_err_with(|| format!("clearing {}", destination.display()))?;
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).into_diagnostic()?;
    }
    if fs::rename(source, destination).is_ok() {
        return Ok(());
    }
    // A rename across filesystems fails; copy instead. The run directory is a
    // temporary directory, which on macOS is on a different volume than a
    // build tree under /Volumes.
    fs::create_dir_all(destination).into_diagnostic()?;
    let entries = fs::read_dir(source).into_diagnostic()?;
    for entry in entries {
        let entry = entry.into_diagnostic()?;
        if !entry.file_type().into_diagnostic()?.is_file() {
            continue;
        }
        fs::copy(entry.path(), destination.join(entry.file_name())).into_diagnostic()?;
    }
    Ok(())
}

/// Captures a baseline for one subject, proving determinism first.
pub fn capture(transpiler: &Path, subject: &Subject, snapshot_root: &Path) -> Result<Capture> {
    let scratch = tempfile::tempdir().into_diagnostic()?;
    let first = scratch.path().join("first");
    let second = scratch.path().join("second");
    run_into(transpiler, &subject.invocation, &first)?;
    run_into(transpiler, &subject.invocation, &second)?;
    let repeat = compare(&first, &second)?;
    if !repeat.is_clean() {
        miette::bail!(
            "two runs of {} on {} disagree, so a baseline would report noise \
             as regression:\n{}",
            transpiler.display(),
            subject.name,
            render(&repeat)
        );
    }
    let reproduces_build_tree = if subject.build_output.is_file() {
        let recorded = fs::read(&subject.build_output).into_diagnostic()?;
        let replayed = fs::read(first.join("generated_targets.cmake")).into_diagnostic()?;
        Some(recorded == replayed)
    } else {
        None
    };
    let destination = snapshot_root.join(&subject.name);
    replace_directory(&first, &destination)?;
    write_manifest(&destination)?;
    let products = products(&destination)?.len();
    Ok(Capture {
        products,
        destination,
        reproduces_build_tree,
    })
}

/// Compares a fresh run against the captured baseline for one subject.
pub fn verify(
    transpiler: &Path,
    subject: &Subject,
    snapshot_root: &Path,
) -> Result<(Comparison, PathBuf)> {
    let baseline = snapshot_root.join(&subject.name);
    if !baseline.is_dir() {
        miette::bail!(
            "no baseline at {}; run `aros golden capture` before the change \
             you want to check",
            baseline.display()
        );
    }
    let scratch = tempfile::tempdir().into_diagnostic()?;
    let fresh = scratch.path().join("fresh");
    run_into(transpiler, &subject.invocation, &fresh)?;
    let comparison = compare(&baseline, &fresh)?;
    Ok((comparison, baseline))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(directory: &Path, name: &str, content: &str) {
        fs::create_dir_all(directory).unwrap();
        fs::write(directory.join(name), content).unwrap();
    }

    #[test]
    fn identical_directories_compare_clean() {
        let scratch = tempfile::tempdir().unwrap();
        let left = scratch.path().join("left");
        let right = scratch.path().join("right");
        write(&left, "generated_targets.cmake", "a\nb\n");
        write(&right, "generated_targets.cmake", "a\nb\n");
        let comparison = compare(&left, &right).unwrap();
        assert!(comparison.is_clean());
        assert_eq!(comparison.identical, 1);
    }

    #[test]
    fn a_changed_file_names_its_first_differing_line() {
        let scratch = tempfile::tempdir().unwrap();
        let left = scratch.path().join("left");
        let right = scratch.path().join("right");
        write(&left, "generated_targets.cmake", "a\nb\nc\n");
        write(&right, "generated_targets.cmake", "a\nB\nc\n");
        let comparison = compare(&left, &right).unwrap();
        assert_eq!(comparison.identical, 0);
        match comparison.changes.as_slice() {
            [Change::Changed {
                first_differing_line,
                ..
            }] => assert_eq!(*first_differing_line, Some(2)),
            other => panic!("expected one changed file, got {other:?}"),
        }
    }

    #[test]
    fn a_shorter_file_is_a_change_without_a_differing_line() {
        let scratch = tempfile::tempdir().unwrap();
        let left = scratch.path().join("left");
        let right = scratch.path().join("right");
        write(&left, "report.txt", "a\nb\n");
        write(&right, "report.txt", "a\n");
        let comparison = compare(&left, &right).unwrap();
        match comparison.changes.as_slice() {
            [Change::Changed {
                first_differing_line,
                baseline,
                fresh,
            }] => {
                assert_eq!(*first_differing_line, None);
                assert_eq!(baseline.lines, 2);
                assert_eq!(fresh.lines, 1);
            }
            other => panic!("expected one changed file, got {other:?}"),
        }
        assert!(render(&comparison).contains("one is a prefix of the other"));
    }

    /// The transpiler removes a report once it has nothing left to say, so a
    /// vanished report is a real change and has to be reported as one.
    #[test]
    fn an_added_and_a_removed_report_are_both_changes() {
        let scratch = tempfile::tempdir().unwrap();
        let left = scratch.path().join("left");
        let right = scratch.path().join("right");
        write(&left, "generated_targets.cmake", "a\n");
        write(&left, "generated_targets.skipped-flags.txt", "one\n");
        write(&right, "generated_targets.cmake", "a\n");
        write(&right, "generated_targets.missing-sources.txt", "two\n");
        let comparison = compare(&left, &right).unwrap();
        assert_eq!(comparison.identical, 1);
        let report = render(&comparison);
        assert!(
            report.contains("added    generated_targets.missing-sources.txt"),
            "{report}"
        );
        assert!(
            report.contains("removed  generated_targets.skipped-flags.txt"),
            "{report}"
        );
    }

    #[test]
    fn the_manifest_is_not_compared_with_the_products() {
        let scratch = tempfile::tempdir().unwrap();
        let left = scratch.path().join("left");
        let right = scratch.path().join("right");
        write(&left, "generated_targets.cmake", "a\n");
        write(&left, MANIFEST_NAME, "whatever\n");
        write(&right, "generated_targets.cmake", "a\n");
        let comparison = compare(&left, &right).unwrap();
        assert!(comparison.is_clean(), "{}", render(&comparison));
    }

    /// An empty recorded value is a real case: `--variant ""` is how the
    /// ordinary variant is spelled, and `--float-abi ""` is what CMake writes
    /// for x86_64. Dropping the blank line would move every later argument onto
    /// the wrong flag, so the recorded form has to survive intact.
    #[test]
    fn the_replayed_argv_keeps_empty_values_and_replaces_only_the_output() {
        let scratch = tempfile::tempdir().unwrap();
        let invocation = scratch.path().join(INVOCATION_SUFFIX);
        fs::write(
            &invocation,
            "--source-dir\n/tree\n--output\n/build/pc/generated_targets.cmake\n\
             --variant\n\n--cpu\nx86_64\n--float-abi\n\n",
        )
        .unwrap();
        let arguments = replayed_arguments(&invocation, Path::new("/tmp/run/out.cmake")).unwrap();
        assert_eq!(
            arguments,
            [
                "--source-dir",
                "/tree",
                "--output",
                "/tmp/run/out.cmake",
                "--variant",
                "",
                "--cpu",
                "x86_64",
                "--float-abi",
                "",
            ]
        );
    }

    #[test]
    fn an_invocation_without_an_output_is_refused() {
        let scratch = tempfile::tempdir().unwrap();
        let invocation = scratch.path().join(INVOCATION_SUFFIX);
        fs::write(&invocation, "--source-dir\n/tree\n").unwrap();
        let error = replayed_arguments(&invocation, Path::new("/tmp/out.cmake")).unwrap_err();
        assert!(
            format!("{error}").contains("does not carry --output"),
            "{error}"
        );
    }
}
