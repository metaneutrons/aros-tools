use crate::local_make_includes::{
    inline_native_make_configuration_for_context, LocalMakeIncludeLimits,
};
use crate::TargetContext;
use std::fs;
use std::path::Path;

const OPTIONS: &str = "arch/x86_64-all/utility/make.opts";
const RECIPE: &str = "arch/x86_64-all/utility/mmakefile.src";

fn fixture() -> (tempfile::TempDir, TargetContext) {
    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("arch/x86_64-all/utility")).unwrap();
    let mut context = TargetContext {
        cpu: Some("x86_64".into()),
        platform: Some("pc".into()),
        native_kernel_sources_in_target_role: true,
        ..TargetContext::default()
    };
    seal(
        tree.path(),
        &mut context,
        OPTIONS,
        "USER_CPPFLAGS := -DUSE_SSE_COPYMEM\n",
    );
    (tree, context)
}

fn seal(root: &Path, context: &mut TargetContext, name: &str, text: &str) {
    fs::create_dir_all(root.join(name).parent().unwrap()).unwrap();
    fs::write(root.join(name), text).unwrap();
    context.native_configuration_inputs.insert(
        name.into(),
        aros_common::sha256_bytes(text.as_bytes()).to_string(),
    );
}

fn scan(root: &Path, context: &TargetContext, text: &str) -> crate::LocalMakeIncludeScan {
    inline_native_make_configuration_for_context(
        text,
        root,
        Path::new(RECIPE),
        LocalMakeIncludeLimits::default(),
        context,
    )
}

#[test]
fn legacy_fragment_decoding_preserves_original_byte_seals_and_syntax_guards() {
    let (tree, mut context) = fixture();
    let original = b"# Copyright \xa9\nUSER_CPPFLAGS := -DFIRST -UFIRST -DFIRST\n";
    fs::write(tree.path().join(OPTIONS), original).unwrap();
    context.native_configuration_inputs.insert(
        OPTIONS.into(),
        aros_common::sha256_bytes(original).to_string(),
    );
    let input = format!("include $(SRCDIR)/{OPTIONS}\n");
    let accepted = scan(tree.path(), &context, &input);
    assert!(accepted.issues.is_empty(), "{:?}", accepted.issues);
    assert!(accepted.expanded.contains("# Copyright ©"));
    assert!(accepted.expanded.contains("-DFIRST -UFIRST -DFIRST"));

    // The equivalent displayed UTF-8 text has different original bytes and
    // cannot satisfy the retained Latin-1 source seal.
    fs::write(
        tree.path().join(OPTIONS),
        aros_common::decode_source_bytes(original.to_vec()),
    )
    .unwrap();
    assert!(!scan(tree.path(), &context, &input).issues.is_empty());

    let unsafe_bytes = b"# Copyright \xa9\nUSER_CPPFLAGS := $(shell touch sentinel)\n";
    fs::write(tree.path().join(OPTIONS), unsafe_bytes).unwrap();
    context.native_configuration_inputs.insert(
        OPTIONS.into(),
        aros_common::sha256_bytes(unsafe_bytes).to_string(),
    );
    let refused = scan(tree.path(), &context, &input);
    assert!(!refused.issues.is_empty());
    assert_eq!(refused.expanded, input);
    assert!(refused.fragments.is_empty());
    assert!(!tree.path().join("sentinel").exists());
}

