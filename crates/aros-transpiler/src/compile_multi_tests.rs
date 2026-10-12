use super::collect;
use crate::dirs::DirVars;
use crate::parser::{parse_mmakefile_with_context, TargetContext};
use std::fs;
use std::path::{Path, PathBuf};

const RECIPE: &str = "arch/example/mmakefile.src";
const OWNER: &str = "object-owner";
const CALL: &str = "%rule_compile_multi mmake=unit_ns basenames=\"unit other\" cppflags=\"$(CPPFLAGS)\" cflags=\"$(CFLAGS)\" dflags=\"$(DFLAGS)\" srcdir=\"\" targetdir=\"$(GENDIR)/objects\" compiler=target usetree=no incextra=\"$(TOP)/$(CURDIR)\"";

struct Fixture {
    temporary: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary source root");
        let root = temporary.path().to_path_buf();
        let recipe_dir = root.join("arch/example");
        fs::create_dir_all(&recipe_dir).expect("recipe directory");
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::create_dir_all(root.join("scripts")).expect("script directory");
        fs::write(
            root.join("config/make.cfg.in"),
            concat!(
                "AROSDIR := $(TARGETDIR)/SYS\n",
                "AROS_DIR_DEVELOPER := Developer\n",
                "AROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\n",
                "AROS_DIR_LIB := lib\n",
                "AROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\n",
                // The direct collector receives bind_native_target_tool_roles;
                // the parser entry point loads DirVars itself.
                "NATIVE_TARGET_CC := ${CMAKE_C_COMPILER}\n",
            ),
        )
        .expect("write simple Make configuration");

        let checked =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compile-multi-contract");
        fs::copy(
            checked.join("config/make.tmpl"),
            root.join("config/make.tmpl"),
        )
        .expect("copy checked Make template");
        fs::copy(checked.join("scripts/mkdep"), root.join("scripts/mkdep"))
            .expect("copy checked mkdep script");

        for basename in ["unit", "other", "sub/unit"] {
            let source = recipe_dir.join(format!("{basename}.c"));
            fs::create_dir_all(source.parent().expect("source parent"))
                .expect("source parent directory");
            fs::write(source, "int compile_multi_fixture;\n").expect("C source fixture");
        }
        Self { temporary, root }
    }

    fn dirs(&self) -> DirVars {
        let mut dirs = DirVars::load(&self.root);
        dirs.bind_native_target_tool_roles();
        dirs
    }

    fn write_recipe(&self, source: &str) {
        fs::write(self.root.join(RECIPE), source).expect("write mmakefile source");
    }

    fn collect(
        &self,
        source: &str,
    ) -> (
        Vec<super::LiteralObjectGroupDecl>,
        Vec<super::LiteralObjectRejection>,
    ) {
        self.write_recipe(source);
        let dirs = self.dirs();
        collect(
            source,
            Path::new(RECIPE),
            &self.root,
            &dirs,
            Some(&TargetContext::default()),
        )
    }

    fn collect_with_target(
        &self,
        source: &str,
        target: &TargetContext,
    ) -> (
        Vec<super::LiteralObjectGroupDecl>,
        Vec<super::LiteralObjectRejection>,
    ) {
        self.write_recipe(source);
        let dirs = self.dirs();
        collect(source, Path::new(RECIPE), &self.root, &dirs, Some(target))
    }
}

fn valid_source() -> String {
    format!(
        concat!(
            "TARGET_CC := $(NATIVE_TARGET_CC)\n",
            "TARGET_SYSROOT := --sysroot=$(AROS_DEVELOPER)\n",
            "CFLAGS_IQUOTE := -iquote\n",
            "CFLAGS_IQUOTE_END :=\n",
            "MKDEPEND := $(SRCDIR)/scripts/mkdep\n",
            "CFLAGS := -O2 -g -Wall -march=armv7-a -DC_ONLY=1\n",
            "DFLAGS := -O3 -g -Wall -march=dep-order -DDEP_ONLY=1\n",
            "SAFETY_CFLAGS := -fstack-protector-strong -DSAFETY_ONLY=1\n",
            "CPPFLAGS := -DCPP_ONLY=1\n",
            "unit_ns_MC_EXTRA_CPPFLAGS := -DEXTRA_ONLY=1\n",
            "{CALL}\n",
            ".PHONY: {OWNER}\n",
            "{OWNER}: $(GENDIR)/objects/unit.o $(GENDIR)/objects/other.o\n",
            "%include_deps depstargets={OWNER} deps=\"$(GENDIR)/objects/unit.d $(GENDIR)/objects/other.d\"\n",
        ),
        CALL = CALL,
        OWNER = OWNER,
    )
}

