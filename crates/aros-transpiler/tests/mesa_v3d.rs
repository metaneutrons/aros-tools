use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars,
    parse_mmakefile_with_dirs_and_context_and_fetches, TargetContext,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
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
fn production_v3d_has_a_closed_generator_graph_on_every_current_profile() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let fetch_file = root.join("workbench/libs/mesa/mmakefile.src");
    let v3d_file = root.join("workbench/hidds/v3d/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(&fetch_file, &root, &context)
            .expect("collect the central Mesa fetch declarations");
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &v3d_file, &root, &dirs, &context, &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse V3D: {error:#}"));

        assert!(
            parsed.capability_errors.is_empty(),
            "{cpu}: {:#?}",
            parsed.capability_errors
        );
        assert!(
            parsed.partial_source_lists.is_empty(),
            "{cpu}: {:#?}",
            parsed.partial_source_lists
        );
        assert!(
            parsed
                .generated_file_rules
                .iter()
                .all(|rule| { !rule.contains("v3dx-gen") && !rule.contains("cle-gen") }),
            "{cpu}: {:#?}",
            parsed.generated_file_rules
        );

        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "linklibs-gallium_v3d")
            .unwrap_or_else(|| panic!("{cpu}: missing V3D archive"));
        assert_eq!(target.source_files.len(), 50, "{cpu}");
        assert_eq!(target.undefines, ["HAVE_VALGRIND"], "{cpu}");
        assert_eq!(
            target.linklib_output_dir.as_deref(),
            Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8"),
            "{cpu}"
        );

        let owners = parsed
            .python_outputs
            .iter()
            .map(|declaration| declaration.owner.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            owners,
            [
                "linklibs-gallium_v3d-gen-cle",
                "linklibs-gallium_v3d-gen-v3dx",
            ]
            .into_iter()
            .collect(),
            "{cpu}"
        );
        let wrappers = parsed
            .python_outputs
            .iter()
            .find(|declaration| declaration.owner.ends_with("gen-v3dx"))
            .unwrap();
        assert_eq!(wrappers.source_inputs.len(), 6, "{cpu}");
        assert_eq!(wrappers.jobs.len(), 12, "{cpu}");
        assert!(wrappers.jobs.iter().all(|job| {
            job.output.starts_with("v3dx-gen/v3d")
                && Path::new(&job.output)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("c"))
                && job.arguments.first().map(String::as_str) == Some("v3dx-wrapper")
        }));

        let cle = parsed
            .python_outputs
            .iter()
            .find(|declaration| declaration.owner.ends_with("gen-cle"))
            .unwrap();
        assert_eq!(
            cle.source_inputs,
            ["src/broadcom/cle/v3d_packet_v33.xml"],
            "{cpu}"
        );
        assert_eq!(cle.jobs.len(), 3, "{cpu}");
        assert!(cle.jobs.iter().all(|job| {
            job.output.starts_with("broadcom/cle/v3d_packet_v") && job.output.ends_with("_pack.h")
        }));
    }
}
