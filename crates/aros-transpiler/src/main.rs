use aros_common::{
    ArosError, Diagnostic, DiagnosticCode, DiagnosticSet, DiagnosticSeverity, DiagnosticStage,
    Result, SourceLocation,
};
use aros_transpiler::dirs::DirVars;
use aros_transpiler::{
    collect_mmakefile_fetches_with_context, default_link_set_available, generate_cmake,
    generated_header, parse_mmakefile_with_dirs, parse_mmakefile_with_dirs_and_context_and_fetches,
    read_default_link_set, DependencyGraph, TargetContext,
};
use clap::{Parser, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tracing::info;
use walkdir::WalkDir;

mod publication;

use publication::Publication;

#[derive(Parser, Debug)]
#[command(author, version, about = "AROS-NG Parallel mmakefile Transpiler")]
struct Args {
    /// Root directory of AROS source tree
    #[arg(short, long, default_value = ".")]
    source_dir: PathBuf,

    /// Output path for generated CMake targets file
    #[arg(short, long, default_value = "build/generated_targets.cmake")]
    output: PathBuf,

    /// Physical configure-time path behind `${AROS_PORTS_DIR}`
    #[arg(long)]
    ports_dir: Option<PathBuf>,

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

    /// Diagnostic renderer used for failures
    #[arg(long, value_enum, default_value_t = DiagnosticFormat::Human)]
    diagnostic_format: DiagnosticFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DiagnosticFormat {
    Human,
    Json,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if matches!(args.diagnostic_format, DiagnosticFormat::Json) {
        tracing_subscriber::fmt().with_writer(std::io::sink).init();
    } else {
        tracing_subscriber::fmt::init();
    }

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            render_diagnostics(&error_to_diagnostics(error), args.diagnostic_format);
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<()> {
    if let Err(error) = aros_transpiler::fingerprints::validate() {
        return Err(diagnostics_error(vec![Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            error,
        )
        .with_location(SourceLocation::new(
            "tools/aros-tools/crates/aros-transpiler/capability-fingerprints.pins",
        ))
        .with_hint(
            "repair the embedded registry and rebuild the transpiler binary",
        )]));
    }
    println!(
        "⚡ AROS-NG Transpiler v0.1.0 — Scanning MetaMake inputs in {}...",
        args.source_dir.display()
    );

    // Build trees must be skipped. The SDK staging step copies whole source
    // directories, mmakefile.src included, so scanning build/ would parse those
    // copies a second time and attribute their rules to the wrong location.
    let skip_dirs = ["build", "target", ".git"];
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&args.source_dir)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || e.depth() == 0
                || !skip_dirs
                    .iter()
                    .any(|d| e.file_name().to_string_lossy() == *d)
        })
    {
        let entry = entry.map_err(|error| {
            diagnostics_error(vec![Diagnostic::error(
                DiagnosticCode::SourceWalk,
                DiagnosticStage::SourceWalk,
                format!("cannot walk MetaMake source tree: {error}"),
            )
            .with_location(SourceLocation::new("."))])
        })?;
        // MetaMake reads both generated-template inputs (`mmakefile.src`) and
        // direct make fragments (`mmakefile`).  The latter include the
        // top-level AROS/AROS-complete roots and 32 further dependency files;
        // omitting them leaves an apparently valid but disconnected graph.
        if matches!(
            entry.file_name().to_str(),
            Some("mmakefile.src" | "mmakefile")
        ) {
            files.push(entry.into_path());
        }
    }
    // Stable source order matters for duplicate-output semantics: GNU Make's
    // first satisfiable icon rule wins, and the CMake output registry mirrors
    // that choice while reporting conflicting later claims.
    files.sort();

    println!(
        "📦 Found {} MetaMake input files. Parsing in parallel...",
        files.len()
    );

    let pb = if matches!(args.diagnostic_format, DiagnosticFormat::Json) {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(files.len() as u64)
    };
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap(),
    );

    let mut dirs = DirVars::load(&args.source_dir);
    if let Some(ports_dir) = &args.ports_dir {
        dirs.set_materialized_path("AROS_PORTS_DIR", ports_dir.clone());
    }
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
    let known_fetches = if let Some(target) = target.as_ref() {
        let results: Vec<_> = files
            .par_iter()
            .map(|path| {
                (
                    path,
                    collect_mmakefile_fetches_with_context(path, &args.source_dir, target),
                )
            })
            .collect();
        let mut fetches = Vec::new();
        let mut errors = Vec::new();
        for (path, result) in results {
            match result {
                Ok(found) => fetches.extend(found),
                Err(error) => errors.push(
                    Diagnostic::error(
                        DiagnosticCode::FetchDiscovery,
                        DiagnosticStage::FetchDiscovery,
                        error.to_string(),
                    )
                    .with_location(source_location(path, &args.source_dir)),
                ),
            }
        }
        if !errors.is_empty() {
            return Err(diagnostics_error(errors));
        }
        fetches
    } else {
        Vec::new()
    };
    let parsed_results: Vec<_> = files
        .par_iter()
        .map(|path| {
            let res = match &target {
                Some(target) => parse_mmakefile_with_dirs_and_context_and_fetches(
                    path,
                    &args.source_dir,
                    &dirs,
                    target,
                    &known_fetches,
                ),
                None => parse_mmakefile_with_dirs(path, &args.source_dir, &dirs),
            };
            pb.inc(1);
            (path, res)
        })
        .collect();

    pb.finish_with_message("Parsing complete");

    let mut parse_errors = Vec::new();
    let mut parsed_files = Vec::new();
    for (path, result) in parsed_results {
        match result {
            Ok(parsed) => parsed_files.push(parsed),
            Err(error) => parse_errors.push(
                Diagnostic::error(
                    DiagnosticCode::SourceParse,
                    DiagnosticStage::Parsing,
                    error.to_string(),
                )
                .with_location(source_location(path, &args.source_dir)),
            ),
        }
    }
    if !parse_errors.is_empty() {
        return Err(diagnostics_error(parse_errors));
    }

    let mut graph = DependencyGraph::new();
    let mut capability_errors: Vec<Diagnostic> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut skipped_headers: Vec<String> = Vec::new();
    let mut skipped_copy_directories: Vec<String> = Vec::new();
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
    let mut source_inventory_patterns: Vec<String> = Vec::new();
    let mut skipped_client_archives: Vec<String> = Vec::new();
    let mut skipped_binary_objects: Vec<String> = Vec::new();
    let mut skipped_host_generated_headers: Vec<String> = Vec::new();
    let mut skipped_hidd_stubs: Vec<String> = Vec::new();
    let mut skipped_script_outputs: Vec<String> = Vec::new();
    let mut unresolved_output_paths: Vec<String> = Vec::new();
    let mut skipped_packages: Vec<String> = Vec::new();
    let mut skipped_icons: Vec<String> = Vec::new();
    let mut skipped_catalogs: Vec<String> = Vec::new();
    let mut skipped_flexcat_sources: Vec<String> = Vec::new();
    let mut skipped_meta_rules: Vec<String> = Vec::new();
    for parsed in parsed_files {
        capability_errors.extend(parsed.capability_errors);
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
        graph.add_flexcat_sources(parsed.flexcat_sources);
        skipped_flexcat_sources.extend(parsed.skipped_flexcat_sources);
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_icons(parsed.icon_targets, parsed.icons);
        skipped_icons.extend(parsed.skipped_icons);
        graph.add_catalogs(parsed.catalogs);
        skipped_catalogs.extend(parsed.skipped_catalogs);
        skipped_meta_rules.extend(parsed.skipped_meta_rules);
        graph.add_host_generated_headers(parsed.host_generated_headers);
        graph.add_hidd_stubs(parsed.hidd_stubs);
        skipped_hidd_stubs.extend(parsed.skipped_hidd_stubs);
        graph.add_script_outputs(parsed.script_outputs);
        skipped_script_outputs.extend(parsed.skipped_script_outputs);
        skipped_host_generated_headers.extend(parsed.skipped_host_generated_headers);
        graph.add_binary_objects(parsed.binary_objects);
        skipped_binary_objects.extend(parsed.skipped_binary_objects);
        graph.add_arch_decls(parsed.arch_decls);
        graph.add_copy_includes(parsed.copy_includes);
        skipped_copy_directories.extend(graph.add_copy_directories(parsed.copy_directories));
        graph.add_adhoc_header_rules(parsed.adhoc_header_rules);
        graph.add_header_transforms(parsed.header_transforms);
        graph.add_define_headers(parsed.define_headers);
        generated_file_rules.extend(parsed.generated_file_rules);
        skipped_programs.extend(parsed.skipped_programs);
        partial_source_lists.extend(parsed.partial_source_lists);
        source_inventory_patterns.extend(parsed.source_inventory_patterns);
        skipped_client_archives.extend(parsed.skipped_client_archives);
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
        skipped_copy_directories.extend(parsed.skipped_copy_directories);
        skipped_flags.extend(parsed.flags.skipped);
        if parsed.flags.ambiguous {
            ambiguous_flags += 1;
        }
    }

    if !capability_errors.is_empty() {
        return Err(diagnostics_error(capability_errors));
    }

    source_inventory_patterns.sort();
    source_inventory_patterns.dedup();
    partial_source_lists.extend(
        graph
            .resolve_source_inventory_fetches(&source_inventory_patterns)
            .into_iter()
            .map(|pattern| {
                format!(
                    "fetched-tree source wildcard has no owning %fetch declaration: `{pattern}`"
                )
            }),
    );

    // Architecture includes are declared in the arch/ tree but consumed in
    // rom/, so they can only be joined once every file has been parsed.
    graph.resolve_arch_includes();

    // Package membership names modules, not targets, so it can only be
    // resolved once every mmakefile has contributed its targets.
    skipped_packages.extend(graph.resolve_packages());

    // uselibs names a link library by its libname, which only resolves once
    // every %build_linklib in the tree has been seen.
    // The HIDD stub archive has to exist before uselibs are resolved: 61
    // declarations name `uselibs=hiddstubs`, and until %make_hidd_stubs was
    // modelled every one of them was reported as having no link library.
    skipped_hidd_stubs.extend(graph.resolve_hidd_stubs());

    // Before uselibs, because a generated source has to be registered before
    // the target that names it is emitted.
    skipped_script_outputs.extend(graph.resolve_script_outputs());

    let unresolved_libs = graph.resolve_use_libs();

    // The compiler spec's default link set. configure.in:3044 selects
    // config/<object-format>-specs.in and falls back to config/elf-specs.in;
    // only the ELF template exists in the tree, and every target this build
    // supports is ELF, so the format is not a transpiler argument yet. A new
    // object format would need one, and read_default_link_set would then pick
    // its template up automatically.
    let mut unresolved_default_link_set: Vec<String> = Vec::new();
    if default_link_set_available(&args.source_dir) {
        match read_default_link_set(&args.source_dir, "elf") {
            Ok(set) => {
                unresolved_default_link_set = graph.resolve_default_link_set(&set);
            }
            // A spec we cannot represent must stop the build rather than
            // quietly produce links without the default set.
            Err(error) => {
                return Err(diagnostics_error(vec![Diagnostic::error(
                    DiagnosticCode::SourceParse,
                    DiagnosticStage::Parsing,
                    format!("cannot read the compiler default link set: {error}"),
                )
                .with_location(SourceLocation::new("config/elf-specs.in"))
                .with_hint(
                    "update the default-link-set parser before generating an incomplete link graph",
                )]));
            }
        }
    } else {
        unresolved_default_link_set.push(
            "compiler/autoinit/auto is absent, so no default link set was applied".to_owned(),
        );
    }

    // The section-ordering script a kickstart member's partial link needs.
    // config/make.tmpl:2168 and :2758 pass $(KERNEL_KOBJ_LDSCRIPT) to the `-Ur`
    // link, and config/make.cfg.in sets it. Without that ordering a link merges
    // every module's Resident tag into one block and every End marker into a
    // block behind it, so the first rt_EndSkip the romtag scanner reads leaps
    // over the rest of the kickstart.
    let mut unresolved_kobj_ldscript: Vec<String> = Vec::new();
    match dirs.expand("$(KERNEL_KOBJ_LDSCRIPT)") {
        Some(value) => {
            let tokens: Vec<&str> = value.split_whitespace().collect();
            match tokens.as_slice() {
                [] => {}
                ["-T", script] => {
                    graph.kickstart_kobj_ldscript = vec!["-T".to_owned(), (*script).to_owned()];
                }
                _ => unresolved_kobj_ldscript.push(format!(
                    "KERNEL_KOBJ_LDSCRIPT is `{value}`, and only an empty value \
                     or `-T <script>` is modelled"
                )),
            }
        }
        None => unresolved_kobj_ldscript
            .push("KERNEL_KOBJ_LDSCRIPT could not be resolved from config/make.cfg.in".to_owned()),
    }
    // DirVars cannot decide `ifeq ($(AROS_TARGET_ARCH),amiga)`, because this
    // build supplies the arch as a CMake expression, so the amiga-m68k override
    // of the script is never taken. Say so when that is the target being
    // configured rather than let the generic script stand in for it.
    if args.cpu.as_deref() == Some("m68k") && args.platform.as_deref() == Some("amiga") {
        unresolved_kobj_ldscript.push(
            "the amiga-m68k override of KERNEL_KOBJ_LDSCRIPT was not applied: \
             config/make.cfg.in guards it with ifeq on AROS_TARGET_ARCH, which \
             this build supplies as a CMake expression"
                .to_owned(),
        );
    }

    let unresolved_generated_headers = graph.resolve_define_headers();
    // Architecture source overrides are declared in arch/ but belong to a
    // target defined elsewhere, so they too need the full parse first.
    // A lane whose `arch=` is a name rather than an architecture gets the tag of
    // the lane that pulls it in, before the overrides are resolved.
    let arch_lane_attachments = graph.resolve_arch_lane_attachments();
    let inherited_arch_sources = graph.resolve_arch_sources();
    // Catalog generators can emit a source/header adjacent to a module's
    // sources. CMake rehomes that output, so preserve the completed logical
    // source-tree consumer relationship before rendering the build graph.
    graph.resolve_catalog_consumers();
    graph.resolve_flexcat_source_consumers();
    // A concrete target must order its own fetched sources. Depending on a
    // sibling which happens to use the same archive does not constrain a
    // direct Ninja invocation of this target.
    let mut unowned_port_sources = graph.resolve_port_source_fetches();
    unowned_port_sources.extend(graph.resolve_header_transforms());
    skipped_copy_directories.extend(graph.resolve_copy_directories());
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

    let mut publication = Publication::default();
    write_report(
        &mut publication,
        &args.output,
        "skipped-script-outputs.txt",
        skipped_script_outputs,
        "script-generated file rule(s) could not be bound",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-hidd-stubs.txt",
        skipped_hidd_stubs,
        "%make_hidd_stubs declaration(s) could not be used",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-host-generated-headers.txt",
        skipped_host_generated_headers,
        "host-tool header rule(s) could not be represented",
    );
    write_report(
        &mut publication,
        &args.output,
        "kickstart-kobj-ldscript.txt",
        unresolved_kobj_ldscript,
        "kickstart section-ordering script issue(s)",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-binary-objects.txt",
        skipped_binary_objects,
        "%rule_link_binary declaration(s) could not be resolved",
    );
    write_report(
        &mut publication,
        &args.output,
        "arch-lane-attachments.txt",
        arch_lane_attachments,
        "architecture lane(s) attached to another lane by a #MM edge",
    );
    write_report(
        &mut publication,
        &args.output,
        "inherited-arch-sources.txt",
        inherited_arch_sources,
        "declaration(s) inherit architecture sources through a shared arch object root",
    );
    write_report(
        &mut publication,
        &args.output,
        "unresolved-default-link-set.txt",
        unresolved_default_link_set,
        "compiler-spec default link set item(s) have no archive in this configuration",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-client-archives.txt",
        skipped_client_archives,
        "module(s) need a genmodule client archive that only modtype=library builds",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-make-opts.txt",
        skipped_make_opts,
        "make.opts file(s) not applied (Make conditionals or an unmapped path)",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-local-make-includes.txt",
        skipped_local_make_includes,
        "local Make include fragment(s) remain unsafe, unresolved, or outside the plain source-list scope",
    );
    // A skipped fetch means a third-party dependency the build cannot obtain.
    write_report(
        &mut publication,
        &args.output,
        "skipped-fetches.txt",
        skipped_fetches,
        "%fetch declaration(s) reference unmapped Make variables",
    );
    write_report(
        &mut publication,
        &args.output,
        "unowned-port-sources.txt",
        unowned_port_sources,
        "port source(s) have no matching %fetch destination owner",
    );
    write_report(
        &mut publication,
        &args.output,
        "unresolved-generated-headers.txt",
        unresolved_generated_headers,
        "generated literal header(s) have no concrete provider target",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-arch-sources.txt",
        skipped_arch_sources,
        "%build_archspecific declaration(s) had no resolvable file list",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-icons.txt",
        skipped_icons,
        "%build_icons declaration(s) or target variant(s) could not be resolved",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-catalogs.txt",
        skipped_catalogs,
        "%build_catalogs declaration(s) could not be resolved",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-flexcat-sources.txt",
        skipped_flexcat_sources,
        "hand-written FlexCat source/header rule(s) could not be resolved",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-meta-rules.txt",
        skipped_meta_rules,
        "#MM target/dependency token(s) reference unmapped Make variables",
    );
    write_report(
        &mut publication,
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
        &mut publication,
        &args.output,
        "skipped-header-staging.txt",
        skipped_headers,
        "%copy_includes declaration(s) skipped (out-of-tree or unresolved)",
    );
    println!(
        "📂 {} recursive directory staging rule(s) from %copy_dir_recursive",
        graph.copy_directories.len()
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-directory-staging.txt",
        skipped_copy_directories,
        "%copy_dir_recursive declaration(s) skipped (unsafe, unresolved, or ambiguous)",
    );

    write_report(
        &mut publication,
        &args.output,
        "unresolved-uselibs.txt",
        unresolved_libs,
        "uselibs/link-option name(s) matched no public link library",
    );
    // A package missing a member still builds. The gap only shows up as a
    // system that does not boot, so it has to be visible here.
    write_report(
        &mut publication,
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
        &mut publication,
        &args.output,
        "unmodelled-declarations.txt",
        skipped_programs,
        "build declaration(s) of a kind the target model does not express",
    );
    write_report(
        &mut publication,
        &args.output,
        "partial-source-lists.txt",
        partial_source_lists,
        "source lane(s) omitted from otherwise retained targets",
    );
    write_report(
        &mut publication,
        &args.output,
        "unresolved-output-paths.txt",
        unresolved_output_paths,
        "explicit program output path(s) could not be evaluated",
    );
    // Not headers, so these do not break a compile; they break a link or a
    // package step, which is harder to trace back. Listed for that reason.
    write_report(
        &mut publication,
        &args.output,
        "generated-file-rules.txt",
        generated_file_rules,
        "hand-written $(GENDIR) rule(s) build something other than a header",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-flags.txt",
        skipped_flags,
        "compiler flag(s) not propagated (not a simple -D, or an unmapped variable)",
    );
    write_report(
        &mut publication,
        &args.output,
        "skipped-conditions.txt",
        skipped_conditions,
        "Make conditional(s) guard flags in a way that is not an architecture test",
    );
    if ambiguous_flags > 0 {
        publication.notice(format!(
            "⚠️  [AT1032] {ambiguous_flags} mmakefile(s) reassign USER_CPPFLAGS/USER_CFLAGS and build several modules; flags are read file-globally there and may differ from Make"
        ));
    }
    publication.record_coverage(
        "AT1032",
        DiagnosticSeverity::Warning,
        None,
        ambiguous_flags,
        "mmakefiles reassign flags while declaring several modules",
    );
    // The full list is long and dominated by third-party ports, so it goes next
    // to the generated CMake file rather than into the build log.
    write_report(
        &mut publication,
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

    let cmake_content = format!(
        "{}{}",
        generated_header(target.as_ref()),
        generate_cmake(&graph)
    );
    let coverage_json = publication.coverage_json()?;
    publication.present(args.output.with_extension("coverage.json"), coverage_json);
    publication.present(
        args.output.with_extension("source-inventory.cmake"),
        render_source_inventory_manifest(&graph),
    );

    // The default link set is applied by CMake, which needs to know which
    // declarations suppress part of it. These are driver switches and must not
    // reach ld.lld, so they travel as a manifest rather than as link items.
    let mut spec_switch_lines: Vec<String> = graph
        .targets
        .iter()
        .filter(|(_, target)| !target.spec_switches.is_empty())
        .map(|(mmake, target)| format!("{mmake}\t{}", target.spec_switches.join("\t")))
        .collect();
    spec_switch_lines.sort();
    let spec_switch_path = args.output.with_extension("spec-switches.txt");
    if spec_switch_lines.is_empty() {
        publication.absent(spec_switch_path);
    } else {
        publication.present(
            spec_switch_path.clone(),
            format!("{}\n", spec_switch_lines.join("\n")),
        );
        publication.notice(format!(
            "🔒 {} declaration(s) suppress part of the default link set -> {}",
            spec_switch_lines.len(),
            spec_switch_path.display()
        ));
    }

    // The graph is the commit marker consumed by CMake and is deliberately
    // replaced after every sidecar and report in the same transaction.
    publication.present(args.output.clone(), cmake_content);
    publication.publish().map_err(|error| {
        diagnostics_error(vec![Diagnostic::error(
            DiagnosticCode::OutputIo,
            DiagnosticStage::OutputPublication,
            format!("cannot publish the generated output set: {error}"),
        )
        .with_location(source_location(&args.output, &args.source_dir))
        .with_hint(
            "the previous complete generation was retained whenever rollback succeeded",
        )])
    })?;

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

fn cmake_quoted_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn diagnostics_error(diagnostics: Vec<Diagnostic>) -> ArosError {
    ArosError::Diagnostics(DiagnosticSet::new(diagnostics))
}

fn source_location(path: &Path, root: &Path) -> SourceLocation {
    SourceLocation::new(
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
    )
}

fn error_to_diagnostics(error: ArosError) -> DiagnosticSet {
    match error {
        ArosError::Diagnostics(diagnostics) => diagnostics,
        ArosError::TranspilerSyntax { file, message } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::SourceParse,
                DiagnosticStage::Parsing,
                message,
            )
            .with_location(SourceLocation::new(file)),
        ),
        ArosError::DependencyCycle { target } => DiagnosticSet::single(
            Diagnostic::error(
                DiagnosticCode::GraphValidation,
                DiagnosticStage::GraphValidation,
                format!("dependency cycle detected in module: {target}"),
            )
            .with_hint("break or explicitly model the cycle before publishing the graph"),
        ),
        ArosError::Io(error) => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::OutputIo,
            DiagnosticStage::OutputPublication,
            error.to_string(),
        )),
        ArosError::Json(error) => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            format!("diagnostic serialization failed: {error}"),
        )),
        ArosError::ToolchainNotFound { binary } => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            format!("unexpected toolchain lookup for `{binary}`"),
        )),
        ArosError::CommandFailed { cmd } => DiagnosticSet::single(Diagnostic::error(
            DiagnosticCode::InternalInvariant,
            DiagnosticStage::Internal,
            format!("unexpected command failure: {cmd}"),
        )),
    }
}