#[test]
fn exact_optional_absence_and_sealed_assignment_select_sources_in_order() {
    let (tree, context) = fixture();
    let text = concat!(
        "-include $(SRCDIR)/arch/$(CPU)-$(ARCH)/utility/make.opts\n",
        "-include $(SRCDIR)/arch/$(CPU)-all/utility/make.opts\n",
        "FILES := utility_archinit\n",
        "ifneq (,$(findstring DUSE_SSE_COPYMEM,$(USER_CPPFLAGS)))\n",
        "FILES += setmem\nendif\n",
        "%build_archspecific mainmmake=kernel-utility modname=utility maindir=rom/utility arch=x86_64 files=$(FILES)\n",
    );
    fs::write(tree.path().join(RECIPE), text).unwrap();
    let result = scan(tree.path(), &context, text);
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments.len(), 2);
    assert!(result.fragments[0].is_proven_absent());
    assert_eq!(
        result.fragments[0].path,
        Path::new("arch/x86_64-pc/utility/make.opts")
    );
    assert!(!result.fragments[1].is_proven_absent());
    let dirs = crate::dirs::DirVars::default();
    let joined = crate::parser::join_continuations(&result.expanded);
    let (scope, states) = crate::make_vars::collect_vars_impl(&joined, Some(&context));
    let (sources, errors) = crate::arch_sources::collect_arch_sources_with_scope(
        &joined,
        Path::new("arch/x86_64-all/utility"),
        Some(&context),
        &scope,
        &states,
        &dirs,
        tree.path(),
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].files, ["utility_archinit", "setmem"]);
    let snapshot = crate::assembly_headers::native_configuration_snapshot(
        text,
        &context,
        &dirs,
        tree.path(),
        Path::new(RECIPE),
    )
    .unwrap();
    assert_eq!(snapshot.physical_owner_lines.last(), Some(&Some(6)));
    assert!(snapshot.physical_owner_lines[1].is_none());
}

#[test]
fn later_include_replaces_earlier_assignment_without_globbing() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "arch/x86_64-pc/utility/make.opts",
        "USER_CPPFLAGS := -DFIRST\n",
    );
    // An unsealed other-architecture file cannot enter the selected scope.
    fs::create_dir_all(tree.path().join("arch/arm-all/utility")).unwrap();
    fs::write(
        tree.path().join("arch/arm-all/utility/make.opts"),
        "USER_CPPFLAGS := -DDECOY\n",
    )
    .unwrap();
    let result = scan(
        tree.path(),
        &context,
        concat!(
            "include $(SRCDIR)/arch/$(CPU)-$(ARCH)/utility/make.opts\n",
            "include $(SRCDIR)/arch/$(CPU)-all/utility/make.opts\n"
        ),
    );
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    let (scope, _) = crate::make_vars::collect_vars_impl(&result.expanded, Some(&context));
    assert_eq!(
        scope.raw_at("USER_CPPFLAGS", usize::MAX).as_deref(),
        Some("-DUSE_SSE_COPYMEM")
    );
    assert!(!result.expanded.contains("DECOY"));
}

#[test]
fn include_path_uses_preceding_not_later_assignment() {
    let (tree, context) = fixture();
    let result = scan(tree.path(), &context,
        "CHOICE := x86_64-all\ninclude $(SRCDIR)/arch/$(CHOICE)/utility/make.opts\nCHOICE := missing\n");
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments[0].path, Path::new(OPTIONS));
}

#[test]
fn missing_required_include_refuses_at_physical_line() {
    let (tree, context) = fixture();
    let result = scan(
        tree.path(),
        &context,
        "# physical line one\ninclude $(SRCDIR)/arch/$(CPU)-$(ARCH)/utility/make.opts\n",
    );
    assert_eq!(result.issues.len(), 1);
    assert_eq!(result.issues[0].line, 2);
    assert!(result.issues[0].detail.contains("required"));
}

#[test]
fn unsealed_or_changed_present_bytes_never_enter_scope() {
    let (tree, mut context) = fixture();
    context.native_configuration_inputs.clear();
    for context in [context, fixture().1] {
        fs::write(tree.path().join(OPTIONS), "USER_CPPFLAGS := -DCHANGED\n").unwrap();
        let result = scan(
            tree.path(),
            &context,
            "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
        );
        assert_eq!(result.issues.len(), 1);
        assert!(result.issues[0]
            .detail
            .contains("unsealed or its bytes changed"));
        assert!(!result.expanded.contains("-DCHANGED"));
    }
}

