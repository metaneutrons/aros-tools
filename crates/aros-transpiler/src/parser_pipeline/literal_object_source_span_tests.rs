#[test]
fn all_proven_owners_get_diagnostics_and_unknown_proof_is_not_suppressed() {
    let path = std::path::Path::new("arch/example/mmakefile.src");
    let proofs = ["owner-a", "owner-b"]
        .into_iter()
        .map(|owner| crate::source_rule_ownership::SourceRuleOwnership {
            owner: owner.into(),
            chain: vec!["output.o".into(), owner.into()],
        })
        .collect();
    let diagnostics = super::source_rejection_diagnostics(
        path,
        Some(7),
        None,
        "unimplemented producer".into(),
        Some(proofs),
    );
    assert_eq!(diagnostics.len(), 2);
    for (diagnostic, owner) in diagnostics.iter().zip(["owner-a", "owner-b"]) {
        assert_eq!(
            diagnostic.context.as_ref().unwrap().target.as_deref(),
            Some(owner)
        );
        assert_eq!(diagnostic.location.as_ref().unwrap().line, Some(7));
        assert!(diagnostic.message.contains(&format!("output.o -> {owner}")));
    }
    for proofs in [None, Some(Vec::new())] {
        let diagnostics = super::source_rejection_diagnostics(
            path,
            None,
            None,
            "unimplemented producer".into(),
            proofs,
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].context.is_none());
    }
}

#[test]
fn rejected_source_meta_provider_keeps_only_known_selector_ownership() {
    let path = std::path::Path::new("compiler/include/mmakefile.src");
    for owner in ["plain-owner", "includes-asm_h-${AROS_TARGET_CPU}"] {
        let diagnostic =
            super::source_meta_provider_diagnostic(path, owner, "unimplemented".into());
        assert_eq!(diagnostic.context.unwrap().target.as_deref(), Some(owner));
    }
    for owner in [
        "",
        "${ARBITRARY}-owner",
        "$(shell command)",
        "../owner",
        "name;other",
    ] {
        let diagnostic =
            super::source_meta_provider_diagnostic(path, owner, "unimplemented".into());
        assert!(diagnostic.context.is_none(), "unsafe selector {owner}");
    }
}

#[test]
fn path_valued_rejected_rule_owner_uses_exact_source_chain_proof() {
    let tree = tempfile::tempdir().unwrap();
    let source = Path::new("arch/example/mmakefile.src");
    let snapshot = "build/objects/unit.o: src/unit.c\nbuild/tools/helper: build/objects/unit.o\n#MM canonical-owner : build/tools/helper\n";
    let (scope, line_states) = crate::make_vars::collect_vars_impl(
        snapshot,
        Some(&crate::parser::TargetContext::default()),
    );
    let dirs = crate::dirs::DirVars::load(tree.path());
    let proofs = super::rejected_rule_owner_proofs(
        "build/tools/helper",
        true,
        (snapshot, Some(&line_states)),
        &scope,
        &dirs,
        (tree.path(), Path::new("arch/example")),
        2,
    )
    .expect("path-valued Make target must be attributed through its exact #MM chain");

    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].owner, "canonical-owner");
    assert_eq!(
        proofs[0].chain,
        vec![
            "build/tools/helper".to_owned(),
            "canonical-owner".to_owned()
        ]
    );
    let diagnostics = super::source_rejection_diagnostics(
        source,
        Some(2),
        Some("build/tools/helper"),
        "rejected producer".into(),
        Some(proofs),
    );
    assert_eq!(
        diagnostics[0]
            .context
            .as_ref()
            .and_then(|context| context.target.as_deref()),
        Some("canonical-owner")
    );
    assert!(diagnostics[0]
        .message
        .contains("exact source consumer chain: build/tools/helper -> canonical-owner"));

    assert!(super::rejected_rule_owner_proofs(
        "canonical-owner",
        true,
        (snapshot, Some(&line_states)),
        &scope,
        &dirs,
        (tree.path(), Path::new("arch/example")),
        2,
    )
    .is_none());
    let canonical = super::source_rejection_diagnostics(
        source,
        Some(2),
        Some("canonical-owner"),
        "existing diagnostic".into(),
        None,
    );
    assert_eq!(
        canonical[0]
            .context
            .as_ref()
            .and_then(|context| context.target.as_deref()),
        Some("canonical-owner")
    );
}

