//! Read-only parity of a real source recipe with GNU Make. This is a header
//! producer test, not native SDK or runtime ABI qualification.

use aros_transpiler::{
    generate_cmake, parse_mmakefile_with_context, DependencyGraph, TargetContext,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn checked(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
#[ignore = "requires an explicit current LLVM SDK source with its native configuration"]
fn real_softfloat_header_matches_independent_make_for_both_endian_selections() {
    let source = PathBuf::from(std::env::var_os("AROS_TEST_SOURCE_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let make = "make";
    assert!(
        String::from_utf8_lossy(&checked(Command::new(make).arg("--version")).stdout)
            .contains("GNU Make")
    );
    let file = source.join("compiler/softfloat/mmakefile.src");
    let original = fs::read_to_string(&file).unwrap();
    let marker = "$(GENDIR)/$(CURDIR)/platform.h :";
    assert_eq!(original.matches(marker).count(), 1);
    let (prefix, rule) = original.split_once(marker).unwrap();
    let recipe = format!("{marker}{}", rule.split_once("\n\n").unwrap().0);
    // Only non-Make macro invocations and the unavailable configured include
    // are removed from the independent assignment/conditional prelude.
    let mut prelude = String::new();
    let mut skipping_macro = false;
    for line in prefix.lines() {
        if skipping_macro || line.trim_start().starts_with('%') {
            skipping_macro = line.trim_end().ends_with('\\');
            continue;
        }
        if line == "include $(SRCDIR)/config/aros.cfg" {
            continue;
        }
        prelude.push_str(line);
        prelude.push('\n');
    }
    assert_eq!(recipe.matches("%mkdir_q dir=").count(), 1);
    let reference_recipe = recipe.replace("%mkdir_q dir=", "@mkdir -p ");
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .unwrap();
    let source_contract: aros_common::sdk_consumer_contract::SdkConsumerContract =
        serde_json::from_slice(
            &fs::read(source.join("compiler/sdk/arm-raspi/consumer-v1.json")).unwrap(),
        )
        .unwrap();
    assert_eq!(
        source_contract
            .make_variables
            .get("ECHO")
            .map(String::as_str),
        Some("echo")
    );
    for (variant, endian) in [("", "1"), ("be", "0")] {
        let context = TargetContext {
            cpu: Some("arm".into()),
            platform: Some("raspi".into()),
            family: Some(String::new()),
            variant: Some(variant.into()),
            make_variables: source_contract.make_variables.clone(),
            make_include_bindings: BTreeMap::from([(
                "config/aros.cfg".into(),
                "compiler/sdk/config-common.mk".into(),
            )]),
            ..TargetContext::default()
        };
        let parsed = parse_mmakefile_with_context(&file, &source, &context).unwrap();
        let [declaration] = parsed.source_literal_text_rules.as_slice() else {
            panic!("{:#?}", parsed.native_graph_errors);
        };
        assert!(declaration
            .text
            .ends_with(&format!("#define LITTLEENDIAN {endian}\n")));
        let mut graph = DependencyGraph::default();
        graph.source_literal_text_rules = vec![declaration.clone()];
        let generated = generate_cmake(&graph);
        let start = "aros_write_source_literal_text(\n";
        assert_eq!(generated.matches(start).count(), 1);
        let (_, call) = generated.split_once(start).unwrap();
        let call = format!("{start}{})\n", call.split_once(")\n").unwrap().0);

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        let build = root.join("build");
        let reference = root.join("reference");
        for directory in [&project, &build, &reference] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(project.join("CMakeLists.txt"), format!(
            "cmake_minimum_required(VERSION 3.22)\nproject(reference NONE)\nset(AROS_SOURCE_DIR \"{}\")\nset(AROS_BUILD_DIR \"{}\")\ninclude(\"{}/SourceLiteralTextRules.cmake\")\n{call}",
            source.display(), build.display(), engine.display())).unwrap();
        checked(
            Command::new("cmake")
                .arg("-S")
                .arg(&project)
                .arg("-B")
                .arg(&build)
                .args(["-G", "Ninja"]),
        );
        checked(
            Command::new("cmake")
                .arg("--build")
                .arg(&build)
                .args(["--target", &declaration.owner]),
        );

        let makefile = reference.join("Makefile");
        fs::write(&makefile, format!(
            "SRCDIR := {}\nGENDIR := {}/gen\nCURDIR := compiler/softfloat\nAROS_TARGET_CPU := arm\nAROS_TARGET_VARIANT := {variant}\nECHO := echo\n{prelude}\n{reference_recipe}\n",
            source.display(), reference.display())).unwrap();
        let reference_output = reference.join("gen/compiler/softfloat/platform.h");
        checked(
            Command::new(make)
                .args(["--no-builtin-rules", "--no-builtin-variables", "-f"])
                .arg(&makefile)
                .arg(&reference_output),
        );
        assert_eq!(
            fs::read(build.join("gen/compiler/softfloat/platform.h")).unwrap(),
            fs::read(reference_output).unwrap()
        );
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
}