#[test]
fn optional_absence_is_rechecked_when_file_appears() {
    let (tree, context) = fixture();
    let text = "-include $(SRCDIR)/arch/x86_64-pc/utility/make.opts\n";
    let result = scan(tree.path(), &context, text);
    assert!(result.issues.is_empty());
    let root = tree.path().canonicalize().unwrap();
    super::verify_absent(&root, &result.fragments[0].path).unwrap();
    fs::create_dir_all(root.join("arch/x86_64-pc/utility")).unwrap();
    fs::write(
        root.join("arch/x86_64-pc/utility/make.opts"),
        "VALUE := appeared\n",
    )
    .unwrap();
    assert!(super::verify_absent(&root, &result.fragments[0].path).is_err());
    assert!(crate::parser::architecture_scope_positions(
        &root,
        Path::new(RECIPE),
        text,
        &result,
        &result.expanded
    )
    .is_none());
    assert!(!scan(&root, &context, text).issues.is_empty());
}

#[test]
fn variable_uncertainty_wildcards_and_traversal_refuse() {
    let (tree, context) = fixture();
    for text in [
        "-include $(SRCDIR)/arch/$(UNKNOWN)/make.opts\n",
        "-include $(SRCDIR)/arch/*/make.opts\n",
        "-include $(SRCDIR)/../outside/make.opts\n",
        "-include $(SRCDIR)/arch/$(shell echo x)/make.opts\n",
        "-include $(SRCDIR)/a.opts $(SRCDIR)/b.opts\n",
        "ifeq ($(UNKNOWN),yes)\n-include $(SRCDIR)/arch/absent/make.opts\nendif\n",
        "ifeq ($(UNKNOWN),yes)\nCHOICE := x86_64-all\nendif\n-include $(SRCDIR)/arch/$(CHOICE)/utility/make.opts\n",
    ] {
        let result = scan(tree.path(), &context, text);
        assert!(!result.issues.is_empty(), "unsafe include admitted: {text}");
        assert!(result.fragments.is_empty());
    }
}

#[test]
fn known_inactive_include_remains_inactive_without_file_access() {
    let (tree, context) = fixture();
    let text = "ifeq ($(CPU),arm)\ninclude $(SRCDIR)/missing/make.opts\nendif\nVALUE := yes\n";
    let result = scan(tree.path(), &context, text);
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert!(result.fragments.is_empty());
    assert_eq!(result.expanded, text);
}

#[test]
fn sealed_options_cannot_introduce_recipe_or_metamake_owner() {
    let (tree, mut context) = fixture();
    for body in [
        "#MM foreign-owner :\n",
        "%build_prog mmake=foreign files=unit\n",
        "foreign: input\n\techo injected\n",
        "VALUE := $(shell echo injected)\n",
    ] {
        seal(tree.path(), &mut context, OPTIONS, body);
        let result = scan(
            tree.path(),
            &context,
            "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
        );
        assert!(
            !result.issues.is_empty(),
            "unsafe fragment admitted: {body}"
        );
        assert!(result.fragments.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn optional_symlinks_and_non_directory_ancestors_are_not_absence() {
    let (tree, context) = fixture();
    let root = tree.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(root.join("missing"), root.join("dangling.opts")).unwrap();
    std::os::unix::fs::symlink(root.join("arch"), root.join("alias")).unwrap();
    fs::write(root.join("not-directory"), "file").unwrap();
    for path in [
        "dangling.opts",
        "alias/x86_64-pc/utility/make.opts",
        "not-directory/missing.opts",
    ] {
        let result = scan(&root, &context, &format!("-include $(SRCDIR)/{path}\n"));
        assert!(!result.issues.is_empty(), "false absence admitted: {path}");
    }
}

#[test]
fn nested_sealed_options_see_preceding_parent_scope() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "arch/wrapper.opts",
        "include $(SRCDIR)/arch/$(CHOICE)/utility/make.opts\n",
    );
    let result = scan(
        tree.path(),
        &context,
        "CHOICE := x86_64-all\ninclude $(SRCDIR)/arch/wrapper.opts\n",
    );
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments.len(), 2);
    assert!(result.expanded.contains("-DUSE_SSE_COPYMEM"));
}

