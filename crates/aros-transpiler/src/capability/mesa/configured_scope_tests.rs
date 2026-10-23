use super::{project, GCA_ASSIGNMENT};
use crate::dirs::DirVars;
use crate::parser::TargetContext;
use aros_common::mesa_configuration::{
    MesaConfigurationIdentity, ValidatedMesaConfiguration, MESA_BASELINE_VARIABLES,
    MESA_COMPILER_OPTION_VARIABLES,
};
use aros_common::{sha256_bytes, ArosCompilerIdentity};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const RELATIVE_DIR: &str = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium";
const VC4_MMAKE: &str = "linklibs-gallium_vc4";
const TARGET_FORMULAS: &str = "\
TARGET_CFLAGS = $(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CFLAGS) $(USER_CFLAGS))\n\
TARGET_CXXFLAGS = $(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CXXFLAGS) $(USER_CXXFLAGS))\n\
TARGET_CPPFLAGS = $(strip $(USER_INCLUDES) $(CONFIG_CPPFLAGS) $(USER_CPPFLAGS))\n";
const MAKE_FORMULAS: &str = "\
CFLAGS = $(strip $(TARGET_ISA_CFLAGS) $(TARGET_CFLAGS) $(LLVM_NOWARN))\n\
CXXFLAGS = $(strip $(TARGET_ISA_CXXFLAGS) $(TARGET_CXXFLAGS))\n\
CPPFLAGS = $(strip $(TARGET_CPPFLAGS))\n";

#[test]
fn initial_namespace_is_not_moved_before_uninterpreted_owner_prefix() {
    let first = "# source header\n\ninclude $(SRCDIR)/config/aros.cfg\nLOCAL := value\n";
    assert!(super::remove_aros_wrapper_include(first).is_ok());
    for prefix in [
        "PRE_WRAPPER := value\n",
        "PRE_WRAPPER := $(CC)\n",
        "ifeq ($(CC),value)\nendif\n",
    ] {
        let input = format!("{prefix}{first}");
        assert!(super::remove_aros_wrapper_include(&input)
            .unwrap_err()
            .contains("must precede owner Make statements"));
    }
}

#[test]
fn effective_target_template_includes_cannot_override_reviewed_root_blocks() {
    let source = TempDir::new().unwrap();
    let root = source.path().canonicalize().unwrap();
    let config = root.join("config");
    fs::create_dir(&config).unwrap();
    let blocks = "%define compile_q arg=\noriginal\n%end\n%define rule_compile_multi arg=\noriginal\n%end\n%define build_linklib arg=\noriginal\n%end\n";
    fs::write(
        config.join("make.tmpl"),
        format!("{blocks}%include override.tmpl\n"),
    )
    .unwrap();
    fs::write(
        config.join("override.tmpl"),
        "%define compile_q arg=\nchanged\n%end\n",
    )
    .unwrap();
    let error = super::verify_target_option_templates(&root).unwrap_err();
    assert!(error.contains("hidden or overridden"), "{error}");
    // Removing the override changes the refusal to the actual block digest:
    // locations alone must not qualify synthetic replacement template bytes.
    fs::write(config.join("override.tmpl"), "# no override\n").unwrap();
    assert!(super::verify_target_option_templates(&root)
        .unwrap_err()
        .contains("template changed"));
}

#[test]
#[ignore = "requires an explicit retained real Source/configuration pair"]
fn retained_hidd_gallium_path_projection() {
    let source = PathBuf::from(std::env::var_os("AROS_MESA_PROBE_SOURCE").unwrap());
    let configuration_path =
        PathBuf::from(std::env::var_os("AROS_MESA_PROBE_CONFIGURATION").unwrap());
    let bytes = fs::read(configuration_path).unwrap();
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    let identity = serde_json::from_value(wire["identity"].clone()).unwrap();
    let configuration = ValidatedMesaConfiguration::parse_bound(&bytes, &identity).unwrap();
    let build = source.join("build/arm-raspi");
    let ports = build.join("Ports");
    let mut dirs = DirVars::load(&source);
    dirs.set_materialized_path("AROS_PORTS_DIR", ports.clone());
    let target = TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    };
    let projection = project(
        &source,
        Path::new("workbench/hidds/gallium"),
        "hidd-gallium",
        &dirs,
        &target,
        &configuration,
        (&build, &ports),
    )
    .unwrap();
    let source_context = super::super::configured_source_context::project(
        &source,
        Path::new("workbench/hidds/gallium"),
        "hidd-gallium",
        &dirs,
        &configuration.recipe_configuration().make_coordinates,
        &projection.context_input,
    )
    .unwrap();
    assert_eq!(
        source_context.quote_extra,
        Path::new(
            &configuration
                .recipe_configuration()
                .make_coordinates
                .build_root
        )
        .join("workbench/hidds")
    );
    super::super::configured_source_context::verify_templates(&source).unwrap();
    let module = project(
        &source,
        Path::new("workbench/libs/mesa"),
        "mesa3dgl-library",
        &dirs,
        &target,
        &configuration,
        (&build, &ports),
    )
    .unwrap();
    let module_context = super::super::configured_source_context::project(
        &source,
        Path::new("workbench/libs/mesa"),
        "mesa3dgl-library",
        &dirs,
        &configuration.recipe_configuration().make_coordinates,
        &module.context_input,
    )
    .unwrap();
    assert_eq!(
        module_context.object_directory,
        Path::new(
            &configuration
                .recipe_configuration()
                .make_coordinates
                .generated_root
        )
        .join("workbench/libs/mesa/26.0.0")
    );
    // Print only this source-owned path atom, never arbitrary configured defines.
    let include = &projection.options.c[29];
    assert_eq!(
        include,
        &format!(
            "-I{}/SYS/Developer/include/gallium",
            build.canonicalize().unwrap().display()
        )
    );
    eprintln!("retained hidd-gallium C option 29: {include}");
    let auxiliary = project(
        &source,
        Path::new("workbench/libs/mesa/libgalliumaux"),
        "mesa3d-linklib-galliumauxiliary",
        &dirs,
        &target,
        &configuration,
        (&build, &ports),
    )
    .unwrap();
    assert!(!auxiliary.options.c.iter().any(|flag| flag == "-DDEBUG"));
    let draw = project(
        &source,
        Path::new("workbench/libs/mesa/libgalliumaux"),
        "mesa3d-linklib-galliumdrawllvm",
        &dirs,
        &target,
        &configuration,
        (&build, &ports),
    )
    .unwrap();
    assert_eq!(draw.options.c.last().unwrap(), "-DDRAW_LLVM_AVAILABLE=1");
    assert_eq!(
        draw.options
            .c
            .iter()
            .filter(|flag| flag.as_str() == "-DDRAW_LLVM_AVAILABLE=1")
            .count(),
        1
    );
    assert!(!draw
        .options
        .cxx
        .iter()
        .any(|flag| flag == "-DDRAW_LLVM_AVAILABLE=1"));
    let core = project(
        &source,
        Path::new("workbench/libs/mesa/libmesa"),
        "mesa3d-linklib-mesa",
        &dirs,
        &target,
        &configuration,
        (&build, &ports),
    )
    .unwrap();
    let core_context = super::super::configured_source_context::project(
        &source,
        Path::new("workbench/libs/mesa/libmesa"),
        "mesa3d-linklib-mesa",
        &dirs,
        &configuration.recipe_configuration().make_coordinates,
        &core.context_input,
    )
    .unwrap();
    assert_eq!(
        core_context.quote_extra,
        Path::new(
            &configuration
                .recipe_configuration()
                .make_coordinates
                .build_root
        )
        .join("workbench/libs/mesa/libmesa")
    );
    assert!(!core.options.c.is_empty());
    assert_eq!(
        core_context.python_build_directory,
        Some(
            Path::new(
                &configuration
                    .recipe_configuration()
                    .make_coordinates
                    .generated_root
            )
            .join("workbench/libs/mesa/26.0.0")
        )
    );
    assert!(core
        .options
        .c
        .iter()
        .all(|option| !option.contains("PRIVATE_SCRIPT") && !option.contains("PRIVATE_XML")));
    super::verify_target_option_templates(&source).unwrap();
    let drift = TempDir::new().unwrap();
    let drift_root = drift.path().canonicalize().unwrap();
    fs::create_dir(drift_root.join("config")).unwrap();
    for include in ["make-autotools.tmpl", "make-cmake.tmpl", "make-meson.tmpl"] {
        fs::copy(
            source.join("config").join(include),
            drift_root.join("config").join(include),
        )
        .unwrap();
    }
    let template = fs::read(source.join("config/make.tmpl")).unwrap();
    for name in [
        "compile_q",
        "rule_compile_multi",
        "build_linklib",
        "build_module",
        "build_module_library",
        "build_module_core",
    ] {
        let mut changed = template.clone();
        let header = format!("%define {name} ");
        let offset = changed
            .windows(header.len())
            .position(|window| window == header.as_bytes())
            .unwrap();
        changed[offset + header.len()] = b'X';
        fs::write(drift_root.join("config/make.tmpl"), &changed).unwrap();
        assert!(
            super::super::configured_source_context::verify_templates(&drift_root)
                .unwrap_err()
                .contains("changed")
        );
    }
}

