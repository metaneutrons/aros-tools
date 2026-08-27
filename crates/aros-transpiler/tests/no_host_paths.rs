//! The transpiler's output must not name the checkout it was run in.
//!
//! `generated_targets.cmake` is meant to depend only on the source tree, with
//! every location written as `${CMAKE_SOURCE_DIR}/...` or one of the other
//! CMake variables. Twelve lines did not: FlexCat's DESCRIPTION,
//! HEADER_TEMPLATE and SOURCE_TEMPLATE carried the absolute host path, because
//! the helper that was supposed to render them normalised separators and never
//! stripped the root.
//!
//! Checking the three fields that were wrong would not stop the next one, so
//! this asserts on the whole parse result: no declaration from any mmakefile in
//! the tree may mention the source root, for any of the three architectures.

use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars,
    parse_mmakefile_with_dirs_and_context_and_fetches, TargetContext,
};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
}

fn mmakefiles(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() == "mmakefile.src")
        .map(walkdir::DirEntry::into_path)
        .collect()
}

fn target_context(cpu: &str, platform: &str, float_abi: &str) -> TargetContext {
    TargetContext {
        cpu: Some(cpu.to_owned()),
        platform: Some(platform.to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(if cpu == "x86_64" { "i386" } else { "" }.to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(float_abi.to_owned()),
    }
}

#[test]
fn no_declaration_names_the_checkout_it_was_parsed_in() {
    let root = source_root();
    let root_text = root.to_string_lossy().to_string();
    let files = mmakefiles(&root);
    assert!(
        !files.is_empty(),
        "no mmakefile.src found under {root_text}"
    );
    let dirs = DirVars::load(&root);

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = files
            .iter()
            .flat_map(|file| {
                collect_mmakefile_fetches_with_context(file, &root, &context)
                    .unwrap_or_else(|error| panic!("{}: {error}", file.display()))
            })
            .collect::<Vec<_>>();

        let mut offenders: Vec<String> = Vec::new();
        for file in &files {
            let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
                file, &root, &dirs, &context, &fetches,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));

            // Debug renders every field of every declaration, so a host path
            // anywhere in the parse result is caught, not only in the fields
            // that were known to be wrong. The reports are excluded: they are
            // diagnostics for a reader, not build input, and they legitimately
            // quote paths.
            let rendered = format!(
                "{:?}{:?}{:?}{:?}{:?}{:?}",
                parsed.targets,
                parsed.flexcat_sources,
                parsed.copy_includes,
                parsed.copy_directories,
                parsed.icons,
                parsed.catalogs,
            );
            if rendered.contains(&root_text) {
                offenders.push(format!(
                    "{}: {}",
                    file.strip_prefix(&root).unwrap_or(file).display(),
                    rendered
                        .split_whitespace()
                        .filter(|word| word.contains(&root_text))
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
        }

        assert!(
            offenders.is_empty(),
            "{cpu}-{platform}: declarations naming the checkout: {offenders:#?}"
        );
    }
}
