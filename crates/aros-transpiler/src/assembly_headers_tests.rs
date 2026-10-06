use super::*;
use std::collections::BTreeMap;
use tempfile::TempDir;

const SNAPSHOT: &str = r#"# physical line mapping \
continuation line
OBJDIR := $(GENDIR)/header
GENINCDIR := $(GENDIR)/include
CPU := $(AROS_TARGET_CPU)
#MM
provider-$(CPU) : $(GENINCDIR)/generated/$(CPU)/table.h
#MM aggregate : prep provider-$(CPU)
#MM
aggregate:
	@$(NOP)
ifeq ($(AROS_TOOLCHAIN),gnu)
GREPTOKEN := ".asciz"
else
GREPTOKEN := ".ascii"
endif
$(OBJDIR)/table.s : $(SRCDIR)/$(CURDIR)/table.c | $(OBJDIR)
	@$(ECHO) "Compiling  $<..."
	@$(TARGET_CC) $(TARGET_SYSROOT) $(CFLAGS) $(PRIV_EXEC_INCLUDES) -S $< -o $@
$(GENINCDIR)/generated/$(CPU)/table.h : $(OBJDIR)/table.s | $(GENINCDIR)/generated/$(AROS_TARGET_CPU)
	@$(ECHO) Generating $@...
	@grep $(GREPTOKEN) $< | cut -d'"' -f2 | sed 's/\$$//g' >$@
"#;

fn fixture() -> (TempDir, PathBuf, TargetContext, DirVars) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let directory = root.join("hardware/header");
    fs::create_dir_all(&directory).expect("source directory");
    fs::write(directory.join("mmakefile.src"), SNAPSHOT).expect("mmakefile");
    fs::write(directory.join("table.c"), "int table;\n").expect("C input");

    let mut make_variables = BTreeMap::new();
    make_variables.insert("TARGET_CC".into(), "$(NATIVE_TARGET_CC)".into());
    make_variables.insert(
        "TARGET_SYSROOT".into(),
        "--sysroot=$(AROS_DEVELOPER)".into(),
    );
    make_variables.insert(
        "AROS_DEVELOPER".into(),
        "$(AROS_BUILD_DIR)/Developer".into(),
    );
    make_variables.insert("CFLAGS".into(), "-O2 -DTEST_FLAG=1".into());
    make_variables.insert(
        "PRIV_EXEC_INCLUDES".into(),
        "-I$(SRCDIR)/rom/exec -I$(SRCDIR)/rom/kernel".into(),
    );
    let target = TargetContext {
        cpu: Some("othercpu".into()),
        toolchain: Some("gnu".into()),
        make_variables,
        ..TargetContext::default()
    };
    let mut dirs = DirVars::load(root);
    dirs.bind_native_target_tool_roles();
    (temp, directory, target, dirs)
}

#[test]
fn collects_generic_provider_and_resolved_assembly_argv() {
    let (_temp, directory, target, dirs) = fixture();
    let (decls, rejections) = collect_from_snapshot(
        SNAPSHOT,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );

    assert!(rejections.is_empty(), "{rejections:?}");
    let [decl] = decls.as_slice() else {
        panic!("expected one declaration, got {decls:?}");
    };
    assert_eq!(decl.owner, "provider-othercpu");
    assert_eq!(
        decl.line,
        SNAPSHOT
            .lines()
            .position(|line| line.starts_with("provider-"))
            .expect("provider physical line")
            + 1
    );
    assert_eq!(decl.aggregate_owner, "aggregate");
    assert_eq!(decl.aggregate_dependencies, ["prep", "provider-othercpu"]);
    assert_eq!(decl.source, "${AROS_SOURCE_DIR}/header/table.c");
    assert_eq!(decl.assembly_output, "${AROS_BUILD_DIR}/gen/header/table.s");
    assert_eq!(
        decl.header_output,
        "${AROS_BUILD_DIR}/gen/include/generated/othercpu/table.h"
    );
    assert_eq!(decl.header_root, "${AROS_BUILD_DIR}/gen/include");
    assert_eq!(decl.token, ".asciz");
    assert_eq!(
        decl.arguments,
        [
            "--sysroot=${AROS_BUILD_DIR}/Developer",
            "-O2",
            "-DTEST_FLAG=1",
            "-I",
            "${AROS_SOURCE_DIR}/rom/exec",
            "-I",
            "${AROS_SOURCE_DIR}/rom/kernel",
        ]
    );
}