#[test]
fn generator_binding_classification_keeps_rhs_side_effect_guards() {
    let binding =
        "$(top_builddir)/$(CUR_MESADIR)/main/dispatch.h: PRIVATE_SCRIPT := $(PYTHON) generator.py";
    let states = vec![crate::make_vars::ConditionalTruth::True];
    let allowed = std::collections::BTreeSet::from([0]);
    assert!(super::reject_unmodeled_evaluation(binding, &states, &allowed).is_ok());
    assert!(super::reject_unmodeled_evaluation(
        binding,
        &states,
        &std::collections::BTreeSet::new()
    )
    .is_err());
    for effect in [
        "$(shell touch forbidden)",
        "${eval USER_CFLAGS+=-DLEAK=1}",
        "$${USER_CFLAGS}",
    ] {
        let changed = binding.replace("$(PYTHON) generator.py", effect);
        assert!(super::reject_unmodeled_evaluation(&changed, &states, &allowed).is_err());
    }
}

#[test]
fn concrete_build_mapping_preserves_order_and_rejects_other_expressions() {
    let mut options = super::OrderedMesaCompilerOptions {
        c: vec![
            "-I${AROS_BUILD_DIR}/SYS/Developer/include".into(),
            "-DORDER=1".into(),
            "-DORDER=1".into(),
        ],
        cxx: vec!["-I${AROS_BUILD_DIR}/gen".into()],
        vc4_bridge: vec!["-I${AROS_BUILD_DIR}/bridge".into()],
        gca_abi_flags: "-marm -DORDER=1 -DORDER=1".into(),
    };
    let original_gca = options.gca_abi_flags.clone();
    super::materialize_build_options(&mut options, Path::new("/selected/build")).unwrap();
    assert_eq!(
        options.c,
        [
            "-I/selected/build/SYS/Developer/include",
            "-DORDER=1",
            "-DORDER=1"
        ]
    );
    assert_eq!(options.cxx, ["-I/selected/build/gen"]);
    assert_eq!(options.vc4_bridge, ["-I/selected/build/bridge"]);
    assert_eq!(options.gca_abi_flags, original_gca);
    for unknown in [
        "${OTHER}",
        "$(AROS_BUILD_DIR)",
        "$<TARGET_PROPERTY:x,y>",
        "${AROS_BUILD_DIR_SUFFIX}",
    ] {
        let mut invalid = options.clone();
        invalid.cxx.push(unknown.into());
        assert!(
            super::materialize_build_options(&mut invalid, Path::new("/selected/build")).is_err()
        );
    }
    let mut invalid = options;
    invalid.c.push("-I${AROS_BUILD_DIR}/gen".into());
    assert!(super::materialize_build_options(&mut invalid, Path::new("/bad/$root")).is_err());
}

#[test]
fn configured_declaration_overrides_cannot_borrow_the_default_recipe_contract() {
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    for argument in [
        "cflags=-DOTHER=1",
        "cxxflags=-DOTHER=1",
        "dflags=-DOTHER=1",
        "cppflags=-DOTHER=1",
        "lto=yes",
        "compiler=host",
        "includedir=/other",
    ] {
        fs::write(
            &path,
            original.replace("%build_linklib", &format!("%build_linklib {argument}")),
        )
        .unwrap();
        assert!(
            fixture
                .project(&configuration("", ""))
                .unwrap_err()
                .contains("measured default recipe"),
            "{argument}"
        );
    }
    fs::write(
        &path,
        original.replace(
            "%build_linklib",
            "%build_linklib cppflags=\"$(CPPFLAGS)\" lto=\"$(TARGET_LTO)\" compiler=target",
        ),
    )
    .unwrap();
    assert!(fixture.project(&configuration("", "")).is_ok());
}

#[test]
fn escaped_source_dollars_never_become_trusted_build_references() {
    for assignment in [
        "USER_CFLAGS +=",
        "USER_CFLAGS :=",
        "USER_CPPFLAGS +=",
        "USER_CXXFLAGS +=",
    ] {
        let fixture = Fixture::new();
        let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replace(
                "%build_linklib",
                &format!("{assignment} -DVALUE=$${{AROS_BUILD_DIR}}\n%build_linklib"),
            ),
        )
        .unwrap();
        let error = fixture.project(&configuration("", "")).unwrap_err();
        assert!(
            error.contains("escaped literal dollar"),
            "{assignment}: {error}"
        );
    }
}