#[test]
fn variable_named_include_is_expanded_or_explicitly_refused() {
    let (tree, context) = fixture();
    let result = scan(
        tree.path(),
        &context,
        "OPTIONS := $(SRCDIR)/arch/x86_64-all/utility/make.opts\ninclude $(OPTIONS)\n",
    );
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments[0].path, Path::new(OPTIONS));
    let result = scan(tree.path(), &context, "include $(UNKNOWN)\n");
    assert!(!result.issues.is_empty());
}

#[test]
fn exact_source_options_cannot_be_replaced_by_mk_projection() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "arch/other.mk",
        "USER_CPPFLAGS := -DWRONG\n",
    );
    context
        .make_include_bindings
        .insert(OPTIONS.into(), "arch/other.mk".into());
    let result = scan(
        tree.path(),
        &context,
        "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
    );
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.detail.contains("may not be replaced")));
    assert!(!result.expanded.contains("WRONG"));
}

#[test]
fn optional_absence_obeys_include_budget() {
    let (tree, context) = fixture();
    let result = inline_native_make_configuration_for_context(
        "-include $(SRCDIR)/missing/one.opts\n-include $(SRCDIR)/missing/two.opts\n",
        tree.path(),
        Path::new(RECIPE),
        LocalMakeIncludeLimits {
            files: 1,
            ..LocalMakeIncludeLimits::default()
        },
        &context,
    );
    assert_eq!(result.fragments.len(), 1);
    assert!(result
        .issues
        .iter()
        .any(|issue| issue.kind == crate::LocalMakeIncludeIssueKind::FileLimit));
}

#[test]
fn define_body_does_not_execute_includes_or_create_providers() {
    let (tree, context) = fixture();
    let text = "override private define BODY\ninclude $(SRCDIR)/missing.opts\nendef\n";
    let result = scan(tree.path(), &context, text);
    assert!(result.issues.is_empty());
    assert!(result.fragments.is_empty());
    assert_eq!(result.expanded, text);
}

#[test]
fn architecture_effects_cannot_fallback_after_bad_source_options() {
    let (tree, context) = fixture();
    let text = "include $(SRCDIR)/missing.opts\n%build_archspecific mainmmake=kernel-unit modname=unit maindir=rom/unit arch=x86_64 files=unit\n";
    let result = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
        text,
        Path::new(RECIPE),
        tree.path(),
        Some(&context),
    );
    let result = result.unwrap();
    assert!(result.effects.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0]
        .reason
        .contains("source scope is incomplete"));
    assert_eq!(
        result.rejected[0].endpoint.as_deref(),
        Some("kernel-unit-x86_64")
    );
}

#[test]
fn options_cannot_import_unsealed_non_options_fragment() {
    let (tree, mut context) = fixture();
    fs::write(tree.path().join("arch/sibling.cfg"), "FILES := injected\n").unwrap();
    seal(
        tree.path(),
        &mut context,
        OPTIONS,
        "include $(SRCDIR)/arch/sibling.cfg\n",
    );
    let result = scan(
        tree.path(),
        &context,
        "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
    );
    assert!(!result.issues.is_empty());
    assert!(result.fragments.is_empty());
    assert!(!result.expanded.contains("injected"));
}

#[test]
fn dynamic_sealed_make_fragment_is_not_misclassified_as_options() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "arch/x86_64-all/utility/rules.mk",
        "FILES := generic\n",
    );
    let result = scan(
        tree.path(),
        &context,
        "include $(SRCDIR)/arch/$(CPU)-all/utility/rules.mk\n",
    );
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert!(result.expanded.contains("FILES := generic"));
}