fn assert_rejected(source: &str, expected_reason: &str) {
    let fixture = Fixture::new();
    let (groups, rejections) = fixture.collect(source);
    assert!(groups.is_empty(), "unexpected groups: {groups:#?}");
    assert!(
        rejections
            .iter()
            .any(|rejection| rejection.reason.contains(expected_reason)),
        "expected rejection containing {expected_reason:?}; got {rejections:#?}"
    );
}

#[test]
fn refuses_oversized_scopes_before_repeated_macro_context_work() {
    let dirs = Fixture::new().dirs();
    assert!(
        super::flags(&"-DVALUE=".repeat(8193), &dirs, Path::new("arch/example"))
            .unwrap_err()
            .contains("flag vector exceeds its byte bound")
    );
    assert_rejected(
        &format!("{}{}", "#".repeat(super::MAX_BYTES), valid_source()),
        "source snapshot exceeds its byte bound",
    );
    assert_rejected(
        &format!(
            "{}{}",
            format!("{CALL}\n").repeat(super::MAX_INVOCATIONS),
            valid_source()
        ),
        "invocation set exceeds its work bound",
    );
    let large = CALL.replace(
        "unit other",
        &std::iter::repeat_n("unit", super::MAX_OBJECTS)
            .collect::<Vec<_>>()
            .join(" "),
    );
    let source = valid_source().replace(
        CALL,
        &format!("{large}\n{}", CALL.replace("unit_ns", "other_ns")),
    );
    assert_rejected(&source, "object set exceeds its shared work bound");
}

#[test]
fn rejects_a_source_hidden_by_the_verified_vpath_search_order() {
    let fixture = Fixture::new();
    for source in ["z/foo.c", "a/bar.c", "a/foo.c"] {
        let path = fixture.root.join("arch/example").join(source);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "int source_lookup_fixture;\n").unwrap();
    }
    let source = valid_source()
        .replace("unit other", "z/foo a/bar")
        .replace("objects/unit.", "objects/foo.")
        .replace("objects/other.", "objects/bar.");
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(
        rejected.iter().any(|rejection| rejection.reason.contains(
            "vpath selects arch/example/a/foo.c instead of declared arch/example/z/foo.c"
        )),
        "{rejected:#?}"
    );
    // The same valid declaration becomes admissible without the shadowing
    // source; neither compiler flags nor aggregate requirements are relaxed.
    fs::remove_file(fixture.root.join("arch/example/a/foo.c")).unwrap();
    let (groups, rejected) = fixture.collect(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(
        groups[0].objects[0].source,
        "${AROS_SOURCE_DIR}/arch/example/z/foo.c"
    );
}

#[test]
fn rejects_make_environment_mutations_not_represented_by_the_compiler_contract() {
    for prefix in [
        ".EXPORT_ALL_VARIABLES:\n",
        "export\n",
        "export CPATH := $(SRCDIR)/alternate\n",
        "unexport CPATH\n",
    ] {
        assert_rejected(&format!("{prefix}{}", valid_source()), "opaque controls");
    }
    assert_rejected(
        &format!("CPATH := $(SRCDIR)/alternate\n{}", valid_source()),
        "mutates compiler environment CPATH",
    );
}

#[test]
fn refuses_adjacent_macro_effects_instead_of_borrowing_the_visible_flags() {
    let fixture = Fixture::new();
    let template = fixture.root.join("config/make.tmpl");
    let original = fs::read_to_string(&template).unwrap();
    fs::write(
        &template,
        format!("{original}\n%define hidden_context\nCFLAGS := -DHIDDEN_CONTEXT=1\n%end\n"),
    )
    .unwrap();
    let source = format!("%hidden_context\n{}", valid_source());
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(
        rejected
            .iter()
            .any(|rejection| rejection.reason.contains("unproven adjacent GenMF macro")),
        "{rejected:#?}"
    );
}

