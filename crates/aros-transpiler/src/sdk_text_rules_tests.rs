use super::*;
use crate::make_vars::collect_vars_with_context;
use crate::parser::TargetContext;
use std::fs;
use tempfile::tempdir;

fn fixture_root() -> tempfile::TempDir {
    let temp = tempdir().expect("temporary source root");
    fs::create_dir_all(temp.path().join("config")).expect("config directory");
    fs::write(
        temp.path().join("config/make.cfg.in"),
        "AROS_DIR_DEVELOPER := Developer\nAROS_DIR_LIB := lib\nAROSDIR := $(TARGETDIR)/SYS\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\n",
    )
    .expect("config paths");
    temp
}

fn scan(source: &str) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
    scan_with_states(source, None)
}

fn scan_with_states(
    source: &str,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
    let root = fixture_root();
    let target = TargetContext::default();
    let scope = collect_vars_with_context(source, &target);
    let dirs = DirVars::load(root.path());
    collect_sdk_text_rules_with_context(
        source,
        root.path(),
        Path::new("workbench/libs/example"),
        &scope,
        &dirs,
        line_states,
    )
}

fn scan_with_physical_source(
    content: &str,
    physical_source: &str,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
    let root = fixture_root();
    let target = TargetContext::default();
    let scope = collect_vars_with_context(content, &target);
    let dirs = DirVars::load(root.path());
    collect_sdk_text_rules_with_physical_source(
        content,
        physical_source,
        root.path(),
        Path::new("workbench/libs/example"),
        &scope,
        &dirs,
        line_states,
    )
}

#[test]
fn unresolved_non_pc_make_target_is_not_an_sdk_text_candidate() {
    let source = "$(top_builddir)/$(CUR_MESADIR)/main/dispatch.h: $(GLAPI_DEPS)\n\t@python3 gen_dispatch.py\n";
    let (declarations, rejected) = scan(source);

    assert!(declarations.is_empty());
    assert!(
        rejected.is_empty(),
        "unrelated generated headers must not be diagnosed as SDK text: {rejected:#?}"
    );

    let source = "AROS_LIB := $(MISSING_LIBROOT)\n/foreign/pkgconfig/dispatch.h: input.h\n";
    let (declarations, rejected) = scan(source);
    assert!(declarations.is_empty());
    assert!(
        rejected.is_empty(),
        "a non-PC target cannot become a candidate merely because its path contains `pkgconfig`: {rejected:#?}"
    );
}

#[test]
fn unresolved_pc_targets_are_rejected_directly_and_through_local_aliases() {
    let cases = [
        "AROS_LIB := $(MISSING_LIBROOT)\nexample-pkgc: $(AROS_LIB)/pkgconfig/example.pc\n$(AROS_LIB)/pkgconfig/example.pc: $(PORTSDIR)/example.pc.in\n",
        "AROS_LIB := $(MISSING_LIBROOT)\nSDK_TARGET := $(AROS_LIB)/pkgconfig/example.pc\nexample-pkgc: $(SDK_TARGET)\n$(SDK_TARGET): $(PORTSDIR)/example.pc.in\n",
        "AROS_LIB := $(MISSING_LIBROOT)\nPC_PATH := $(AROS_LIB)/pkgconfig/example.pc\nSDK_TARGET := $(PC_PATH)\nexample-pkgc: $(SDK_TARGET)\n$(SDK_TARGET): $(PORTSDIR)/example.pc.in\n",
    ];

    for source in cases {
        let (declarations, rejected) = scan(source);
        assert!(declarations.is_empty());
        assert!(
            rejected.iter().any(|item| {
                item.owner == "example-pkgc"
                    && item.reason.contains("cannot resolve SDK text output")
            }),
            "unresolved `.pc` output must remain a named fail-closed diagnostic: {rejected:#?}"
        );
    }
}

fn source(recipe: &str) -> String {
    format!(
        "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n#MM example-pkgc : example-fetch\nexample-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n$(AROS_LIB)/pkgconfig/example.pc : $(ARCHSRCDIR)/example.pc.in\n{recipe}\n"
    )
}

