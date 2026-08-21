//! Checks the transpiler's output against the historic build's own expansion.
//!
//! The build has two ways of being wrong that a compiler cannot report. A
//! target the transpiler never emits produces no error, because nothing asks
//! for it; and a target it emits with the wrong shape, such as one executable
//! per source file where the reference builds one from all of them, compiles
//! perfectly well and links the wrong binaries.
//!
//! `tools/genmf/genmf.py` expands an mmakefile into the makefile the historic
//! build actually runs, so it answers both questions. This tool runs it over
//! the tree and compares:
//!
//!   * **Coverage** -- every `mmake=` declaration in the tree against the
//!     `MMAKE_ID` entries in `generated_targets.cmake`.
//!   * **Shape** -- for a program target, the reference's `_PROGNAME` against
//!     the name the transpiler gave it, and how many targets each declaration
//!     produced on either side.
//!
//! Both are reported as counts and as files, and the exit code is non-zero
//! when something is missing, so this can gate a build.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result};
use aros_common::read_source;
use clap::{Parser, ValueEnum};
use rayon::prelude::*;
use regex::Regex;

#[derive(Parser, Debug)]
#[command(
    name = "aros-verify",
    about = "Compare transpiled CMake targets against the genmf reference expansion"
)]
struct Args {
    /// Source tree root.
    #[arg(long, default_value = ".")]
    source: PathBuf,

    /// The transpiler's output to check.
    #[arg(long)]
    generated: PathBuf,

    /// Where to cache genmf expansions and write reports.
    #[arg(long)]
    work: PathBuf,

    /// The configured build directory, to check that emitted declarations
    /// actually became CMake targets.
    #[arg(long)]
    build_dir: Option<PathBuf>,

    /// Target CPU for architecture-scoped coverage (for example x86_64).
    #[arg(long, value_parser = parse_arch_component, requires = "platform")]
    cpu: Option<String>,

    /// Target platform for architecture-scoped coverage (for example pc).
    #[arg(long, value_parser = parse_arch_component, requires = "cpu")]
    platform: Option<String>,

    /// Coverage profile. Only architecture eligibility is currently
    /// evidence-backed; core/distribution reachability needs verified roots.
    #[arg(long, value_enum, requires_all = ["cpu", "platform"])]
    profile: Option<Profile>,

    /// Re-run genmf even when a cached expansion exists.
    #[arg(long)]
    refresh: bool,

    /// Report only; exit 0 even when targets are missing.
    #[arg(long)]
    no_gate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Profile {
    /// Filter declarations by the configured CMake architecture directories.
    Architecture,
}

impl Profile {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
        }
    }
}

/// The exact architecture directory sets CMake constructs in AROS.cmake.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchitectureScope {
    cpu: String,
    platform: String,
    source_dirs: BTreeSet<String>,
    package_dirs: BTreeSet<String>,
}

impl ArchitectureScope {
    fn new(cpu: &str, platform: &str) -> Self {
        let compatible_cpus: &[&str] = match cpu {
            "x86_64" => &["i386", "x86_64"],
            "aarch64" => &["arm", "aarch64"],
            "riscv64" => &["riscv", "riscv64"],
            _ => &[cpu],
        };

        // cmake/AROS.cmake starts with all-native, then appends these four
        // spellings for every compatible CPU and removes duplicates.
        let mut source_dirs = BTreeSet::from(["all-native".to_owned()]);
        for compatible in compatible_cpus {
            source_dirs.insert(format!("{compatible}-all"));
            source_dirs.insert(format!("{compatible}-native"));
            source_dirs.insert(format!("all-{platform}"));
            source_dirs.insert(format!("{compatible}-{platform}"));
        }

        // Packages are narrower: sources may come from a compatible CPU, but
        // only the configured CPU's package may write an architecture-relative
        // output such as boot/<platform>/aros-bsp.pkg.
        let package_dirs = BTreeSet::from([
            "all-native".to_owned(),
            format!("{cpu}-all"),
            format!("{cpu}-native"),
            format!("all-{platform}"),
            format!("{cpu}-{platform}"),
        ]);

        Self {
            cpu: cpu.to_owned(),
            platform: platform.to_owned(),
            source_dirs,
            package_dirs,
        }
    }

    fn from_args(args: &Args) -> Option<Self> {
        args.cpu
            .as_deref()
            .zip(args.platform.as_deref())
            .map(|(cpu, platform)| Self::new(cpu, platform))
    }

    fn key(&self) -> String {
        format!("architecture-{}-{}", self.cpu, self.platform)
    }

