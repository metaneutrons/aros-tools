use super::{
    collect_arch_effect_scope_with_context, collect_arch_endpoint_effects, ArchEndpointEffectData,
    ArchEndpointEffectScan,
};
use crate::parser::TargetContext;
use std::collections::BTreeMap;
use std::path::Path;

const RECIPE: &str = "arch/pc-x86_64/demo/mmakefile.src";
const DECLARATION: &str = "%build_archspecific mainmmake=demo maindir=rom/demo modname=demo arch=pc-x86_64 files=\"$(FILES)\"\n";
const CONDITIONAL_FILES: &str = concat!(
    "FILES := base\n",
    "ifeq ($(AROS_TARGET_VARIANT),smp)\n",
    "FILES += smp\n",
    "endif\n",
    "ifeq ($(FEATURE),1)\n",
    "FILES += opt\n",
    "endif\n",
);

fn context(variant: Option<&str>, variables: &[(&str, &str)]) -> TargetContext {
    TargetContext {
        variant: variant.map(str::to_owned),
        make_variables: variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        ..TargetContext::default()
    }
}

fn scan(content: &str, target: &TargetContext) -> ArchEndpointEffectScan {
    let (scope, line_states) = collect_arch_effect_scope_with_context(content, Some(target));
    collect_arch_endpoint_effects(content, Path::new(RECIPE), &scope, Some(&line_states))
        .expect("test recipe is a safe source-relative path")
}

#[test]
fn metadata_refuses_configured_append_with_unknown_variable_flavor() {
    let input = concat!(
        "VALUE := -DEARLY\n",
        "FLAGS += $(VALUE)\n",
        "VALUE := -DLATE\n",
        "%set_archincludes mainmmake=demo maindir=rom/demo modname=demo pri=2 arch=pc-x86_64 includes=\"$(FLAGS)\"\n",
    );
    let result = scan(input, &context(Some(""), &[("FLAGS", "-DCONFIGURED")]));
    assert!(result.effects.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("configuration value"));
}

#[test]
fn architecture_flag_arguments_preserve_interleaving_and_repetitions() {
    let input = "%set_archincludes mainmmake=demo maindir=rom/demo modname=demo pri=02 arch=pc-x86_64 includes=\"-DFIRST=1 -I$(SRCDIR)/first -DSECOND=2 -I$(SRCDIR)/first -I $(SRCDIR)/last\"\n";
    let result = scan(input, &context(Some(""), &[]));
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    let [effect] = result.effects.as_slice() else {
        panic!("one producer")
    };
    let ArchEndpointEffectData::SetArchIncludes {
        arguments,
        include_dirs,
        priority_token,
        ..
    } = &effect.data
    else {
        panic!("metadata")
    };
    assert_eq!(
        arguments,
        &[
            "-DFIRST=1",
            "-I${AROS_SOURCE_DIR}/first",
            "-DSECOND=2",
            "-I${AROS_SOURCE_DIR}/first",
            "-I${AROS_SOURCE_DIR}/last"
        ]
    );
    assert_eq!(
        include_dirs,
        &["${AROS_SOURCE_DIR}/first", "${AROS_SOURCE_DIR}/last"]
    );
    assert_eq!(priority_token, "02");
}

#[test]
fn native_parser_arch_sources_use_complete_configuration_at_physical_callsite() {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path()).unwrap();
    let recipe = Path::new(RECIPE);
    std::fs::create_dir_all(root.join(recipe.parent().unwrap())).unwrap();
    std::fs::create_dir(root.join("config")).unwrap();
    // A binding replaces an existing source file; the original must exist.
    std::fs::write(root.join("config/aros.cfg"), "# classic configuration\n").unwrap();
    std::fs::write(
        root.join("config/native.mk"),
        "FEATURE := 1\nUSER_INCLUDES :=\nUSER_CFLAGS :=\nUSER_CPPFLAGS :=\n",
    )
    .unwrap();
    let source = concat!(
        "include $(SRCDIR)/config/aros.cfg\n",
        "FILES := base\n",
        "ifeq ($(FEATURE),1)\nFILES += optional\n",
        "else ifeq ($(FEATURE),2)\nFILES += alternative\nendif\n",
        "%build_archspecific mainmmake=demo maindir=rom/demo modname=demo arch=pc-x86_64 files=\"$(FILES)\"\n",
        "FILES := late\n",
    );
    std::fs::write(root.join(recipe), source).unwrap();
    let mut target = context(Some(""), &[]);
    target
        .make_include_bindings
        .insert("config/aros.cfg".into(), "config/native.mk".into());
    let dirs = crate::dirs::DirVars::load(&root);
    let parsed =
        crate::parse_mmakefile_with_dirs_and_context(&root.join(recipe), &root, &dirs, &target)
            .unwrap();
    assert!(
        parsed.skipped_arch_sources.is_empty(),
        "{:?}",
        parsed.skipped_arch_sources
    );
    let [declaration] = parsed.arch_sources.as_slice() else {
        panic!("one declaration: {:?}", parsed.arch_sources)
    };
    assert_eq!(declaration.files, &["base", "optional"]);
    assert_eq!(declaration.line, 7);

    std::fs::write(
        root.join("config/native.mk"),
        "FEATURE := 1\ninclude $(SRCDIR)/config/missing.mk\n",
    )
    .unwrap();
    let refused =
        crate::parse_mmakefile_with_dirs_and_context(&root.join(recipe), &root, &dirs, &target)
            .unwrap();
    assert!(refused.arch_sources.is_empty());
    assert!(!refused.skipped_arch_sources.is_empty());
}

