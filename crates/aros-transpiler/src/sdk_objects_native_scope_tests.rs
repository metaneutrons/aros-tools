use super::{collect_with_native_configuration, SdkObjectSnapshot};
use crate::dirs::DirVars;
use crate::make_vars::collect_vars_impl;
use crate::parser::{join_continuations, macro_invocations, TargetContext};
use std::fs;
use std::path::PathBuf;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    relative: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().to_path_buf();
        let relative = PathBuf::from("compiler/startup");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join(&relative)).unwrap();
        fs::write(
            root.join("config/make.cfg.in"),
            concat!(
                "AROS_LIB := ${AROS_BUILD_DIR}/SYS/Developer/lib\n",
                "AROS_LIB32 := ${AROS_BUILD_DIR}/SYS/Developer/lib/32\n",
            ),
        )
        .unwrap();
        fs::write(
            root.join("config/aros.cfg"),
            "# source configuration replaced by an explicit binding\n",
        )
        .unwrap();
        Self {
            _temporary: temporary,
            root,
            relative,
        }
    }

    fn source(&self, basename: &str, extension: &str) {
        fs::write(
            self.root
                .join(&self.relative)
                .join(format!("{basename}.{extension}")),
            "int sdk_object_fixture;\n",
        )
        .unwrap();
    }

    fn write_native_config(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn target_with_binding(config: &str) -> TargetContext {
        TargetContext {
            make_include_bindings: std::iter::once(("config/aros.cfg".into(), config.into()))
                .collect(),
            ..TargetContext::default()
        }
    }

    fn collect_native(
        &self,
        physical_source: &str,
        target: Option<&TargetContext>,
    ) -> (
        Vec<super::SdkObjectGroupDecl>,
        Vec<super::SdkObjectRejection>,
    ) {
        self.collect_native_roles(physical_source, target, &[])
    }

    fn collect_native_roles(
        &self,
        physical_source: &str,
        target: Option<&TargetContext>,
        roles: &[aros_common::SdkCompanionRole],
    ) -> (
        Vec<super::SdkObjectGroupDecl>,
        Vec<super::SdkObjectRejection>,
    ) {
        fs::write(
            self.root.join(&self.relative).join("mmakefile.src"),
            physical_source,
        )
        .unwrap();
        let text = join_continuations(physical_source);
        let (scope, line_states) = collect_vars_impl(&text, target);
        let invocations = macro_invocations(&text);
        let legacy = SdkObjectSnapshot {
            invocations: &invocations,
            scope: &scope,
            line_states: Some(&line_states),
            text: &text,
            physical_positions: None,
        };
        let mut dirs = DirVars::load(&self.root);
        dirs.bind_sdk_companion_roles(roles);
        collect_with_native_configuration(
            legacy,
            physical_source,
            target,
            &dirs,
            &self.root,
            &self.relative,
        )
    }

    fn collect_legacy(
        &self,
        physical_source: &str,
        target: Option<&TargetContext>,
    ) -> (
        Vec<super::SdkObjectGroupDecl>,
        Vec<super::SdkObjectRejection>,
    ) {
        let text = join_continuations(physical_source);
        let (scope, line_states) = collect_vars_impl(&text, target);
        let invocations = macro_invocations(&text);
        let dirs = DirVars::load(&self.root);
        super::collect_from_snapshot(
            &invocations,
            &scope,
            &dirs,
            &self.root,
            &self.relative,
            Some(&line_states),
            &text,
        )
    }
}