    fn declaration_is_eligible(&self, declaration: &Declaration) -> bool {
        if matches!(
            declaration.macro_name.as_str(),
            "make_package" | "link_kickstart"
        ) {
            let Some(arch_dir) = declaration_arch_dir(&declaration.file) else {
                return !is_under_arch(&declaration.file);
            };
            self.package_dirs.contains(arch_dir)
        } else {
            self.file_is_eligible(&declaration.file)
        }
    }

    fn file_is_eligible(&self, file: &str) -> bool {
        let Some(arch_dir) = declaration_arch_dir(file) else {
            // CMake gates only paths below arch/<cpu>-<platform>. Everything
            // outside arch/ is shared by all target architectures.
            return !is_under_arch(file);
        };
        self.source_dirs.contains(arch_dir)
    }
}

fn parse_arch_component(value: &str) -> std::result::Result<String, String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(format!(
            "'{value}' is not an architecture component (expected ASCII letters, digits, '_' or '-')"
        ));
    }
    Ok(value.to_owned())
}

fn is_under_arch(file: &str) -> bool {
    file.split(['/', '\\']).next() == Some("arch")
}

fn declaration_arch_dir(file: &str) -> Option<&str> {
    let mut parts = file.split(['/', '\\']);
    (parts.next()? == "arch").then_some(())?;
    let dir = parts.next()?;
    dir.split_once('-').map(|_| dir)
}

/// One `%build_*` declaration found in an mmakefile.
#[derive(Debug, Clone)]
struct Declaration {
    mmake: String,
    macro_name: String,
    file: String,
}

/// What the reference expansion says about one target.
#[derive(Debug, Clone, Default)]
struct RefShape {
    /// `<target>_PROGNAME`, set for `%build_prog`.
    progname: Option<String>,
    /// Whether the expansion carries a module's target list.
    is_module: bool,
}

#[derive(Debug)]
struct ExpansionResult {
    expanded: Vec<(String, PathBuf)>,
    failures: Vec<ExpansionFailure>,
}

#[derive(Debug)]
struct ExpansionFailure {
    file: String,
    message: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let architecture = ArchitectureScope::from_args(&args);
    let root = args
        .source
        .canonicalize()
        .with_context(|| format!("source tree not found: {}", args.source.display()))?;

    fs::create_dir_all(&args.work)?;
    let cache = args.work.join("genmf");
    fs::create_dir_all(&cache)?;
    let report_dir = architecture
        .as_ref()
        .map_or_else(|| args.work.clone(), |scope| args.work.join(scope.key()));
    fs::create_dir_all(&report_dir)?;

    let mmakefiles = find_mmakefiles(&root);
    if mmakefiles.is_empty() {
        anyhow::bail!(
            "no mmakefile or mmakefile.src found under {}",
            root.display()
        );
    }

    // 1. What the tree declares. Read straight from the mmakefiles, with line
    //    continuations joined, so this measure does not depend on the
    //    transpiler's own parser being right.
    let declarations = collect_declarations(&root, &mmakefiles);
    let scoped_declarations: Vec<&Declaration> = declarations
        .iter()
        .filter(|declaration| {
            architecture
                .as_ref()
                .is_none_or(|scope| scope.declaration_is_eligible(declaration))
        })
        .collect();

    // 2. What the historic build makes of it.
    let expansion = expand_all(&root, &cache, &mmakefiles, args.refresh);
    let shapes = collect_shapes(&expansion.expanded);
    let expansion_failures: Vec<String> = expansion
        .failures
        .iter()
        .filter(|failure| {
            architecture
                .as_ref()
                .is_none_or(|scope| scope.file_is_eligible(&failure.file))
        })
        .map(|failure| failure.message.clone())
        .collect();

    // 3. What we produced.
    let generated = fs::read_to_string(&args.generated)
        .with_context(|| format!("cannot read {}", args.generated.display()))?;
    let ours = collect_ours(&generated);

    // ---- Coverage -------------------------------------------------------

    let all_declared: BTreeSet<&str> = declarations.iter().map(|d| d.mmake.as_str()).collect();
    let declared: BTreeSet<&str> = scoped_declarations
        .iter()
        .map(|d| d.mmake.as_str())
        .collect();
    let missing: Vec<&Declaration> = scoped_declarations
        .iter()
        .copied()
        .filter(|d| !ours.contains_key(&d.mmake))
        .collect();

    let mut by_macro: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for d in &scoped_declarations {
        let e = by_macro.entry(d.macro_name.as_str()).or_default();
        e.0 += 1;
        if !ours.contains_key(&d.mmake) {
            e.1 += 1;
        }
    }