#[test]
fn native_parser_rejects_arch_sources_declared_by_included_configuration() {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path()).unwrap();
    let recipe = Path::new(RECIPE);
    std::fs::create_dir_all(root.join(recipe.parent().unwrap())).unwrap();
    std::fs::create_dir(root.join("config")).unwrap();
    std::fs::write(root.join("config/aros.cfg"), "# classic configuration\n").unwrap();
    std::fs::write(
        root.join("config/native.mk"),
        "FILES := injected\n%build_archspecific mainmmake=demo maindir=rom/demo modname=demo arch=pc-x86_64 files=\"$(FILES)\"\n",
    )
    .unwrap();
    let source = concat!(
        "include $(SRCDIR)/config/aros.cfg\n",
        "FILES := base\n",
        "%build_archspecific mainmmake=demo maindir=rom/demo modname=demo arch=pc-x86_64 files=\"$(FILES)\"\n",
    );
    std::fs::write(root.join(recipe), source).unwrap();
    let mut target = context(Some(""), &[]);
    target
        .make_include_bindings
        .insert("config/aros.cfg".into(), "config/native.mk".into());
    let dirs = crate::dirs::DirVars::load(&root);
    let parsed =
        crate::parse_mmakefile_with_dirs_and_context(&root.join(recipe), &root, &dirs, &target)
            .unwrap();
    // Neither the injected nor the physical declaration survives: ownership
    // is all-or-nothing for the file's architecture lanes.
    assert!(parsed.arch_sources.is_empty(), "{:?}", parsed.arch_sources);
    assert!(
        !parsed.skipped_arch_sources.is_empty(),
        "an injected declaration must be diagnosed: {:?}",
        parsed.skipped_arch_sources
    );
}

#[test]
fn source_include_scope_keeps_physical_origins_and_declaration_time_values() {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path()).unwrap();
    let recipe = Path::new(RECIPE);
    std::fs::create_dir_all(root.join(recipe.parent().unwrap())).unwrap();
    std::fs::write(
        root.join(recipe.parent().unwrap()).join("selection.mk"),
        "FEATURE := 1\nFLAGS := -DSELECTED=1\n",
    )
    .unwrap();
    let input = concat!(
        "include $(SRCDIR)/$(CURDIR)/selection.mk\n",
        "%set_archincludes mainmmake=demo maindir=rom/demo modname=demo pri=2 arch=pc-x86_64 includes=\"$(FLAGS)\"\n",
        "FLAGS := -DLATER=1\n",
    );
    std::fs::write(root.join(recipe), input).unwrap();
    let result = super::collect_source_arch_endpoint_effects(
        input,
        recipe,
        &root,
        Some(&context(Some(""), &[])),
    )
    .unwrap();
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    let [effect] = result.effects.as_slice() else {
        panic!("one effect")
    };
    assert_eq!(effect.line, 2);
    let ArchEndpointEffectData::SetArchIncludes { definitions, .. } = &effect.data else {
        panic!("metadata")
    };
    assert_eq!(definitions, &["SELECTED=1"]);
    std::fs::write(
        root.join(recipe.parent().unwrap()).join("selection.mk"),
        "FLAGS := -DPARTIAL=1\ninclude $(SRCDIR)/$(CURDIR)/missing.mk\n",
    )
    .unwrap();
    let rejected = super::collect_source_arch_endpoint_effects(
        input,
        recipe,
        &root,
        Some(&context(Some(""), &[])),
    )
    .unwrap();
    assert!(rejected.effects.is_empty());
    assert_eq!(rejected.rejected.len(), 1);
}