#[test]
fn rejects_an_oversized_physical_mmakefile_even_with_small_snapshot() {
    let (_temp, directory, target, dirs) = fixture();
    let oversized = format!("{SNAPSHOT}{}", "x".repeat(MAX_SNAPSHOT_BYTES + 1));
    fs::write(directory.join("mmakefile.src"), oversized).expect("oversized mmakefile");

    let (decls, rejections) = collect_from_snapshot(
        SNAPSHOT,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );

    assert!(decls.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("read limit")));
}

#[test]
fn ignores_ordinary_sed_and_source_header_rules() {
    let (_temp, directory, target, dirs) = fixture();
    let extended = format!(
        "{SNAPSHOT}\n$(GENINCDIR)/pkgconfig.h : $(SRCDIR)/pkgconfig.in\n\t@sed 's/old/new/' $< >$@\n$(GENINCDIR)/source.h : $(SRCDIR)/include.src\n\t@sed 's/old/new/' $< >$@\n"
    );
    fs::write(directory.join("mmakefile.src"), &extended).expect("extended mmakefile");

    let (decls, rejections) = collect_from_snapshot(
        &extended,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );

    assert_eq!(decls.len(), 1);
    assert!(rejections.is_empty(), "{rejections:?}");
}

#[test]
fn rejects_unresolved_compiler_flags_instead_of_guessing() {
    let (_temp, directory, mut target, dirs) = fixture();
    target.make_variables.remove("CFLAGS");
    let (decls, rejections) = collect_from_snapshot(
        SNAPSHOT,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );
    assert!(decls.is_empty());
    assert!(rejections.iter().any(|item| item.reason.contains("CFLAGS")));
}

#[test]
fn rejects_compiler_output_and_mode_flags() {
    for flag in ["-oother", "-E", "--", "-fsyntax-only"] {
        let (_temp, directory, mut target, dirs) = fixture();
        target.make_variables.insert("CFLAGS".into(), flag.into());
        let (decls, rejections) = collect_from_snapshot(
            SNAPSHOT,
            &target,
            &dirs,
            directory.parent().expect("fixture root"),
            Path::new("header"),
        );
        assert!(decls.is_empty(), "accepted forbidden mode flag {flag}");
        assert!(
            rejections
                .iter()
                .any(|item| item.reason.contains("compiler flag")),
            "missing rejection for {flag}: {rejections:?}"
        );
    }
}

#[test]
fn rejects_unknown_token_condition() {
    let (_temp, directory, mut target, dirs) = fixture();
    target.toolchain = None;
    let (decls, rejections) = collect_from_snapshot(
        SNAPSHOT,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );
    assert!(decls.is_empty());
    assert!(rejections
        .iter()
        .any(|item| { item.reason.contains("GREPTOKEN") || item.reason.contains("conditional") }));
}

#[test]
fn rejects_include_escape_and_changed_pipeline_recipe() {
    let (_temp, directory, mut target, dirs) = fixture();
    target.make_variables.insert(
        "PRIV_EXEC_INCLUDES".into(),
        "-I$(SRCDIR)/../../outside".into(),
    );
    let changed = SNAPSHOT.replace(
        "@grep $(GREPTOKEN) $< | cut -d'\"' -f2 | sed 's/\\$$//g' >$@",
        "@grep $(GREPTOKEN) $< >$@",
    );
    fs::write(directory.join("mmakefile.src"), &changed).expect("changed source");
    let (decls, rejections) = collect_from_snapshot(
        &changed,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );
    assert!(decls.is_empty());
    assert!(rejections
        .iter()
        .any(|item| { item.reason.contains("pipeline") || item.reason.contains("include path") }));
}

#[test]
fn rejects_duplicate_header_rules() {
    let (_temp, directory, target, dirs) = fixture();
    let duplicated = format!(
        "{SNAPSHOT}\n$(GENINCDIR)/generated/$(CPU)/table.h : $(OBJDIR)/table.s | $(OBJDIR)\n\t@$(ECHO) Generating $@...\n\t@grep $(GREPTOKEN) $< | cut -d'\"' -f2 | sed 's/\\$$//g' >$@\n"
    );
    fs::write(directory.join("mmakefile.src"), &duplicated).expect("duplicated source");
    let (decls, rejections) = collect_from_snapshot(
        &duplicated,
        &target,
        &dirs,
        directory.parent().expect("fixture root"),
        Path::new("header"),
    );
    assert!(decls.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("multiple Make rules")));
}
