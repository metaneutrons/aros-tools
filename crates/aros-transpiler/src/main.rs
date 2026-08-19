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

    let files: Vec<PathBuf> = WalkDir::new(&args.source_dir)
        .into_iter()
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

    let parsed_targets: Vec<_> = files
        .par_iter()
        .filter_map(|path| {
            let res = parse_mmakefile(path, &args.source_dir).ok();
            pb.inc(1);
            res
        })
        .flatten()
        .collect();

    pb.finish_with_message("Parsing complete");

    println!(
        "🔨 Assembling Dependency Graph with {} targets...",
        parsed_targets.len()
    );
    let mut graph = DependencyGraph::new();
    for target in parsed_targets {
        graph.add_target(target);
    }

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
        "✅ Successfully generated {} targets in {}!",
        graph.targets.len(),
        args.output.display()
    );
    Ok(())
}