#[test]
fn source_cannot_hide_opaque_or_literal_writes_to_execution_roots() {
    for name in ["TOP", "SRCDIR", "CURDIR", "GENDIR", "PORTSDIR"] {
        for write in [
            format!("define {name}\n\t$${{AROS_BUILD_DIR}}\nendef"),
            format!("{name} := /other/root"),
            format!("undefine {name}"),
            format!("undefine\t{name}"),
        ] {
            let fixture = Fixture::new();
            let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
            let original = fs::read_to_string(&path).unwrap();
            fs::write(
                &path,
                original.replace("%build_linklib", &format!("{write}\n%build_linklib")),
            )
            .unwrap();
            let error = fixture.project(&configuration("", "")).unwrap_err();
            assert!(
                error.contains("immutable execution root"),
                "{name}: {error}"
            );
        }
    }
}

#[test]
fn computed_root_writes_do_not_disappear_from_source_scope() {
    for write in [
        "$(root_name) := /other",
        "${root_name} = /other",
        "pre$(root_name) += /other",
        "undefine $(root_name)",
        "undefine\t${root_name}",
    ] {
        let fixture = Fixture::new();
        let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replace(
                "%build_linklib",
                &format!("root_name := TOP\n{write}\n%build_linklib"),
            ),
        )
        .unwrap();
        let error = fixture.project(&configuration("", "")).unwrap_err();
        assert!(
            error.contains("computed Make assignment name"),
            "{write}: {error}"
        );
    }
}

#[test]
fn inactive_object_option_rule_does_not_require_or_change_object_values() {
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    let before = fixture.project(&configuration("", "")).unwrap();
    let rule =
        format!("$(UNKNOWN_OBJECTS) $(UNKNOWN_DEPS): {VC4_MMAKE}_MC_EXTRA_CPPFLAGS:=-DINACTIVE=1");
    fs::write(
        &path,
        original.replace(
            "%build_linklib",
            &format!("ifeq (0,1)\n{rule}\nendif\n%build_linklib"),
        ),
    )
    .unwrap();
    assert_eq!(fixture.project(&configuration("", "")).unwrap(), before);
    fs::write(
        &path,
        original.replace(
            "%build_linklib",
            &format!("ifeq ($(UNKNOWN_SELECTION),yes)\n{rule}\nendif\n%build_linklib"),
        ),
    )
    .unwrap();
    assert!(fixture
        .project(&configuration("", ""))
        .unwrap_err()
        .contains("unresolved selection"));
    fs::write(
        &path,
        original.replace(
            "%build_linklib",
            "$(UNKNOWN_OBJECTS): unknown-owner_MC_EXTRA_CPPFLAGS:=-DOTHER=1\n%build_linklib",
        ),
    )
    .unwrap();
    assert!(fixture
        .project(&configuration("", ""))
        .unwrap_err()
        .contains("no local link-library owner"));
}

#[test]
fn option_rules_must_be_top_level_and_literal_unsupported_assignments_refuse() {
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    let baseline = fixture.project(&configuration("", "")).unwrap();
    let rule = format!("$(UNKNOWN_OBJECTS): {VC4_MMAKE}_MC_EXTRA_CPPFLAGS:=-DNOT_A_RULE=1");
    for inert in [
        format!("synthetic-target:\n\t{rule}"),
        format!("define deferred_rule\n{rule}\nendef"),
    ] {
        fs::write(
            &path,
            original.replace("%build_linklib", &format!("{inert}\n%build_linklib")),
        )
        .unwrap();
        assert_eq!(fixture.project(&configuration("", "")).unwrap(), baseline);
    }
    for operator in [":=", "+=", "="] {
        fs::write(
            &path,
            original.replace(
                "%build_linklib",
                &format!(
                    "/obj/a.o: {VC4_MMAKE}_MC_EXTRA_CPPFLAGS{operator}-DOTHER=1\n%build_linklib"
                ),
            ),
        )
        .unwrap();
        assert!(fixture
            .project(&configuration("", ""))
            .unwrap_err()
            .contains("target-specific assignment"));
    }
    fs::write(&path, original).unwrap();
    for suffix in ["OBJDIR", "MC_EXTRA_CPPFLAGS", "MC_CPPFLAGS", "CFLAGS"] {
        let mut selected = fixture.target.clone();
        selected
            .make_variables
            .insert(format!("{VC4_MMAKE}_{suffix}"), "override".into());
        let error = project(
            &fixture.source,
            Path::new(RELATIVE_DIR),
            VC4_MMAKE,
            &DirVars::default(),
            &selected,
            &configuration("", ""),
            (&fixture.build, &fixture.ports),
        )
        .err()
        .unwrap();
        assert!(error.contains("external owner-internal"));
    }
}

#[test]
fn declarations_in_recipes_and_definitions_are_not_executed_owners() {
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    let baseline = fixture.project(&configuration("", "")).unwrap();
    for inert in [
        format!("synthetic-target:\n\t%build_linklib mmake={VC4_MMAKE}"),
        format!("define deferred_declaration\n%build_linklib mmake={VC4_MMAKE}\nendef"),
    ] {
        // Exercise inert declarations after the configuration boundary. A
        // statement before it has a separate ordering refusal regression.
        fs::write(
            &path,
            original.replacen(
                "include $(SRCDIR)/config/aros.cfg\n",
                &format!("include $(SRCDIR)/config/aros.cfg\n{inert}\n"),
                1,
            ),
        )
        .unwrap();
        assert_eq!(fixture.project(&configuration("", "")).unwrap(), baseline);
    }
    for inert in [
        "synthetic-target:\n\t%build_linklib mmake=phantom-owner",
        "define deferred_owner\n%build_linklib mmake=phantom-owner\nendef",
    ] {
        fs::write(
            &path,
            original.replacen("include $(SRCDIR)/config/aros.cfg\n", &format!("include $(SRCDIR)/config/aros.cfg\n{inert}\n$(UNKNOWN_OBJECTS): phantom-owner_MC_EXTRA_CPPFLAGS:=-DPHANTOM=1\n"), 1),
        )
        .unwrap();
        assert!(fixture
            .project(&configuration("", ""))
            .unwrap_err()
            .contains("no local link-library owner"));
    }
}

#[test]
fn matching_target_rule_after_declaration_is_not_retroactively_applied() {
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{original}\n$(UNKNOWN_OBJECTS): {VC4_MMAKE}_MC_EXTRA_CPPFLAGS:=-DLATE=1\n"),
    )
    .unwrap();
    assert!(fixture
        .project(&configuration("", ""))
        .unwrap_err()
        .contains("after their declaration"));
}

struct Fixture {
    _temporary: TempDir,
    source: PathBuf,
    build: PathBuf,
    ports: PathBuf,
    target: TargetContext,
}

