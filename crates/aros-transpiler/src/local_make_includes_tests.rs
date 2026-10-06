use super::{
    inline_native_make_configuration, inline_native_make_configuration_with_templates,
    LocalMakeIncludeLimits,
};
use super::{is_local_candidate, parse_include_directive};
use aros_common::native_make_template::ResolvedGeneratedMakeTemplate;
use std::collections::BTreeMap;
use std::path::Path;

fn native_fixture() -> (tempfile::TempDir, BTreeMap<String, String>) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("config")).unwrap();
    std::fs::create_dir_all(directory.path().join("arch/native")).unwrap();
    std::fs::write(
        directory.path().join("config/aros.cfg"),
        "include generated/target.cfg\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("arch/native/compile.mk"),
        "undefine UNUSED\nFLAGS = -DFLAG=1 -UFLAG -DFLAG=2 -DFLAG=2\n",
    )
    .unwrap();
    (
        directory,
        BTreeMap::from([("config/aros.cfg".into(), "arch/native/compile.mk".into())]),
    )
}

#[test]
fn native_configuration_expands_only_explicit_regular_endpoints_in_order() {
    let (directory, bindings) = native_fixture();
    let scan = inline_native_make_configuration(
        "BEFORE := first\ninclude $(SRCDIR)/config/aros.cfg\nFLAGS += -DTAIL\n",
        directory.path(),
        std::path::Path::new("compiler/libinit/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &bindings,
    );
    assert!(scan.issues.is_empty(), "{:#?}", scan.issues);
    assert_eq!(scan.fragments.len(), 1);
    assert!(!scan.expanded.contains("include generated/target.cfg"));
    assert!(!scan.expanded.contains("include $(SRCDIR)/config/aros.cfg"));
    assert!(scan.expanded.contains("undefine UNUSED"));
    assert!(scan.expanded.contains("-DFLAG=1 -UFLAG -DFLAG=2 -DFLAG=2"));
    assert!(scan.expanded.find("BEFORE").unwrap() < scan.expanded.find("FLAGS =").unwrap());
    assert!(scan.expanded.find("FLAGS =").unwrap() < scan.expanded.find("FLAGS +=").unwrap());
}

#[test]
fn native_configuration_keeps_unbound_or_unsafe_includes_explicit() {
    let (directory, bindings) = native_fixture();
    let input = "include $(SRCDIR)/config/aros.cfg\n";
    let unbound = inline_native_make_configuration(
        input,
        directory.path(),
        std::path::Path::new("compiler/libinit/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
    );
    assert_eq!(unbound.expanded, input);
    for body in [
        "FLAGS = -DGOOD\ninclude generated/target.cfg\n",
        "FLAGS = $(shell touch sentinel)\n",
        "entry.o : entry.c\n\tcc -c entry.c\n",
    ] {
        std::fs::write(directory.path().join("arch/native/compile.mk"), body).unwrap();
        let scan = inline_native_make_configuration(
            input,
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(!scan.issues.is_empty());
        assert_eq!(scan.expanded, input);
        assert!(scan.fragments.is_empty());
        assert!(!directory.path().join("sentinel").exists());
    }
}

#[test]
fn native_configuration_refuses_missing_endpoints_limits_and_bad_replacements() {
    let (directory, mut bindings) = native_fixture();
    for replacement in [
        "../compile.mk",
        "/tmp/compile.mk",
        "arch/native/compile.cfg",
        "arch/native/missing.mk",
        "arch/native/$(NAME).mk",
    ] {
        bindings.insert("config/aros.cfg".into(), replacement.into());
        let scan = inline_native_make_configuration(
            "include $(SRCDIR)/config/aros.cfg\n",
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(!scan.issues.is_empty(), "{replacement}");
        assert!(scan.fragments.is_empty());
    }
    bindings.insert("config/aros.cfg".into(), "arch/native/compile.mk".into());
    let scan = inline_native_make_configuration(
        "include $(SRCDIR)/config/aros.cfg\n",
        directory.path(),
        std::path::Path::new("compiler/libinit/mmakefile.src"),
        LocalMakeIncludeLimits {
            bytes: 4,
            ..LocalMakeIncludeLimits::default()
        },
        &bindings,
    );
    assert!(!scan.issues.is_empty());
    assert!(scan.fragments.is_empty());
    std::fs::remove_file(directory.path().join("config/aros.cfg")).unwrap();
    let scan = inline_native_make_configuration(
        "-include $(SRCDIR)/config/aros.cfg\n",
        directory.path(),
        std::path::Path::new("compiler/libinit/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &bindings,
    );
    assert!(!scan.issues.is_empty());
    assert!(scan.fragments.is_empty());
}

#[cfg(unix)]
#[test]
fn native_configuration_refuses_source_and_projection_symlinks() {
    use std::os::unix::fs::symlink;
    let (directory, bindings) = native_fixture();
    for path in ["config/aros.cfg", "arch/native/compile.mk"] {
        let original = directory.path().join(path);
        let saved = original.with_extension("saved");
        std::fs::rename(&original, &saved).unwrap();
        symlink(&saved, &original).unwrap();
        let scan = inline_native_make_configuration(
            "include $(SRCDIR)/config/aros.cfg\n",
            directory.path(),
            std::path::Path::new("compiler/libinit/mmakefile.src"),
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(!scan.issues.is_empty(), "{path}");
        assert!(scan.fragments.is_empty());
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&saved, &original).unwrap();
    }
}

fn resolved_geninc_template() -> BTreeMap<String, ResolvedGeneratedMakeTemplate> {
    BTreeMap::from([(
        "compiler/include/geninc.cfg".into(),
        ResolvedGeneratedMakeTemplate {
            template_relative: "compiler/include/geninc.cfg.in".into(),
            expanded_text: "%common\nEXECSMP=\"\"\n".into(),
            substitutions: BTreeMap::from([("@ENABLE_EXECSMP@".into(), String::new())]),
        },
    )])
}

#[test]
fn generated_template_uses_only_sealed_text_and_preserves_provenance() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("compiler/include")).unwrap();
    std::fs::write(
        directory.path().join("compiler/include/geninc.cfg.in"),
        "%common\nEXECSMP=\"@ENABLE_EXECSMP@\"\n",
    )
    .unwrap();
    let generated = directory.path().join("compiler/include/geninc.cfg");
    assert!(!generated.exists());

    let scan = inline_native_make_configuration_with_templates(
        "include $(TOP)/$(CURDIR)/geninc.cfg\nAFTER=present\n",
        directory.path(),
        Path::new("compiler/include/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
        &resolved_geninc_template(),
    );

    assert!(scan.issues.is_empty(), "{:#?}", scan.issues);
    assert_eq!(scan.fragments.len(), 1);
    assert_eq!(
        scan.fragments[0].path,
        Path::new("compiler/include/geninc.cfg.in")
    );
    assert_eq!(
        scan.fragments[0].generated_output.as_deref(),
        Some(Path::new("compiler/include/geninc.cfg"))
    );
    assert_eq!(
        scan.fragments[0].template_substitutions.as_ref().unwrap()["@ENABLE_EXECSMP@"],
        ""
    );
    assert!(scan.expanded.contains("%common\nEXECSMP=\"\"\n"));
    assert!(scan.expanded.contains("AFTER=present"));
    assert!(!scan
        .expanded
        .contains("include $(TOP)/$(CURDIR)/geninc.cfg"));
    assert!(!generated.exists());
}

#[test]
fn generated_template_never_reads_a_hostile_existing_output() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("compiler/include")).unwrap();
    std::fs::write(
        directory.path().join("compiler/include/geninc.cfg"),
        "$(shell touch should-not-run)\n",
    )
    .unwrap();
    let scan = inline_native_make_configuration_with_templates(
        "include $(TOP)/$(CURDIR)/geninc.cfg\n",
        directory.path(),
        Path::new("compiler/include/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
        &resolved_geninc_template(),
    );

    assert!(scan.issues.is_empty(), "{:#?}", scan.issues);
    assert!(scan.expanded.contains("EXECSMP=\"\""));
    assert!(!scan.expanded.contains("should-not-run"));
    assert!(!directory.path().join("should-not-run").exists());
}

#[test]
fn generated_template_without_binding_is_left_visible_and_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let input = "include $(TOP)/$(CURDIR)/geninc.cfg\n";
    let scan = inline_native_make_configuration(
        input,
        directory.path(),
        Path::new("compiler/include/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
    );
    assert_eq!(scan.expanded, input);
    assert!(scan.fragments.is_empty());
    assert_eq!(scan.issues.len(), 1);
    assert_eq!(
        scan.issues[0].kind,
        super::LocalMakeIncludeIssueKind::UnresolvedPath
    );
}

#[test]
fn generated_template_rejects_overlapping_keys_and_resource_overruns_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let input = "include $(TOP)/$(CURDIR)/geninc.cfg\n";
    let mut overlapping = BTreeMap::new();
    overlapping.insert(
        "compiler/include/geninc.cfg".into(),
        "compiler/include/other.mk".into(),
    );
    let scan = inline_native_make_configuration_with_templates(
        input,
        directory.path(),
        Path::new("compiler/include/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &overlapping,
        &resolved_geninc_template(),
    );
    assert_eq!(scan.expanded, input);
    assert!(scan.fragments.is_empty());
    assert_eq!(scan.issues.len(), 1);

    let templates = resolved_geninc_template();
    for limits in [
        LocalMakeIncludeLimits {
            files: 0,
            ..LocalMakeIncludeLimits::default()
        },
        LocalMakeIncludeLimits {
            bytes: 4,
            ..LocalMakeIncludeLimits::default()
        },
    ] {
        let scan = inline_native_make_configuration_with_templates(
            input,
            directory.path(),
            Path::new("compiler/include/mmakefile.src"),
            limits,
            &BTreeMap::new(),
            &templates,
        );
        assert_eq!(scan.expanded, input);
        assert!(scan.fragments.is_empty());
        assert_eq!(scan.issues.len(), 1);
    }
}

#[test]
fn generated_template_rejects_noncanonical_paths() {
    let directory = tempfile::tempdir().unwrap();
    let mut templates = resolved_geninc_template();
    let value = templates.remove("compiler/include/geninc.cfg").unwrap();
    templates.insert("compiler/include/../geninc.cfg".into(), value);
    let input = "include $(TOP)/$(CURDIR)/geninc.cfg\n";
    let scan = inline_native_make_configuration_with_templates(
        input,
        directory.path(),
        Path::new("compiler/include/mmakefile.src"),
        LocalMakeIncludeLimits::default(),
        &BTreeMap::new(),
        &templates,
    );
    assert_eq!(scan.expanded, input);
    assert!(scan.fragments.is_empty());
    assert_eq!(scan.issues.len(), 1);
}

#[test]
fn literal_source_root_cfg_is_local_but_global_and_dynamic_scopes_are_not() {
    assert!(is_local_candidate("$(SRCDIR)/workbench/libs/mesa/mesa.cfg"));
    assert!(is_local_candidate("$(SRCDIR)/$(CURDIR)/sources.inc"));
    assert!(!is_local_candidate("$(SRCDIR)/config/aros.cfg"));
    assert!(!is_local_candidate(
        "$(SRCDIR)/tools/crosstools/$(AROS_TOOLCHAIN).cfg"
    ));
}

#[test]
fn an_indented_compiler_include_option_is_not_a_make_include_directive() {
    assert!(parse_include_directive("include $(SRCDIR)/workbench/libs/mesa/mesa.cfg").is_some());
    assert!(
        parse_include_directive("    -include $(SRCDIR)/$(CURDIR)/v3d_aros_override.h").is_none()
    );
}