    // A target we emit that the tree never declares points at a naming or
    // splitting mistake, which is how the %build_prog / %build_progs mix-up
    // showed up: four executables named after source files instead of one
    // named by progname.
    // Generated output is target-agnostic, and an undeclared id has no source
    // path from which an architecture can be inferred. Keep this global
    // integrity error in every profile rather than guessing it away.
    let undeclared: Vec<&String> = ours
        .keys()
        .filter(|k| !all_declared.contains(k.as_str()))
        .collect();
    let emitted: Vec<&String> = ours
        .keys()
        .filter(|id| {
            architecture.is_none()
                || declared.contains(id.as_str())
                // There is no architecture evidence for an undeclared id.
                // Keep it in every scoped integrity gate rather than silently
                // assigning it to an arbitrary architecture.
                || !all_declared.contains(id.as_str())
        })
        .collect();

    // ---- Realisation ----------------------------------------------------
    //
    // Coverage above measures what the transpiler emitted, which is not the
    // same as what CMake built. A declaration emitted with an empty source
    // list makes every builder return early, so the target never exists, and
    // nothing said so: aros_add_custom_target was an empty stub for 97
    // declarations with 313 source files and this check would have caught it
    // on the first run.
    //
    // CMakeFiles/<id>.dir is the evidence. CMake creates it for any target it
    // configured, and for none it did not.
    let unrealised = args.build_dir.as_ref().map_or_else(Vec::new, |dir| {
        let cmakefiles = dir.join("CMakeFiles");
        let mut present: BTreeSet<String> = BTreeSet::new();
        if let Ok(entries) = fs::read_dir(&cmakefiles) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(stem) = name.strip_suffix(".dir") {
                    present.insert(stem.to_owned());
                }
            }
        }
        // A package or kickstart declaration becomes an add_custom_target,
        // which gets no CMakeFiles/<id>.dir. Ninja records it as a phony
        // edge instead, so both places have to be read or the four
        // pc-x86_64 packages read as unrealised.
        if let Ok(ninja) = fs::read_to_string(dir.join("build.ninja")) {
            let phony = Regex::new(r"(?m)^build ([^:$ ]+): phony").unwrap();
            for c in phony.captures_iter(&ninja) {
                if let Some(name) = c[1].rsplit('/').next() {
                    present.insert(name.to_owned());
                }
            }
        }
        if present.is_empty() {
            vec![format!(
                "cannot read {} -- configure the build first",
                cmakefiles.display()
            )]
        } else {
            emitted
                .iter()
                .copied()
                .filter(|id| !present.contains(id.as_str()))
                .map(|id| {
                    let where_ = scoped_declarations
                        .iter()
                        .find(|d| d.mmake.as_str() == id.as_str())
                        .map_or_else(|| "?".to_owned(), |d| d.file.clone());
                    format!("{id:44} {where_}")
                })
                .collect()
        }
    });

    // ---- Shape ----------------------------------------------------------

    let mut wrong_name = Vec::new();
    for (mmake, target) in &ours {
        if architecture.is_some() && !declared.contains(mmake.as_str()) {
            continue;
        }
        if let Some(shape) = shapes.get(mmake) {
            if let Some(expected) = &shape.progname {
                if !expected.eq_ignore_ascii_case(target) {
                    wrong_name.push(format!(
                        "{mmake}: reference builds {expected}, we build {target}"
                    ));
                }
            }
        }
    }

    // ---- Report ---------------------------------------------------------

    let pct = if declared.is_empty() {
        100.0
    } else {
        100.0 * (declared.len() - missing.len()) as f64 / declared.len() as f64
    };

    println!("📐 aros-verify");
    if let Some(scope) = &architecture {
        let profile = args.profile.unwrap_or(Profile::Architecture);
        println!(
            "   scope         {} {}-{}",
            profile.as_str(),
            scope.cpu,
            scope.platform
        );
        println!("   reachability  not filtered (no verified core/distribution roots available)");
    }
    println!(
        "   coverage      {}/{} declared targets ({pct:.1}%)",
        declared.len() - missing.len(),
        declared.len()
    );
    let reference_count = if architecture.is_some() {
        shapes
            .keys()
            .filter(|id| declared.contains(id.as_str()))
            .count()
    } else {
        shapes.len()
    };
    println!("   reference     {reference_count} targets in the genmf expansion");
    println!("   emitted       {} MMAKE_IDs", emitted.len());
    if args.build_dir.is_some() {
        let built = emitted.len().saturating_sub(unrealised.len());
        println!(
            "   realised      {built}/{} emitted became CMake targets",
            emitted.len()
        );
    }

    write_failure_report(
        &report_dir.join("genmf-errors.txt"),
        expansion_failures.clone(),
        &format!(
            "{} mmakefile(s) could not be expanded by genmf",
            expansion_failures.len()
        ),
    )?;

    write_report(
        &report_dir.join("missing-targets.txt"),
        missing
            .iter()
            .map(|d| format!("{:32} %{:22} {}", d.mmake, d.macro_name, d.file))
            .collect(),
        &format!("{} declared target(s) not transpiled", missing.len()),
    )?;

    write_report(
        &report_dir.join("undeclared-targets.txt"),
        undeclared.iter().map(|s| (*s).clone()).collect(),
        &format!(
            "{} emitted target(s) the tree does not declare",
            undeclared.len()
        ),
    )?;

    write_report(
        &report_dir.join("wrong-program-name.txt"),
        wrong_name.clone(),
        &format!("{} target(s) built under the wrong name", wrong_name.len()),
    )?;

    write_report(
        &report_dir.join("unrealised-targets.txt"),
        unrealised.clone(),
        &format!(
            "{} emitted declaration(s) never became a CMake target",
            unrealised.len()
        ),
    )?;

    if !by_macro.is_empty() {
        println!("\n   {:24} {:>8} {:>8}", "macro", "declared", "missing");
        for (m, (total, miss)) in &by_macro {
            if *miss > 0 {
                println!("   %{m:23} {total:8} {miss:8}");
            }
        }
    }

    let failed = !missing.is_empty()
        || !undeclared.is_empty()
        || !wrong_name.is_empty()
        || !unrealised.is_empty()
        || !expansion_failures.is_empty();
    if failed && !args.no_gate {
        anyhow::bail!(
            "verification found gaps; see the reports in {}",
            report_dir.display()
        );
    }
    Ok(())
}