#[test]
fn parser_pipeline_reports_bad_native_configuration_as_capability_failure() {
    let (tree, context) = fixture();
    let text = "include $(SRCDIR)/missing.opts\n#MM native-unit :\n%build_linklib mmake=native-unit modname=unit files=unit\n";
    fs::write(tree.path().join(RECIPE), text).unwrap();
    fs::write(
        tree.path().join("arch/x86_64-all/utility/unit.c"),
        "int unit;\n",
    )
    .unwrap();
    let dirs = crate::dirs::DirVars::default();
    let result = crate::parse_mmakefile_with_dirs_and_context(
        &tree.path().join(RECIPE),
        tree.path(),
        &dirs,
        &context,
    );
    match result {
        Err(error) => assert!(error.to_string().contains("scope is incomplete")),
        Ok(parsed) => assert!(parsed
            .native_graph_errors
            .iter()
            .any(|error| error.message.contains("source configuration is incomplete"))),
    }
}

#[test]
fn variable_suffix_options_name_is_resolved_and_hash_bound() {
    let (tree, context) = fixture();
    let result = scan(
        tree.path(),
        &context,
        "BASE := $(SRCDIR)/arch/x86_64-all/utility/make\nEXT := opts\ninclude $(BASE).$(EXT)\n",
    );
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments[0].path, Path::new(OPTIONS));
}

#[test]
fn native_parser_adopts_nested_options_without_replacement_binding() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "arch/sibling.cfg",
        "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
    );
    let text = concat!("include $(SRCDIR)/arch/sibling.cfg\n",
        "FILES := utility_archinit\n",
        "ifneq (,$(findstring DUSE_SSE_COPYMEM,$(USER_CPPFLAGS)))\n",
        "FILES += setmem\nendif\n",
        "%build_archspecific mainmmake=kernel-utility modname=utility maindir=rom/utility arch=x86_64 files=$(FILES)\n");
    fs::write(tree.path().join(RECIPE), text).unwrap();
    for file in ["utility_archinit.c", "setmem.c"] {
        fs::write(
            tree.path().join("arch/x86_64-all/utility").join(file),
            "int test;\n",
        )
        .unwrap();
    }
    let parsed = crate::parse_mmakefile_with_dirs_and_context(
        &tree.path().join(RECIPE),
        tree.path(),
        &crate::dirs::DirVars::from_config_text(concat!(
            "GENDIR := $(TOP)/gen\n",
            "AROS_LIB := $(TOP)/SYS/Developer/lib\n",
            "AROS_LIB32 := $(TOP)/SYS/Developer/lib/32\n"
        )),
        &context,
    )
    .unwrap();
    assert_eq!(
        parsed.arch_sources.len(),
        1,
        "{:?}",
        parsed.skipped_arch_sources
    );
    assert_eq!(parsed.arch_sources[0].files, ["utility_archinit", "setmem"]);
    assert_eq!(parsed.arch_sources[0].line, 5);
    assert!(
        parsed.native_graph_errors.is_empty(),
        "{:?}",
        parsed.native_graph_errors
    );
}

#[test]
fn remaining_active_nonlocal_include_cannot_lend_stale_values_to_effect() {
    let (tree, context) = fixture();
    let text = concat!(
        "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
        "DIRECTORY := rom/unit\ninclude $(TOP)/config.mk\n",
        "%set_archincludes mainmmake=kernel-unit modname=unit maindir=$(DIRECTORY) arch=x86_64 includes=-I$(SRCDIR)/rom/unit\n");
    let scan = scan(tree.path(), &context, text);
    assert!(scan
        .issues
        .iter()
        .any(|issue| issue.line == 3 && issue.detail.contains("remains unbound")));
    let result = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
        text,
        Path::new(RECIPE),
        tree.path(),
        Some(&context),
    )
    .unwrap();
    assert!(result.effects.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(
        result.rejected[0].endpoint.as_deref(),
        Some("kernel-unit-x86_64-set-archincludes")
    );
}