fn source_with_disabled_owner() -> String {
    source(actual_shape_recipe())
        .replace(
            "#MM example-pkgc : example-fetch\n",
            "##MM\n#example-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n",
        )
        .replace("\nexample-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n", "\n")
}

fn actual_shape_recipe() -> &'static str {
    "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|@PREFIX@|/Developer|g' \\\n\t    -e 's|@VERSION@|$(VERSION)|g' \\\n\t    -e '/^Libs\\.private/d' \\\n\t    -e 's|^exec_prefix=.*|exec_prefix=$${prefix}|' \\\n\t    $< > $@"
}

fn implicit_fetch_source(prerequisites: &str, recipe: &str) -> String {
    format!(
        "VERSION := 3.4.5\nARCHBASE := bundle+variant\n%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\nlib-tools+config : $(AROS_LIB)/pkgconfig/lib.tools+config.pc\n$(AROS_LIB)/pkgconfig/lib.tools+config.pc : {prerequisites}\n{recipe}\n"
    )
}

fn echo_mkdir_q_sed_recipe(sed: &str) -> String {
    format!(
        "\t@$(ECHO) \"Generating /Developer/lib/pkgconfig/lib.tools+config.pc ...\"\n\t%mkdir_q dir=$(AROS_LIB)/pkgconfig\n\t@$(SED) {sed}\n"
    )
}

#[test]
fn scans_ordered_literal_sdk_template_operations() {
    let input = source(actual_shape_recipe());
    let (decls, rejected) = scan(&input);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(decls.len(), 1);
    let declaration = &decls[0];
    assert_eq!(declaration.owner, "example-pkgc");
    assert_eq!(declaration.fetch_owner, "example-fetch");
    assert!(declaration.input.ends_with("/pkg/archive/example.pc.in"));
    assert!(declaration
        .output
        .ends_with("/SYS/Developer/lib/pkgconfig/example.pc"));
    assert_eq!(
        declaration.operations,
        vec![
            SdkTextOperation::ReplaceAll {
                token: "@PREFIX@".into(),
                replacement: "/Developer".into(),
            },
            SdkTextOperation::ReplaceAll {
                token: "@VERSION@".into(),
                replacement: "1.2.3".into(),
            },
            SdkTextOperation::DeleteLinePrefix {
                prefix: "Libs.private".into(),
            },
            SdkTextOperation::ReplaceLine {
                prefix: "exec_prefix=".into(),
                replacement: "exec_prefix=${prefix}".into(),
            },
        ]
    );
    assert!(declaration
        .operations
        .last()
        .unwrap()
        .cmake_argument()
        .contains("${prefix}"));
}

#[test]
fn disabled_exact_meta_owner_is_attributed_for_diagnostics_only() {
    let input = source_with_disabled_owner();
    let (decls, rejected) = scan(&input);

    assert!(
        decls.is_empty(),
        "a disabled MetaMake owner is not a producer"
    );
    let [diagnostic] = rejected.as_slice() else {
        panic!("expected one rejected SDK text output: {rejected:#?}");
    };
    assert_eq!(diagnostic.owner, "example-pkgc");
    assert!(diagnostic.disabled_owner_only);
    assert_eq!(
        diagnostic.reason,
        "SDK text output has no unique named Make owner rule"
    );
}

#[test]
fn disabled_owner_attribution_refuses_ambiguous_or_unknown_claims() {
    let ambiguous = format!(
        "{}##MM second-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n",
        source_with_disabled_owner()
    );
    let (decls, rejected) = scan(&ambiguous);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| {
        item.owner == "<unknown-owner>"
            && !item.disabled_owner_only
            && item.reason == "SDK text output has no unique named Make owner rule"
    }));

    let unknown = source_with_disabled_owner();
    let owner_line = unknown
        .lines()
        .position(|line| line.starts_with("#example-pkgc :"))
        .expect("disabled owner line");
    let mut states = vec![ConditionalTruth::True; unknown.lines().count()];
    states[owner_line] = ConditionalTruth::Unknown;
    let (decls, rejected) = scan_with_states(&unknown, Some(&states));
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.owner == "<unknown-owner>" && !item.disabled_owner_only));
}