fn write_report(path: &Path, mut lines: Vec<String>, headline: &str) -> Result<()> {
    if lines.is_empty() {
        let _ = fs::remove_file(path);
        println!("   ✅ {headline}");
        return Ok(());
    }
    lines.sort_unstable();
    lines.dedup();
    fs::write(path, lines.join("\n") + "\n")?;
    println!("   ⚠️  {headline} -> {}", path.display());
    Ok(())
}

/// Writes only actionable reference-expansion failures. A clean run removes a
/// stale report without adding a line to the long-established global output.
fn write_failure_report(path: &Path, mut lines: Vec<String>, headline: &str) -> Result<()> {
    if lines.is_empty() {
        let _ = fs::remove_file(path);
        return Ok(());
    }
    lines.sort_unstable();
    lines.dedup();
    fs::write(path, lines.join("\n") + "\n")?;
    println!("   ⚠️  {headline} -> {}", path.display());
    Ok(())
}

fn find_mmakefiles(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // The build directory holds generated copies, and .git is large.
                if name == "build" || name == ".git" {
                    continue;
                }
                stack.push(p);
            } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if matches!(name, "mmakefile" | "mmakefile.src") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// Reads every `%build_* ... mmake=<name>` from the tree.
///
/// Line continuations are joined first: most declarations spread their
/// arguments over several lines, and `mmake=` is often not on the first one.
fn collect_declarations(root: &Path, files: &[PathBuf]) -> Vec<Declaration> {
    let cont = Regex::new(r"\\\s*\n\s*").unwrap();
    let decl = Regex::new(r"(?m)^\s*%(build_\w+|make_package|link_kickstart)\b([^\n]*)").unwrap();
    let mmake = Regex::new(r"\bmmake=([\w.-]+)").unwrap();

    let mut out = Vec::new();
    for f in files {
        let Ok(text) = read_source(f) else {
            continue;
        };
        let joined = cont.replace_all(&text, " ");
        let rel = f
            .strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .to_string();
        for c in decl.captures_iter(&joined) {
            let macro_name = c[1].to_string();
            if let Some(m) = mmake.captures(&c[2]) {
                out.push(Declaration {
                    mmake: m[1].to_string(),
                    macro_name,
                    file: rel.clone(),
                });
            }
        }
    }
    out
}

