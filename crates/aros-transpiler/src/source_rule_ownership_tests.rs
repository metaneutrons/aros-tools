use super::*;
use crate::make_vars::collect_vars_impl;
use std::fmt::Write as _;
use tempfile::tempdir;

fn fixture(source: &str, output: &str, verified: VerifiedMacros) -> Option<SourceRuleOwnership> {
    let root = tempdir().unwrap();
    let source = with_clean_macro_configuration(source);
    let joined = crate::parser::join_continuations(&source);
    let (scope, states) =
        collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(root.path());
    let line = joined.lines().position(|candidate| {
        candidate
            .split_once(':')
            .is_some_and(|(target, _)| target.trim() == output)
    })? + 1;
    attribute_with_verified_macros(
        &joined,
        &scope,
        &dirs,
        (root.path(), Path::new("fixture")),
        Some(&states),
        (output, line),
        verified,
    )
}

fn owners_fixture(source: &str, rejected_rule_target: &str) -> Option<Vec<SourceRuleOwnership>> {
    owners_fixture_with_verified(source, rejected_rule_target, VerifiedMacros::default())
}

fn owners_fixture_with_verified(
    source: &str,
    rejected_rule_target: &str,
    verified: VerifiedMacros,
) -> Option<Vec<SourceRuleOwnership>> {
    let (graph, rejected_line, outputs) =
        source_graph_fixture(source, rejected_rule_target, verified)?;
    attribute_graph_outputs(&graph, rejected_line, &outputs)
}

fn source_graph_fixture(
    source: &str,
    rejected_rule_target: &str,
    verified: VerifiedMacros,
) -> Option<(SourceGraph, usize, Vec<String>)> {
    let root = tempdir().unwrap();
    let source = with_clean_macro_configuration(source);
    let joined = crate::parser::join_continuations(&source);
    let (scope, states) =
        collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(root.path());
    let lines = joined.lines().collect::<Vec<_>>();
    let line = joined.lines().position(|candidate| {
        candidate
            .split_once(':')
            .is_some_and(|(target, _)| target.trim() == rejected_rule_target)
    })? + 1;
    let rejected_line = line.checked_sub(1)?;
    let mut graph = parse_source_graph(
        &lines,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        &states,
    );
    add_verified_macro_edges(
        &mut graph,
        &lines,
        &scope,
        &dirs,
        (root.path(), Path::new("fixture")),
        &states,
        verified,
    );
    instantiate_pattern_edges(&mut graph);
    if graph.overflow || graph.uncertain {
        return None;
    }
    let outputs = graph.targets_by_line.get(&rejected_line)?.clone();
    Some((graph, rejected_line, outputs))
}

fn with_clean_macro_configuration(source: &str) -> String {
    format!(
        "CPPFLAGS := -DTEST_CPPFLAGS\nCFLAGS := -DTEST_CFLAGS\nAFLAGS := -DTEST_AFLAGS\nCC := test-cc\nTARGET_SYSROOT := --sysroot=test\nTARGET_COVERAGEINSTR :=\nTARGET_FUNCINSTR :=\nTARGET_LTO :=\n{source}"
    )
}

#[test]
fn follows_exact_ordinary_prerequisite_and_meta_owner_chain() {
    let ownership = fixture(
        "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM library-owner : archive.a\n",
        "obj/unit.o",
        VerifiedMacros::default(),
    )
    .unwrap();
    assert_eq!(ownership.owner, "library-owner");
    assert_eq!(
        ownership.chain,
        ["obj/unit.o", "archive.a", "library-owner"]
    );
}

#[test]
fn parser_pipeline_entrypoint_uses_the_rejected_rule_line() {
    let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM library-owner : archive.a\n";
    let root = tempdir().unwrap();
    let joined = crate::parser::join_continuations(source);
    let (scope, states) = collect_vars_impl(&joined, None);
    let dirs = DirVars::load(root.path());
    let rejected_line = 1;
    let result = attribute_rejected_rule_line(
        &joined,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        Some(&states),
        rejected_line,
    )
    .unwrap();
    assert_eq!(result.owner, "library-owner");
    assert!(attribute_rejected_rule_line(
        &joined,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        Some(&states),
        3,
    )
    .is_none());
}

#[test]
fn build_prog_explicit_inputs_do_not_prove_implicit_consumer_closure() {
    let source = "obj/unit.o: src/unit.c\nobj/extra.o: src/extra.c\n%build_prog mmake=build-owner progname=app files=src/unit objs=obj/extra.o objdir=obj\n";
    for output in ["obj/unit.o", "obj/extra.o"] {
        assert!(fixture(
            source,
            output,
            VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
        )
        .is_none());
    }
}

#[test]
fn build_prog_user_objects_and_namespace_overrides_cannot_hide_consumers() {
    for ambient in [
        "USER_OBJS := obj/unit.o\n",
        "second_OBJS := obj/unit.o\n",
        "second_ARCHOBJS := obj/unit.o\n",
    ] {
        let source = format!(
            "obj/unit.o: src/unit.c\n#MM first : obj/unit.o\n{ambient}%build_prog mmake=second progname=other files=other objdir=obj\n"
        );
        assert!(
            fixture(
                &source,
                "obj/unit.o",
                VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
            )
            .is_none(),
            "{ambient}"
        );
    }
    let inactive = "obj/unit.o: src/unit.c\n#MM first : obj/unit.o\nifeq (0,1)\n%build_prog mmake=second progname=other files=other objdir=obj\nendif\n";
    assert_eq!(
        fixture(
            inactive,
            "obj/unit.o",
            VerifiedMacros::from_forms(&[MacroForm::BuildProg])
        )
        .unwrap()
        .owner,
        "first"
    );
}

#[test]
fn direct_meta_owner_on_rejected_target_is_exact_proof() {
    let source = "direct.o: source.c\n#MM direct.o : source.c\n";
    let proof = fixture(source, "direct.o", VerifiedMacros::default()).unwrap();
    assert_eq!(proof.owner, "direct.o");
    assert_eq!(proof.chain, ["direct.o"]);
}