#[test]
fn disabled_competitors_with_extra_or_unresolved_prerequisites_block_attribution() {
    let cases = [
        "##MM second-pkgc : $(AROS_LIB)/pkgconfig/example.pc extra-input\n",
        "##MM second-pkgc : $(AROS_LIB)/pkgconfig/example.pc | order-only-input\n",
        "##MM second-pkgc : $(UNKNOWN_PC_OUTPUT)\n",
    ];
    for competitor in cases {
        let input = format!("{}{competitor}", source_with_disabled_owner());
        let (decls, rejected) = scan(&input);
        assert!(decls.is_empty());
        assert!(
            rejected.iter().any(|item| {
                item.owner == "<unknown-owner>"
                    && item.reason == "SDK text output has no unique named Make owner rule"
            }),
            "disabled competitor must veto attribution: {competitor:?}: {rejected:#?}"
        );
    }
}

#[test]
fn potentially_matching_continued_disabled_competitor_blocks_attribution() {
    let input = format!(
        "{}##MM second-pkgc : \\\n$(AROS_LIB)/pkgconfig/example.pc\n",
        source_with_disabled_owner()
    );
    let (decls, rejected) = scan(&input);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));

    // The parser pipeline supplies continuation-joined text. The extra
    // separator left by this unsupported continuation must still prevent
    // a false unique-owner attribution.
    let joined = crate::parser::join_continuations(&input);
    let (decls, rejected) = scan(&joined);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));
}

#[test]
fn physical_source_rejects_a_continued_marker_that_joined_text_makes_valid() {
    let physical = format!(
        "{}##MM \\\nsecond-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n",
        source_with_disabled_owner()
    );
    let joined = crate::parser::join_continuations(&physical);
    let (decls, rejected) = scan_with_physical_source(&joined, &physical, None);

    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| {
        item.owner == "<unknown-owner>"
            && item.reason == "SDK text output has no unique named Make owner rule"
    }));
}

#[test]
fn unrelated_physical_snapshot_cannot_supply_diagnostic_ownership() {
    let physical = source_with_disabled_owner();
    let parsed = physical.replace("##MM\n", "# unrelated marker\n");
    let (decls, rejected) = scan_with_physical_source(&parsed, &physical, None);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));
}

#[test]
fn define_bodies_are_inert_for_owner_rules_edges_and_consumers() {
    let candidate_in_define = format!(
        "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\ndefine HIDDEN\n{}\nendef\n",
        source_with_disabled_owner()
    );
    let (decls, rejected) = scan(&candidate_in_define);
    assert!(decls.is_empty());
    assert!(
        rejected.is_empty(),
        "Make define bodies are data, not active SDK text rules: {rejected:#?}"
    );

    for prefix in ["override define", "export define", "unexport define"] {
        let candidate_in_define = format!(
            "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n{prefix} HIDDEN\n{}\nendef\n",
            source_with_disabled_owner()
        );
        let (decls, rejected) = scan(&candidate_in_define);
        assert!(decls.is_empty(), "{prefix} body admitted an SDK producer");
        assert!(
            rejected.is_empty(),
            "{prefix} body was parsed as Make rules: {rejected:#?}"
        );
    }

    let inert_competitors = format!(
        "define HIDDEN\n##MM hidden-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n#MM hidden-meta-consumer : $(AROS_LIB)/pkgconfig/example.pc\nhidden-consumer : $(AROS_LIB)/pkgconfig/example.pc extra\nendef\n{}",
        source_with_disabled_owner()
    );
    let (decls, rejected) = scan(&inert_competitors);
    assert!(decls.is_empty());
    assert_eq!(rejected.len(), 1, "{rejected:#?}");
    assert_eq!(rejected[0].owner, "example-pkgc");
}

