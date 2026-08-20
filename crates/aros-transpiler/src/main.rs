use aros_common::Result;
use aros_transpiler::{generate_cmake, parse_mmakefile, DependencyGraph};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fs;
use std::path::PathBuf;
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
    let files: Vec<PathBuf> = WalkDir::new(&args.source_dir)
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

    let parsed_results: Vec<_> = files
        .par_iter()
        .filter_map(|path| {
            let res = parse_mmakefile(path, &args.source_dir).ok();
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
    for parsed in parsed_results {
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_arch_decls(parsed.arch_decls);
        graph.add_copy_includes(parsed.copy_includes);
        graph.add_adhoc_header_rules(parsed.adhoc_header_rules);
        graph.add_arch_sources(parsed.arch_sources);
        graph.add_fetches(parsed.fetches);
        skipped_fetches.extend(parsed.skipped_fetches);
        skipped_make_opts.extend(parsed.skipped_make_opts);
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
    // Architecture source overrides are declared in arch/ but belong to a
    // target defined elsewhere, so they too need the full parse first.
    graph.resolve_arch_sources();

    let n_overrides: usize = graph.arch_sources.values().map(Vec::len).sum();
    println!("🔧 {n_overrides} architecture source override(s) from %build_archspecific");
    println!(
        "🌐 {} third-party source fetch rule(s) from %fetch",
        graph.fetches.len()
    );
    skipped_make_opts.sort_unstable();
    skipped_make_opts.dedup();
    if !skipped_make_opts.is_empty() {
        let report = args.output.with_extension("skipped-make-opts.txt");
        let body = skipped_make_opts.join("\n");
        if fs::write(&report, format!("{body}\n")).is_ok() {
            println!(
                "⚠️  {} make.opts file(s) not applied (Make conditionals or an unmapped path) -> {}",
                skipped_make_opts.len(),
                report.display()
            );
        }
    }
    skipped_fetches.sort_unstable();
    skipped_fetches.dedup();
    if !skipped_fetches.is_empty() {
        // Written out, not just counted: a skipped fetch means a third-party
        // dependency the build cannot obtain.
        let report = args.output.with_extension("skipped-fetches.txt");
        let body = skipped_fetches.join("\n");
        if fs::write(&report, format!("{body}\n")).is_ok() {
            println!(
                "⚠️  {} %fetch declaration(s) reference unmapped Make variables -> {}",
                skipped_fetches.len(),
                report.display()
            );
        } else {
            println!(
                "⚠️  {} %fetch declaration(s) reference unmapped Make variables",
                skipped_fetches.len()
            );
        }
    }
    skipped_arch_sources.sort_unstable();
    skipped_arch_sources.dedup();
    if !skipped_arch_sources.is_empty() {
        println!(
            "⚠️  {} %build_archspecific declaration(s) had no resolvable file list",
            skipped_arch_sources.len()
        );
    }

    println!(
        "📥 {} SDK header staging rule(s) from %copy_includes",
        graph.copy_includes.len()
    );
    skipped_headers.sort_unstable();
    skipped_headers.dedup();
    if !skipped_headers.is_empty() {
        // Written out, not just counted: a skipped declaration means a header
        // never reaches the SDK, and that has to be inspectable.
        let report = args.output.with_extension("skipped-header-staging.txt");
        let body = skipped_headers.join("\n");
        if fs::write(&report, format!("{body}\n")).is_ok() {
            println!(
                "⚠️  {} %copy_includes declaration(s) skipped (out-of-tree or unresolved) -> {}",
                skipped_headers.len(),
                report.display()
            );
        } else {
            println!(
                "⚠️  {} %copy_includes declaration(s) skipped (out-of-tree or unresolved)",
                skipped_headers.len()
            );
        }
    }

    skipped_flags.sort_unstable();
    skipped_flags.dedup();
    if !skipped_flags.is_empty() {
        let report = args.output.with_extension("skipped-flags.txt");
        let body = skipped_flags.join("\n");
        if fs::write(&report, format!("{body}\n")).is_ok() {
            println!(
                "⚠️  {} compiler flag(s) not propagated (not a simple -D, or an unmapped variable) -> {}",
                skipped_flags.len(),
                report.display()
            );
        }
    }
    if ambiguous_flags > 0 {
        println!(
            "⚠️  {ambiguous_flags} mmakefile(s) reassign USER_CPPFLAGS/USER_CFLAGS and build several modules; \
             flags are read file-globally there and may differ from Make"
        );
    }

    unresolved.sort_unstable();
    unresolved.dedup();
    if !unresolved.is_empty() {
        // The full list is long and dominated by third-party ports, so it goes
        // next to the generated CMake file rather than into the build log.
        let report = args.output.with_extension("unresolved-includes.txt");
        let body = unresolved.join("\n");
        if fs::write(&report, format!("{body}\n")).is_ok() {
            println!(
                "⚠️  {} include path(s) reference unmapped Make variables and were skipped -> {}",
                unresolved.len(),
                report.display()
            );
        } else {
            println!(
                "⚠️  {} include path(s) reference unmapped Make variables and were skipped",
                unresolved.len()
            );
        }
    }

    println!(
        "🔨 Assembling Dependency Graph with {} concrete targets and {} meta-targets...",
        graph.targets.len(),
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
        "✅ Successfully generated {} concrete targets and {} meta-targets in {}!",
        graph.targets.len(),
        graph.meta_targets.len(),
        args.output.display()
    );
    Ok(())
}
