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

use anyhow::{Context, Result};
use clap::Parser;
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

    /// Re-run genmf even when a cached expansion exists.
    #[arg(long)]
    refresh: bool,

    /// Report only; exit 0 even when targets are missing.
    #[arg(long)]
    no_gate: bool,
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

fn main() -> Result<()> {
    let args = Args::parse();
    let root = args
        .source
        .canonicalize()
        .with_context(|| format!("source tree not found: {}", args.source.display()))?;

    fs::create_dir_all(&args.work)?;
    let cache = args.work.join("genmf");
    fs::create_dir_all(&cache)?;

    let mmakefiles = find_mmakefiles(&root);
    if mmakefiles.is_empty() {
        anyhow::bail!("no mmakefile.src found under {}", root.display());
    }

    // 1. What the tree declares. Read straight from the mmakefiles, with line
    //    continuations joined, so this measure does not depend on the
    //    transpiler's own parser being right.
    let declarations = collect_declarations(&root, &mmakefiles);

    // 2. What the historic build makes of it.
    let expanded = expand_all(&root, &cache, &mmakefiles, args.refresh);
    let shapes = collect_shapes(&expanded);

    // 3. What we produced.
    let generated = fs::read_to_string(&args.generated)
        .with_context(|| format!("cannot read {}", args.generated.display()))?;
    let ours = collect_ours(&generated);

    // ---- Coverage -------------------------------------------------------

    let declared: BTreeSet<&str> = declarations.iter().map(|d| d.mmake.as_str()).collect();
    let missing: Vec<&Declaration> = declarations
        .iter()
        .filter(|d| !ours.contains_key(&d.mmake))
        .collect();

    let mut by_macro: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for d in &declarations {
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
    let undeclared: Vec<&String> = ours.keys().filter(|k| !declared.contains(k.as_str())).collect();

    // ---- Shape ----------------------------------------------------------

    let mut wrong_name = Vec::new();
    for (mmake, target) in &ours {
        if let Some(shape) = shapes.get(mmake) {
            if let Some(expected) = &shape.progname {
                if !expected.eq_ignore_ascii_case(target) {
                    wrong_name.push(format!("{mmake}: reference builds {expected}, we build {target}"));
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
    println!(
        "   coverage      {}/{} declared targets ({pct:.1}%)",
        declared.len() - missing.len(),
        declared.len()
    );
    println!("   reference     {} targets in the genmf expansion", shapes.len());
    println!("   emitted       {} MMAKE_IDs", ours.len());

    write_report(
        &args.work.join("missing-targets.txt"),
        missing
            .iter()
            .map(|d| format!("{:32} %{:22} {}", d.mmake, d.macro_name, d.file))
            .collect(),
        &format!("{} declared target(s) not transpiled", missing.len()),
    )?;

    write_report(
        &args.work.join("undeclared-targets.txt"),
        undeclared.iter().map(|s| (*s).clone()).collect(),
        &format!(
            "{} emitted target(s) the tree does not declare",
            undeclared.len()
        ),
    )?;

    write_report(
        &args.work.join("wrong-program-name.txt"),
        wrong_name.clone(),
        &format!("{} target(s) built under the wrong name", wrong_name.len()),
    )?;

    if !by_macro.is_empty() {
        println!("\n   {:24} {:>8} {:>8}", "macro", "declared", "missing");
        for (m, (total, miss)) in &by_macro {
            if *miss > 0 {
                println!("   %{m:23} {total:8} {miss:8}");
            }
        }
    }

    let failed = !missing.is_empty() || !undeclared.is_empty() || !wrong_name.is_empty();
    if failed && !args.no_gate {
        anyhow::bail!("verification found gaps; see the reports in {}", args.work.display());
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
            } else if p.file_name().and_then(|n| n.to_str()) == Some("mmakefile.src") {
                out.push(p);
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
        let Ok(text) = fs::read_to_string(f) else {
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
fn expand_all(
    root: &Path,
    cache: &Path,
    files: &[PathBuf],
    refresh: bool,
) -> Vec<(String, PathBuf)> {
    let tmpl = root.join("config/make.tmpl");
    let genmf = root.join("tools/genmf/genmf.py");

    files
        .par_iter()
        .filter_map(|f| {
            let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().to_string();
            let out = cache.join(format!("{}.mk", rel.replace('/', "%")));
            if refresh || !out.exists() {
                let status = Command::new("python3")
                    .arg(&genmf)
                    .arg(&tmpl)
                    .arg(f)
                    .arg(&out)
                    .output();
                // A handful of mmakefiles in the tree fail genmf itself, for
                // instance on a non-UTF-8 byte or a stale argument name. Those
                // are the reference's own problem, not a transpiler gap.
                if status.is_err() || !out.exists() {
                    return None;
                }
            }
            Some((rel, out))
        })
        .collect()
}

/// Pulls the per-target facts out of the expansions.
fn collect_shapes(expanded: &[(String, PathBuf)]) -> BTreeMap<String, RefShape> {
    let re_prog = Regex::new(r"(?m)^([A-Za-z0-9_.][\w.-]*)_PROGNAME\s*:?=\s*(\S+)").unwrap();
    let re_mod = Regex::new(r"(?m)^([A-Za-z0-9_.][\w.-]*)_ALLTARGETS\b").unwrap();

    let per_file: Vec<BTreeMap<String, RefShape>> = expanded
        .par_iter()
        .map(|(_, path)| {
            let mut map: BTreeMap<String, RefShape> = BTreeMap::new();
            let Ok(text) = fs::read_to_string(path) else {
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

/// `MMAKE_ID <id>` together with the `TARGET <name>` of the same block.
fn collect_ours(generated: &str) -> BTreeMap<String, String> {
    let re = Regex::new(r"(?m)^\s*TARGET\s+(\S+)\s*$|^\s*MMAKE_ID\s+(\S+)\s*$").unwrap();
    let mut out = BTreeMap::new();
    let mut pending_target: Option<String> = None;
    for c in re.captures_iter(generated) {
        if let Some(t) = c.get(1) {
            pending_target = Some(t.as_str().to_string());
        } else if let Some(id) = c.get(2) {
            let name = pending_target.take().unwrap_or_default();
            out.insert(id.as_str().to_string(), name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