#[test]
fn active_dynamic_make_expansion_vetoes_sdk_text_owner_admission() {
    let hidden_rule = "hidden-consumer : $(AROS_LIB)/pkgconfig/example.pc";
    let cases = [
        format!(
            "define HIDDEN\n{hidden_rule}\nendef\n{}\n$(eval $(HIDDEN))",
            source_with_disabled_owner()
        ),
        format!(
            "define HIDDEN\n{hidden_rule}\nendef\nRULES := $(eval $(HIDDEN))\n{}",
            source_with_disabled_owner()
        ),
        format!(
            "define HIDDEN\n$(eval $(RULE_TEXT))\nendef\ndefine RULE_TEXT\n{hidden_rule}\nendef\nEXPANDED := $(HIDDEN)\n{}",
            source_with_disabled_owner()
        ),
        format!(
            "define HIDDEN\n$(eval $(RULE_TEXT))\nendef\ndefine RULE_TEXT\n{hidden_rule}\nendef\nEXPANDED := $(call HIDDEN)\n{}",
            source_with_disabled_owner()
        ),
        format!(
            "define HIDDEN\n{hidden_rule}\nendef\n{}\n$(call HIDDEN)",
            source(actual_shape_recipe())
        ),
        format!(
            "define RULE_TEXT\n{hidden_rule}\nendef\n{}\n$(RULE_TEXT)",
            source(actual_shape_recipe())
        ),
    ];

    for input in cases {
        let (declarations, rejected) = scan(&input);
        assert!(declarations.is_empty(), "dynamic source was admitted");
        assert!(
            rejected.iter().any(|item| {
                item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
            }),
            "dynamic expansion must remain unowned: {rejected:#?}"
        );
    }
}

#[test]
fn opaque_define_alias_cannot_hide_an_active_eval_consumer() {
    let input = format!(
        "define ALIAS\n$(eval extra-consumer : $(AROS_LIB)/pkgconfig/example.pc)\nendef\ndefine HIDDEN\n$(ALIAS)\nendef\nreal-target : $(if 1,$(HIDDEN))\n{}",
        source_with_disabled_owner()
    );
    let (declarations, rejected) = scan(&input);
    assert!(declarations.is_empty());
    assert!(
        rejected.iter().any(|item| {
            item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
        }),
        "opaque alias must not prove a disabled owner: {rejected:#?}"
    );
}

#[test]
fn branching_alias_scan_has_a_complete_file_work_budget() {
    use std::fmt::Write as _;
    let mut input = String::from("A14 = literal\n");
    for index in (0..14).rev() {
        writeln!(
            input,
            "A{index} = {}",
            vec![format!("$(A{})", index + 1); 8].join(" ")
        )
        .unwrap();
    }
    input.push_str("real-target : $(A0)\n");
    input.push_str(&source_with_disabled_owner());
    let (declarations, rejected) = scan(&input);
    assert!(declarations.is_empty());
    assert!(
        rejected.iter().any(|item| {
            item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
        }),
        "exhausted proof budget must veto admission: {rejected:#?}"
    );
}

#[test]
fn unreferenced_define_expansion_is_inert_but_unknown_active_expansion_vetoes() {
    let inert = format!(
        "define HIDDEN\n$(eval $(RULE_TEXT))\nendef\ndefine RULE_TEXT\nhidden-consumer : $(AROS_LIB)/pkgconfig/example.pc\nendef\n{}",
        source(actual_shape_recipe())
    );
    let (declarations, rejected) = scan(&inert);
    assert_eq!(
        declarations.len(),
        1,
        "unreferenced define is inert: {rejected:#?}"
    );
    assert!(rejected.is_empty(), "{rejected:#?}");

    let dynamic = format!("{}\n$(eval $(HIDDEN))", source(actual_shape_recipe()));
    let eval_line = dynamic
        .lines()
        .position(|line| line == "$(eval $(HIDDEN))")
        .expect("active eval line");
    let mut states = vec![ConditionalTruth::True; dynamic.lines().count()];
    states[eval_line] = ConditionalTruth::Unknown;
    let (declarations, rejected) = scan_with_states(&dynamic, Some(&states));
    assert!(declarations.is_empty());
    assert!(
        rejected.iter().any(|item| {
            item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
        }),
        "unknown active expansion must veto admission: {rejected:#?}"
    );

    states[eval_line] = ConditionalTruth::False;
    let (declarations, rejected) = scan_with_states(&dynamic, Some(&states));
    assert_eq!(
        declarations.len(),
        1,
        "literal false expansion is ignored: {rejected:#?}"
    );
    assert!(rejected.is_empty(), "{rejected:#?}");
}