#[test]
fn rejected_rule_owner_proof_keeps_incomplete_and_unknown_snapshots_unowned() {
    let tree = tempfile::tempdir().unwrap();
    let snapshot = "build/objects/unit.o: src/unit.c\nbuild/tools/helper: build/objects/unit.o\n#MM canonical-owner : build/tools/helper\nifeq ($(UNKNOWN),yes)\nconditional-consumer: build/tools/helper\nendif\n";
    let (scope, line_states) = crate::make_vars::collect_vars_impl(
        snapshot,
        Some(&crate::parser::TargetContext::default()),
    );
    let dirs = crate::dirs::DirVars::load(tree.path());
    for configuration_is_complete in [false, true] {
        assert!(super::rejected_rule_owner_proofs(
            "build/tools/helper",
            configuration_is_complete,
            (snapshot, Some(&line_states)),
            &scope,
            &dirs,
            (tree.path(), Path::new("arch/example")),
            2,
        )
        .is_none());
    }
}

#[test]
fn rejected_rule_owner_proof_cannot_ignore_optional_generated_dependency_includes() {
    let tree = tempfile::tempdir().unwrap();
    let snapshot = "build/objects/unit.o: src/unit.c\nbuild/tools/helper: build/objects/unit.o\n#MM canonical-owner : build/tools/helper\n-include build/objects/unit.d\n";
    let (scope, line_states) = crate::make_vars::collect_vars_impl(
        snapshot,
        Some(&crate::parser::TargetContext::default()),
    );
    let dirs = crate::dirs::DirVars::load(tree.path());
    assert!(
        super::rejected_rule_owner_proofs(
            "build/tools/helper",
            true,
            (snapshot, Some(&line_states)),
            &scope,
            &dirs,
            (tree.path(), Path::new("arch/example")),
            2,
        )
        .is_none(),
        "an optional .d include still has unmodeled future Make semantics"
    );
}

use super::{join_continuations, literal_object_source_anchors};
use crate::local_make_includes::{inline_native_make_configuration, LocalMakeIncludeLimits};
use std::collections::BTreeMap;
use std::path::Path;

#[test]
fn continuation_rejection_anchor_uses_first_physical_line() {
    let tree = tempfile::tempdir().unwrap();
    let source = Path::new("arch/example/mmakefile.src");
    let text = "before := value\noutput.o: first.o \\\n  second.o\nafter := value\n";
    let scan = inline_native_make_configuration(
        text,
        tree.path(),
        source,
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
    );
    let joined = join_continuations(&scan.expanded);
    let anchors = literal_object_source_anchors(tree.path(), source, text, &scan, &joined)
        .expect("unmodified source must have an exact line map");

    assert_eq!(anchors.len(), 3);
    assert_eq!(anchors[0].as_ref().unwrap().line, 1);
    assert_eq!(anchors[1].as_ref().unwrap().line, 2);
    assert_eq!(anchors[2].as_ref().unwrap().line, 4);
}