#[test]
fn explicit_companion_preserves_same_basename_in_distinct_abi_libraries() {
    let fixture = Fixture::new();
    fixture.source("foo", "c");
    fixture.write_native_config(
        "config/make.cfg.in",
        concat!(
            "AROS_DEVELOPER := ${AROS_BUILD_DIR}/SYS/Developer\n",
            "AROS_LIB := $(AROS_DEVELOPER)/lib\n",
            "AROS_LIB32 := $(AROS_DEVELOPER)/lib32\n",
        ),
    );
    fixture.write_native_config(
        "config/native.mk",
        concat!(
            "SDK_INPUTS = $(AROS_LIB)/foo.o $(AROS_LIB32)/foo.o\n",
            "TARGET_ISA_32_CFLAGS = $(NATIVE_TARGET_32_CFLAGS)\n",
            "TARGET_CFLAGS = -O2 -fno-common\n",
        ),
    );
    let target = Fixture::target_with_binding("config/native.mk");
    let source = concat!(
        "include $(SRCDIR)/config/aros.cfg\n",
        "startup-objects: $(SDK_INPUTS)\n",
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)\n",
        "$(AROS_LIB32)/%.o : $(GENDIR)/$(CURDIR)/32bit/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile mmake=companion basename=% targetdir=$(GENDIR)/$(CURDIR)/32bit cflags=\"$(strip $(TARGET_ISA_32_CFLAGS) $(TARGET_CFLAGS))\"\n",
    );
    let role: aros_common::SdkCompanionRole = serde_json::from_value(serde_json::json!({
        "selector_cpu": "i386", "target": {"target_triple": "i386-unknown-aros", "abi": {
            "source_cpu": "i386", "pointer_width": 32, "elf_class": 1, "elf_machine": 3, "endian": "little"
        }}, "compiler_options": ["--target=i386-unknown-aros"], "library_dir": "lib32",
        "required_runtime_library": "clang_rt.builtins-i386"
    })).unwrap();
    let (groups, rejected) =
        fixture.collect_native_roles(source, Some(&target), std::slice::from_ref(&role));
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].objects.len(), 2);
    let companion = groups[0]
        .objects
        .iter()
        .find(|object| object.companion_cpu.is_some())
        .unwrap();
    assert_eq!(companion.companion_cpu.as_deref(), Some("i386"));
    assert!(companion.output.ends_with("/lib32/foo.o"));
    assert!(companion.options.contains(&"-O2".into()));
    let (_, rejected) = fixture.collect_native(source, Some(&target));
    assert!(
        !rejected.is_empty(),
        "CPU32/output spelling must not infer a role"
    );
    fixture.write_native_config(
        "config/native.mk",
        "SDK_INPUTS = $(AROS_LIB32)/foo.o\nTARGET_ISA_32_CFLAGS = -m32\nTARGET_CFLAGS = -O2\n",
    );
    let (_, rejected) = fixture.collect_native_roles(source, Some(&target), &[role]);
    assert!(!rejected.is_empty(), "unbound source ABI flags must fail");
}

fn physical_recipe(compile_macro: &str) -> String {
    format!(
        "include $(SRCDIR)/config/aros.cfg\n\
         startup-objects: $(SDK_INPUTS)\n\
         $(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n\
         \t@$(CP) $< $@\n\
         {compile_macro}\n"
    )
}

fn physical_recipe_with_local_list(compile_macro: &str) -> String {
    format!(
        "SELECTED_OBJECTS = $(AROS_LIB)/foo.o\n\
         include $(SRCDIR)/config/aros.cfg\n\
         startup-objects: $(SELECTED_OBJECTS)\n\
         $(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n\
         \t@$(CP) $< $@\n\
         {compile_macro}\n"
    )
}

#[test]
fn invalid_native_scope_remaps_legacy_fragment_owner_to_physical_source() {
    let fixture = Fixture::new();
    fixture.source("foo", "c");
    fs::write(
        fixture.root.join(&fixture.relative).join("core.files"),
        "FILES := foo\n",
    )
    .unwrap();
    let physical = concat!(
        "include $(SRCDIR)/config/aros.cfg\n",
        "include $(SRCDIR)/$(CURDIR)/core.files\n",
        "startup-objects: $(addprefix $(AROS_LIB)/,$(addsuffix .o,$(FILES)))\n",
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)\n",
    );
    fs::write(
        fixture.root.join(&fixture.relative).join("mmakefile.src"),
        physical,
    )
    .unwrap();
    let recipe = fixture.relative.join("mmakefile.src");
    let scan = crate::local_make_includes::inline_local_make_includes(
        physical,
        &fixture.root,
        &recipe,
        crate::local_make_includes::LocalMakeIncludeLimits::default(),
        crate::local_make_includes::LocalMakeFragmentPolicy::PlainSourceLists,
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    let joined = join_continuations(&scan.expanded);
    let positions = crate::parser::architecture_scope_positions(
        &fixture.root,
        &recipe,
        physical,
        &scan,
        &joined,
    )
    .unwrap();
    let target = Fixture::target_with_binding("config/absent.mk");
    let (scope, states) = collect_vars_impl(&joined, Some(&target));
    let invocations = macro_invocations(&joined);
    let dirs = DirVars::load(&fixture.root);
    let (legacy, errors) = super::collect_from_snapshot(
        &invocations,
        &scope,
        &dirs,
        &fixture.root,
        &fixture.relative,
        Some(&states),
        &joined,
    );
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(legacy.len(), 1);
    assert_ne!(
        legacy[0].line, 3,
        "the inserted fragment must move the legacy slot"
    );
    let (groups, rejected) = collect_with_native_configuration(
        SdkObjectSnapshot {
            invocations: &invocations,
            scope: &scope,
            line_states: Some(&states),
            text: &joined,
            physical_positions: Some(&positions),
        },
        physical,
        Some(&target),
        &dirs,
        &fixture.root,
        &fixture.relative,
    );
    assert!(groups.is_empty());
    assert_eq!(rejected.len(), 1, "{rejected:#?}");
    assert_eq!(rejected[0].owner, "startup-objects");
    assert_eq!(rejected[0].line, 3);
    assert!(rejected[0]
        .reason
        .contains("native SDK configuration is not proved"));
}

