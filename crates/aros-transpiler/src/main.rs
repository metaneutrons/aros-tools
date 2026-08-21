use aros_common::Result;
use aros_transpiler::dirs::DirVars;
use aros_transpiler::{generate_cmake, parse_mmakefile_with_dirs, DependencyGraph};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(author, version, about = "AROS-NG Parallel mmakefile Transpiler")]
struct Args {
    /// Root directory of AROS source tree
    #[arg(short, long, default_value = ".")]
    source_dir: PathBuf,

    /// Output path for generated CMake targets file
    #[arg(short, long, default_value = "build/generated_targets.cmake")]
    output: PathBuf,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    println!(
        "⚡ AROS-NG Transpiler v0.1.0 — Scanning mmakefile.src files in {}...",
        args.source_dir.display()
    );

    // Build trees must be skipped. The SDK staging step copies whole source
    // directories, mmakefile.src included, so scanning build/ would parse those
    // copies a second time and attribute their rules to the wrong location.
    let skip_dirs = ["build", "target", ".git"];
    let mut files: Vec<PathBuf> = WalkDir::new(&args.source_dir)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || e.depth() == 0
                || !skip_dirs
                    .iter()
                    .any(|d| e.file_name().to_string_lossy() == *d)
        })
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name() == "mmakefile.src")
        .map(walkdir::DirEntry::into_path)
        .collect();
    // Stable source order matters for duplicate-output semantics: GNU Make's
    // first satisfiable icon rule wins, and the CMake output registry mirrors
    // that choice while reporting conflicting later claims.
    files.sort();

    println!(
        "📦 Found {} mmakefile.src files. Parsing in parallel...",
        files.len()
    );

    let pb = ProgressBar::new(files.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap(),
    );

    let dirs = DirVars::load(&args.source_dir);
    let parsed_results: Vec<_> = files
        .par_iter()
        .filter_map(|path| {
            let res = parse_mmakefile_with_dirs(path, &args.source_dir, &dirs).ok();
            pb.inc(1);
            res
        })
        .collect();

    pb.finish_with_message("Parsing complete");

    let mut graph = DependencyGraph::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut skipped_headers: Vec<String> = Vec::new();
    let mut skipped_flags: Vec<String> = Vec::new();
    let mut ambiguous_flags = 0usize;
    let mut skipped_arch_sources: Vec<String> = Vec::new();
    let mut skipped_fetches: Vec<String> = Vec::new();
    let mut skipped_make_opts: Vec<String> = Vec::new();
    let mut skipped_conditions: Vec<String> = Vec::new();
    let mut generated_file_rules: Vec<String> = Vec::new();
    let mut skipped_programs: Vec<String> = Vec::new();
    let mut skipped_packages: Vec<String> = Vec::new();
    let mut skipped_icons: Vec<String> = Vec::new();
    let mut skipped_meta_rules: Vec<String> = Vec::new();
    for parsed in parsed_results {
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_icons(parsed.icon_targets, parsed.icons);
        skipped_icons.extend(parsed.skipped_icons);
        skipped_meta_rules.extend(parsed.skipped_meta_rules);
        graph.add_arch_decls(parsed.arch_decls);
        graph.add_copy_includes(parsed.copy_includes);
        graph.add_adhoc_header_rules(parsed.adhoc_header_rules);
        generated_file_rules.extend(parsed.generated_file_rules);
        skipped_programs.extend(parsed.skipped_programs);
        graph.add_packages(parsed.packages);
        skipped_packages.extend(parsed.skipped_packages);
        graph.add_arch_sources(parsed.arch_sources);
        graph.add_fetches(parsed.fetches);
        skipped_fetches.extend(parsed.skipped_fetches);
        skipped_make_opts.extend(parsed.skipped_make_opts);
        skipped_conditions.extend(parsed.skipped_conditions);
        skipped_arch_sources.extend(parsed.skipped_arch_sources);
        unresolved.extend(parsed.unresolved_includes);
        skipped_headers.extend(parsed.skipped_copy_includes);
        skipped_flags.extend(parsed.flags.skipped);
        if parsed.flags.ambiguous {
            ambiguous_flags += 1;
        }
    }

    // Architecture includes are declared in the arch/ tree but consumed in
    // rom/, so they can only be joined once every file has been parsed.
    graph.resolve_arch_includes();

    // Package membership names modules, not targets, so it can only be
    // resolved once every mmakefile has contributed its targets.
    skipped_packages.extend(graph.resolve_packages());

    // uselibs names a link library by its libname, which only resolves once
    // every %build_linklib in the tree has been seen.
    let unresolved_libs = graph.resolve_use_libs();
    // Architecture source overrides are declared in arch/ but belong to a
    // target defined elsewhere, so they too need the full parse first.
    graph.resolve_arch_sources();
    // GNU Make drops a circular phony prerequisite during traversal; CMake
    // rejects utility-target cycles outright. Collapse each meta-only SCC to
    // its shared external prerequisite closure and make that visible.
    let flattened_meta_cycles = graph.flatten_meta_cycles();
    let n_overrides: usize = graph.arch_sources.values().map(Vec::len).sum();
    println!("🔧 {n_overrides} architecture source override(s) from %build_archspecific");
    println!(
        "🌐 {} third-party source fetch rule(s) from %fetch",
        graph.fetches.len()
    );

    write_report(
        &args.output,
        "skipped-make-opts.txt",
        skipped_make_opts,
        "make.opts file(s) not applied (Make conditionals or an unmapped path)",
    );
    // A skipped fetch means a third-party dependency the build cannot obtain.
    write_report(
        &args.output,
        "skipped-fetches.txt",
        skipped_fetches,
        "%fetch declaration(s) reference unmapped Make variables",
    );
    write_report(
        &args.output,
        "skipped-arch-sources.txt",
        skipped_arch_sources,
        "%build_archspecific declaration(s) had no resolvable file list",
    );
    write_report(
        &args.output,
        "skipped-icons.txt",
        skipped_icons,
        "%build_icons declaration(s) or target variant(s) could not be resolved",
    );
    write_report(
        &args.output,
        "skipped-meta-rules.txt",
        skipped_meta_rules,
        "#MM target/dependency token(s) reference unmapped Make variables",
    );
    write_report(
        &args.output,
        "meta-cycles.txt",
        flattened_meta_cycles,
        "cyclic #MM component(s) flattened to shared dependencies",
    );

    println!(
        "📥 {} SDK header staging rule(s) from %copy_includes",
        graph.copy_includes.len()
    );
    // A skipped declaration means a header never reaches the SDK, and that has
    // to be inspectable.
    write_report(
        &args.output,
        "skipped-header-staging.txt",
        skipped_headers,
        "%copy_includes declaration(s) skipped (out-of-tree or unresolved)",
    );

    write_report(
        &args.output,
        "unresolved-uselibs.txt",
        unresolved_libs,
        "uselibs name(s) matched no link library",
    );
    // A package missing a member still builds. The gap only shows up as a
    // system that does not boot, so it has to be visible here.
    write_report(
        &args.output,
        "unresolved-package-members.txt",
        skipped_packages,
        "package member(s) could not be resolved to a target",
    );
    if !graph.packages.is_empty() {
        let members: usize = graph.packages.iter().map(|p| p.resolved.len()).sum();
        println!(
            "📦 {} package/kickstart declaration(s) with {members} member(s)",
            graph.packages.len()
        );
    }

    // These are build declarations, not flags or headers: each one is a target
    // the historic build produces and this one does not.
    write_report(
        &args.output,
        "unmodelled-declarations.txt",
        skipped_programs,
        "build declaration(s) of a kind the target model does not express",
    );
    // Not headers, so these do not break a compile; they break a link or a
    // package step, which is harder to trace back. Listed for that reason.
    write_report(
        &args.output,
        "generated-file-rules.txt",
        generated_file_rules,
        "hand-written $(GENDIR) rule(s) build something other than a header",
    );
    write_report(
        &args.output,
        "skipped-flags.txt",
        skipped_flags,
        "compiler flag(s) not propagated (not a simple -D, or an unmapped variable)",
    );
    write_report(
        &args.output,
        "skipped-conditions.txt",
        skipped_conditions,
        "Make conditional(s) guard flags in a way that is not an architecture test",
    );
    if ambiguous_flags > 0 {
        println!(
            "⚠️  {ambiguous_flags} mmakefile(s) reassign USER_CPPFLAGS/USER_CFLAGS and build several modules; \
             flags are read file-globally there and may differ from Make"
        );
    }
    // The full list is long and dominated by third-party ports, so it goes next
    // to the generated CMake file rather than into the build log.
    write_report(
        &args.output,
        "unresolved-includes.txt",
        unresolved,
        "include path(s) reference unmapped Make variables and were skipped",
    );

    println!(
        "🖼️  {} resolved icon declaration variant(s), {} unique icon target(s)",
        graph.icons.len(),
        graph.icon_targets.len()
    );
    println!(
        "🔨 Assembling Dependency Graph with {} concrete targets, {} icon targets and {} meta-targets...",
        graph.targets.len(),
        graph.icon_targets.len(),
        graph.meta_targets.len()
    );

    graph.validate_cycles()?;
    info!("Dependency graph validated: 0 cycles detected");

    println!(
        "📝 Generating CMake target definitions -> {}...",
        args.output.display()
    );
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }

    let cmake_content = generate_cmake(&graph);
    fs::write(&args.output, cmake_content)?;

    println!(
        "✅ Successfully generated {} concrete targets, {} icon targets and {} meta-targets in {}!",
        graph.targets.len(),
        graph.icon_targets.len(),
        graph.meta_targets.len(),
        args.output.display()
    );
    Ok(())
}

/// Writes one skip report next to the generated CMake file.
///
/// Removes the file when there is nothing left to report. Every report used to
/// be written only in the non-empty case, so a file outlived the change that
/// emptied it and went on naming declarations that were no longer skipped. That
/// is worse than no report: the numbers are what the next step is chosen from.
///
/// A write failure is announced rather than swallowed. The count is printed
/// either way, so a read-only build directory costs the detail, not the signal.
fn write_report(output: &Path, extension: &str, mut lines: Vec<String>, what: &str) {
    let report = output.with_extension(extension);
    if lines.is_empty() {
        let _ = fs::remove_file(&report);
        return;
    }
    lines.sort_unstable();
    lines.dedup();
    let n = lines.len();
    let body = lines.join("\n");
    if fs::write(&report, format!("{body}\n")).is_ok() {
        println!("⚠️  {n} {what} -> {}", report.display());
    } else {
        println!(
            "⚠️  {n} {what} (report could not be written to {})",
            report.display()
        );
    }
}