#[test]
fn optional_sinclude_synonyms_expand_sealed_options_and_prove_absence() {
    let (tree, context) = fixture();
    for directive in ["sinclude", "-sinclude"] {
        let text = format!("{directive} $(SRCDIR)/missing.opts\n{directive} $(SRCDIR)/arch/x86_64-all/utility/make.opts\n");
        let result = scan(tree.path(), &context, &text);
        assert!(result.issues.is_empty(), "{:?}", result.issues);
        assert!(result.fragments[0].is_proven_absent());
        assert_eq!(result.fragments[1].path, Path::new(OPTIONS));
        assert!(result.expanded.contains("-DUSE_SSE_COPYMEM"));
    }
}

#[test]
fn optional_sinclude_nonlocal_paths_cannot_evade_residual_guard() {
    let (tree, context) = fixture();
    for directive in ["sinclude", "-sinclude"] {
        let text = format!("include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n{directive} $(TOP)/config.mk\n%set_archincludes mainmmake=kernel-unit modname=unit maindir=rom/unit arch=x86_64 includes=-I$(SRCDIR)/rom/unit\n");
        let result = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
            &text,
            Path::new(RECIPE),
            tree.path(),
            Some(&context),
        )
        .unwrap();
        assert!(result.effects.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0].reason.contains("remains unbound"));
    }
}

#[test]
fn independently_proved_plain_inventory_is_not_displaced_by_native_options_scope() {
    let (tree, context) = fixture();
    let dir = tree.path().join("arch/x86_64-all/utility");
    fs::write(dir.join("inventory.files"), "FILES := unit helper\n").unwrap();
    for file in ["unit.c", "helper.c"] {
        fs::write(dir.join(file), "int unit;\n").unwrap();
    }
    let text = "include $(SRCDIR)/$(CURDIR)/inventory.files\n%build_linklib mmake=native-library libname=unit files=$(FILES)\n";
    fs::write(tree.path().join(RECIPE), text).unwrap();
    let dirs = crate::dirs::DirVars::from_config_text(concat!(
        "GENDIR := $(TOP)/gen\nAROS_LIB := $(TOP)/SYS/Developer/lib\n",
        "AROS_LIB32 := $(TOP)/SYS/Developer/lib/32\n"
    ));
    let parsed = crate::parse_mmakefile_with_dirs_and_context(
        &dir.join("mmakefile.src"),
        tree.path(),
        &dirs,
        &context,
    )
    .unwrap();
    let target = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "native-library")
        .expect("the source-owned plain inventory remains available");
    assert_eq!(target.source_files, ["unit", "helper"]);
    assert!(!parsed
        .native_graph_errors
        .iter()
        .any(|error| error.message.contains("source configuration is incomplete")));
}

#[test]
fn resolved_variable_non_options_include_does_not_select_options_scope() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "arch/rules.mk",
        "FLAGS := -DUNIT\n",
    );
    let text = "BASE := $(SRCDIR)/arch/rules.mk\ninclude $(BASE)\n";
    let result = scan(tree.path(), &context, text);
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments[0].path, Path::new("arch/rules.mk"));
    assert!(!super::contains_options(&result));
    let unresolved = scan(tree.path(), &context, "include $(UNKNOWN)\n");
    assert!(!unresolved.issues.is_empty());
    assert!(super::contains_options(&unresolved));
    let options = scan(
        tree.path(),
        &context,
        "BASE := $(SRCDIR)/arch/x86_64-all/utility/make.opts\ninclude $(BASE)\n",
    );
    assert!(options.issues.is_empty(), "{:?}", options.issues);
    assert!(super::contains_options(&options));
}

#[test]
fn inactive_options_do_not_displace_independent_plain_scope() {
    let (tree, context) = fixture();
    let result = scan(
        tree.path(),
        &context,
        "ifeq (x,y)\ninclude $(SRCDIR)/missing.opts\nendif\n",
    );
    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert!(!super::contains_options(&result));
}

