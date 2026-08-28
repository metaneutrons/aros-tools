use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, parse_mmakefile_with_dirs_and_context,
    parse_mmakefile_with_dirs_and_context_and_fetches, ModuleType, TargetContext,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn formerly_colliding_programs_keep_distinct_ids_outputs_and_parent_edges() {
    let root = source_root();
    let dirs = DirVars::load(&root);

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
        ("riscv64", "opensbi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let expectations = [
            (
                "developer/demos/mmakefile.src",
                "demos",
                "demos",
                "childchild",
                ModuleType::ProgramGroup,
            ),
            (
                "developer/demos/2View/mmakefile.src",
                "demos-2view",
                "2View",
                "2View",
                ModuleType::Program,
            ),
            (
                "workbench/c/mmakefile.src",
                "workbench-c",
                "workbench-c",
                "AddBuffers",
                ModuleType::ProgramGroup,
            ),
            (
                "workbench/c/HDTool/mmakefile.src",
                "workbench-c-hdtool",
                "HDTool",
                "main",
                ModuleType::Program,
            ),
            (
                "workbench/c/CPUInfo/mmakefile.src",
                "workbench-c-cpuinfo",
                "CPUInfo",
                "main",
                ModuleType::Program,
            ),
            (
                "tools/zopfli/mmakefile.src",
                "workbench-c-zopfli",
                "zopfli",
                "zopfli_bin",
                ModuleType::Program,
            ),
        ];

        for (file, mmake, name, source, kind) in expectations {
            let parsed =
                parse_mmakefile_with_dirs_and_context(&root.join(file), &root, &dirs, &context)
                    .unwrap_or_else(|error| panic!("{file}: {error}"));
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap_or_else(|| panic!("{cpu}: {file} did not declare {mmake}"));
            assert_eq!(target.target_name, name, "{cpu}: {mmake}");
            assert_eq!(target.module_type, kind, "{cpu}: {mmake}");
            assert!(
                target.source_files.iter().any(|item| item == source),
                "{cpu}: {mmake} sources were {:#?}",
                target.source_files
            );
        }

        for (file, parent, child) in [
            (
                "developer/demos/2View/mmakefile.src",
                "demos",
                "demos-2view",
            ),
            (
                "workbench/c/HDTool/mmakefile.src",
                "workbench-c",
                "workbench-c-hdtool",
            ),
            (
                "tools/zopfli/mmakefile.src",
                "workbench-c",
                "workbench-c-zopfli",
            ),
        ] {
            let parsed =
                parse_mmakefile_with_dirs_and_context(&root.join(file), &root, &dirs, &context)
                    .unwrap_or_else(|error| panic!("{file}: {error}"));
            assert!(
                parsed.meta_rules.iter().any(|rule| {
                    rule.name == parent
                        && rule
                            .dependencies
                            .iter()
                            .any(|dependency| dependency == child)
                }),
                "{cpu}: {parent} does not retain {child}: {:#?}",
                parsed.meta_rules
            );
        }
    }
}

fn mmakefiles(root: &Path) -> Vec<PathBuf> {
    let mut files = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || entry.depth() == 0
                || !matches!(
                    entry.file_name().to_str(),
                    Some("build" | "target" | ".git")
                )
        })
        .filter_map(Result::ok)
        .filter(|entry| {
            matches!(
                entry.file_name().to_str(),
                Some("mmakefile.src" | "mmakefile")
            )
        })
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    files.sort();
    files
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
fn concrete_profiles_have_unique_compiled_mmake_ids() {
    let root = source_root();
    let files = mmakefiles(&root);
    let dirs = DirVars::load(&root);

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
        ("riscv64", "opensbi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = files
            .iter()
            .flat_map(|file| {
                collect_mmakefile_fetches_with_context(file, &root, &context)
                    .unwrap_or_else(|error| panic!("{}: {error}", file.display()))
            })
            .collect::<Vec<_>>();
        let mut declarations = BTreeMap::new();
        let mut duplicates = Vec::new();

        for file in &files {
            let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
                file, &root, &dirs, &context, &fetches,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
            for target in parsed.targets {
                let current = (target.target_name, target.dir_path);
                if let Some(previous) =
                    declarations.insert(target.mmake_name.clone(), current.clone())
                {
                    duplicates.push((target.mmake_name, previous, current));
                }
            }
        }

        assert!(
            duplicates.is_empty(),
            "{cpu}-{platform} has ambiguous compiled MMAKE_ID declarations: {duplicates:#?}"
        );
    }
}