/// Runs genmf over each mmakefile, caching the result.
///
/// genmf is quick (about 20 ms per file) but there are over a thousand files,
/// so the expansions are kept and only redone on request.
fn expand_all(root: &Path, cache: &Path, files: &[PathBuf], refresh: bool) -> ExpansionResult {
    let tmpl = root.join("config/make.tmpl");
    let genmf = root.join("tools/genmf/genmf.py");
    let genmf_dependencies = genmf_dependency_files(root);

    let outcomes: Vec<std::result::Result<(String, PathBuf), ExpansionFailure>> = files
        .par_iter()
        .map(|f| {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(f)
                .to_string_lossy()
                .to_string();
            let out = cache.join(format!("{}.mk", rel.replace('/', "%")));
            let failure = |detail: String| ExpansionFailure {
                file: rel.clone(),
                message: format!("{rel}: {detail}"),
            };
            let mut inputs = Vec::with_capacity(genmf_dependencies.len() + 1);
            inputs.push(f.as_path());
            inputs.extend(genmf_dependencies.iter().map(PathBuf::as_path));
            if refresh || !cache_is_fresh(&out, &inputs) {
                // Never let a failed regeneration make a stale or partial
                // output look fresh on the next run.
                let _ = fs::remove_file(&out);
                let result = Command::new("python3")
                    .arg(&genmf)
                    .arg(&tmpl)
                    .arg(f)
                    .arg(&out)
                    .output();

                let command_output =
                    result.map_err(|error| failure(format!("could not start genmf: {error}")))?;
                if !command_output.status.success() {
                    let _ = fs::remove_file(&out);
                    let detail = String::from_utf8_lossy(&command_output.stderr)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let detail = if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    };
                    return Err(failure(format!(
                        "genmf exited with {}{detail}",
                        command_output.status
                    )));
                }
                if !out.is_file() {
                    return Err(failure(
                        "genmf succeeded without producing cache output".to_owned(),
                    ));
                }
            }
            Ok((rel, out))
        })
        .collect();

    let mut expanded = Vec::new();
    let mut failures = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(expansion) => expanded.push(expansion),
            Err(failure) => failures.push(failure),
        }
    }
    expanded.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    failures.sort_unstable_by(|left, right| left.message.cmp(&right.message));
    failures.dedup_by(|left, right| left.message == right.message);
    ExpansionResult { expanded, failures }
}

/// Files whose contents affect every genmf expansion.
///
/// MetaMake's `genmakefiledeps` names the main template and its three current
/// includes. Discover the includes from the template itself so adding another
/// one cannot leave a previously cached reference expansion looking fresh.
fn genmf_dependency_files(root: &Path) -> Vec<PathBuf> {
    let mut dependencies = BTreeSet::from([root.join("tools/genmf/genmf.py")]);
    let mut pending = vec![root.join("config/make.tmpl")];

    while let Some(template) = pending.pop() {
        if !dependencies.insert(template.clone()) {
            continue;
        }
        let Ok(text) = read_source(&template) else {
            continue;
        };
        let parent = template.parent().unwrap_or(root);
        for line in text.lines() {
            let Some(raw_include) = line.strip_prefix("%include") else {
                continue;
            };
            if !raw_include.chars().next().is_some_and(char::is_whitespace) {
                continue;
            }
            let mut include = raw_include.trim();
            if include.len() > 1 && include.starts_with('"') && include.ends_with('"') {
                include = &include[1..include.len() - 1];
            }
            if !include.is_empty() {
                let include = Path::new(include);
                pending.push(if include.is_absolute() {
                    include.to_path_buf()
                } else {
                    parent.join(include)
                });
            }
        }
    }

    dependencies.into_iter().collect()
}

fn cache_is_fresh(output: &Path, inputs: &[&Path]) -> bool {
    let Ok(output_modified) = fs::metadata(output).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let mut input_modified = Vec::with_capacity(inputs.len());
    for input in inputs {
        let Ok(modified) = fs::metadata(input).and_then(|metadata| metadata.modified()) else {
            return false;
        };
        input_modified.push(modified);
    }
    timestamps_are_fresh(output_modified, &input_modified)
}

fn timestamps_are_fresh(output: SystemTime, inputs: &[SystemTime]) -> bool {
    inputs.iter().all(|input| output > *input)
}

/// Pulls the per-target facts out of the expansions.
fn collect_shapes(expanded: &[(String, PathBuf)]) -> BTreeMap<String, RefShape> {
    let re_prog = Regex::new(r"(?m)^([A-Za-z0-9_.][\w.-]*)_PROGNAME\s*:?=\s*(\S+)").unwrap();
    let re_mod = Regex::new(r"(?m)^([A-Za-z0-9_.][\w.-]*)_ALLTARGETS\b").unwrap();

    let per_file: Vec<BTreeMap<String, RefShape>> = expanded
        .par_iter()
        .map(|(_, path)| {
            let mut map: BTreeMap<String, RefShape> = BTreeMap::new();
            let Ok(text) = read_source(path) else {
                return map;
            };
            for c in re_prog.captures_iter(&text) {
                let name = c[1].to_string();
                let value = c[2].to_string();
                // An unresolved Make variable tells us nothing.
                if value.contains('$') {
                    continue;
                }
                map.entry(name).or_default().progname = Some(value);
            }
            for c in re_mod.captures_iter(&text) {
                map.entry(c[1].to_string()).or_default().is_module = true;
            }
            map
        })
        .collect();

    let mut all = BTreeMap::new();
    for m in per_file {
        for (k, v) in m {
            let e: &mut RefShape = all.entry(k).or_default();
            if v.progname.is_some() {
                e.progname = v.progname;
            }
            e.is_module |= v.is_module;
        }
    }
    all
}