fn render_diagnostics(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    match format {
        DiagnosticFormat::Human => eprint!("{diagnostics}"),
        DiagnosticFormat::Json => match serde_json::to_string_pretty(&diagnostics) {
            Ok(json) => eprintln!("{json}"),
            Err(_) => eprintln!(
                "{{\"schema\":\"{}\",\"diagnostics\":[{{\"code\":\"AT0007\",\"severity\":\"error\",\"stage\":\"internal\",\"message\":\"diagnostic serialization failed\"}}]}}",
                DiagnosticSet::SCHEMA
            ),
        },
    }
}

fn render_source_inventory_manifest(graph: &DependencyGraph) -> String {
    let mut fetches: Vec<_> = graph
        .source_inventory_fetches
        .iter()
        .filter_map(|name| graph.fetches.iter().find(|fetch| &fetch.name == name))
        .collect();
    fetches.sort_by(|left, right| left.name.cmp(&right.name));

    let mut body = format!("set(AROS_SOURCE_INVENTORY_FETCH_COUNT {})\n", fetches.len());
    for (index, fetch) in fetches.into_iter().enumerate() {
        let fields = [
            ("NAME", fetch.name.as_str()),
            ("ARCHIVE", fetch.archive.as_str()),
            ("SUFFIXES", fetch.suffixes.as_str()),
            ("ORIGINS", fetch.origins.as_str()),
            ("LOCATION", fetch.location.as_str()),
            ("DESTINATION", fetch.destination.as_str()),
            ("BASE", fetch.base.as_str()),
            ("PATCH_ORIGINS", fetch.patch_origins.as_str()),
            ("PATCHES", fetch.patches.as_str()),
        ];
        for (field, value) in fields {
            let _ = writeln!(
                body,
                "set(AROS_SOURCE_INVENTORY_FETCH_{index}_{field} \"{}\")",
                cmake_quoted_value(value)
            );
        }
    }
    body
}