#[test]
fn define_bodies_are_opaque_to_rules_meta_owners_and_macros() {
    let source = with_clean_macro_configuration("override define hidden_rules\nphantom.o: rejected.o\n#MM phantom-owner : rejected.o\n%build_prog mmake=macro-owner progname=phantom files=phantom objdir=obj\nexport define nested_hidden\nnested.o: rejected.o\n#MM nested-owner : rejected.o\n%rule_compile_multi mmake=nested-output basenames=nested targetdir=obj\nendef\nendef\nrejected.o: source.c\n#MM real-owner : rejected.o\n");
    let root = tempdir().unwrap();
    let joined = crate::parser::join_continuations(&source);
    let (scope, states) =
        collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
    let lines = joined.lines().collect::<Vec<_>>();
    let dirs = DirVars::load(root.path());
    let mut graph = parse_source_graph(
        &lines,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        &states,
    );
    add_verified_macro_edges(
        &mut graph,
        &lines,
        &scope,
        &dirs,
        (root.path(), Path::new("fixture")),
        &states,
        VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
    );
    assert!(!graph.owners.contains("phantom-owner"));
    assert!(!graph.owners.contains("nested-owner"));
    assert!(!graph.owners.contains("macro-owner"));
    assert!(!graph.macro_owners.contains_key("obj/phantom.o"));
    assert!(!graph.uncertain);
    assert_eq!(
        fixture(
            &source,
            "rejected.o",
            VerifiedMacros::from_forms(&[MacroForm::BuildProg])
        )
        .unwrap()
        .owner,
        "real-owner"
    );
}

#[test]
fn active_eval_of_an_opaque_define_vetoes_partial_owner_proof() {
    let source = "define HIDDEN\n#MM competing-owner : rejected.o\nendef\nrejected.o: source.c\n#MM real-owner : rejected.o\n$(eval $(HIDDEN))\n";
    assert!(fixture(source, "rejected.o", VerifiedMacros::default()).is_none());

    let braced = "define HIDDEN\n#MM competing-owner : rejected.o\nendef\nrejected.o: source.c\n#MM real-owner : rejected.o\n${eval ${HIDDEN}}\n";
    assert!(fixture(braced, "rejected.o", VerifiedMacros::default()).is_none());
}

#[test]
fn assignment_expansion_must_be_proven_side_effect_free() {
    let hidden_variable = "define HIDDEN\n$(eval #MM competing-owner : rejected.o)\nendef\nDYNAMIC := $(HIDDEN)\nrejected.o: source.c\n#MM real-owner : rejected.o\n";
    assert!(fixture(hidden_variable, "rejected.o", VerifiedMacros::default()).is_none());

    let hidden_call = "define HIDDEN\n$(eval #MM competing-owner : rejected.o)\nendef\nDYNAMIC := $(call HIDDEN)\nrejected.o: source.c\n#MM real-owner : rejected.o\n";
    assert!(fixture(hidden_call, "rejected.o", VerifiedMacros::default()).is_none());
}

#[test]
fn known_false_branch_and_inert_define_do_not_veto_owner_proof() {
    let source = "ifeq (0,1)\ndefine HIDDEN\n#MM inactive-owner : rejected.o\nendef\nDYNAMIC := $(HIDDEN)\n$(eval $(HIDDEN))\nendif\nrejected.o: source.c\n#MM real-owner : rejected.o\n";
    assert_eq!(
        fixture(source, "rejected.o", VerifiedMacros::default())
            .unwrap()
            .owner,
        "real-owner"
    );
}