#[test]
fn context_retains_complete_source_after_the_selected_declaration() {
    let fixture = Fixture::new();
    let file = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        format!("# Bound configured AROS make wrapper\n{original}"),
    )
    .unwrap();
    let projection = project(
        &fixture.source,
        Path::new(RELATIVE_DIR),
        VC4_MMAKE,
        &DirVars::default(),
        &fixture.target,
        &configuration("", ""),
        (&fixture.build, &fixture.ports),
    )
    .unwrap();
    let mut input = projection.context_input;
    assert!(
        input
            .source_locations
            .iter()
            .flatten()
            .any(|origin| origin.path == file && origin.line == 1),
        "a literal comment matching the synthetic marker retains its origin"
    );
    assert!(
        !input
            .source_locations
            .iter()
            .flatten()
            .any(|origin| origin.path == file && origin.line == 2),
        "only the actual replaced include is synthetic"
    );
    let prefix = input.prefix().unwrap();
    assert!(prefix.contains("USER_CPPFLAGS += -DLOCAL_CPP=1"));
    assert!(!prefix.contains("-DAFTER_GCA=1"));
    assert!(!prefix.contains(GCA_ASSIGNMENT));
    assert!(input.source_program.contains(GCA_ASSIGNMENT));
    assert_eq!(
        input.source_program.lines().last(),
        Some("USER_CPPFLAGS += -DAFTER_GCA=1")
    );
    assert_eq!(
        input
            .source_program
            .lines()
            .nth(input.declaration_line)
            .unwrap()
            .trim(),
        input.invocation.trim()
    );
    let origin = input.source_locations[input.declaration_line]
        .as_ref()
        .unwrap();
    let physical =
        fs::read_to_string(fixture.source.join(RELATIVE_DIR).join("mmakefile.src")).unwrap();
    assert_eq!(
        origin.path,
        fixture.source.join(RELATIVE_DIR).join("mmakefile.src")
    );
    assert!(physical
        .lines()
        .nth(origin.line - 1)
        .unwrap()
        .starts_with("%build_linklib"));
    assert!(input
        .source_locations
        .iter()
        .flatten()
        .any(|origin| origin.path.ends_with("workbench/libs/mesa/mesa.cfg")));
    let saved = input.source_locations.pop().unwrap();
    assert_eq!(
        input.prefix().unwrap_err(),
        "Mesa source origin map does not cover its complete program"
    );
    input.source_locations.push(saved);
    input.declaration_line = input.source_program.lines().count();
    assert_eq!(
        input.prefix().unwrap_err(),
        "Mesa source declaration position is outside its complete program"
    );
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("source");
        let build = root.join("build");
        let ports = root.join("Ports");
        let declaration_dir = source.join(RELATIVE_DIR);
        fs::create_dir_all(source.join("config")).unwrap();
        fs::create_dir_all(source.join("workbench/libs/mesa")).unwrap();
        fs::create_dir_all(&declaration_dir).unwrap();
        fs::create_dir_all(build.join("gen")).unwrap();
        fs::create_dir_all(&ports).unwrap();
        fs::write(source.join("config/target.cfg.in"), TARGET_FORMULAS).unwrap();
        fs::write(source.join("config/make.cfg.in"), MAKE_FORMULAS).unwrap();
        fs::write(
            source.join("config/aros.cfg"),
            "# global configuration wrapper\ninclude $(TOP)/config/make.cfg\n-include $(GENDIR)/config/compiler.cfg\n",
        )
        .unwrap();
        fs::write(
            source.join("workbench/libs/mesa/mesa.cfg"),
            "USER_INCLUDES := -Ibase -Ibase\n\
             USER_CPPFLAGS := -DORDER=1 -UORDER -DORDER=1\n\
             USER_CFLAGS := -DUSER_C=1\n\
             USER_CXXFLAGS := -DUSER_CXX=1\n",
        )
        .unwrap();
        fs::write(
            declaration_dir.join("mmakefile.src"),
            format!(
                "include $(SRCDIR)/config/aros.cfg\n\
                 include $(SRCDIR)/workbench/libs/mesa/mesa.cfg\n\
                 USER_CFLAGS += -DLOCAL_C=1\n\
                 USER_CXXFLAGS += -DLOCAL_CXX=1\n\
                 USER_CPPFLAGS += -DLOCAL_CPP=1\n\
                 %build_linklib mmake={VC4_MMAKE} libname=gallium_vc4\n\
                 {GCA_ASSIGNMENT}\n\
                 USER_CPPFLAGS += -DAFTER_GCA=1\n"
            ),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            source,
            build,
            ports,
            target: TargetContext {
                cpu: Some("arm".into()),
                platform: Some("raspi".into()),
                toolchain: Some("llvm".into()),
                mesa_version: Some("26.0.0".into()),
                ..TargetContext::default()
            },
        }
    }

    fn project(
        &self,
        configuration: &ValidatedMesaConfiguration,
    ) -> Result<super::OrderedMesaCompilerOptions, String> {
        project(
            &self.source,
            Path::new(RELATIVE_DIR),
            VC4_MMAKE,
            &DirVars::default(),
            &self.target,
            configuration,
            (&self.build, &self.ports),
        )
        .map(|projection| projection.options)
    }
}

fn configuration(cflags: &str, cxxflags: &str) -> ValidatedMesaConfiguration {
    configuration_with_options(cflags, cxxflags, &[])
}

fn configuration_with_options(
    cflags: &str,
    cxxflags: &str,
    extra_options: &[(&str, &str)],
) -> ValidatedMesaConfiguration {
    configuration_with_llvm_options(
        cflags,
        cxxflags,
        extra_options,
        &json!({"opt_llvm":{"state":"undefined"},"target_llvm_ver":{"state":"defined","value":"11.0.0"}}),
    )
}