#[test]
fn options_before_non_options_in_multi_file_include_cannot_disappear() {
    let (tree, context) = fixture();
    for tail in [
        "arch/a.opts arch/b.mk",
        "$(SRCDIR)/arch/a.opts $(SRCDIR)/arch/b.mk",
        "$(UNKNOWN) arch/b.mk",
        "arch/b.mk arch/a.opts",
    ] {
        let text = format!("include {tail}\n");
        let result = scan(tree.path(), &context, &text);
        assert!(!result.issues.is_empty(), "unsafe list admitted: {tail}");
        assert!(result.fragments.is_empty());
        assert!(
            super::contains_options(&result),
            "options discarded: {tail}"
        );
        fs::write(
            tree.path().join(RECIPE),
            format!("{text}%build_linklib mmake=unit libname=unit files=unit\n"),
        )
        .unwrap();
        let parsed = crate::parse_mmakefile_with_dirs_and_context(
            &tree.path().join(RECIPE),
            tree.path(),
            &crate::dirs::DirVars::default(),
            &context,
        )
        .unwrap();
        assert!(
            parsed
                .native_graph_errors
                .iter()
                .any(|error| error.message.contains("source configuration is incomplete")),
            "list error was dropped: {tail}"
        );
    }
}

#[test]
fn rejected_variable_with_non_options_suffix_cannot_hide_options_list() {
    let (tree, context) = fixture();
    let text = concat!(
        "BASE := $(SRCDIR)/arch/x86_64-all/utility/make.opts $(SRCDIR)/arch/x86_64-all/utility/base\n",
        "include $(BASE).mk\n",
        "%build_linklib mmake=unit libname=unit files=unit\n");
    let result = scan(tree.path(), &context, text);
    assert!(!result.issues.is_empty());
    assert!(result.fragments.is_empty());
    assert!(super::contains_options(&result));
    fs::write(tree.path().join(RECIPE), text).unwrap();
    let parsed = crate::parse_mmakefile_with_dirs_and_context(
        &tree.path().join(RECIPE),
        tree.path(),
        &crate::dirs::DirVars::default(),
        &context,
    )
    .unwrap();
    assert!(parsed
        .native_graph_errors
        .iter()
        .any(|error| error.message.contains("source configuration is incomplete")));
}

#[test]
fn sealed_generated_include_path_is_not_misclassified_as_source_options() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "config/make.cfg.in",
        "GENDIR := $(TOP)/gen\n",
    );
    let text = "-include $(GENDIR)/$(CURDIR)/unit.entrypoints\n";
    let result = scan(tree.path(), &context, text);
    assert!(!result.issues.is_empty()); // This belongs to the generated-header capability.
    assert!(!super::contains_options(&result));
    fs::write(
        tree.path().join("config/make.cfg.in"),
        "GENDIR := $(SRCDIR)/other\n",
    )
    .unwrap();
    let corrupt = scan(tree.path(), &context, text);
    assert!(!corrupt.issues.is_empty());
    assert!(corrupt
        .issues
        .iter()
        .any(|issue| issue.detail.contains("bytes changed")));
    let unknown = scan(tree.path(), &context, "include $(UNKNOWN).mk\n");
    assert!(!unknown.issues.is_empty());
    assert!(super::contains_options(&unknown));
}

#[test]
fn sealed_directory_snapshot_consumes_the_shared_read_budget() {
    let (tree, mut context) = fixture();
    seal(
        tree.path(),
        &mut context,
        "config/make.cfg.in",
        "GENDIR := $(TOP)/gen\n",
    );
    let result = inline_native_make_configuration_for_context(
        "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
        tree.path(),
        Path::new(RECIPE),
        LocalMakeIncludeLimits {
            files: 1,
            ..LocalMakeIncludeLimits::default()
        },
        &context,
    );
    assert!(!result.issues.is_empty());
    assert!(result.fragments.is_empty());
    let result = inline_native_make_configuration_for_context(
        "include $(SRCDIR)/arch/x86_64-all/utility/make.opts\n",
        tree.path(),
        Path::new(RECIPE),
        LocalMakeIncludeLimits {
            bytes: 4,
            ..LocalMakeIncludeLimits::default()
        },
        &context,
    );
    assert!(!result.issues.is_empty());
    assert!(result.fragments.is_empty());
}
