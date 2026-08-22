use aros_common::Result;
use aros_transpiler::dirs::DirVars;
use aros_transpiler::{
    collect_mmakefile_fetches_with_context, generate_cmake, parse_mmakefile_with_dirs,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, TargetContext,
};
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

    /// Target instruction set (for example x86_64, arm, or aarch64)
    #[arg(long)]
    cpu: Option<String>,

    /// Target machine/platform (for example pc or raspi)
    #[arg(long)]
    platform: Option<String>,

    /// MetaMake target family
    #[arg(long)]
    family: Option<String>,

    /// MetaMake target variant; pass an empty value for the ordinary variant
    #[arg(long)]
    variant: Option<String>,

    /// Toolchain family (gnu or llvm)
    #[arg(long)]
    toolchain: Option<String>,

    /// Optional 32-bit companion CPU
    #[arg(long)]
    cpu32: Option<String>,

    /// Historic USE_MMU value (0 or 1)
    #[arg(long)]
    use_mmu: Option<String>,

    /// Historic GCC_CONFIG_FLOAT_ABI value
    #[arg(long)]
    float_abi: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    println!(
        "⚡ AROS-NG Transpiler v0.1.0 — Scanning MetaMake inputs in {}...",
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
        // MetaMake reads both generated-template inputs (`mmakefile.src`) and
        // direct make fragments (`mmakefile`).  The latter include the
        // top-level AROS/AROS-complete roots and 32 further dependency files;
        // omitting them leaves an apparently valid but disconnected graph.
        .filter(|e| matches!(e.file_name().to_str(), Some("mmakefile.src" | "mmakefile")))
        .map(walkdir::DirEntry::into_path)
        .collect();
    // Stable source order matters for duplicate-output semantics: GNU Make's
    // first satisfiable icon rule wins, and the CMake output registry mirrors
    // that choice while reporting conflicting later claims.
    files.sort();

    println!(
        "📦 Found {} MetaMake input files. Parsing in parallel...",
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
    let target = [
        &args.cpu,
        &args.platform,
        &args.family,
        &args.variant,
        &args.toolchain,
        &args.cpu32,
        &args.use_mmu,
        &args.float_abi,
    ]
    .iter()
    .any(|value| value.is_some())
    .then(|| TargetContext {
        cpu: args.cpu.clone(),
        platform: args.platform.clone(),
        family: args.family.clone(),
        variant: args.variant.clone(),
        toolchain: args.toolchain.clone(),
        cpu32: args.cpu32.clone(),
        use_mmu: args.use_mmu.clone(),
        float_abi: args.float_abi.clone(),
    });
    let known_fetches = target.as_ref().map_or_else(Vec::new, |target| {
        files
            .par_iter()
            .filter_map(|path| {
                collect_mmakefile_fetches_with_context(path, &args.source_dir, target).ok()
            })
            .flatten()
            .collect()
    });
    let parsed_results: Vec<_> = files
        .par_iter()
        .filter_map(|path| {
            let res = match &target {
                Some(target) => parse_mmakefile_with_dirs_and_context_and_fetches(
                    path,
                    &args.source_dir,
                    &dirs,
                    target,
                    &known_fetches,
                ),
                None => parse_mmakefile_with_dirs(path, &args.source_dir, &dirs),
            }
            .ok();
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
    let mut skipped_local_make_includes: Vec<String> = Vec::new();
    let mut skipped_conditions: Vec<String> = Vec::new();
    let mut generated_file_rules: Vec<String> = Vec::new();
    let mut skipped_programs: Vec<String> = Vec::new();
    let mut partial_source_lists: Vec<String> = Vec::new();
    let mut unresolved_output_paths: Vec<String> = Vec::new();
    let mut skipped_packages: Vec<String> = Vec::new();
    let mut skipped_icons: Vec<String> = Vec::new();
    let mut skipped_catalogs: Vec<String> = Vec::new();
    let mut skipped_meta_rules: Vec<String> = Vec::new();
    for parsed in parsed_results {
        for target in parsed.targets {
            graph.add_target(target);
        }
        for declaration in parsed.external_cmake {
            graph.add_external_cmake(declaration);
        }
        for declaration in parsed.configure_builds {
            graph.add_configure_build(declaration);
        }
        for declaration in parsed.grub_builds {
            graph.add_grub_build(declaration);
        }
        for declaration in parsed.ahi_builds {
            graph.add_ahi_build(declaration);
        }
        for declaration in parsed.python_outputs {
            graph.add_python_outputs(declaration);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_icons(parsed.icon_targets, parsed.icons);
        skipped_icons.extend(parsed.skipped_icons);
        graph.add_catalogs(parsed.catalogs);
        skipped_catalogs.extend(parsed.skipped_catalogs);
        skipped_meta_rules.extend(parsed.skipped_meta_rules);
        graph.add_arch_decls(parsed.arch_decls);
        graph.add_copy_includes(parsed.copy_includes);
        graph.add_adhoc_header_rules(parsed.adhoc_header_rules);
        graph.add_header_transforms(parsed.header_transforms);
        graph.add_define_headers(parsed.define_headers);
        generated_file_rules.extend(parsed.generated_file_rules);
        skipped_programs.extend(parsed.skipped_programs);
        partial_source_lists.extend(parsed.partial_source_lists);
        unresolved_output_paths.extend(parsed.unresolved_output_paths);
        graph.add_packages(parsed.packages);
        skipped_packages.extend(parsed.skipped_packages);
        graph.add_arch_sources(parsed.arch_sources);
        graph.add_fetches(parsed.fetches);
        skipped_fetches.extend(parsed.skipped_fetches);
        skipped_make_opts.extend(parsed.skipped_make_opts);
        skipped_local_make_includes.extend(parsed.skipped_local_make_includes);
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
    let unresolved_generated_headers = graph.resolve_define_headers();
    // Architecture source overrides are declared in arch/ but belong to a
    // target defined elsewhere, so they too need the full parse first.
    graph.resolve_arch_sources();
    // A concrete target must order its own fetched sources. Depending on a
    // sibling which happens to use the same archive does not constrain a
    // direct Ninja invocation of this target.
    let mut unowned_port_sources = graph.resolve_port_source_fetches();
    unowned_port_sources.extend(graph.resolve_header_transforms());
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
    write_report(
        &args.output,
        "skipped-local-make-includes.txt",
        skipped_local_make_includes,
        "local Make include fragment(s) remain unsafe, unresolved, or outside the plain source-list scope",
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
        "unowned-port-sources.txt",
        unowned_port_sources,
        "port source(s) have no matching %fetch destination owner",
    );
    write_report(
        &args.output,
        "unresolved-generated-headers.txt",
        unresolved_generated_headers,
        "generated literal header(s) have no concrete provider target",
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
        "skipped-catalogs.txt",
        skipped_catalogs,
        "%build_catalogs declaration(s) could not be resolved",
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
        "uselibs/link-option name(s) matched no public link library",
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
    write_report(
        &args.output,
        "partial-source-lists.txt",
        partial_source_lists,
        "source lane(s) omitted from otherwise retained targets",
    );
    write_report(
        &args.output,
        "unresolved-output-paths.txt",
        unresolved_output_paths,
        "explicit program output path(s) could not be evaluated",
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
        "🌐 {} resolved catalog declaration(s)",
        graph.catalogs.len()
    );
    println!(
        "🔨 Assembling Dependency Graph with {} concrete targets, {} external CMake targets, {} configure-style targets, {} GRUB2 host-tool lanes, {} AHI subsystem builds, {} Python output groups, {} icon targets, {} catalog targets and {} meta-targets...",
        graph.targets.len(),
        graph.external_cmake.len(),
        graph.configure_builds.len(),
        graph.grub_builds.len(),
        graph.ahi_builds.len(),
        graph.python_outputs.len(),
        graph.icon_targets.len(),
        graph.catalogs.len(),
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
        "✅ Successfully generated {} concrete targets, {} external CMake targets, {} configure-style targets, {} GRUB2 host-tool lanes, {} AHI subsystem builds, {} Python output groups, {} icon targets, {} catalog targets and {} meta-targets in {}!",
        graph.targets.len(),
        graph.external_cmake.len(),
        graph.configure_builds.len(),
        graph.grub_builds.len(),
        graph.ahi_builds.len(),
        graph.python_outputs.len(),
        graph.icon_targets.len(),
        graph.catalogs.len(),
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