fn configuration_with_llvm_options(
    cflags: &str,
    cxxflags: &str,
    extra_options: &[(&str, &str)],
    llvm_configuration: &Value,
) -> ValidatedMesaConfiguration {
    let identity = MesaConfigurationIdentity {
        target_profile: "arm-raspi".into(),
        target_triple: "arm-unknown-aros".into(),
        compiler: ArosCompilerIdentity::Llvm {
            version: "20.1.0".into(),
        },
        compiler_tree_sha256: sha256_bytes(b"compiler identity"),
        source_inputs_sha256: sha256_bytes(b"source identity"),
        configure_invocation_sha256: sha256_bytes(b"configure identity"),
    };
    let mut baseline = Map::new();
    for name in MESA_BASELINE_VARIABLES {
        baseline.insert((*name).into(), json!([]));
    }
    for (name, value) in [
        ("TARGET_ISA_CXXFLAGS", "-marm-cxx"),
        ("CONFIG_CFLAGS", cflags),
        ("CONFIG_CXXFLAGS", cxxflags),
        ("CONFIG_CPPFLAGS", "-DCONFIG_CPP=1"),
        ("TARGET_CFLAGS", "-FROZEN_TARGET_C"),
        ("TARGET_CXXFLAGS", "-FROZEN_TARGET_CXX"),
        ("TARGET_CPPFLAGS", "-FROZEN_TARGET_CPP"),
        ("OPTIMIZATION_CFLAGS", "-O2"),
        ("LLVM_NOWARN", "-Wno-missing-field-initializers"),
    ] {
        baseline.insert(
            name.into(),
            json!(value.split_whitespace().collect::<Vec<_>>()),
        );
    }
    let mut options = Map::new();
    for name in MESA_COMPILER_OPTION_VARIABLES {
        options.insert((*name).into(), json!([]));
    }
    options.insert("CFLAGS_GNU11".into(), json!(["-std=gnu11"]));
    for (name, value) in extra_options {
        options.insert((*name).into(), json!([value]));
    }
    let selectors = [
        "FFS",
        "FFSLL",
        "BSWAP32",
        "BSWAP64",
        "CLZ",
        "CLZLL",
        "CTZ",
        "EXPECT",
        "POPCOUNT",
        "POPCOUNTLL",
        "UNREACHABLE",
    ]
    .into_iter()
    .map(|name| (format!("CFLAGS_NO_BUILTIN_{name}"), Value::Bool(false)))
    .collect::<serde_json::Map<_, _>>();
    let bytes = serde_json::to_vec(&json!({
        "version": 1,
        "identity": identity,
        "baseline_variables": baseline,
        "undefined_variables": ["CUR_MESADIR"],
        "recipe_configuration": {
            "make_coordinates": {
                "source_root": "/source",
                "build_root": "/make-build",
                "ports_root": "/make-build/Ports",
                "generated_root": "/make-build/gen",
            },
            "safety_cflags": {"state":"defined","value":[]}, "target_lto":"no",
            "lto_cflags":[], "cflags_iquote":["-iquote"], "cflags_iquote_end":[],
            "target_cc":"/compiler/bin/clang", "target_cxx":"/compiler/bin/clang++",
            "target_sysroot":[]
        },
        "llvm_configuration": llvm_configuration,
        "target_isa_flags": ["-marm", "-DABI=1"],
        "builtin_selectors": selectors,
        "compiler_options": options,
        "mesa_compile_options": []
    }))
    .unwrap();
    ValidatedMesaConfiguration::parse_bound(&bytes, &identity).unwrap()
}

#[test]
fn preserves_duplicate_words_and_uses_declaration_time_user_flags() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let fixture = Fixture::new();
    let options = fixture.project(&configuration).unwrap();

    assert!(options
        .c
        .windows(3)
        .any(|words| { words == ["-DORDER=1", "-UORDER", "-DORDER=1"] }));
    assert!(options
        .cxx
        .windows(3)
        .any(|words| { words == ["-DORDER=1", "-UORDER", "-DORDER=1"] }));
    assert!(options.c.contains(&"-DLOCAL_C=1".to_owned()));
    assert!(options.cxx.contains(&"-DLOCAL_CXX=1".to_owned()));
    assert!(options.c.contains(&"-DLOCAL_CPP=1".to_owned()));
    assert!(!options.c.contains(&"-DAFTER_GCA=1".to_owned()));
    assert!(!options.cxx.contains(&"-DAFTER_GCA=1".to_owned()));
    assert!(!options
        .c
        .iter()
        .any(|word| word.starts_with("-FROZEN_TARGET_")));
    assert!(!options
        .cxx
        .iter()
        .any(|word| word.starts_with("-FROZEN_TARGET_")));
}

#[test]
fn keeps_c_and_cxx_configuration_contributions_isolated() {
    let base = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let fixture = Fixture::new();
    let original = fixture.project(&base).unwrap();

    let changed_c = configuration("-DCONFIG_C=2", "-DCONFIG_CXX=1");
    let c_changed = fixture.project(&changed_c).unwrap();
    assert_ne!(original.c, c_changed.c);
    assert_eq!(original.cxx, c_changed.cxx);

    let changed_cxx = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=2");
    let cxx_changed = fixture.project(&changed_cxx).unwrap();
    assert_eq!(original.c, cxx_changed.c);
    assert_ne!(original.cxx, cxx_changed.cxx);
}

#[test]
fn freezes_gca_at_the_exact_immediate_assignment() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let fixture = Fixture::new();
    let options = fixture.project(&configuration).unwrap();

    assert_eq!(
        options.gca_abi_flags,
        "-marm -DABI=1 -DORDER=1 -UORDER -DORDER=1 -DLOCAL_CPP=1"
    );
}

#[test]
fn reading_frozen_gca_does_not_admit_changed_or_missing_source_assignments() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    for replacement in ["", "GCA_ABI_FLAGS := -DSOURCE=1"] {
        let fixture = Fixture::new();
        let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
        let original = fs::read_to_string(&path).unwrap();
        assert!(original.contains(GCA_ASSIGNMENT));
        fs::write(&path, original.replace(GCA_ASSIGNMENT, replacement)).unwrap();
        let error = fixture.project(&configuration).unwrap_err();
        assert!(
            error.contains("exactly one reviewed immediate GCA_ABI_FLAGS assignment"),
            "{error}"
        );
    }
}

#[test]
fn rejects_missing_and_changed_recursive_template_formulas() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let missing = Fixture::new();
    fs::write(
        missing.source.join("config/target.cfg.in"),
        TARGET_FORMULAS.replace(
            "TARGET_CXXFLAGS = $(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CXXFLAGS) $(USER_CXXFLAGS))\n",
            "",
        ),
    )
    .unwrap();
    assert!(missing.project(&configuration).is_err());

    let changed = Fixture::new();
    fs::write(
        changed.source.join("config/target.cfg.in"),
        TARGET_FORMULAS.replace("TARGET_CFLAGS =", "TARGET_CFLAGS :="),
    )
    .unwrap();
    let error = changed.project(&configuration).unwrap_err();
    assert!(error.contains("not recursive"));
}

#[test]
fn rejects_a_recursive_formula_hidden_in_a_conditional() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let fixture = Fixture::new();
    fs::write(
        fixture.source.join("config/target.cfg.in"),
        TARGET_FORMULAS
            .replace(
                "TARGET_CFLAGS =",
                "ifeq ($(AROS_TARGET_CPU),arm)\nTARGET_CFLAGS =",
            )
            .replace("TARGET_CXXFLAGS =", "endif\nTARGET_CXXFLAGS ="),
    )
    .unwrap();
    let error = fixture.project(&configuration).unwrap_err();
    assert!(error.contains("conditional and ambiguous"));
}