#[test]
fn malformed_define_vetoes_sdk_text_producer_selection() {
    let input = format!("{}define UNTERMINATED\n", source(actual_shape_recipe()));
    let (decls, rejected) = scan(&input);

    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| {
        item.reason
            .contains("malformed or unclosed Make `define` body")
    }));
}

#[test]
fn active_consumers_and_comment_drift_do_not_prove_disabled_owner() {
    let active_consumer = format!(
        "{}unrelated-target : $(AROS_LIB)/pkgconfig/example.pc extra-input\n",
        source_with_disabled_owner()
    );
    let (decls, rejected) = scan(&active_consumer);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| {
        item.owner == "<unknown-owner>"
            && item.reason == "SDK text output has no unique named Make owner rule"
    }));

    let drifted = source_with_disabled_owner().replace(
        "##MM\n#example-pkgc :",
        "##MM documentation drift\n#example-pkgc :",
    );
    let (decls, rejected) = scan(&drifted);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));
}

#[test]
fn unresolved_double_colon_and_pattern_consumers_block_disabled_attribution() {
    let cases = [
        "unresolved-consumer : $(UNKNOWN_PC_PATH) other-input\n",
        "double-colon-consumer :: $(AROS_LIB)/pkgconfig/example.pc other-input\n",
        "pattern-consumer : $(AROS_LIB)/pkgconfig/%.pc\n",
    ];
    for active_rule in cases {
        let input = format!("{}{active_rule}", source_with_disabled_owner());
        let (decls, rejected) = scan(&input);
        assert!(decls.is_empty());
        assert!(
            rejected.iter().any(|item| {
                item.owner == "<unknown-owner>"
                    && item.reason == "SDK text output has no unique named Make owner rule"
            }),
            "active rule must block diagnostic owner attribution: {active_rule:?}: {rejected:#?}"
        );
    }
}

#[test]
fn rejects_unsafe_regex_and_extra_commands_with_owner() {
    let unsafe_regex = source(
        "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|@PREFIX@.*|/Developer|g' $< > $@",
    );
    let (decls, rejected) = scan(&unsafe_regex);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "example-pkgc"));

    let extra_command = source(&format!(
        "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(ECHO) unexpected\n{}",
        actual_shape_recipe()
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n")
    ));
    let (decls, rejected) = scan(&extra_command);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "example-pkgc"));
}

#[test]
fn infers_unique_most_specific_fetch_without_requiring_an_mm_edge() {
    let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
    let source = implicit_fetch_source(
        "$(PORTSDIR)/bundle/archive/template.pc.in",
        &recipe,
    )
    .replace(
        "%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
        "%fetch mmake=fetch-bundle archive=root destination=$(PORTSDIR)\n%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
    );
    let (decls, rejected) = scan(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].owner, "lib-tools+config");
    assert_eq!(decls[0].fetch_owner, "fetch-bundle.v2");
    assert_eq!(decls[0].file_sha256, "");
    assert!(decls[0].input.ends_with("/bundle/archive/template.pc.in"));
    assert!(decls[0].output.ends_with("/lib.tools+config.pc"));
}