#[test]
fn bound_native_sdk_inputs_use_physical_recipe_lines_for_c_and_cxx() {
    for (extension, compile_macro, expected_language) in [
        (
            "c",
            "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)",
            "C",
        ),
        (
            "cpp",
            "%rule_compile_cxx basename=% targetdir=$(GENDIR)/$(CURDIR)",
            "CXX",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.source("foo", extension);
        fixture.write_native_config(
            "config/native.mk",
            concat!(
                "NATIVE_TARGET_CC := clang\n",
                "NATIVE_TARGET_CXX := clang++\n",
                "TARGET_CC := $(NATIVE_TARGET_CC)\n",
                "TARGET_CXX := $(NATIVE_TARGET_CXX)\n",
                "SDK_INPUTS = $(AROS_LIB)/foo.o\n",
            ),
        );
        let target = Fixture::target_with_binding("config/native.mk");
        let source = physical_recipe(compile_macro);

        let (groups, rejected) = fixture.collect_native(&source, Some(&target));

        assert!(rejected.is_empty(), "{extension}: {rejected:#?}");
        let [group] = groups.as_slice() else {
            panic!("one source-owned aggregate for {extension}: {groups:#?}");
        };
        assert_eq!(group.owner, "startup-objects");
        assert_eq!(group.file, "compiler/startup/mmakefile.src");
        assert_eq!(group.line, 2);
        let [object] = group.objects.as_slice() else {
            panic!("one selected object for {extension}: {:#?}", group.objects);
        };
        assert_eq!(object.line, 5);
        assert_eq!(object.language, expected_language);
        assert_eq!(
            object.source,
            format!("${{AROS_SOURCE_DIR}}/compiler/startup/foo.{extension}")
        );
        assert_eq!(
            object.intermediate,
            "${AROS_BUILD_DIR}/gen/compiler/startup/foo.o"
        );
        assert_eq!(object.output, "${AROS_BUILD_DIR}/SYS/Developer/lib/foo.o");
    }
}

#[test]
fn invalid_bound_native_configuration_does_not_fall_back_to_a_local_legacy_list() {
    for (bound_path, config_text) in [
        ("config/missing.mk", None),
        (
            "config/wrong.mk",
            Some("SELECTED_OBJECTS = $(AROS_LIB)/not-the-local-source.o\n"),
        ),
    ] {
        let fixture = Fixture::new();
        fixture.source("foo", "c");
        let source = physical_recipe_with_local_list(
            "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)",
        );
        if let Some(config_text) = config_text {
            fixture.write_native_config(bound_path, config_text);
        }
        let target = Fixture::target_with_binding(bound_path);
        let (legacy_groups, legacy_rejected) = fixture.collect_legacy(&source, Some(&target));
        assert!(
            legacy_rejected.is_empty(),
            "{bound_path}: {legacy_rejected:#?}"
        );
        assert_eq!(legacy_groups.len(), 1, "{bound_path}");
        assert_eq!(
            legacy_groups[0].objects[0].source,
            "${AROS_SOURCE_DIR}/compiler/startup/foo.c"
        );

        let (groups, rejected) = fixture.collect_native(&source, Some(&target));

        assert!(groups.is_empty(), "{bound_path}: {groups:#?}");
        assert!(
            !rejected.is_empty(),
            "{bound_path}: invalid native configuration must be diagnosed"
        );
    }
}

#[test]
fn included_configuration_rules_and_compile_macros_cannot_own_object_producers() {
    let rule = Fixture::new();
    rule.source("foo", "c");
    rule.source("ghost", "c");
    rule.write_native_config("config/native.mk", "foreign-owner: $(AROS_LIB)/ghost.o\n");
    let rule_source =
        physical_recipe_with_local_list("%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)");
    let rule_target = Fixture::target_with_binding("config/native.mk");
    let (groups, rejected) = rule.collect_native(&rule_source, Some(&rule_target));
    assert!(
        groups.is_empty(),
        "included Make rule became an owner: {groups:#?}"
    );
    assert!(
        !rejected.is_empty(),
        "included configuration with a rule must fail closed"
    );

    let compile = Fixture::new();
    compile.source("foo", "c");
    compile.source("ghost", "cpp");
    compile.write_native_config(
        "config/native.mk",
        "%rule_compile_cxx basename=% targetdir=$(GENDIR)/$(CURDIR)/native\n",
    );
    let compile_source = concat!(
        "SELECTED_OBJECTS = $(AROS_LIB)/foo.o\n",
        "include $(SRCDIR)/config/aros.cfg\n",
        "startup-objects: $(SELECTED_OBJECTS)\n",
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)\n",
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/native/%.o\n",
        "\t@$(CP) $< $@\n",
        "foreign-owner: $(AROS_LIB)/ghost.o\n",
    );
    let compile_target = Fixture::target_with_binding("config/native.mk");
    let (groups, rejected) = compile.collect_native(compile_source, Some(&compile_target));
    assert!(
        groups.is_empty(),
        "included %rule_compile became a producer: {groups:#?}"
    );
    assert!(
        !rejected.is_empty(),
        "included configuration with %rule_compile must fail closed"
    );
}