#[test]
fn ordinary_endpoint_budget_is_global_across_rules() {
    let target_names = (0..32_768)
        .map(|index| format!("output-{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!("TARGETS := {target_names}\n$(TARGETS): input.o\n$(TARGETS): input.o\n");
    let root = tempdir().unwrap();
    let joined = crate::parser::join_continuations(&source);
    let (scope, states) =
        collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(root.path());
    let lines = joined.lines().collect::<Vec<_>>();
    let graph = parse_source_graph(
        &lines,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        &states,
    );
    assert!(graph.overflow);
    assert!(graph.identity_references <= MAX_IDENTITIES);
    assert!(graph.identities.len() <= MAX_IDENTITIES);
    assert_eq!(graph.targets_by_line.len(), 1);
}

#[test]
fn repeated_finite_macro_seeds_share_one_global_identity_budget() {
    let basenames = (0..32_768)
        .map(|index| format!("unit-{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!(
        "BASENAMES := {basenames}\n%rule_compile_multi basenames=\"$(BASENAMES)\" targetdir=first\n%rule_compile_multi basenames=\"$(BASENAMES)\" targetdir=second\n"
    );
    let root = tempdir().unwrap();
    let source = with_clean_macro_configuration(&source);
    let joined = crate::parser::join_continuations(&source);
    let (scope, states) =
        collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(root.path());
    let lines = joined.lines().collect::<Vec<_>>();
    let mut graph = parse_source_graph(
        &lines,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        &states,
    );
    add_verified_macro_edges(
        &mut graph,
        &lines,
        &scope,
        &dirs,
        (root.path(), Path::new("fixture")),
        &states,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    );
    assert!(graph.overflow);
    assert_eq!(graph.identity_references, MAX_IDENTITIES);
    assert_eq!(graph.macro_outputs.len(), MAX_IDENTITIES);
    assert!(graph.identities.len() <= MAX_IDENTITIES);
}

#[test]
fn compile_assemble_and_link_binary_macro_seeds_are_bounded() {
    let source = "compiled.o: src/compiled.c\nassembled.o: src/assembled.s\nobj/link-input.o: src/link-input.c\ncompiled-archive.a: compiled.o\nassembled-archive.a: assembled.o\n%rule_compile_multi basenames=compiled\n%rule_assemble_multi mmake=assemble-namespace basenames=assembled\n%rule_link_binary file=bin/image.o name=image objs=obj/link-input.o\n#MM compile-owner : compiled-archive.a\n#MM assemble-owner : assembled-archive.a\n#MM link-owner : bin/image.o\n";
    let all_macros = VerifiedMacros::from_forms(&[
        MacroForm::CompileMulti,
        MacroForm::AssembleMulti,
        MacroForm::LinkBinary,
    ]);
    let proofs = [
        ("compiled.o", "compile-owner"),
        ("assembled.o", "assemble-owner"),
        ("obj/link-input.o", "link-owner"),
    ];
    for (output, expected) in proofs {
        assert_eq!(fixture(source, output, all_macros).unwrap().owner, expected);
    }
}

#[test]
fn paired_compile_sidecars_require_exact_full_invocation_and_closed_consumers() {
    let source = "#MM compile-owner : unit-a.o unit-b.o\n#MM depfile-owner : unit-a.d\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
    let rejected = "unit-a.o unit-b.o unit-a.d unit-b.d";
    let proofs = owners_fixture_with_verified(
        source,
        rejected,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .unwrap();
    assert_eq!(
        proofs
            .iter()
            .map(|proof| proof.owner.as_str())
            .collect::<Vec<_>>(),
        ["compile-owner", "depfile-owner"]
    );
    assert_eq!(proofs[0].chain, ["unit-a.o", "compile-owner"]);
    // The unconsumed unit-b.d is attributed through its paired object's
    // real chain; no synthetic `unit-b.d -> unit-b.o` edge is reported.
    assert!(!proofs
        .iter()
        .any(|proof| proof.chain.first().is_some_and(|id| id == "unit-b.d")));
    assert!(owners_fixture_with_verified(source, rejected, VerifiedMacros::default()).is_none());

    let partial = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-a.d : | obj\n";
    assert!(owners_fixture_with_verified(
        partial,
        "unit-a.o unit-a.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let extra = "#MM compile-owner : unit-a.o unit-b.o extra\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d extra : | obj\n";
    assert!(owners_fixture_with_verified(
        extra,
        "unit-a.o unit-b.o unit-a.d unit-b.d extra",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let ambiguous = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
    assert!(owners_fixture_with_verified(
        ambiguous,
        rejected,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let extra_recipe = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n\t@printf alternate-producer\n";
    assert!(owners_fixture_with_verified(
        extra_recipe,
        rejected,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let competing_pattern = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n%.d : %.source\n";
    assert!(owners_fixture_with_verified(
        competing_pattern,
        rejected,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let partial_objects = "#MM compile-owner : unit-a.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
    assert!(owners_fixture_with_verified(
        partial_objects,
        rejected,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());
}

#[test]
fn paired_compile_sidecars_reject_unknown_or_cyclic_depfile_consumers() {
    let unknown = "#MM compile-owner : unit-a.o unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\nifeq ($(UNKNOWN),yes)\n#MM conditional-depfile-owner : unit-a.d\nendif\n";
    assert!(owners_fixture_with_verified(
        unknown,
        "unit-a.o unit-b.o unit-a.d unit-b.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let cycle = "#MM compile-owner : unit-a.o unit-b.o\n#MM depfile-owner : unit-a.d\n#MM unit-a.d : depfile-owner\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
    let (graph, rejected_line, outputs) = source_graph_fixture(
        cycle,
        "unit-a.o unit-b.o unit-a.d unit-b.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .unwrap();
    assert!(trace_all_owners(&graph, "unit-a.d", &mut DiagnosticBudget::default()).is_none());
    assert!(attribute_graph_outputs(&graph, rejected_line, &outputs).is_none());
}

#[test]
fn predecessor_trace_preserves_longest_supported_chain_and_sorted_ties() {
    let mut source = String::from("unit.o: source.c\n");
    for index in 0..256 {
        let target = format!("chain-{index:03}");
        let prerequisite = if index == 0 {
            "unit.o".to_owned()
        } else {
            format!("chain-{:03}", index - 1)
        };
        writeln!(source, "{target}: {prerequisite}").unwrap();
    }
    source.push_str("#MM long-owner : chain-255\n");
    let proof = fixture(&source, "unit.o", VerifiedMacros::default()).unwrap();
    assert_eq!(proof.owner, "long-owner");
    assert_eq!(proof.chain.len(), 258);
    assert_eq!(proof.chain.first().map(String::as_str), Some("unit.o"));
    assert_eq!(proof.chain.get(1).map(String::as_str), Some("chain-000"));
    assert_eq!(proof.chain.last().map(String::as_str), Some("long-owner"));

    let tied = "unit.o: source.c\nleft: unit.o\nright: unit.o\n#MM tie-owner : left\n#MM tie-owner : right\n";
    let proof = fixture(tied, "unit.o", VerifiedMacros::default()).unwrap();
    assert_eq!(proof.chain, ["unit.o", "left", "tie-owner"]);
}

#[test]
fn predecessor_trace_bounds_many_roots_over_a_long_chain() {
    let output_names = (0..1_000)
        .map(|index| format!("output-{index:04}"))
        .collect::<Vec<_>>();
    let output_list = output_names.join(" ");
    let mut source = format!("{output_list}: source.c\nhub: {output_list}\n");
    for index in 0..16_000 {
        let target = format!("chain-{index:05}");
        let prerequisite = if index == 0 {
            "hub".to_owned()
        } else {
            format!("chain-{:05}", index - 1)
        };
        writeln!(source, "{target}: {prerequisite}").unwrap();
    }
    source.push_str("#MM stress-owner : chain-15999\n");

    let (graph, rejected_line, outputs) =
        source_graph_fixture(&source, &output_list, VerifiedMacros::default()).unwrap();
    assert_eq!(outputs.len(), 1_000);
    assert!(attribute_graph_outputs(&graph, rejected_line, &outputs).is_none());
}

#[test]
fn returned_path_byte_limit_is_checked_before_materialization() {
    let (graph, _, _) = source_graph_fixture(
        "unit.o: source.c\narchive.a: unit.o\n#MM byte-owner : archive.a\n",
        "unit.o",
        VerifiedMacros::default(),
    )
    .unwrap();
    let mut budget = DiagnosticBudget {
        retained_path_bytes: MAX_DIAGNOSTIC_PATH_BYTES - 1,
        ..DiagnosticBudget::default()
    };
    assert!(trace_all_owners(&graph, "unit.o", &mut budget).is_none());
    assert_eq!(budget.retained_path_bytes, MAX_DIAGNOSTIC_PATH_BYTES - 1);
}

#[test]
fn paired_fallback_does_not_reset_budget_after_ordinary_trace() {
    let source = "#MM compile-owner : unit-a.o unit-b.o\n#MM depfile-owner : unit-a.d\n%rule_compile_multi basenames=\"unit-a unit-b\"\nunit-a.o unit-b.o unit-a.d unit-b.d : | obj\n";
    let (graph, rejected_line, outputs) = source_graph_fixture(
        source,
        "unit-a.o unit-b.o unit-a.d unit-b.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .unwrap();

    assert!(trace_all_then_paired(
        &graph,
        &outputs,
        rejected_line,
        &mut DiagnosticBudget::default(),
    )
    .is_some());

    let mut nearly_exhausted = DiagnosticBudget {
        work: MAX_DIAGNOSTIC_WORK - 1,
        ..DiagnosticBudget::default()
    };
    assert!(
        trace_all_then_paired(&graph, &outputs, rejected_line, &mut nearly_exhausted,).is_none()
    );
    assert_eq!(nearly_exhausted.work, MAX_DIAGNOSTIC_WORK);
}

#[test]
fn paired_compile_sidecar_fallback_respects_macro_identity_budget() {
    let basenames = (0..=MAX_IDENTITIES / 2)
        .map(|index| format!("unit-{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!(
        "BASENAMES := {basenames}\n#MM compile-owner : probe.o\n%rule_compile_multi basenames=\"$(BASENAMES)\" targetdir=gen\nprobe.o probe.d : | gen\n"
    );
    assert!(owners_fixture_with_verified(
        &source,
        "probe.o probe.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let long_targetdir = "x".repeat(4_000);
    let many_basenames = (0..300)
        .map(|index| format!("u{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let args = closed_macro_arguments(&format!(
        "basenames=\"{many_basenames}\" targetdir={long_targetdir}"
    ))
    .unwrap();
    let root = tempdir().unwrap();
    let scope = collect_vars_impl("", Some(&crate::parser::TargetContext::default())).0;
    let dirs = DirVars::load(root.path());
    assert!(
        multi_compile_pairs(&args, &scope, &dirs, root.path(), Path::new("fixture"), 0).is_empty()
    );

    let basenames = (0..300)
        .map(|index| format!("unit-{index}"))
        .collect::<Vec<_>>();
    let objects = basenames
        .iter()
        .map(|basename| format!("{basename}.o"))
        .collect::<Vec<_>>();
    let rejected = basenames
        .iter()
        .flat_map(|basename| [format!("{basename}.o"), format!("{basename}.d")])
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!(
        "#MM compile-owner : all-objects\nall-objects: {}\n%rule_compile_multi basenames=\"{}\"\n{} : | obj\n",
        objects.join(" "),
        basenames.join(" "),
        rejected,
    );
    let proofs = owners_fixture_with_verified(
        &source,
        &rejected,
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .expect("a finite shared predecessor graph stays below the work/path limits");
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].owner, "compile-owner");
    assert_eq!(
        proofs[0].chain,
        ["unit-0.o", "all-objects", "compile-owner"]
    );
}

#[test]
fn link_binary_does_not_bypass_unsealed_program_context() {
    // CURDIR is the independently selected declaring directory, not a
    // source-local override. The fixture always declares from `fixture`.
    let source = "GENDIR := generated\nCURDIR := arch/boot\n%build_prog mmake=bootstrap-owner progname=bootstrap files=bootstrap\n%rule_link_binary mmake=bootstrap-owner file=obj/vesa.bin.o name=vesa files=vesa asmfiles=loader\ngenerated/fixture/vesa.o: src/vesa.c\ngenerated/fixture/loader.o: src/loader.S\ngenerated/arch/boot/vesa.o: src/vesa.c\n#MM binary-owner : obj/vesa.bin.o\n";
    let macros = VerifiedMacros::from_forms(&[MacroForm::BuildProg, MacroForm::LinkBinary]);
    for output in ["generated/fixture/vesa.o", "generated/fixture/loader.o"] {
        assert!(fixture(source, output, macros).is_none());
    }
    assert!(fixture(source, "generated/arch/boot/vesa.o", macros).is_none());

    let missing_program = "%rule_link_binary mmake=bootstrap-owner file=obj/vesa.bin.o name=vesa files=vesa\nobj/vesa.o: src/vesa.c\n#MM binary-owner : obj/vesa.bin.o\n";
    assert!(fixture(
        missing_program,
        "obj/vesa.o",
        VerifiedMacros::from_forms(&[MacroForm::LinkBinary])
    )
    .is_none());
}

#[test]
fn duplicate_macro_arguments_cannot_prove_paired_sidecar_ownership() {
    for arguments in [
        "basenames=\"unit-a\" basenames=\"unit-b\"",
        "basenames=\"unit-a\" targetdir= targetdir=obj",
        "basenames=\"unit-a\" usetree=no usetree=yes",
        "basenames=\"unit-a\" srcdir=one srcdir=two",
        "basenames=\"unit-a\" mmake=TMP mmake=other",
    ] {
        let source = format!(
            "#MM compile-owner : unit-a.o\n%rule_compile_multi {arguments}\nunit-a.o unit-a.d : | obj\n"
        );
        assert!(
            owners_fixture_with_verified(
                &source,
                "unit-a.o unit-a.d",
                VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
            )
            .is_none(),
            "{arguments}"
        );
    }
}

#[test]
fn closed_macro_arguments_ignore_quoted_and_nested_fake_keywords() {
    let arguments = closed_macro_arguments(
        r#"cflags="$(if 1, 'basenames=ghost targetdir=evil mmake=fake',)" basenames="$(addsuffix .c,$(FILES))" targetdir="$(if $(USE),obj,targetdir=other)" mmake="$(if 1,real-owner,mmake=fake)""#,
    )
    .unwrap();
    assert_eq!(
        arguments.keys().map(String::as_str).collect::<Vec<_>>(),
        ["basenames", "cflags", "mmake", "targetdir"]
    );
    assert_eq!(
        macro_value(&arguments, "mmake"),
        Some("$(if 1,real-owner,mmake=fake)")
    );
    assert_eq!(
        macro_value(&arguments, "targetdir"),
        Some("$(if $(USE),obj,targetdir=other)")
    );
}

#[test]
fn hash_bound_macro_contracts_reject_unknown_or_missing_required_arguments() {
    assert_eq!(
        macro_value(
            &closed_macro_arguments("basenames='unit-a'").unwrap(),
            "basenames"
        ),
        Some("'unit-a'")
    );
    assert!(closed_macro_arguments("basenames=unit-a unit-b").is_none());
    // Python's GenMF lexer recognizes Unicode whitespace. Do not let
    // the narrower byte lexer consume a second token as part of a value.
    for separator in ['\u{00a0}', '\u{2003}', '\u{2028}'] {
        assert!(closed_macro_arguments(&format!(
            "basenames=unit-a{separator}unit-b targetdir=obj"
        ))
        .is_none());
    }

    let build_prog = test_macro_contract(MacroForm::BuildProg as usize).unwrap();
    assert!(build_prog["mmake"].required);
    assert!(build_prog["progname"].required);
    let compile_multi = test_macro_contract(MacroForm::CompileMulti as usize).unwrap();
    assert!(compile_multi["basenames"].required);
    let link_binary = test_macro_contract(MacroForm::LinkBinary as usize).unwrap();
    assert!(link_binary["file"].required);
    assert!(link_binary["name"].required);

    let compile = VerifiedMacros::from_forms(&[MacroForm::CompileMulti]);
    for arguments in [
        "basenames=unit-a targetdir=obj unexpected=ignored",
        "basenames=unit-a # ignored-comment",
        "targetdir=obj",
        "basenames=unit-a unit-b targetdir=obj",
        "basenames='unit-a' targetdir=obj",
    ] {
        let source = format!(
            "#MM compile-owner : obj/unit-a.o\n%rule_compile_multi {arguments}\nobj/unit-a.o obj/unit-a.d : | obj\n"
        );
        assert!(
            owners_fixture_with_verified(&source, "obj/unit-a.o obj/unit-a.d", compile).is_none(),
            "{arguments}"
        );
    }

    // progname is required by GenMF even if `files` would otherwise
    // provide an output stem to this bounded projection.
    let missing_progname = "obj/unit.o: src/unit.c\n#MM build-owner : obj/unit.o\n%build_prog mmake=build-owner files=src/unit objdir=obj\n";
    assert!(fixture(
        missing_progname,
        "obj/unit.o",
        VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
    )
    .is_none());
}

fn synthetic_macro_definitions(template: &str) -> NativeMacroDefinitions {
    let (definitions, includes) = parse_native_macro_file(template).unwrap();
    assert!(includes.is_empty());
    let mut result = NativeMacroDefinitions::new();
    for (name, definition) in definitions {
        result.entry(name).or_default().push(definition);
    }
    result
}

fn synthetic_macro_hashes(definitions: &NativeMacroDefinitions) -> BTreeMap<String, String> {
    definitions
        .iter()
        .filter(|(_, matches)| matches.len() == 1)
        .map(|(name, matches)| (name.clone(), matches[0].sha256.clone()))
        .collect()
}

#[test]
fn macro_closure_detects_direct_and_transitive_helper_mutations() {
    let original = "%define root value=\n%helper value=yes\n%end\n%define helper value=\nhelper-output: item\n%end\n%define leaf value=\nleaf-output: item\n%end\n";
    let trusted = synthetic_macro_definitions(original);
    let hashes = synthetic_macro_hashes(&trusted);
    assert!(verified_macro_closure_with_hashes(
        "root",
        &trusted,
        |name| { hashes.get(name).cloned() }
    ));

    let direct_mutation = original.replace("helper-output: item", "changed-output: item");
    let direct_definitions = synthetic_macro_definitions(&direct_mutation);
    assert_eq!(
        trusted["root"][0].sha256, direct_definitions["root"][0].sha256,
        "outer definition remains byte-identical"
    );
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &direct_definitions,
        |name| hashes.get(name).cloned()
    ));

    let transitive = "%define root value=\n%helper value=yes\n%end\n%define helper value=\n%leaf value=yes\n%end\n%define leaf value=\nleaf-output: item\n%end\n";
    let transitive_trusted = synthetic_macro_definitions(transitive);
    let transitive_hashes = synthetic_macro_hashes(&transitive_trusted);
    assert!(verified_macro_closure_with_hashes(
        "root",
        &transitive_trusted,
        |name| transitive_hashes.get(name).cloned()
    ));
    let leaf_mutation = transitive.replace("leaf-output: item", "changed-leaf: item");
    let leaf_definitions = synthetic_macro_definitions(&leaf_mutation);
    assert_eq!(
        transitive_trusted["root"][0].sha256, leaf_definitions["root"][0].sha256,
        "transitive mutation leaves the outer definition unchanged"
    );
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &leaf_definitions,
        |name| transitive_hashes.get(name).cloned()
    ));
}

#[test]
fn macro_closure_fails_closed_on_missing_duplicate_cycle_and_budget() {
    let missing = synthetic_macro_definitions("%define root value=\n%compile_q\n%end\n");
    let missing_hashes = synthetic_macro_hashes(&missing);
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &missing,
        |name| missing_hashes.get(name).cloned()
    ));

    let duplicate = synthetic_macro_definitions(
        "%define root value=\n%helper\n%end\n%define helper value=\nfirst: input\n%end\n%define helper value=\nsecond: input\n%end\n",
    );
    let duplicate_hashes = synthetic_macro_hashes(&duplicate);
    let tab_duplicate = synthetic_macro_definitions(
        "%define root value=\n%helper\n%end\n%define helper value=\nfirst: input\n%end\n%define\thelper value=\nsecond: input\n%end\n",
    );
    assert_eq!(tab_duplicate["helper"].len(), 2);
    let tab_duplicate_hashes = synthetic_macro_hashes(&tab_duplicate);
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &tab_duplicate,
        |name| tab_duplicate_hashes.get(name).cloned()
    ));
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &duplicate,
        |name| duplicate_hashes.get(name).cloned()
    ));

    let cycle = synthetic_macro_definitions(
        "%define root value=\n%helper\n%end\n%define helper value=\n%root\n%end\n",
    );
    let cycle_hashes = synthetic_macro_hashes(&cycle);
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &cycle,
        |name| { cycle_hashes.get(name).cloned() }
    ));

    let mut deep = String::new();
    for index in 0..=MAX_TEMPLATE_CLOSURE_DEPTH + 1 {
        let next = if index == MAX_TEMPLATE_CLOSURE_DEPTH + 1 {
            String::new()
        } else {
            format!("%node{}\n", index + 1)
        };
        writeln!(deep, "%define node{index} value=\n{next}%end").unwrap();
    }
    let deep_definitions = synthetic_macro_definitions(&deep);
    let deep_hashes = synthetic_macro_hashes(&deep_definitions);
    assert!(!verified_macro_closure_with_hashes(
        "node0",
        &deep_definitions,
        |name| deep_hashes.get(name).cloned()
    ));

    let repeated_callers =
        std::iter::repeat_n("%helper\n", MAX_TEMPLATE_CLOSURE_WORK).collect::<String>();
    let over_budget = format!(
        "%define root value=\n{repeated_callers}%end\n%define helper value=\nhelper-output: input\n%end\n"
    );
    let over_budget_definitions = synthetic_macro_definitions(&over_budget);
    let over_budget_hashes = synthetic_macro_hashes(&over_budget_definitions);
    assert!(!verified_macro_closure_with_hashes(
        "root",
        &over_budget_definitions,
        |name| over_budget_hashes.get(name).cloned()
    ));
}

#[test]
fn verified_macro_arguments_accept_ordinary_double_quoted_flag_lists() {
    let source = "#MM compile-owner : obj/unit-a.o obj/unit-b.o\n%rule_compile_multi basenames=\"unit-a unit-b\" targetdir=obj cflags=\"-O2 -g\" cppflags=\"-DVALUE=2\"\nobj/unit-a.o obj/unit-b.o obj/unit-a.d obj/unit-b.d : | obj\n";
    let proofs = owners_fixture_with_verified(
        source,
        "obj/unit-a.o obj/unit-b.o obj/unit-a.d obj/unit-b.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .unwrap();
    assert_eq!(proofs[0].owner, "compile-owner");
}

#[test]
fn default_and_explicit_macro_flags_cannot_hide_eval_side_effects() {
    let hidden_default = "define HIDDEN\n$(eval #MM hidden-owner : obj/unit.o)\nendef\nCPPFLAGS := $(HIDDEN)\n%rule_compile_multi basenames=unit targetdir=obj\n#MM compile-owner : obj/unit.o\nobj/unit.o obj/unit.d : | obj\n";
    let root = tempdir().unwrap();
    let isolated = with_clean_macro_configuration(
        "define HIDDEN\n$(eval #MM hidden-owner : obj/unit.o)\nendef\nCPPFLAGS := $(HIDDEN)\n",
    );
    let joined = crate::parser::join_continuations(&isolated);
    let (scope, _) = collect_vars_impl(&joined, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(root.path());
    let contract = test_macro_contract(MacroForm::CompileMulti as usize).unwrap();
    assert!(effective_macro_arguments(
        "basenames=unit targetdir=obj",
        &contract,
        &scope,
        &dirs,
        root.path(),
        Path::new("fixture"),
        0,
    )
    .is_none());
    assert!(owners_fixture_with_verified(
        hidden_default,
        "obj/unit.o obj/unit.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let hidden_explicit = "define HIDDEN\n$(eval #MM hidden-owner : obj/unit.o)\nendef\n%rule_compile_multi basenames=unit targetdir=obj cflags=$(HIDDEN)\n#MM compile-owner : obj/unit.o\nobj/unit.o obj/unit.d : | obj\n";
    assert!(owners_fixture_with_verified(
        hidden_explicit,
        "obj/unit.o obj/unit.d",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());
}

#[test]
fn unmodeled_active_macro_vetoes_owner_attribution_even_with_literal_arguments() {
    let source =
        "object.o: source.c\n#MM object-owner : object.o\n%unverified_macro output=object.o\n";
    assert!(fixture(source, "object.o", VerifiedMacros::default()).is_none());

    let inactive =
        "ifeq (0,1)\n%unverified_macro output=object.o\nendif\nobject.o: source.c\n#MM object-owner : object.o\n";
    assert_eq!(
        fixture(inactive, "object.o", VerifiedMacros::default())
            .unwrap()
            .owner,
        "object-owner"
    );

    let inert =
        "define HIDDEN\n%unverified_macro output=object.o\nendef\nobject.o: source.c\n#MM object-owner : object.o\n";
    assert_eq!(
        fixture(inert, "object.o", VerifiedMacros::default())
            .unwrap()
            .owner,
        "object-owner"
    );
}

#[test]
fn inline_genmf_macros_veto_but_make_comments_do_not() {
    let chain = "object.o: source.c\n#MM object-owner : object.o\n";
    let inline_unmodeled = format!("{chain}prefix %rule_compile output=object.o\n");
    assert!(fixture(&inline_unmodeled, "object.o", VerifiedMacros::default(),).is_none());

    let inline_modeled = format!("{chain}prefix %rule_compile_multi basenames=unit\n");
    assert!(fixture(
        &inline_modeled,
        "object.o",
        VerifiedMacros::from_forms(&[MacroForm::CompileMulti]),
    )
    .is_none());

    let comment = format!("{chain}# this is a Make comment %rule_compile output=object.o\n");
    assert_eq!(
        fixture(&comment, "object.o", VerifiedMacros::default())
            .unwrap()
            .owner,
        "object-owner"
    );
}

#[test]
fn surviving_make_include_directives_veto_source_ownership() {
    let literal = "object.o: source.c\n#MM object-owner : object.o\ninclude fragment.mk\n";
    assert!(fixture(literal, "object.o", VerifiedMacros::default()).is_none());

    let variable = "FRAGMENT := fragment.mk\nobject.o: source.c\n#MM object-owner : object.o\ninclude $(FRAGMENT)\n";
    assert!(fixture(variable, "object.o", VerifiedMacros::default()).is_none());

    let comment = "object.o: source.c\n#MM object-owner : object.o\n# include fragment.mk\n";
    assert_eq!(
        fixture(comment, "object.o", VerifiedMacros::default())
            .unwrap()
            .owner,
        "object-owner"
    );
}

#[test]
fn malformed_closed_macro_argument_lists_are_unattributable() {
    for arguments in [
        "basenames=\"unit-a\" targetdir",
        "basenames=\"unit-a\" targetdir=\"unterminated",
        "basenames=$(if 1,unit-a,targetdir=evil",
        "basenames=\"unit-a\" basenames=\"unit-b\"",
        "basenames=unit-a\" targetdir=evil",
        "basenames=$(if 1,unit-a,${BROKEN))",
    ] {
        assert!(closed_macro_arguments(arguments).is_none(), "{arguments}");
    }
}

#[test]
fn quoted_and_nested_fake_macro_fields_cannot_select_outputs_or_owners() {
    let compile = VerifiedMacros::from_forms(&[MacroForm::CompileMulti]);
    let fake_basenames = "#MM fake-owner : obj/ghost.o\n%rule_compile_multi cflags=\"$(if 1, basenames=ghost ,)\" basenames=\"unit-a\" targetdir=obj\nobj/ghost.o obj/ghost.d : | obj\n";
    assert!(
        owners_fixture_with_verified(fake_basenames, "obj/ghost.o obj/ghost.d", compile,).is_none()
    );

    let fake_targetdir = "#MM fake-owner : evil/unit-a.o\n%rule_compile_multi cflags=\"$(if 1, targetdir=evil ,)\" basenames=\"unit-a\" targetdir=obj\nevil/unit-a.o evil/unit-a.d : | obj\n";
    assert!(
        owners_fixture_with_verified(fake_targetdir, "evil/unit-a.o evil/unit-a.d", compile,)
            .is_none()
    );

    let fake_mmake = "obj/unit.o: src/unit.c\n%build_prog cflags=\"$(if 1, mmake=fake-owner ,)\" mmake=real-owner progname=app files=unit objdir=obj\n";
    let arguments = closed_macro_arguments(
        "cflags=\"$(if 1, mmake=fake-owner ,)\" mmake=real-owner progname=app files=unit objdir=obj",
    ).unwrap();
    assert_eq!(macro_value(&arguments, "mmake"), Some("real-owner"));
    assert!(fixture(
        fake_mmake,
        "obj/unit.o",
        VerifiedMacros::from_forms(&[MacroForm::BuildProg])
    )
    .is_none());

    let fake_binary = "obj/unit.o: src/unit.c\n#MM real-owner : bin/real.o\n%rule_link_binary ldflags=\"$(if 1, file=bin/fake.o ,)\" file=bin/real.o name=real objs=obj/unit.o mmake=link-namespace\n";
    assert_eq!(
        fixture(
            fake_binary,
            "obj/unit.o",
            VerifiedMacros::from_forms(&[MacroForm::LinkBinary]),
        )
        .unwrap()
        .owner,
        "real-owner"
    );
}

#[test]
fn finite_multi_target_order_only_rule_returns_each_complete_owner() {
    let source = "one.o two.o: | order-only\none-archive.a: one.o\ntwo-archive.a: two.o\n#MM owner-one : one-archive.a\n#MM owner-two : two-archive.a\n";
    let proofs = owners_fixture(source, "one.o two.o").unwrap();
    assert_eq!(
        proofs
            .iter()
            .map(|proof| proof.owner.as_str())
            .collect::<Vec<_>>(),
        ["owner-one", "owner-two"]
    );
    let partial =
        "one.o two.o: | order-only\none-archive.a: one.o\n#MM owner-one : one-archive.a\n";
    assert!(owners_fixture(partial, "one.o two.o").is_none());
}

#[test]
fn double_colon_rules_add_exact_prerequisite_edges() {
    let source = "shared.o:: src/first.c\nshared.o:: src/second.c\narchive.a: shared.o\n#MM archive-owner : archive.a\n";
    let proof = fixture(source, "shared.o", VerifiedMacros::default()).unwrap();
    assert_eq!(proof.owner, "archive-owner");

    let unsupported =
        "shared.o::: src/first.c\narchive.a: shared.o\n#MM archive-owner : archive.a\n";
    assert!(fixture(unsupported, "shared.o", VerifiedMacros::default()).is_none());
}

#[test]
fn one_stem_pattern_rules_match_only_finite_source_identities() {
    let source = "OBJDIR := obj\nEXEDIR := bin\nTOOL := build-tool\nEXES := bin/app\nbuild-tool.a: obj/tool.o\nbin/app: bin/app.o\n$(EXEDIR)/% : $(OBJDIR)/%.o $(TOOL).a | $(EXEDIR)\n#MM app-owner : $(EXES)\n";
    let proof = fixture(source, "build-tool.a", VerifiedMacros::default()).unwrap();
    assert_eq!(proof.owner, "app-owner");
    assert!(proof.chain.contains(&"bin/app".to_owned()));

    let no_finite_target = "OBJDIR := obj\nEXEDIR := bin\nTOOL := build-tool\nbuild-tool.a: obj/tool.o\n$(EXEDIR)/% : $(OBJDIR)/%.o $(TOOL).a | $(EXEDIR)\n";
    assert!(fixture(no_finite_target, "build-tool.a", VerifiedMacros::default()).is_none());
}

#[test]
fn unsupported_multi_stem_pattern_vetoes_full_snapshot() {
    let source =
        "object.o: input.c\narchive.a: object.o\n#MM owner : archive.a\nbin/%/sub% : object.o\n";
    assert!(fixture(source, "object.o", VerifiedMacros::default()).is_none());
}

#[test]
fn one_stem_pattern_closure_has_a_hard_work_budget() {
    let mut graph = SourceGraph::default();
    graph
        .make_identities
        .extend((0..300).map(|index| format!("file-{index}")));
    graph.patterns.extend((0..300).map(|index| PatternRule {
        target: format!("target-{index}/%"),
        prerequisites: vec!["input-%.o".to_owned()],
    }));
    instantiate_pattern_edges(&mut graph);
    assert!(graph.uncertain);
    assert!(graph.edge_count <= MAX_IDENTITIES);
}

#[test]
fn unowned_consumer_branch_and_cycles_refuse_partial_attribution() {
    let unowned_branch = "object.o: input.c\narchive.a: object.o\n#MM owner-a : archive.a\norphan-target: object.o\n";
    assert!(fixture(unowned_branch, "object.o", VerifiedMacros::default()).is_none());

    let cycle = "object.o: input.c\nfirst: object.o\nobject.o: first\narchive.a: object.o\n#MM owner-a : archive.a\n";
    assert!(fixture(cycle, "object.o", VerifiedMacros::default()).is_none());
}

#[test]
fn distinct_owners_for_one_output_are_ambiguous() {
    let source = "obj/shared.o: src/shared.c\n%build_prog mmake=first progname=one files=src/shared objdir=obj\n%build_prog mmake=second progname=two files=src/shared objdir=obj\n";
    assert!(fixture(
        source,
        "obj/shared.o",
        VerifiedMacros::from_forms(&[MacroForm::BuildProg]),
    )
    .is_none());
}

#[test]
fn disabled_tiff_style_meta_bridge_does_not_own_neighboring_rule() {
    let source = "pkgconfig/libtiff.pc: libtiff.pc.in\n\t@$(ECHO) generated >$@\n##MM workbench-libs-tiff-pkgconfig : pkgconfig/libtiff.pc\n#MM workbench-libs-tiff : linklibs-tiff\n";
    assert!(fixture(source, "pkgconfig/libtiff.pc", VerifiedMacros::default(),).is_none());
}

#[test]
fn unresolved_or_conditional_edges_never_bridge_owners() {
    let unresolved = "obj/unit.o: src/unit.c\n#MM owner : $(UNKNOWN_PREREQUISITES)\n";
    assert!(fixture(unresolved, "obj/unit.o", VerifiedMacros::default()).is_none());

    let unresolved_target = "$(UNKNOWN_OUTPUT).o: src/unit.c\n#MM owner : $(UNKNOWN_OUTPUT).o\n";
    assert!(fixture(
        unresolved_target,
        "$(UNKNOWN_OUTPUT).o",
        VerifiedMacros::default(),
    )
    .is_none());

    let joined = crate::parser::join_continuations(
        "obj/unit.o: src/unit.c\nifeq ($(UNKNOWN),yes)\n#MM owner : obj/unit.o\nendif\n",
    );
    let root = tempdir().unwrap();
    let (scope, states) = collect_vars_impl(&joined, None);
    let dirs = DirVars::load(root.path());
    let output_line = 1;
    assert!(attribute_with_verified_macros(
        &joined,
        &scope,
        &dirs,
        (root.path(), Path::new("fixture")),
        Some(&states),
        ("obj/unit.o", output_line),
        VerifiedMacros::default(),
    )
    .is_none());
}

#[test]
fn known_owner_is_not_unique_when_unknown_conditional_consumer_may_exist() {
    let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nifeq ($(UNKNOWN),yes)\n#MM owner-b : obj/unit.o\nendif\n";
    assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
}

#[test]
fn unknown_conditional_rule_cannot_hide_a_second_consumer() {
    let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nifeq ($(UNKNOWN),yes)\nconditional-archive.a: obj/unit.o\nendif\n#MM owner-b : conditional-archive.a\n";
    assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
}

#[test]
fn unknown_conditional_macro_cannot_hide_a_second_consumer() {
    let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nifeq ($(UNKNOWN),yes)\n%rule_link_binary mmake=owner-b file=bin/conditional.o objs=obj/unit.o\nendif\n#MM owner-b : bin/conditional.o\n";
    assert!(fixture(
        source,
        "obj/unit.o",
        VerifiedMacros::from_forms(&[MacroForm::LinkBinary]),
    )
    .is_none());
}

#[test]
fn unresolved_active_second_consumer_invalidates_known_owner() {
    let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\nconditional-archive.a: $(UNKNOWN_OBJECTS)\n#MM owner-b : conditional-archive.a\n";
    assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
}

#[test]
fn unresolved_active_target_invalidates_known_owner() {
    let source = "obj/unit.o: src/unit.c\narchive.a: obj/unit.o\n#MM owner-a : archive.a\n$(UNKNOWN_ARCHIVES): obj/unit.o\n";
    assert!(fixture(source, "obj/unit.o", VerifiedMacros::default()).is_none());
}

#[test]
fn native_macro_verification_fails_closed_on_changed_definition() {
    let changed = "%define rule_compile_multi mmake=TMP\n%(mmake)_MC_TARGETS := changed\n%end\n";
    assert!(!verified_macro(
        changed,
        "rule_compile_multi",
        COMPILE_MULTI_SHA256
    ));
}