/// Writes one skip report next to the generated CMake file.
///
/// Removes the file when there is nothing left to report. Every report used to
/// be written only in the non-empty case, so a file outlived the change that
/// emptied it and went on naming declarations that were no longer skipped. That
/// is worse than no report: the numbers are what the next step is chosen from.
///
/// Reports are part of the same publication transaction as the generated
/// graph. A stale report is removed only when the replacement generation
/// commits successfully.
fn write_report(
    publication: &mut Publication,
    output: &Path,
    extension: &str,
    mut lines: Vec<String>,
    what: &str,
) {
    let report = output.with_extension(extension);
    lines.sort_unstable();
    lines.dedup();
    let n = lines.len();
    let (code, severity) = report_metadata(extension);
    publication.record_coverage(code, severity, Some(&report), n, what);
    if lines.is_empty() {
        publication.absent(report);
        return;
    }
    let body = lines.join("\n");
    publication.present(report.clone(), format!("{body}\n"));
    let marker = if severity == DiagnosticSeverity::Info {
        "ℹ️ "
    } else {
        "⚠️ "
    };
    publication.notice(format!(
        "{marker} [{code}] {n} {what} -> {}",
        report.display()
    ));
}

fn report_metadata(extension: &str) -> (&'static str, DiagnosticSeverity) {
    use DiagnosticSeverity::{Error, Info, Warning};
    match extension {
        "skipped-script-outputs.txt" => ("AT1001", Warning),
        "skipped-hidd-stubs.txt" => ("AT1002", Warning),
        "skipped-host-generated-headers.txt" => ("AT1003", Warning),
        "kickstart-kobj-ldscript.txt" => ("AT1004", Warning),
        "skipped-binary-objects.txt" => ("AT1005", Warning),
        "arch-lane-attachments.txt" => ("AT1006", Info),
        "inherited-arch-sources.txt" => ("AT1007", Info),
        "unresolved-default-link-set.txt" => ("AT1008", Warning),
        "skipped-client-archives.txt" => ("AT1009", Warning),
        "skipped-make-opts.txt" => ("AT1010", Warning),
        "skipped-local-make-includes.txt" => ("AT1011", Warning),
        "skipped-fetches.txt" => ("AT1012", Warning),
        "unowned-port-sources.txt" => ("AT1013", Warning),
        "unresolved-generated-headers.txt" => ("AT1014", Warning),
        "skipped-arch-sources.txt" => ("AT1015", Warning),
        "skipped-icons.txt" => ("AT1016", Warning),
        "skipped-catalogs.txt" => ("AT1017", Warning),
        "skipped-flexcat-sources.txt" => ("AT1018", Warning),
        "skipped-meta-rules.txt" => ("AT1019", Warning),
        "meta-cycles.txt" => ("AT1020", Info),
        "skipped-header-staging.txt" => ("AT1021", Warning),
        "skipped-directory-staging.txt" => ("AT1022", Warning),
        "unresolved-uselibs.txt" => ("AT1023", Warning),
        "unresolved-package-members.txt" => ("AT1024", Warning),
        "unmodelled-declarations.txt" => ("AT1025", Warning),
        "partial-source-lists.txt" => ("AT1026", Warning),
        "unresolved-output-paths.txt" => ("AT1027", Warning),
        "generated-file-rules.txt" => ("AT1028", Warning),
        "skipped-flags.txt" => ("AT1029", Warning),
        "skipped-conditions.txt" => ("AT1030", Warning),
        "unresolved-includes.txt" => ("AT1031", Warning),
        // Adding a report without assigning a stable code is an internal
        // contract error. Publication rejects Error-severity coverage entries.
        _ => ("AT1099", Error),
    }
}