#[test]
fn no_native_bindings_preserve_the_legacy_object_result_exactly() {
    let fixture = Fixture::new();
    fixture.source("foo", "c");
    let source =
        physical_recipe_with_local_list("%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)");
    let target = TargetContext::default();

    let expected = fixture.collect_legacy(&source, Some(&target));
    let actual = fixture.collect_native(&source, Some(&target));

    assert_eq!(actual, expected);
    assert_eq!(actual.0.len(), 1);
    assert!(actual.1.is_empty(), "{:?}", actual.1);
}

#[test]
fn native_aros_lib32_stages_and_prerequisite_roots_remain_unsupported() {
    let fixture = Fixture::new();
    fixture.source("foo", "c");
    fixture.source("bar", "c");
    fixture.write_native_config(
        "config/native.mk",
        concat!(
            "SDK_INPUTS = $(AROS_LIB)/foo.o\n",
            "SDK_INPUTS_32 = $(AROS_LIB32)/bar.o\n",
        ),
    );
    let target = Fixture::target_with_binding("config/native.mk");
    let source = concat!(
        "include $(SRCDIR)/config/aros.cfg\n",
        "startup-objects: $(SDK_INPUTS)\n",
        "unsupported-32-root: $(SDK_INPUTS_32)\n",
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)\n",
        "$(AROS_LIB32)/%.o : $(GENDIR)/$(CURDIR)/32/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)/32\n",
    );

    let (groups, rejected) = fixture.collect_native(source, Some(&target));

    assert_eq!(
        groups.len(),
        1,
        "unsupported AROS_LIB32 lane admitted: {groups:#?}"
    );
    assert_eq!(groups[0].owner, "startup-objects");
    assert_eq!(groups[0].objects.len(), 1);
    assert_eq!(
        groups[0].objects[0].source,
        "${AROS_SOURCE_DIR}/compiler/startup/foo.c"
    );
    assert!(
        rejected.iter().any(|item| {
            item.owner == "unsupported-32-root" && item.reason.contains("unsupported prerequisite")
        }),
        "AROS_LIB32 prerequisite root must remain unsupported: {rejected:#?}"
    );
    assert!(groups
        .iter()
        .all(|group| group.owner != "unsupported-32-root"));
}