#[test]
fn rejects_active_unknown_include_and_proves_an_inactive_one_false() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let active = Fixture::new();
    let mmake_path = active.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&mmake_path).unwrap();
    fs::write(
        &mmake_path,
        format!("{original}include $(UNKNOWN_FLAG_SCOPE)\n"),
    )
    .unwrap();
    assert!(active.project(&configuration).is_err());

    let inactive = Fixture::new();
    let mmake_path = inactive.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&mmake_path).unwrap();
    fs::write(
        &mmake_path,
        original.replacen("include $(SRCDIR)/config/aros.cfg\n", "include $(SRCDIR)/config/aros.cfg\nifeq ($(AROS_TARGET_CPU),m68k)\ninclude $(UNKNOWN_FLAG_SCOPE)\nendif\n", 1),
    )
    .unwrap();
    assert!(inactive.project(&configuration).is_ok());
}

#[test]
fn rejects_conditional_macro_body_and_recipe_configuration_imports() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    for replacement in [
        "ifeq ($(AROS_TARGET_CPU),m68k)\ninclude $(SRCDIR)/config/aros.cfg\nendif",
        "ifeq ($(UNKNOWN_IMPORT_SWITCH),yes)\ninclude $(SRCDIR)/config/aros.cfg\nendif",
        "override define UNUSED\ninclude $(SRCDIR)/config/aros.cfg\nendef",
        "unused-target:\n\tinclude $(SRCDIR)/config/aros.cfg",
    ] {
        let fixture = Fixture::new();
        let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replace("include $(SRCDIR)/config/aros.cfg", replacement),
        )
        .unwrap();
        let error = fixture.project(&configuration).unwrap_err();
        assert!(error.contains("unconditional top-level import"), "{error}");
    }
}

#[test]
fn make_reference_hashes_cannot_hide_unmodeled_effects() {
    for line in [
        "UNUSED := $(subst #,X,$(shell touch /unused/probe))",
        "UNUSED := ${subst #,X,${file > /unused/probe,content}}",
        "UNUSED := $# $(eval USER_CPPFLAGS += -DUNMODELED)",
        "UNUSED := $(unclosed # $(shell touch /unused/probe)",
    ] {
        let error = super::reject_unmodeled_evaluation(
            line,
            &[crate::make_vars::ConditionalTruth::True],
            &std::collections::BTreeSet::new(),
        )
        .unwrap_err();
        assert!(
            error.contains("unmodeled top-level Make evaluation"),
            "{line}: {error}"
        );
    }
}

#[test]
fn ordinary_make_comments_do_not_become_effects_or_erase_reference_data() {
    for (line, expected) in [
        (
            "UNUSED := plain # $(shell not-executed)",
            "UNUSED := plain ",
        ),
        (
            "UNUSED := $(subst #,X,a#b) # trailing",
            "UNUSED := $(subst #,X,a#b) ",
        ),
        ("UNUSED := $#data # trailing", "UNUSED := $#data "),
        (
            r"UNUSED := escaped\#data # trailing",
            r"UNUSED := escaped\#data ",
        ),
    ] {
        assert_eq!(super::strip_make_comment(line), expected);
        super::reject_unmodeled_evaluation(
            line,
            &[crate::make_vars::ConditionalTruth::True],
            &std::collections::BTreeSet::new(),
        )
        .unwrap();
    }
}

#[test]
fn rejects_unmodeled_top_level_eval_instead_of_omitting_its_flags() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    for expression in [
        "$(eval USER_CPPFLAGS += -DUNMODELED)",
        "${eval USER_CPPFLAGS += -DUNMODELED}",
        "${file > /unused/probe,content}",
        "${file > /unused/probe,content:with-colon}",
        "UNUSED = $(file > /unused/probe,content:with-colon)",
        "UNUSED = ${guile (display 1)}",
        "UNUSED := $(shell touch /unused/probe)",
        "$(error unsupported: active assertion)",
        "LLVM_VERSION_HEX := 0x$(shell printf \"%02x%02x\" $(LLVM_VERSION_MAJOR) $(LLVM_VERSION_MINOR))",
        "UNUSED != touch /unused/probe",
        "override CUR_MESADIR := source/view",
        "export CUR_MESADIR := source/view",
        "private USER_CPPFLAGS += -DUNMODELED",
        "override export private CUR_MESADIR := source/view",
        "export CUR_MESADIR",
        ".RECIPEPREFIX := >",
        "define .RECIPEPREFIX\nL\nendef",
        "define $(INDIRECT_NAME)\nL\nendef",
        "\t@echo '# $(eval USER_CPPFLAGS += -DUNMODELED)'",
        "\t# ${file > /unused/probe,content:with-colon}",
        r"${file \
> /unused/probe,content:with-colon}",
        r"UNUSED := $(shell \
touch /unused/probe)",
    ] {
        let fixture = Fixture::new();
        let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replace("%build_linklib", &format!("{expression}\n%build_linklib")),
        )
        .unwrap();
        let error = fixture.project(&configuration).unwrap_err();
        assert!(
            error.contains("unmodeled top-level Make evaluation"),
            "{expression}: {error}"
        );
    }
}

#[test]
fn continued_flag_references_are_assignment_data_not_standalone_evaluation() {
    let fixture = Fixture::new();
    let path = fixture.source.join("workbench/libs/mesa/mesa.cfg");
    let original = fs::read_to_string(&path).unwrap();
    let multiline = r"MESA_STDC_FLAGS := -DSTD_C=1 -DSTD_C=1
MESA_POSIXC_FLAGS := -DPOSIX_C=1
MESA_COMPILEFLAGS := \
    $(MESA_STDC_FLAGS) \
    $(MESA_POSIXC_FLAGS) \
    -DORDER=1 -UORDER -DORDER=1
USER_CPPFLAGS := $(MESA_COMPILEFLAGS)";
    fs::write(
        &path,
        original.replace("USER_CPPFLAGS := -DORDER=1 -UORDER -DORDER=1", multiline),
    )
    .unwrap();
    let result = fixture.project(&configuration("-DC=1", "-DCXX=1")).unwrap();
    let expected: Vec<String> = [
        "-DSTD_C=1",
        "-DSTD_C=1",
        "-DPOSIX_C=1",
        "-DORDER=1",
        "-UORDER",
        "-DORDER=1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert!(result
        .c
        .windows(expected.len())
        .any(|window| window == expected));
    assert!(result
        .cxx
        .windows(expected.len())
        .any(|window| window == expected));
    assert!(result
        .gca_abi_flags
        .contains("-DSTD_C=1 -DSTD_C=1 -DPOSIX_C=1 -DORDER=1 -UORDER -DORDER=1"));
}

#[test]
fn measured_undefined_source_local_value_stays_empty_until_a_late_default_assigns_it() {
    let fixture = Fixture::new();
    let cfg = fixture.source.join("workbench/libs/mesa/mesa.cfg");
    let original = fs::read_to_string(&cfg).unwrap();
    fs::write(
        &cfg,
        original + "\nUSER_INCLUDES += -I$(CUR_MESADIR)/include\n",
    )
    .unwrap();
    let configuration = configuration("-DC=1", "-DCXX=1");
    let empty = fixture.project(&configuration).unwrap();
    assert!(empty.c.contains(&"-I/include".into()));
    let mmake = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&mmake).unwrap();
    fs::write(
        &mmake,
        original.replace(
            "%build_linklib",
            "CUR_MESADIR ?= source/view\nUSER_INCLUDES += -I$(CUR_MESADIR)/late\n%build_linklib",
        ),
    )
    .unwrap();
    let late = fixture.project(&configuration).unwrap();
    assert!(
        late.c.contains(&"-Isource/view/late".into()),
        "{:?}",
        late.c
    );
    // The earlier append followed a := assignment and froze its empty value.
    assert!(late.c.contains(&"-I/include".into()));
}