#[test]
fn implicit_fetch_inference_rejects_missing_tied_and_unknown_providers() {
    let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
    let base = implicit_fetch_source("$(PORTSDIR)/bundle/archive/template.pc.in", &recipe);

    let missing = base.replace(
        "%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
        "",
    );
    let (decls, rejected) = scan(&missing);
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("no matching `%fetch`")));

    let tied = base.replace(
        "%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
        "%fetch mmake=fetch-left archive=left destination=$(PORTSDIR)/bundle\n%fetch mmake=fetch-right archive=right destination=$(PORTSDIR)/bundle\n",
    );
    let (decls, rejected) = scan(&tied);
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("ambiguous most-specific `%fetch`")));

    let mut states = vec![ConditionalTruth::True; base.lines().count()];
    let fetch_line = base
        .lines()
        .position(|line| line.starts_with("%fetch"))
        .expect("fetch line");
    states[fetch_line] = ConditionalTruth::Unknown;
    let (decls, rejected) = scan_with_states(&base, Some(&states));
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("undecided Make conditional")));
}

#[test]
fn explicit_fetch_edge_is_not_replaced_by_path_inference() {
    let wrong_source = source(actual_shape_recipe()).replace(
        "#MM example-pkgc : example-fetch",
        "#MM example-pkgc : unrelated-fetch",
    );
    let wrong_source = wrong_source.replace(
        "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive",
        "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n%fetch mmake=unrelated-fetch archive=elsewhere destination=$(PORTSDIR)/elsewhere",
    );
    let (decls, rejected) = scan(&wrong_source);
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("not below `%fetch`")));

    let wrong_source = source(actual_shape_recipe()).replace(
        "$(ARCHSRCDIR)/example.pc.in",
        "$(PORTSDIR)/elsewhere/example.pc.in",
    );
    let (decls, rejected) = scan(&wrong_source);
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("not below `%fetch`")));
}

#[test]
fn virtual_and_tabbed_meta_edges_are_binding_and_conflicts_reject() {
    let virtual_edge = source(actual_shape_recipe()).replace(
        "#MM example-pkgc : example-fetch",
        "#MM-\texample-pkgc : example-fetch",
    );
    let (decls, rejected) = scan(&virtual_edge);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].fetch_owner, "example-fetch");

    let conflict = virtual_edge.replace(
        "example-pkgc : $(AROS_LIB)/pkgconfig/example.pc",
        "#MM\texample-pkgc : other-fetch\n%fetch mmake=other-fetch archive=other destination=$(PORTSDIR)/other\nexample-pkgc : $(AROS_LIB)/pkgconfig/example.pc",
    );
    let (decls, rejected) = scan(&conflict);
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("duplicate `#MM` prerequisite edges")));

    let mut states = vec![ConditionalTruth::True; virtual_edge.lines().count()];
    let edge_line = virtual_edge
        .lines()
        .position(|line| line.starts_with("#MM-\t"))
        .expect("virtual edge line");
    states[edge_line] = ConditionalTruth::Unknown;
    let (decls, rejected) = scan_with_states(&virtual_edge, Some(&states));
    assert!(decls.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("undecided Make conditional")));
}

#[test]
fn bare_mm_marker_still_allows_generic_fetch_inference() {
    let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
    let source = implicit_fetch_source("$(PORTSDIR)/bundle/archive/template.pc.in", &recipe)
        .replace("\nlib-tools+config :", "\n#MM\nlib-tools+config :");
    assert!(source.contains("#MM\nlib-tools+config :"));
    let (decls, rejected) = scan(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].fetch_owner, "fetch-bundle.v2");
}

#[test]
fn unresolved_fetches_cannot_be_ignored_beside_an_explicit_edge() {
    let source = source(actual_shape_recipe()).replace(
        "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n",
        "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=unsafe destination=$(PORTSDIR)/pkg/../outside\n",
    );
    let (decls, rejected) = scan(&source);
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| {
        item.reason
            .contains("while a local `%fetch` declaration is unresolved")
    }));
}