fn module_sources(scan: &ArchEndpointEffectScan) -> Option<Vec<String>> {
    scan.effects.iter().find_map(|effect| match &effect.data {
        ArchEndpointEffectData::ArchModuleObjects { module_sources, .. } => {
            Some(module_sources.clone())
        }
        _ => None,
    })
}

#[test]
fn explicit_target_context_selects_variant_and_feature_source_lists() {
    let input = format!("{CONDITIONAL_FILES}{DECLARATION}");

    let empty_values = context(Some(""), &[("FEATURE", "")]);
    let empty_scan = scan(&input, &empty_values);
    assert!(empty_scan.rejected.is_empty(), "{:?}", empty_scan.rejected);
    assert_eq!(
        module_sources(&empty_scan).as_deref(),
        Some(["base".to_owned()].as_slice())
    );

    let selected_values = context(Some("smp"), &[("FEATURE", "1")]);
    let selected_scan = scan(&input, &selected_values);
    assert!(
        selected_scan.rejected.is_empty(),
        "{:?}",
        selected_scan.rejected
    );
    assert_eq!(
        module_sources(&selected_scan).as_deref(),
        Some(["base".to_owned(), "smp".to_owned(), "opt".to_owned()].as_slice())
    );
}

#[test]
fn pure_nested_make_functions_preserve_the_selected_architecture_sources() {
    let input =
        format!("FILES = base $(if $(filter 1,$(FEATURE)),enabled,disabled)\n{DECLARATION}");
    for (feature, selected) in [("", "disabled"), ("1", "enabled")] {
        let result = scan(&input, &context(Some(""), &[("FEATURE", feature)]));
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        assert_eq!(
            module_sources(&result),
            Some(vec!["base".into(), selected.into()])
        );
    }
}

#[test]
fn sealed_architecture_effects_refuse_filesystem_enumeration() {
    for expression in [
        "$(wildcard *.c)",
        "$(call WILDCARD,*.c)",
        "$(shell printf guessed)",
    ] {
        let input = format!("FILES = base {expression}\n{DECLARATION}");
        let result = scan(&input, &context(Some(""), &[]));
        assert!(result.effects.is_empty(), "{expression}");
        assert_eq!(result.rejected.len(), 1, "{expression}");
    }
}

#[test]
fn pure_make_branch_selection_does_not_evaluate_an_inactive_filesystem_call() {
    let input = format!("FILES = $(if 1,base,$(wildcard *.c))\n{DECLARATION}");
    let result = scan(&input, &context(Some(""), &[]));
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(module_sources(&result), Some(vec!["base".into()]));
}

#[test]
fn architecture_include_metadata_retains_source_selected_definitions() {
    let input = concat!(
        "FLAGS = $(if $(filter 1,$(FEATURE)),-DSELECTED=1,)\n",
        "%set_archincludes mainmmake=demo maindir=rom/demo modname=demo pri=2 arch=pc-x86_64 includes=\"-I$(SRCDIR)/$(CURDIR) $(FLAGS)\"\n",
    );
    for (feature, expected) in [("", Vec::new()), ("1", vec!["SELECTED=1".to_owned()])] {
        let result = scan(input, &context(Some(""), &[("FEATURE", feature)]));
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        let [effect] = result.effects.as_slice() else {
            panic!("one metadata effect");
        };
        let ArchEndpointEffectData::SetArchIncludes {
            definitions,
            include_dirs,
            ..
        } = &effect.data
        else {
            panic!("metadata effect");
        };
        assert_eq!(definitions, &expected);
        assert_eq!(include_dirs, &["${AROS_SOURCE_DIR}/arch/pc-x86_64/demo"]);
    }
}

#[test]
fn architecture_include_definitions_refuse_unresolved_and_shell_syntax() {
    for flags in [
        "-DNAME=$(UNKNOWN)",
        "-DNAME=1;command",
        "-DNAME=$<CONFIG>",
        "-D9INVALID=1",
        "-include injected.h",
    ] {
        let input = format!("%set_archincludes mainmmake=demo maindir=rom/demo modname=demo pri=2 arch=pc-x86_64 includes=\"{flags}\"\n");
        let result = scan(&input, &context(Some(""), &[]));
        assert!(result.effects.is_empty(), "{flags}");
        assert_eq!(result.rejected.len(), 1, "{flags}");
    }
}

#[test]
fn missing_feature_value_rejects_the_unresolved_source_list() {
    let input = format!("{CONDITIONAL_FILES}{DECLARATION}");
    let target = context(Some(""), &[]);

    let result = scan(&input, &target);

    assert!(result.effects.is_empty());
    assert_eq!(result.rejected.len(), 1, "{:?}", result.rejected);
    assert_eq!(
        result.rejected[0].endpoint.as_deref(),
        Some("demo-pc-x86_64")
    );
    assert!(result.rejected[0]
        .reason
        .contains("source list variable FILES is conditionally assigned"));
}