#[test]
fn measured_undefined_source_local_value_rejects_a_conflicting_selected_definition() {
    let configuration = configuration("-DC=1", "-DCXX=1");
    for value in ["", "source/view"] {
        let mut fixture = Fixture::new();
        fixture
            .target
            .make_variables
            .insert("CUR_MESADIR".into(), value.into());
        assert!(fixture
            .project(&configuration)
            .unwrap_err()
            .contains("measured undefined"));
    }
}

#[test]
fn measured_llvm_values_affect_only_source_scope_not_frontend_provider_selection() {
    let mut fixture = Fixture::new();
    fixture.target.target_llvm_ver = Some(String::new());
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace(
            "USER_CFLAGS += -DLOCAL_C=1",
            "USER_CFLAGS += -DMEASURED_LLVM=$(TARGET_LLVM_VER)\nUSER_CFLAGS += -DLOCAL_C=1",
        ),
    )
    .unwrap();
    let result = fixture.project(&configuration("-DC=1", "-DCXX=1")).unwrap();
    assert!(result.c.contains(&"-DMEASURED_LLVM=11.0.0".into()));
    assert_eq!(fixture.target.target_llvm_ver, Some(String::new()));
    fixture.target.target_llvm_ver = Some("12.0.0".into());
    assert!(fixture
        .project(&configuration("-DC=1", "-DCXX=1"))
        .unwrap_err()
        .contains("conflicts with measured"));
}

#[test]
fn measured_llvm_undefined_and_defined_empty_retain_late_default_semantics() {
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace(
            "USER_CFLAGS += -DLOCAL_C=1",
            "OPT_LLVM ?= 12.0.0\nUSER_CFLAGS += -DOPT_LLVM=$(OPT_LLVM)\nUSER_CFLAGS += -DLOCAL_C=1",
        ),
    )
    .unwrap();
    for (state, expected) in [
        (json!({"state":"undefined"}), "-DOPT_LLVM=12.0.0"),
        (json!({"state":"defined","value":""}), "-DOPT_LLVM="),
    ] {
        let config = configuration_with_llvm_options(
            "-DC=1",
            "-DCXX=1",
            &[],
            &json!({"opt_llvm":state,"target_llvm_ver":{"state":"defined","value":"11.0.0"}}),
        );
        assert!(fixture
            .project(&config)
            .unwrap()
            .c
            .contains(&expected.into()));
    }
    let mut conflict = fixture;
    conflict
        .target
        .make_variables
        .insert("OPT_LLVM".into(), String::new());
    assert!(conflict
        .project(&configuration("-DC=1", "-DCXX=1"))
        .unwrap_err()
        .contains("conflicts with measured"));
}

#[test]
fn source_llvm_fallback_format_and_inactive_assertion_are_projected_together() {
    let mut fixture = Fixture::new();
    let relative = Path::new("workbench/libs/mesa/libgalliumvm");
    fs::create_dir_all(fixture.source.join(relative)).unwrap();
    let original =
        fs::read_to_string(fixture.source.join(RELATIVE_DIR).join("mmakefile.src")).unwrap();
    let llvm = r#"ifeq ($(OPT_LLVM),)
ifneq ($(TARGET_LLVM_VER),)
OPT_LLVM := $(TARGET_LLVM_VER)
else
OPT_LLVM := 11.0.0
endif
endif
LLVM_VERSION_MAJOR := $(word 1,$(subst ., ,$(OPT_LLVM)))
LLVM_VERSION_MINOR := $(word 2,$(subst ., ,$(OPT_LLVM)))
LLVM_VERSION_HEX := 0x$(shell printf "%02x%02x" $(LLVM_VERSION_MAJOR) $(LLVM_VERSION_MINOR))
ifneq ($(OPT_MESAGL),26.0.0)
$(error Gallivm requires the reviewed Mesa 26.0.0 source inventory)
endif
USER_CPPFLAGS += -DHAVE_LLVM=$(LLVM_VERSION_HEX)
"#;
    let source = original
        .replace("%build_linklib", &format!("{llvm}\n%build_linklib"))
        .replace(VC4_MMAKE, "mesa3d-linklib-galliumvm");
    fs::write(fixture.source.join(relative).join("mmakefile.src"), source).unwrap();
    let config = configuration("-DC=1", "-DCXX=1");
    let options = project(
        &fixture.source,
        relative,
        "mesa3d-linklib-galliumvm",
        &DirVars::default(),
        &fixture.target,
        &config,
        (&fixture.build, &fixture.ports),
    )
    .unwrap()
    .options;
    assert!(options.c.contains(&"-DHAVE_LLVM=0x0b00".into()));
    assert!(options.cxx.contains(&"-DHAVE_LLVM=0x0b00".into()));
    for selection in [None, Some("20.0.8".into())] {
        fixture.target.mesa_version = selection;
        let error = project(
            &fixture.source,
            relative,
            "mesa3d-linklib-galliumvm",
            &DirVars::default(),
            &fixture.target,
            &config,
            (&fixture.build, &fixture.ports),
        )
        .err()
        .unwrap();
        assert!(
            error.contains("unmodeled top-level Make evaluation"),
            "{error}"
        );
    }
}

fn add_inventory_guard(fixture: &Fixture, path_expression: &str) -> PathBuf {
    let inventory = fixture
        .ports
        .join("mesa/src/gallium/drivers/vc4/Makefile.sources");
    fs::create_dir_all(inventory.parent().unwrap()).unwrap();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    let guard = format!(
        "GALLIUM_PATH = {path_expression}\nCUR_GALLIUMDIR = drivers/vc4\n\
         ifneq ($(wildcard $(GALLIUM_PATH)/$(CUR_GALLIUMDIR)/Makefile.sources),)\n\
         include $(GALLIUM_PATH)/$(CUR_GALLIUMDIR)/Makefile.sources\n\
         USER_CFLAGS += -DINVENTORY_PRESENT=1\nelse\n\
         USER_CFLAGS += -DINVENTORY_ABSENT=1\nendif\n"
    );
    fs::write(
        &path,
        original.replace(
            "USER_CFLAGS += -DLOCAL_C=1",
            &(guard + "USER_CFLAGS += -DLOCAL_C=1"),
        ),
    )
    .unwrap();
    inventory
}