#[test]
fn refuses_raw_inline_genmf_calls_in_prefix_comments_recipes_and_false_branches() {
    for prefix in [
        "prefix %rule_compile_multi mmake=shadow basenames=unit targetdir=shadow\n",
        "TEXT := literal # %rule_compile_multi mmake=shadow basenames=unit targetdir=shadow\n",
        "unrelated:\n\t%rule_compile_multi mmake=shadow basenames=unit targetdir=shadow\n",
        "ifeq (no,yes)\nprefix %rule_compile_multi mmake=shadow basenames=unit targetdir=shadow\nendif\n",
    ] {
        assert_rejected(&format!("{prefix}{}", valid_source()), "unproven inline GenMF macro");
    }
    let source = format!(
        "# %rule_compile_multi is a real column-zero GenMF comment\n{}",
        valid_source()
    );
    let (groups, rejected) = Fixture::new().collect(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(groups.len(), 1);
}

#[test]
fn orders_source_root_aliases_like_expanded_make_paths_not_cmake_symbols() {
    let fixture = Fixture::new();
    for source in [
        "z/foo.c",
        "arch/example/.hidden/bar.c",
        "arch/example/.hidden/foo.c",
    ] {
        let path = fixture.root.join(source);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "int source_alias_lookup_fixture;\n").unwrap();
    }
    let source = valid_source()
        .replace("unit other", "$(SRCDIR)/z/foo .hidden/bar")
        .replace("objects/unit.", "objects/foo.")
        .replace("objects/other.", "objects/bar.");
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(
        rejected.iter().any(|rejection| rejection
            .reason
            .contains("vpath selects arch/example/.hidden/foo.c instead of declared z/foo.c")),
        "{rejected:#?}"
    );
    fs::remove_file(fixture.root.join("arch/example/.hidden/foo.c")).unwrap();
    let (groups, rejected) = fixture.collect(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(groups[0].objects[0].source, "${AROS_SOURCE_DIR}/z/foo.c");
}

#[test]
fn collects_complete_physical_owner_and_ordered_compile_and_dependency_argv() {
    let fixture = Fixture::new();
    let source = valid_source();
    let owner_line = source
        .lines()
        .position(|line| line.starts_with("object-owner:"))
        .expect("physical aggregate owner")
        + 1;
    let (groups, rejections) = fixture.collect(&source);

    assert!(rejections.is_empty(), "{rejections:#?}");
    let [group] = groups.as_slice() else {
        panic!("expected one compile-multi group, got {groups:#?}");
    };
    assert_eq!(group.owner, OWNER);
    assert_eq!(group.file, RECIPE);
    assert_eq!(group.line, owner_line);
    assert_eq!(
        group
            .objects
            .iter()
            .map(|object| object.output.as_str())
            .collect::<Vec<_>>(),
        [
            "${AROS_BUILD_DIR}/gen/objects/unit.o",
            "${AROS_BUILD_DIR}/gen/objects/other.o"
        ]
    );

    let [unit, other] = group.objects.as_slice() else {
        panic!("expected both objects, got {:#?}", group.objects);
    };
    assert_eq!(unit.source, "${AROS_SOURCE_DIR}/arch/example/unit.c");
    assert_eq!(other.source, "${AROS_SOURCE_DIR}/arch/example/other.c");
    assert_eq!(
        unit.arguments,
        [
            "--sysroot=${AROS_BUILD_DIR}/SYS/Developer",
            "-iquote",
            "${AROS_SOURCE_DIR}/arch/example",
            "-iquote",
            "${AROS_BUILD_DIR}/arch/example",
            "-iquote",
            "${AROS_SOURCE_DIR}/arch/example",
            "-iquote",
            "${AROS_BUILD_DIR}/arch/example",
            "-O2",
            "-g",
            "-Wall",
            "-march=armv7-a",
            "-DC_ONLY=1",
            "-fstack-protector-strong",
            "-DSAFETY_ONLY=1",
            "-DCPP_ONLY=1",
            "-DEXTRA_ONLY=1",
            "-D__SRCFILENAME__=\"arch/example/unit.c\"",
        ]
        .map(str::to_owned)
    );
    let dependency = unit.dependency.as_ref().expect("per-object depfile");
    assert_eq!(dependency.output, "${AROS_BUILD_DIR}/gen/objects/unit.d");
    assert_eq!(
        dependency.arguments,
        [
            "--sysroot=${AROS_BUILD_DIR}/SYS/Developer",
            "-march=dep-order",
            "-DDEP_ONLY=1",
            "-DCPP_ONLY=1",
            "-DEXTRA_ONLY=1",
            "-I${AROS_BUILD_DIR}/arch/example",
            "-I${AROS_SOURCE_DIR}/arch/example",
        ]
        .map(str::to_owned)
    );
}

#[test]
fn admits_the_same_fixture_through_the_parser_pipeline_without_legacy_owner_rejections() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("config/aros.cfg"),
        "# replaced by the selected source binding\n",
    )
    .expect("write include endpoint");
    fs::write(
        fixture.root.join("config/native.mk"),
        "BOUND_NATIVE_VALUE := selected\n",
    )
    .expect("write bound native configuration");
    let source = format!("include $(SRCDIR)/config/aros.cfg\n{}", valid_source());
    fixture.write_recipe(&source);
    let target = TargetContext {
        make_include_bindings: std::collections::BTreeMap::from([(
            "config/aros.cfg".into(),
            "config/native.mk".into(),
        )]),
        ..TargetContext::default()
    };

    let parsed = parse_mmakefile_with_context(&fixture.root.join(RECIPE), &fixture.root, &target)
        .expect("parse source fixture");
    let [group] = parsed.literal_object_groups.as_slice() else {
        panic!(
            "expected exactly the compile-multi group: {:#?}",
            parsed.literal_object_groups
        );
    };
    assert_eq!(group.owner, OWNER);
    assert_eq!(group.file, RECIPE);
    assert_eq!(
        group
            .objects
            .iter()
            .map(|object| object.output.as_str())
            .collect::<Vec<_>>(),
        [
            "${AROS_BUILD_DIR}/gen/objects/unit.o",
            "${AROS_BUILD_DIR}/gen/objects/other.o"
        ]
    );
    assert!(
        parsed.native_graph_errors.iter().all(|diagnostic| {
            let owner = diagnostic
                .context
                .as_ref()
                .and_then(|context| context.target.as_deref());
            !(owner == Some(OWNER)
                && (diagnostic.message.contains("SDK object producer")
                    || diagnostic.message.contains("literal object producer")))
        }),
        "legacy owner diagnostic duplicated compile-multi admission: {:#?}",
        parsed.native_graph_errors
    );
}