#[test]
fn source_assignment_before_declaration_overrides_configured_fallback() {
    let input = format!("FILES := local\n{DECLARATION}");
    let target = context(Some("smp"), &[("FILES", "configured")]);

    let result = scan(&input, &target);

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(
        module_sources(&result).as_deref(),
        Some(["local".to_owned()].as_slice())
    );
}

#[test]
fn declaration_does_not_borrow_a_later_source_assignment() {
    let input = format!("{DECLARATION}FILES := later\n");
    let target = context(Some("smp"), &[]);

    let result = scan(&input, &target);

    assert!(result.effects.is_empty());
    assert_eq!(result.rejected.len(), 1, "{:?}", result.rejected);
    assert_eq!(result.rejected[0].line, 1);
    assert!(result.rejected[0]
        .reason
        .contains("unresolved source list variable FILES"));
}

#[test]
fn unknown_conditional_assignment_cannot_borrow_configured_fallback() {
    let input = format!("ifeq ($(UNKNOWN_FEATURE),1)\nFILES := branch\nendif\n{DECLARATION}");
    let target = context(Some("smp"), &[("FILES", "configured")]);

    let result = scan(&input, &target);

    assert!(result.effects.is_empty());
    assert_eq!(result.rejected.len(), 1, "{:?}", result.rejected);
    assert_eq!(
        result.rejected[0].endpoint.as_deref(),
        Some("demo-pc-x86_64")
    );
    assert!(result.rejected[0]
        .reason
        .contains("source list variable FILES is conditionally assigned"));
}

#[test]
fn multiline_scope_and_directive_keep_physical_line_indexes() {
    let input = concat!(
        "ifeq ($(AROS_TARGET_VARIANT),excluded)\n",
        "FILES := excluded\n",
        "endif\n",
        "FILES := \\\n",
        "  base \\\n",
        "  helper\n",
        "%build_archspecific \\\n",
        "  mainmmake=demo maindir=rom/demo modname=demo \\\n",
        "  arch=pc-x86_64 files=\"$(FILES)\"\n",
    );
    let target = context(Some("smp"), &[]);

    let result = scan(input, &target);

    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert_eq!(result.effects.len(), 2);
    assert!(result.effects.iter().all(|effect| effect.line == 7));
    assert_eq!(
        module_sources(&result).as_deref(),
        Some(["base".to_owned(), "helper".to_owned()].as_slice())
    );
}

#[test]
fn closed_include_flag_catalog_proves_an_empty_lookup_only_when_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path()).unwrap();
    let recipe = Path::new("compiler/include/mmakefile.src");
    std::fs::create_dir_all(root.join("compiler/include")).unwrap();
    let source = concat!(
        "%get_archincludes modname=exec \\\n",
        "    includeflag=TARGET_EXEC_INCLUDES maindir=compiler/include\n",
        "PRIV := $(TARGET_EXEC_INCLUDES) -Ibase\n",
    );
    std::fs::write(root.join(recipe), source).unwrap();
    let dirs = crate::dirs::DirVars::load(&root);

    let open = context(Some(""), &[]);
    let refused =
        crate::assembly_headers::native_configuration_snapshot(source, &open, &dirs, &root, recipe)
            .err()
            .expect("an open catalog cannot prove the lookup empty");
    assert!(
        refused.contains("no matching source-proved providers"),
        "{refused}"
    );

    let mut closed = context(Some(""), &[]);
    closed.native_arch_include_catalog_closed = true;
    let snapshot = crate::assembly_headers::native_configuration_snapshot(
        source, &closed, &dirs, &root, recipe,
    )
    .unwrap();
    assert!(!snapshot.joined.contains("%get_archincludes"));
    let (scope, _) = crate::make_vars::collect_vars_impl(&snapshot.joined, Some(&closed));
    assert_eq!(scope.raw_at("PRIV", usize::MAX).as_deref(), Some("-Ibase"));
    assert_eq!(snapshot.physical_owner_lines[1], Some(2));

    let probing = format!("{source}ifdef TARGET_EXEC_INCLUDES\nX := 1\nendif\n");
    let refused = crate::assembly_headers::native_configuration_snapshot(
        &probing, &closed, &dirs, &root, recipe,
    )
    .err()
    .expect("a definedness test cannot be answered by an empty append");
    assert!(refused.contains("tested for definition"), "{refused}");
}