#[test]
fn only_known_single_cmake_roots_are_preserved_in_paths() {
    let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
    let valid_input = "$(PORTSDIR)/bundle/archive/template.pc.in";
    let base = implicit_fetch_source(valid_input, &recipe);

    let input_with_injected_root = base.replace(
        "$(PORTSDIR)/bundle/archive/template.pc.in",
        "$(PORTSDIR)/bundle/$${EVIL}/template.pc.in",
    );
    let (decls, rejected) = scan(&input_with_injected_root);
    assert!(decls.is_empty());
    assert!(!rejected.is_empty());

    let output_with_injected_root = base.replace(
        "VERSION := 3.4.5",
        "AROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)/$${EVIL}\nVERSION := 3.4.5",
    );
    let (decls, rejected) = scan(&output_with_injected_root);
    assert!(decls.is_empty());
    assert!(!rejected.is_empty());

    let fetch_with_injected_root = base.replace(
        "destination=$(PORTSDIR)/bundle",
        "destination=$(PORTSDIR)/bundle/$${EVIL}",
    );
    let (decls, rejected) = scan(&fetch_with_injected_root);
    assert!(decls.is_empty());
    assert!(!rejected.is_empty());

    assert!(safe_cmake_path(
        "${AROS_DEVELOPER_LIB_DIR}/pkgconfig/example.pc",
        SDK_TEXT_OUTPUT_PATH_ROOTS
    ));
}