#[test]
fn generated_template_lines_keep_source_template_locations_and_reject_drift() {
    let tree = tempfile::tempdir().unwrap();
    let source = Path::new("compiler/include/mmakefile.src");
    let template = Path::new("compiler/include/geninc.cfg.in");
    std::fs::create_dir_all(tree.path().join("compiler/include")).unwrap();
    std::fs::write(
        tree.path().join(template),
        "%common\nEXECSMP=\"@ENABLE_EXECSMP@\"\n",
    )
    .unwrap();
    let templates = BTreeMap::from([(
        "compiler/include/geninc.cfg".into(),
        aros_common::native_make_template::ResolvedGeneratedMakeTemplate {
            template_relative: template.to_string_lossy().into_owned(),
            expanded_text: "%common\nEXECSMP=\"\"\n".into(),
            substitutions: BTreeMap::from([("@ENABLE_EXECSMP@".into(), String::new())]),
        },
    )]);
    let text = "include $(TOP)/$(CURDIR)/geninc.cfg\noutput.o: input.c\n";
    let scan = crate::local_make_includes::inline_native_make_configuration_with_templates(
        text,
        tree.path(),
        source,
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
        &templates,
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    let joined = join_continuations(&scan.expanded);
    let anchors = literal_object_source_anchors(tree.path(), source, text, &scan, &joined).unwrap();
    assert!(anchors[0].is_none());
    assert_eq!(anchors[1].as_ref().unwrap().source, template);
    assert_eq!(anchors[2].as_ref().unwrap().line, 2);
    assert_eq!(anchors[3].as_ref().unwrap().source, source);
    assert_eq!(anchors[3].as_ref().unwrap().line, 2);
    assert_eq!(
        super::rejection_source_location(source, Some(&anchors), 4),
        (source, Some(2)),
        "header/directory rejections must use physical source lines after expansion"
    );
    assert_eq!(
        super::rejection_source_location(source, Some(&anchors), 1),
        (source, None)
    );
    assert_eq!(
        super::rejection_source_location(source, Some(&anchors), 0),
        (source, None)
    );
    assert_eq!(
        super::rejection_source_location(source, None, 4),
        (source, None)
    );
    std::fs::write(tree.path().join(template), "%common\nEXECSMP=\"changed\"\n").unwrap();
    assert!(literal_object_source_anchors(tree.path(), source, text, &scan, &joined).is_none());
}

#[test]
fn included_configuration_lines_keep_fragment_and_parent_origins() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("arch/example")).unwrap();
    let source = Path::new("arch/example/mmakefile.src");
    let fragment = Path::new("arch/example/native.mk");
    let text = "before := value\ninclude $(SRCDIR)/config/aros.cfg\nafter := value\n";
    std::fs::create_dir_all(tree.path().join("config")).unwrap();
    std::fs::write(tree.path().join("config/aros.cfg"), "ORIGINAL := config\n").unwrap();
    std::fs::write(
        tree.path().join(fragment),
        "# fragment comment\nFRAGMENT_FLAGS := one \\\n  two\n",
    )
    .unwrap();
    let bindings = BTreeMap::from([(
        "config/aros.cfg".to_owned(),
        fragment.to_string_lossy().into_owned(),
    )]);
    let scan = inline_native_make_configuration(
        text,
        tree.path(),
        source,
        LocalMakeIncludeLimits::default(),
        &bindings,
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    let joined = join_continuations(&scan.expanded);
    let anchors = literal_object_source_anchors(tree.path(), source, text, &scan, &joined)
        .expect("included source must reconstruct byte-for-byte");

    assert_eq!(anchors[0].as_ref().unwrap().source, source);
    assert_eq!(anchors[0].as_ref().unwrap().line, 1);
    assert!(
        anchors[1].is_none(),
        "include placeholder has no source line"
    );
    assert_eq!(anchors[2].as_ref().unwrap().source, fragment);
    assert_eq!(anchors[2].as_ref().unwrap().line, 1);
    assert_eq!(anchors[3].as_ref().unwrap().source, fragment);
    assert_eq!(anchors[3].as_ref().unwrap().line, 2);
    assert_eq!(anchors[4].as_ref().unwrap().source, source);
    assert_eq!(anchors[4].as_ref().unwrap().line, 3);

    std::fs::write(tree.path().join(fragment), "FRAGMENT_FLAGS := changed\n").unwrap();
    assert!(
        literal_object_source_anchors(tree.path(), source, text, &scan, &joined,).is_none(),
        "changed include bytes must suppress physical locations"
    );
}

#[test]
#[ignore = "requires AROS_P4_SOURCE_ROOT to point at the P4 source checkout"]
fn actual_p4_sifive_rule_maps_to_its_physical_source_line() {
    let root = std::path::PathBuf::from(
        std::env::var_os("AROS_P4_SOURCE_ROOT").expect("set P4 source root"),
    );
    let source = Path::new("arch/riscv-native/sifive_u/boot/mmakefile.src");
    let text = std::fs::read_to_string(root.join(source)).unwrap();
    let bindings = BTreeMap::from([(
        "config/aros.cfg".to_owned(),
        "arch/riscv-esp32p4/native-kobj-config.mk".to_owned(),
    )]);
    let scan = inline_native_make_configuration(
        &text,
        &root,
        source,
        LocalMakeIncludeLimits::default(),
        &bindings,
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    let joined = join_continuations(&scan.expanded);
    let anchors = literal_object_source_anchors(&root, source, &text, &scan, &joined)
        .expect("actual P4 source must reconstruct exactly");
    let matching = joined
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("$(TARGETDIR)/core.bin.o:"))
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    let anchor = anchors[matching[0].0].as_ref().unwrap();
    assert_eq!(anchor.source, source);
    assert_eq!(anchor.line, 61);
}