#[test]
fn reports_rejection_at_the_physical_invocation_line_after_a_bound_include() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("config/aros.cfg"), "# original include\n")
        .expect("write include endpoint");
    fs::write(
        fixture.root.join("config/native.mk"),
        "BOUND_ONE := value\nBOUND_TWO := value\nBOUND_THREE := value\n",
    )
    .expect("write bound native configuration");
    let source = format!(
        "include $(SRCDIR)/config/aros.cfg\n{}",
        valid_source().replace("compiler=target", "compiler=host")
    );
    let physical_line = source
        .lines()
        .position(|line| line.contains("%rule_compile_multi"))
        .expect("physical invocation")
        + 1;
    let target = TargetContext {
        make_include_bindings: std::collections::BTreeMap::from([(
            "config/aros.cfg".into(),
            "config/native.mk".into(),
        )]),
        ..TargetContext::default()
    };

    let (groups, rejections) = fixture.collect_with_target(&source, &target);
    assert!(groups.is_empty());
    let [rejection] = rejections.as_slice() else {
        panic!("expected one rejection, got {rejections:#?}");
    };
    assert_eq!(rejection.file, RECIPE);
    assert_eq!(rejection.line, physical_line);
    assert!(
        rejection.reason.contains("target compiler"),
        "{rejection:#?}"
    );
}

#[test]
fn rejects_missing_or_late_required_source_role_bindings() {
    let source = valid_source();
    let missing = source.replace("MKDEPEND := $(SRCDIR)/scripts/mkdep\n", "");
    assert_rejected(
        &missing,
        "compile-multi needs one preceding source binding for MKDEPEND",
    );

    let late = source.replace("MKDEPEND := $(SRCDIR)/scripts/mkdep\n", "");
    let late = late.replace(
        &format!("{CALL}\n"),
        &format!("{CALL}\nMKDEPEND := $(SRCDIR)/scripts/mkdep\n"),
    );
    assert_rejected(
        &late,
        "compile-multi needs one preceding source binding for MKDEPEND",
    );
}