#[test]
fn inventory_guard_uses_actual_file_presence_not_the_mesa_version() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    for version in ["26.0.0", "20.0.8"] {
        let mut fixture = Fixture::new();
        fixture.target.mesa_version = Some(version.into());
        let inventory = add_inventory_guard(&fixture, "$(PORTSDIR)/mesa/src/gallium");
        let absent = fixture.project(&configuration).unwrap();
        assert!(absent.c.contains(&"-DINVENTORY_ABSENT=1".into()));
        assert!(!absent.c.contains(&"-DINVENTORY_PRESENT=1".into()));
        fs::write(&inventory, "C_SOURCES = first.c second.c\n").unwrap();
        let present = fixture.project(&configuration).unwrap();
        assert!(present.c.contains(&"-DINVENTORY_PRESENT=1".into()));
        assert!(!present.c.contains(&"-DINVENTORY_ABSENT=1".into()));
    }
}

#[test]
fn inventory_guard_never_treats_invalid_or_unbound_inventory_as_absent() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let fixture = Fixture::new();
    let inventory = add_inventory_guard(&fixture, "$(PORTSDIR)/mesa/src/gallium");
    fs::write(&inventory, "USER_CFLAGS += -DUNMEASURED=1\n").unwrap();
    assert!(fixture
        .project(&configuration)
        .unwrap_err()
        .contains("non-inventory variable"));
    fs::write(&inventory, "C_SOURCES != touch /unused/probe\n").unwrap();
    assert!(fixture
        .project(&configuration)
        .unwrap_err()
        .contains("shell assignment"));
    for expression in [
        "/outside-selected-ports",
        "$(UNMEASURED_PORTS)",
        "$(PORTSDIR)/mesa/*",
    ] {
        let fixture = Fixture::new();
        add_inventory_guard(&fixture, expression);
        assert!(fixture.project(&configuration).is_err(), "{expression}");
    }
}

fn add_component_inventory_guard(fixture: &Fixture, condition: &str) -> PathBuf {
    let inventory = fixture.ports.join("mesa/src/compiler/Makefile.sources");
    fs::create_dir_all(inventory.parent().unwrap()).unwrap();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    let guard = format!(
        "top_srcdir = $(PORTSDIR)/mesa\nCUR_MESADIR := src/compiler\n\
         {condition} ($(wildcard $(top_srcdir)/$(CUR_MESADIR)/Makefile.sources),)\n\
         USER_CFLAGS += -DCOMPONENT_TRUE=1\nelse\n\
         USER_CFLAGS += -DCOMPONENT_FALSE=1\nendif\n\
         ifneq ($(wildcard $(top_srcdir)/$(CUR_MESADIR)/Makefile.sources),)\n\
         include $(top_srcdir)/$(CUR_MESADIR)/Makefile.sources\nendif\n"
    );
    fs::write(
        &path,
        original.replace(
            "USER_CFLAGS += -DLOCAL_C=1",
            &(guard + "USER_CFLAGS += -DLOCAL_C=1"),
        ),
    )
    .unwrap();
    inventory
}

#[test]
fn component_inventory_guards_preserve_both_make_condition_directions() {
    let configuration = configuration("-DC=1", "-DCXX=1");
    for condition in ["ifeq", "ifneq"] {
        let fixture = Fixture::new();
        let inventory = add_component_inventory_guard(&fixture, condition);
        for present in [false, true] {
            if present {
                fs::write(&inventory, "C_SOURCES := retained.c\n").unwrap();
            }
            let options = fixture.project(&configuration).unwrap();
            let expected = present == (condition == "ifneq");
            assert_eq!(options.c.contains(&"-DCOMPONENT_TRUE=1".into()), expected);
            assert_eq!(options.c.contains(&"-DCOMPONENT_FALSE=1".into()), !expected);
        }
    }
}

#[test]
fn component_inventory_guard_refuses_unmeasured_writes_and_unavailable_parent() {
    let configuration = configuration("-DC=1", "-DCXX=1");
    let fixture = Fixture::new();
    let inventory = add_component_inventory_guard(&fixture, "ifneq");
    fs::write(&inventory, "USER_CFLAGS += -DUNMEASURED=1\n").unwrap();
    assert!(fixture
        .project(&configuration)
        .unwrap_err()
        .contains("non-inventory variable"));
    fs::remove_file(&inventory).unwrap();
    fs::remove_dir(inventory.parent().unwrap()).unwrap();
    assert!(fixture
        .project(&configuration)
        .unwrap_err()
        .contains("inventory parent"));
}

#[test]
fn recursive_formula_must_be_an_active_assignment_not_macro_or_recipe_data() {
    let configuration = configuration("-DCONFIG_C=1", "-DCONFIG_CXX=1");
    let formula = "TARGET_CFLAGS = $(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CFLAGS) $(USER_CFLAGS))";
    for replacement in [
        format!("override define UNUSED\n{formula}\nendef"),
        format!("unused-target:\n\t{formula}"),
    ] {
        let fixture = Fixture::new();
        fs::write(
            fixture.source.join("config/target.cfg.in"),
            TARGET_FORMULAS.replace(formula, &replacement),
        )
        .unwrap();
        let error = fixture.project(&configuration).unwrap_err();
        assert!(
            error.contains("exactly one recursive TARGET_CFLAGS"),
            "{error}"
        );
    }
}

#[test]
fn all_four_new_warning_outcomes_reach_only_the_source_selected_language() {
    let additions = [
        ("NOWARN_UNUSED_VARIABLE", "-Wno-unused-variable"),
        ("NOWARN_UNUSED_VALUE", "-Wno-unused-value"),
        ("NOWARN_UNUSED_FUNCTION", "-Wno-unused-function"),
        (
            "NOWARN_MISLEADING_INDENTATION",
            "-Wno-misleading-indentation",
        ),
    ];
    let fixture = Fixture::new();
    let path = fixture.source.join(RELATIVE_DIR).join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replace("%build_linklib", "USER_CFLAGS += $(NOWARN_UNUSED_VARIABLE) $(NOWARN_UNUSED_VALUE) $(NOWARN_UNUSED_FUNCTION) $(NOWARN_MISLEADING_INDENTATION)\n%build_linklib")).unwrap();
    let empty = fixture.project(&configuration("-DC=1", "-DCXX=1")).unwrap();
    let measured = fixture
        .project(&configuration_with_options("-DC=1", "-DCXX=1", &additions))
        .unwrap();
    let flags: Vec<_> = additions
        .iter()
        .map(|(_, value)| (*value).to_owned())
        .collect();
    assert!(measured
        .c
        .windows(flags.len())
        .any(|window| window == flags));
    for flag in flags {
        assert!(!empty.c.contains(&flag));
        assert!(!measured.cxx.contains(&flag));
    }
    assert_eq!(measured.cxx, empty.cxx);
    assert_eq!(measured.gca_abi_flags, empty.gca_abi_flags);
}