/// Every mmake target the generated file declares, with the name it builds
/// under.
///
/// Build targets carry `TARGET <name>` and `MMAKE_ID <id>`. Package and
/// kickstart declarations carry `NAME <id>` instead and have no separate
/// build name; counting only MMAKE_ID reported all 21 of them as missing.
fn collect_ours(generated: &str) -> BTreeMap<String, String> {
    let re =
        Regex::new(r"(?m)^\s*TARGET\s+(\S+)\s*$|^\s*MMAKE_ID\s+(\S+)\s*$|^\s*NAME\s+(\S+)\s*$")
            .unwrap();
    let mut out = BTreeMap::new();
    let mut pending_target: Option<String> = None;
    for c in re.captures_iter(generated) {
        if let Some(t) = c.get(1) {
            pending_target = Some(t.as_str().to_string());
        } else if let Some(id) = c.get(2) {
            let name = pending_target.take().unwrap_or_default();
            out.insert(id.as_str().to_string(), name);
        } else if let Some(id) = c.get(3) {
            // A package has no build name of its own.
            out.entry(id.as_str().to_string()).or_default();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration(file: &str, macro_name: &str) -> Declaration {
        Declaration {
            mmake: "test-target".to_owned(),
            macro_name: macro_name.to_owned(),
            file: file.to_owned(),
        }
    }

    #[test]
    fn architecture_scope_matches_cmake_compatible_cpu_directories() {
        let x86 = ArchitectureScope::new("x86_64", "pc");
        assert!(x86.source_dirs.contains("all-native"));
        assert!(x86.source_dirs.contains("i386-all"));
        assert!(x86.source_dirs.contains("i386-native"));
        assert!(x86.source_dirs.contains("i386-pc"));
        assert!(x86.source_dirs.contains("x86_64-pc"));
        assert!(x86.source_dirs.contains("all-pc"));
        assert!(!x86.source_dirs.contains("arm-native"));
        assert!(!x86.source_dirs.contains("all-all"));

        let aarch64 = ArchitectureScope::new("aarch64", "raspi");
        assert!(aarch64.source_dirs.contains("arm-all"));
        assert!(aarch64.source_dirs.contains("arm-native"));
        assert!(aarch64.source_dirs.contains("arm-raspi"));
        assert!(aarch64.source_dirs.contains("aarch64-raspi"));
        assert!(!aarch64.source_dirs.contains("i386-all"));

        let riscv64 = ArchitectureScope::new("riscv64", "opensbi");
        assert!(riscv64.source_dirs.contains("riscv-all"));
        assert!(riscv64.source_dirs.contains("riscv-native"));
        assert!(riscv64.source_dirs.contains("riscv-opensbi"));
        assert!(riscv64.source_dirs.contains("riscv64-opensbi"));
    }

    #[test]
    fn architecture_scope_uses_the_narrower_cmake_package_set() {
        let scope = ArchitectureScope::new("x86_64", "pc");
        assert!(scope.declaration_is_eligible(&declaration(
            "arch/i386-pc/drivers/mmakefile.src",
            "build_module"
        )));
        assert!(!scope.declaration_is_eligible(&declaration(
            "arch/i386-pc/boot/mmakefile.src",
            "make_package"
        )));
        assert!(scope.declaration_is_eligible(&declaration(
            "arch/x86_64-pc/boot/mmakefile.src",
            "make_package"
        )));
        assert!(scope.declaration_is_eligible(&declaration(
            "arch/all-pc/boot/mmakefile.src",
            "link_kickstart"
        )));
    }

    #[test]
    fn architecture_scope_keeps_common_files_and_rejects_unknown_arch_paths() {
        let scope = ArchitectureScope::new("arm", "raspi");
        assert!(
            scope.declaration_is_eligible(&declaration("rom/exec/mmakefile.src", "build_module"))
        );
        assert!(scope.declaration_is_eligible(&declaration(
            "arch\\arm-native\\kernel\\mmakefile.src",
            "build_module"
        )));
        assert!(!scope.declaration_is_eligible(&declaration(
            "arch/.unmaintained/m68k-pp-native/mmakefile.src",
            "build_module"
        )));
        assert!(!scope
            .declaration_is_eligible(&declaration("arch/all-all/mmakefile.src", "build_module")));
        assert!(!scope.declaration_is_eligible(&declaration("arch/mmakefile.src", "build_module")));
    }

    #[test]
    fn architecture_cli_requires_a_complete_pair_and_validates_profile() {
        let ok = Args::try_parse_from([
            "aros-verify",
            "--generated",
            "generated.cmake",
            "--work",
            "verify",
            "--cpu",
            "x86_64",
            "--platform",
            "pc",
            "--profile",
            "architecture",
        ])
        .unwrap();
        assert_eq!(ok.cpu.as_deref(), Some("x86_64"));
        assert_eq!(ok.platform.as_deref(), Some("pc"));
        assert_eq!(ok.profile, Some(Profile::Architecture));

        assert!(Args::try_parse_from([
            "aros-verify",
            "--generated",
            "generated.cmake",
            "--work",
            "verify",
            "--cpu",
            "x86_64",
        ])
        .is_err());
        assert!(Args::try_parse_from([
            "aros-verify",
            "--generated",
            "generated.cmake",
            "--work",
            "verify",
            "--cpu",
            "x86_64",
            "--platform",
            "pc",
            "--profile",
            "core",
        ])
        .is_err());
    }

    #[test]
    fn architecture_report_key_is_stable_and_path_safe() {
        let scope = ArchitectureScope::new("x86_64", "pc");
        assert_eq!(scope.key(), "architecture-x86_64-pc");
        assert!(parse_arch_component("../pc").is_err());
        assert!(parse_arch_component("aarch64").is_ok());
    }

    #[test]
    fn finds_and_scans_both_mmakefile_names() {
        let dir = std::env::temp_dir().join(format!(
            "aros-verify-test-mmakefiles-{}",
            std::process::id()
        ));
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("mmakefile"), "%build_prog mmake=plain\n").unwrap();
        fs::write(nested.join("mmakefile.src"), "%build_prog mmake=with-src\n").unwrap();
        fs::write(dir.join("mmakefile.txt"), "%build_prog mmake=ignored\n").unwrap();

        let files = find_mmakefiles(&dir);
        let relative: Vec<String> = files
            .iter()
            .map(|file| {
                file.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(relative, ["mmakefile", "nested/mmakefile.src"]);

        let declarations = collect_declarations(&dir, &files);
        let ids: BTreeSet<&str> = declarations
            .iter()
            .map(|declaration| declaration.mmake.as_str())
            .collect();
        assert_eq!(ids, BTreeSet::from(["plain", "with-src"]));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_timestamp_must_be_newer_than_every_input() {
        let base = SystemTime::UNIX_EPOCH;
        let one = base + std::time::Duration::from_secs(1);
        let two = base + std::time::Duration::from_secs(2);
        let three = base + std::time::Duration::from_secs(3);

        assert!(timestamps_are_fresh(three, &[one, two]));
        assert!(!timestamps_are_fresh(two, &[one, two]));
        assert!(!timestamps_are_fresh(two, &[three]));
    }

    #[test]
    fn discovers_recursive_genmf_template_dependencies() {
        let dir = std::env::temp_dir().join(format!(
            "aros-verify-test-genmf-dependencies-{}",
            std::process::id()
        ));
        let config = dir.join("config");
        let tools = dir.join("tools/genmf");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&tools).unwrap();
        fs::write(
            config.join("make.tmpl"),
            "%include make-cmake.tmpl\n%include \"make-meson.tmpl\"\n",
        )
        .unwrap();
        fs::write(
            config.join("make-cmake.tmpl"),
            "%include make-common.tmpl\n",
        )
        .unwrap();
        fs::write(config.join("make-meson.tmpl"), "").unwrap();
        fs::write(config.join("make-common.tmpl"), "").unwrap();
        fs::write(tools.join("genmf.py"), "").unwrap();

        let relative: Vec<String> = genmf_dependency_files(&dir)
            .iter()
            .map(|path| {
                path.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(
            relative,
            [
                "config/make-cmake.tmpl",
                "config/make-common.tmpl",
                "config/make-meson.tmpl",
                "config/make.tmpl",
                "tools/genmf/genmf.py",
            ]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn genmf_failure_report_is_sorted_deduplicated_and_cleared() {
        let report = std::env::temp_dir().join(format!(
            "aros-verify-test-genmf-errors-{}.txt",
            std::process::id()
        ));
        write_failure_report(
            &report,
            vec![
                "z-error".to_owned(),
                "a-error".to_owned(),
                "z-error".to_owned(),
            ],
            "test failures",
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&report).unwrap(), "a-error\nz-error\n");

        write_failure_report(&report, Vec::new(), "no failures").unwrap();
        assert!(!report.exists());
    }

    #[test]
    fn records_a_failed_genmf_expansion_instead_of_dropping_it() {
        let dir = std::env::temp_dir().join(format!(
            "aros-verify-test-genmf-failure-{}",
            std::process::id()
        ));
        let tools = dir.join("tools/genmf");
        let config = dir.join("config");
        let cache = dir.join("cache");
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(config.join("make.tmpl"), "").unwrap();
        fs::write(
            tools.join("genmf.py"),
            "import sys\nsys.stderr.write('intentional genmf failure\\n')\nsys.exit(9)\n",
        )
        .unwrap();
        let mmakefile = dir.join("mmakefile");
        fs::write(&mmakefile, "").unwrap();

        let result = expand_all(&dir, &cache, &[mmakefile], true);
        assert!(result.expanded.is_empty());
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].file, "mmakefile");
        assert!(result.failures[0]
            .message
            .starts_with("mmakefile: genmf exited with"));
        assert!(result.failures[0]
            .message
            .contains("intentional genmf failure"));
        assert!(!cache.join("mmakefile.mk").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_a_declaration_spread_over_several_lines() {
        let dir = std::env::temp_dir().join("aros-verify-test-decl");
        let sub = dir.join("rom/dos");
        fs::create_dir_all(&sub).unwrap();
        let f = sub.join("mmakefile.src");
        fs::write(
            &f,
            "%build_module mmake=kernel-dos \\\n  modname=dos modtype=library \\\n  files=$(FILES)\n",
        )
        .unwrap();
        let decls = collect_declarations(&dir, &[f]);
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].mmake, "kernel-dos");
        assert_eq!(decls[0].macro_name, "build_module");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reads_every_declaration_in_one_file() {
        // The case the transpiler's own regex used to miss: several modules in
        // one mmakefile with a single %common at the end.
        let dir = std::env::temp_dir().join("aros-verify-test-multi");
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("mmakefile.src");
        fs::write(
            &f,
            "%build_module  mmake=a modname=A modtype=mui files=a\n\
             %build_module  mmake=b modname=B modtype=mui files=b\n\
             %build_module  mmake=c modname=C modtype=mui files=c\n\
             %common\n",
        )
        .unwrap();
        let decls = collect_declarations(&dir, &[f]);
        let names: Vec<&str> = decls.iter().map(|d| d.mmake.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn counts_a_package_declaration_too() {
        // %make_package and %link_kickstart emit NAME, not MMAKE_ID.
        let generated = "\
aros_make_package(
    NAME kernel-package-base
    OUTPUT \"x\"
)
aros_link_kickstart(
    NAME kernel-pc-x86_64-kernel
    OUTPUT \"y\"
)
";
        let ours = collect_ours(generated);
        assert!(ours.contains_key("kernel-package-base"));
        assert!(ours.contains_key("kernel-pc-x86_64-kernel"));
    }

    #[test]
    fn pairs_each_mmake_id_with_its_target_name() {
        let generated = "\
aros_add_program(
    TARGET SysLog
    MMAKE_ID aros-tcpip-apps-syslog
)
aros_build_module(
    TARGET dos
    MMAKE_ID kernel-dos
)
";
        let ours = collect_ours(generated);
        assert_eq!(ours.get("aros-tcpip-apps-syslog").unwrap(), "SysLog");
        assert_eq!(ours.get("kernel-dos").unwrap(), "dos");
    }

    #[test]
    fn counts_an_icon_declaration_without_a_compiled_target_name() {
        let generated = "\
aros_declare_icon_target(
    MMAKE_ID iconset-Gorilla-wbench-icons
    DIRECTORY \"images/IconSets/Gorilla\"
)
";
        let ours = collect_ours(generated);
        assert!(ours.contains_key("iconset-Gorilla-wbench-icons"));
        assert_eq!(ours["iconset-Gorilla-wbench-icons"], "");
    }

    #[test]
    fn a_declaration_without_a_name_is_skipped() {
        // %build_icons and friends are sometimes invoked without mmake=; they
        // have no identity to compare against.
        let dir = std::env::temp_dir().join("aros-verify-test-noname");
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("mmakefile.src");
        fs::write(&f, "%build_icons dir=images\n").unwrap();
        assert!(collect_declarations(&dir, &[f]).is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