#[test]
fn requires_an_unconditional_phony_owner_and_the_complete_ordered_object_list() {
    let source = valid_source();
    assert_rejected(
        &source.replace(".PHONY: object-owner", ".PHONY: some-other-owner"),
        "compile-multi requires one explicit physical phony owner",
    );
    assert_rejected(
        &source.replace(
            "$(GENDIR)/objects/unit.o $(GENDIR)/objects/other.o",
            "$(GENDIR)/objects/other.o $(GENDIR)/objects/unit.o",
        ),
        "compile-multi owner does not name the complete ordered object list",
    );
    assert_rejected(
        &source.replace(
            "$(GENDIR)/objects/unit.o $(GENDIR)/objects/other.o",
            "$(GENDIR)/objects/unit.o",
        ),
        "compile-multi owner does not name the complete ordered object list",
    );
}

#[test]
fn requires_the_complete_source_depfile_list_and_rejects_conditional_controls() {
    let source = valid_source();
    assert_rejected(
        &source.replace(
            "$(GENDIR)/objects/unit.d $(GENDIR)/objects/other.d",
            "$(GENDIR)/objects/unit.d",
        ),
        "include-deps does not name the complete source depfile list",
    );

    let conditional = source.replace(
        CALL,
        &format!("ifeq ($(UNKNOWN_CONTROL),yes)\n{CALL}\nendif"),
    );
    assert_rejected(
        &conditional,
        "compile-multi invocation has unresolved Make control",
    );

    let opaque = source.replace("TARGET_CC :=", "define HIDDEN_CONTROL\nendef\nTARGET_CC :=");
    assert_rejected(
        &opaque,
        "compile-multi scope has opaque controls or template/compiler mutations",
    );
}

#[test]
fn rejects_changed_trusted_template_and_mkdep_script() {
    let fixture = Fixture::new();
    let source = valid_source();
    fixture.write_recipe(&source);
    let template = fixture.root.join("config/make.tmpl");
    let original_template = fs::read_to_string(&template).expect("read template");
    let changed_template =
        original_template.replacen("compiler=target usetree=no", "compiler=host usetree=no", 1);
    assert_ne!(
        changed_template, original_template,
        "counterprobe must change the verified macro"
    );
    fs::write(&template, changed_template).expect("mutate trusted template");
    let dirs = fixture.dirs();
    let (groups, rejections) = collect(
        &source,
        Path::new(RECIPE),
        &fixture.root,
        &dirs,
        Some(&TargetContext::default()),
    );
    assert!(groups.is_empty());
    assert!(
        rejections.iter().any(|rejection| {
            rejection
                .reason
                .contains("rule_compile_multi does not match the trusted source contract")
        }),
        "{rejections:#?}"
    );

    let fixture = Fixture::new();
    let source = valid_source();
    fixture.write_recipe(&source);
    let script = fixture.root.join("scripts/mkdep");
    let original_script = fs::read_to_string(&script).expect("read mkdep script");
    fs::write(&script, format!("{original_script}\n# changed\n")).expect("mutate mkdep script");
    let dirs = fixture.dirs();
    let (groups, rejections) = collect(
        &source,
        Path::new(RECIPE),
        &fixture.root,
        &dirs,
        Some(&TargetContext::default()),
    );
    assert!(groups.is_empty());
    assert!(
        rejections.iter().any(|rejection| {
            rejection
                .reason
                .contains("source mkdep script does not match its trusted raw hash")
        }),
        "{rejections:#?}"
    );
}

#[test]
fn rejects_missing_source_files() {
    // `collect` consumes the parser's already-read recipe snapshot. This
    // collector checks independently opened C inputs, while recipe snapshot
    // authenticity belongs to the owning parser/contract boundary.
    let fixture = Fixture::new();
    let source = valid_source();
    fs::remove_file(fixture.root.join("arch/example/other.c")).expect("remove C input");
    assert_rejected_with_fixture(
        &fixture,
        &source,
        "compile-multi source arch/example/other.c is unavailable or unsafe",
    );
}