#[test]
fn static_echo_mkdir_q_recipe_accepts_source_prerequisite_and_first_per_line_sed() {
    let recipe = echo_mkdir_q_sed_recipe(
        "-e 's|@exec_prefix@|$${prefix}|' \\\n\t    -e 's|@includedir@/libtiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@|$${prefix}/include|' \\\n\t    -e 's|-ltiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@|-ltiff|' \\\n\t    -e 's|@libdir@|$${prefix}/lib|' \\\n\t    -e 's|@prefix@|/Developer|' \\\n\t    -e 's|@LIBS@||' \\\n\t    -e 's|@TIFFLIB_VERSION@|$(VERSION)|' \\\n\t    -e 's| -I$${includedir}||' \\\n\t    $< >$@",
    );
    let input_and_own_source =
        "$(PORTSDIR)/bundle/archive/template.pc.in $(SRCDIR)/$(CURDIR)/mmakefile.src";
    let source = implicit_fetch_source(input_and_own_source, &recipe);
    let (decls, rejected) = scan(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].fetch_owner, "fetch-bundle.v2");
    assert_eq!(
        decls[0].operations,
        vec![
            SdkTextOperation::ReplaceFirstPerLine {
                token: "@exec_prefix@".into(),
                replacement: "${prefix}".into(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: "@includedir@/libtiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@".into(),
                replacement: "${prefix}/include".into(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: "-ltiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@".into(),
                replacement: "-ltiff".into(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: "@libdir@".into(),
                replacement: "${prefix}/lib".into(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: "@prefix@".into(),
                replacement: "/Developer".into(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: "@LIBS@".into(),
                replacement: String::new(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: "@TIFFLIB_VERSION@".into(),
                replacement: "3.4.5".into(),
            },
            SdkTextOperation::ReplaceFirstPerLine {
                token: " -I${includedir}".into(),
                replacement: String::new(),
            },
        ]
    );
    assert_eq!(
        decls[0].operations[7].cmake_argument(),
        "REPLACE_FIRST_PER_LINE| -I${includedir}|"
    );
    let repeated = "@exec_prefix@ @exec_prefix@\n@exec_prefix@\n";
    let rendered = repeated
        .split_inclusive('\n')
        .map(|line| line.replacen("@exec_prefix@", "${prefix}", 1))
        .collect::<String>();
    assert_eq!(rendered, "${prefix} @exec_prefix@\n${prefix}\n");
}

#[test]
fn source_prerequisite_must_be_second_and_name_this_mmakefile() {
    let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
    let input = "$(PORTSDIR)/bundle/archive/template.pc.in";
    for prerequisites in [
        format!("$(SRCDIR)/$(CURDIR)/mmakefile.src {input}"),
        format!("{input} $(SRCDIR)/elsewhere/mmakefile.src"),
        format!("{input} $(SRCDIR)/$(CURDIR)/mmakefile.src foreign.in"),
    ] {
        let source = implicit_fetch_source(&prerequisites, &recipe);
        let (decls, rejected) = scan(&source);
        assert!(decls.is_empty(), "accepted prerequisites: {prerequisites}");
        assert!(!rejected.is_empty(), "missing rejection: {prerequisites}");
    }
}

#[test]
fn echo_and_mkdir_q_are_static_and_exactly_for_the_output_parent() {
    let valid_sed = "-e 's|@TOKEN@|x|' $< >$@";
    for recipe in [
        echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@").replace(
            "Generating /Developer/lib/pkgconfig/lib.tools+config.pc ...",
            "Generating $(SHELL) lib.tools+config.pc ...",
        ),
        echo_mkdir_q_sed_recipe(valid_sed)
            .replace("dir=$(AROS_LIB)/pkgconfig", "dir=$(AROS_LIB)/other"),
        echo_mkdir_q_sed_recipe(valid_sed).replace("%mkdir_q dir=", "%mkdir_q dir=extra "),
    ] {
        let input = "$(PORTSDIR)/bundle/archive/template.pc.in";
        let source = implicit_fetch_source(input, &recipe);
        let (decls, rejected) = scan(&source);
        assert!(decls.is_empty(), "accepted unsafe recipe: {recipe}");
        assert!(!rejected.is_empty(), "missing rejection for: {recipe}");
    }
}

#[test]
fn sed_literal_flags_distinguish_default_first_match_from_global() {
    let first = source(
        "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|TOKEN|X|' $< > $@",
    );
    let (decls, rejected) = scan(&first);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(
        decls[0].operations,
        vec![SdkTextOperation::ReplaceFirstPerLine {
            token: "TOKEN".into(),
            replacement: "X".into(),
        }]
    );

    let all = source(
        "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|TOKEN|X|g' $< > $@",
    );
    let (decls, rejected) = scan(&all);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(
        decls[0].operations,
        vec![SdkTextOperation::ReplaceAll {
            token: "TOKEN".into(),
            replacement: "X".into(),
        }]
    );
}

#[test]
fn sed_patterns_and_replacements_reject_dynamic_or_regex_syntax() {
    for sed in [
        "-e 's|TOKEN.*|X|g' $< >$@",
        "-e 's|$(VERSION)|X|g' $< >$@",
        "-e 's|TOKEN|$(UNKNOWN_SDK_VALUE)|g' $< >$@",
        "-e 's|TOKEN|$${untrusted_value}|g' $< >$@",
    ] {
        let input = source(&format!(
            "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) {sed}"
        ));
        let (decls, rejected) = scan(&input);
        assert!(decls.is_empty(), "accepted unsafe sed: {sed}");
        assert!(
            rejected.iter().any(|item| item.owner == "example-pkgc"),
            "missing scanner rejection for sed: {sed}"
        );
    }
}

#[test]
fn unknown_recipe_line_rejects_selected_owner() {
    let input = source(actual_shape_recipe());
    let mut states = vec![ConditionalTruth::True; input.lines().count()];
    let line = input
        .lines()
        .position(|line| line.contains("@$(SED) -e 's|@PREFIX@"))
        .expect("sed recipe line");
    states[line] = ConditionalTruth::Unknown;
    let root = fixture_root();
    let target = TargetContext::default();
    let scope = collect_vars_with_context(&input, &target);
    let dirs = DirVars::load(root.path());
    let (decls, rejected) = collect_sdk_text_rules_with_context(
        &input,
        root.path(),
        Path::new("workbench/libs/example"),
        &scope,
        &dirs,
        Some(&states),
    );
    assert!(decls.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "example-pkgc"));
}