fn assert_rejected_with_fixture(fixture: &Fixture, source: &str, expected_reason: &str) {
    let (groups, rejections) = fixture.collect(source);
    assert!(groups.is_empty(), "unexpected groups: {groups:#?}");
    assert!(
        rejections
            .iter()
            .any(|rejection| rejection.reason.contains(expected_reason)),
        "expected rejection containing {expected_reason:?}; got {rejections:#?}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_source_and_flattened_basename_collisions() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let outside = fixture.temporary.path().join("outside.c");
    fs::write(&outside, "int outside;\n").expect("write external C input");
    let input = fixture.root.join("arch/example/unit.c");
    fs::remove_file(&input).expect("remove original C input");
    symlink(&outside, &input).expect("symlink source input");
    assert_rejected_with_fixture(
        &fixture,
        &valid_source(),
        "compile-multi source arch/example/unit.c is unavailable or unsafe",
    );

    let fixture = Fixture::new();
    let source = valid_source().replace("basenames=\"unit other\"", "basenames=\"unit sub/unit\"");
    assert_rejected_with_fixture(
        &fixture,
        &source,
        "compile-multi flattened basenames collide",
    );
}

#[test]
fn rejects_unsupported_compiler_modes_source_directories_and_paths() {
    let source = valid_source();
    assert_rejected(
        &source.replace("compiler=target", "compiler=host"),
        "compile-multi currently requires target compiler",
    );
    assert_rejected(
        &source.replace("usetree=no", "usetree=yes"),
        "compile-multi currently requires target compiler, usetree=no",
    );
    assert_rejected(
        &source.replace("srcdir=\"\"", "srcdir=\"arch/example\""),
        "compile-multi currently requires target compiler, usetree=no and implicit source directory",
    );
    assert_rejected(
        &source.replace(
            "targetdir=\"$(GENDIR)/objects\"",
            "targetdir=\"/tmp/objects\"",
        ),
        "compile-multi path must be below ${AROS_BUILD_DIR}/gen",
    );
    let escaping_basename =
        source.replace("basenames=\"unit other\"", "basenames=\"../unit other\"");
    assert_rejected(
        &escaping_basename,
        "compile-multi path is not closed and source-relative",
    );
}

#[test]
fn rejects_duplicate_namespaces_additional_owner_recipes_and_target_specific_flags() {
    let source = valid_source();
    let duplicate_same_namespace =
        source.replace(&format!("{CALL}\n"), &format!("{CALL}\n{CALL}\n"));
    let fixture = Fixture::new();
    let (groups, rejections) = fixture.collect(&duplicate_same_namespace);
    assert!(groups.is_empty(), "{groups:#?}");
    assert_eq!(rejections.len(), 2, "{rejections:#?}");
    assert!(
        rejections.iter().all(|rejection| {
            rejection
                .reason
                .contains("compile-multi namespace has a competing or unresolved declaration")
        }),
        "{rejections:#?}"
    );

    let competing = source.replace(
        &format!("{CALL}\n"),
        &format!("{CALL}\n{}\n", CALL.replace("unit_ns", "other_ns")),
    );
    let fixture = Fixture::new();
    let (groups, rejections) = fixture.collect(&competing);
    assert!(groups.is_empty(), "{groups:#?}");
    assert!(
        rejections.iter().any(|rejection| {
            rejection
                .reason
                .contains("compile-multi outputs have competing declarations")
        }),
        "{rejections:#?}"
    );

    assert_rejected(
        &source.replace(
            "object-owner: $(GENDIR)/objects/unit.o $(GENDIR)/objects/other.o",
            "object-owner: $(GENDIR)/objects/unit.o $(GENDIR)/objects/other.o\n\t@echo alternate owner recipe",
        ),
        "compile-multi owner has a physical recipe",
    );
    assert_rejected(
        &source.replace(
            "CFLAGS := -O2",
            "unit_ns_CFLAGS := -DOVERRIDE=1\nCFLAGS := -O2",
        ),
        "compile-multi scope mutates template variable unit_ns_CFLAGS",
    );
    assert_rejected(
        &source.replace(
            "CFLAGS := -O2",
            "unit_ns_MC_TARGETS := $(GENDIR)/unreviewed.o\nCFLAGS := -O2",
        ),
        "compile-multi scope mutates template variable unit_ns_MC_TARGETS",
    );
    assert_rejected(
        &source.replace(
            ".PHONY: object-owner\n",
            ".PHONY: object-owner\nobject-owner: CFLAGS := -DOVERRIDE=1\n",
        ),
        "compile-multi scope has inherited target-specific assignments",
    );
}
