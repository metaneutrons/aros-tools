//! Regression tests for source-local host C generator admission.

use super::*;
use aros_common::native_host_generator::NativeHostFileInput;
use aros_common::Sha256Digest;
use std::fs;
use std::path::PathBuf;

const REL_DIR: &str = "compiler/crt/stdc";

fn declaration() -> NativeHostFileGenerator {
    NativeHostFileGenerator {
        owner: "compiler-stdc-genwcharsupport".into(),
        recipe: format!("{REL_DIR}/mmakefile.src"),
        tool_recipe: "tools/genctbl/Makefile".into(),
        tool_source: "tools/genctbl/genctbl.c".into(),
        tool_variable: "GENCTBL".into(),
        output: format!("gen/{REL_DIR}/defaults/en_GB_ISO8859-1.c"),
        input_directory: "gen/ucd".into(),
        compile_flags: ["-g", "-Wall", "-Werror", "-Wunused", "-O2"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        arguments: vec![
            "@INPUT_DIRECTORY@".into(),
            "@OUTPUT_DIRECTORY@".into(),
            "en_GB_ISO8859-1".into(),
            "--emit-c".into(),
        ],
        inputs: ["UnicodeData.txt", "SpecialCasing.txt"]
            .into_iter()
            .map(|filename| NativeHostFileInput {
                filename: filename.into(),
                url: format!("https://www.unicode.org/Public/17.0.0/ucd/{filename}"),
                sha256: Sha256Digest::parse(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .unwrap(),
                size: 1,
            })
            .collect(),
    }
}

fn delegated_declaration() -> NativeHostFileGenerator {
    declaration()
}

const MAKEFILE: &str = r#"compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c

$(GENDIR)/$(CURDIR)/defaults:
	%mkdirs_q $@

$(GENDIR)/ucd:
	%mkdirs_q $@

$(GENDIR)/ucd/%.txt: $(PORTSSOURCEDIR)/%.txt | $(GENDIR)/ucd
	@$(CP) $< $@

$(GENDIR)/$(CURDIR)/defaults/%.c: $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt | $(GENDIR)/$(CURDIR)/defaults
	@$(ECHO) "Generating $*.c";
	@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c;
"#;

const DELEGATED_MAKEFILE: &str = r#"compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c

$(GENDIR)/$(CURDIR)/defaults:
	%mkdirs_q $@

$(GENDIR)/ucd:
	%mkdirs_q $@

$(GENDIR)/ucd/UnicodeData.txt: $(GENCTBL) | $(GENDIR)/ucd
	@$(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP) all
	@test -s "$@"

$(GENDIR)/ucd/SpecialCasing.txt: $(GENDIR)/ucd/UnicodeData.txt
	@if ! test -s "$@"; then \
	    $(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP) all; \
	fi
	@test -s "$@"

$(GENDIR)/$(CURDIR)/defaults/%.c: $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt | $(GENDIR)/$(CURDIR)/defaults
	@$(ECHO) "Generating $*.c";
	@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c;
"#;

const DELEGATED_TOOL_MAKEFILE: &str = r#"USER_CFLAGS := -Wall -Werror -Wunused -O2

-include $(TOP)/config/make.cfg
-include Makefile.deps

HOST_CC ?= gcc
HOST_CFLAGS ?= $(USER_CFLAGS)
GENCTBL ?= genctbl
GENDIR ?= ./
MKDIR ?= mkdir
FETCH ?= $(SRCDIR)/scripts/fetch.sh

GENCTBL_UCD_VERSION := 17.0.0
GENCTBL_UCD_SHA256 := ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
GENCTBL_UCD_READY := $(GENDIR)/ucd/.ucd-$(GENCTBL_UCD_VERSION)-ready

ifneq ($(words $(wildcard $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt)),2)
.PHONY : $(GENCTBL_UCD_READY)
endif

all : $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt

$(PORTSSOURCEDIR) :
	@$(MKDIR) -p $@

$(GENDIR)/ucd :
	@$(MKDIR) -p $@

$(GENCTBL_UCD_READY) : $(SRCDIR)/tools/genctbl/Makefile | $(GENDIR)/ucd $(PORTSSOURCEDIR)
	@$(ECHO) "Preparing verified Unicode $(GENCTBL_UCD_VERSION) data..."
	@$(FETCH) -ao "https://www.unicode.org/Public/$(GENCTBL_UCD_VERSION)/ucd" \
	    -a UCD -s zip -l "$(PORTSSOURCEDIR)" -d "$(GENDIR)/ucd" -b "$(GENDIR)/ucd" \
	    -cs "UCD.zip=sha256:$(GENCTBL_UCD_SHA256)" -f
	@test -s "$(GENDIR)/ucd/UnicodeData.txt" -a -s "$(GENDIR)/ucd/SpecialCasing.txt"
	@touch "$@"

$(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt : $(GENCTBL_UCD_READY)
	@test -s "$@"

$(GENCTBL) : genctbl.c $(SRCDIR)/tools/genctbl/Makefile $(GENMODULE_DEPS)
	@$(ECHO) "Compiling $(notdir $@)..."
	@$(HOST_CC) -g $(HOST_CFLAGS) -I$(GENINCDIR) -I$(TOP)/$(CURDIR) genctbl.c -o $@
"#;

fn source_root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let declaration = declaration();
    write(&root, &declaration.recipe, MAKEFILE);
    write(
        &root,
        &declaration.tool_recipe,
        r#"USER_CFLAGS := -Wall -Werror -Wunused -O2

-include $(TOP)/config/make.cfg
-include Makefile.deps

HOST_CC ?= gcc
HOST_CFLAGS ?= $(USER_CFLAGS)
GENCTBL ?= genctbl

$(GENCTBL) : genctbl.c $(GENMODULE_DEPS)
	@$(ECHO) "Compiling $(notdir $@)..."
	@$(HOST_CC) -g $(HOST_CFLAGS) -I$(GENINCDIR) -I$(TOP)/$(CURDIR) genctbl.c -o $@
"#,
    );
    write(
        &root,
        &declaration.tool_source,
        "#include <stdio.h>\nint main(void) { return 0; }\n",
    );
    write(
        &root,
        "Makefile.in",
        r"TOP := @AROS_BUILDDIR@
SRCDIR := @SRCDIR@
$(GENCTBL): $(SRCDIR)/tools/genctbl/genctbl.c
	@$(ECHO) Building $(notdir $@)...
	@$(CALL) $(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP)
",
    );
    write(
        &root,
        "configure.in",
        "make_extra_commands=\"$make_extra_commands$export_newline\"\"GENCTBL\t:= $\"\"(TOOLDIR)/genctbl$\"\"(HOST_EXE_SUFFIX)$export_newline\"\n",
    );
    (temp, root)
}

fn delegated_source_root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let declaration = delegated_declaration();
    write(&root, &declaration.recipe, DELEGATED_MAKEFILE);
    write(&root, &declaration.tool_recipe, DELEGATED_TOOL_MAKEFILE);
    write(
        &root,
        &declaration.tool_source,
        "#include <stdio.h>\nint main(void) { return 0; }\n",
    );
    write(
        &root,
        "Makefile.in",
        r"TOP := @AROS_BUILDDIR@
SRCDIR := @SRCDIR@
$(GENCTBL): $(SRCDIR)/tools/genctbl/genctbl.c $(SRCDIR)/tools/genctbl/Makefile
	@$(ECHO) Building $(notdir $@)...
	@$(CALL) $(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP)
",
    );
    write(
        &root,
        "configure.in",
        "make_extra_commands=\"$make_extra_commands$export_newline\"\"GENCTBL\t:= $\"\"(TOOLDIR)/genctbl$\"\"(HOST_EXE_SUFFIX)$export_newline\"\n",
    );
    (temp, root)
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn validate_fixture(
    content: &str,
    root: &Path,
    declaration: &NativeHostFileGenerator,
    states: Option<&[ConditionalTruth]>,
) -> Result<(), String> {
    validate_source_rule(content, root, Path::new(REL_DIR), declaration, states)
}

#[test]
fn accepts_the_bounded_owner_generator_and_source_tool_chain() {
    let (_temp, root) = source_root();
    validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap();
    let joined = crate::parser::join_continuations(MAKEFILE);
    validate_fixture(&joined, &root, &declaration(), None).unwrap();
}

#[test]
fn accepts_the_bounded_delegated_archive_and_source_chain() {
    let (_temp, root) = delegated_source_root();
    validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap();
    let joined = crate::parser::join_continuations(DELEGATED_MAKEFILE);
    validate_fixture(&joined, &root, &delegated_declaration(), None).unwrap();
}

#[test]
fn delegated_chain_rejects_missing_input_and_both_producer_routes() {
    let (_temp, root) = delegated_source_root();
    let missing_start = DELEGATED_MAKEFILE
        .find("$(GENDIR)/ucd/SpecialCasing.txt:")
        .unwrap();
    let missing_end = DELEGATED_MAKEFILE[missing_start..]
        .find("$(GENDIR)/$(CURDIR)/defaults/%.c:")
        .unwrap()
        + missing_start;
    let missing = format!(
        "{}{}",
        &DELEGATED_MAKEFILE[..missing_start],
        &DELEGATED_MAKEFILE[missing_end..]
    );
    write(&root, &delegated_declaration().recipe, &missing);
    let error = validate_fixture(&missing, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("missing required Make rule"), "{error}");

    let both = format!(
        "{DELEGATED_MAKEFILE}\n$(GENDIR)/ucd/%.txt: $(PORTSSOURCEDIR)/%.txt | $(GENDIR)/ucd\n\t@$(CP) $< $@\n"
    );
    write(&root, &delegated_declaration().recipe, &both);
    let error = validate_fixture(&both, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("both copy-pattern and delegated"), "{error}");
}

#[test]
fn delegated_tool_chain_requires_declared_makefile_dependencies() {
    let (_temp, root) = delegated_source_root();
    let tool_path = root.join(delegated_declaration().tool_recipe);
    let original = fs::read_to_string(&tool_path).unwrap();
    let changed = original.replace(
        "genctbl.c $(SRCDIR)/tools/genctbl/Makefile $(GENMODULE_DEPS)",
        "genctbl.c $(GENMODULE_DEPS)",
    );
    assert_ne!(changed, original);
    write(&root, &delegated_declaration().tool_recipe, &changed);
    let error =
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("host tool prerequisites"), "{error}");

    let (_temp, root) = delegated_source_root();
    let top_path = root.join("Makefile.in");
    let original = fs::read_to_string(&top_path).unwrap();
    let changed = original.replace(
        "$(SRCDIR)/tools/genctbl/genctbl.c $(SRCDIR)/tools/genctbl/Makefile",
        "$(SRCDIR)/tools/genctbl/genctbl.c",
    );
    assert_ne!(changed, original);
    write(&root, "Makefile.in", &changed);
    let error =
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("top-level host tool target"), "{error}");

    let (_temp, root) = delegated_source_root();
    let tool_path = root.join(delegated_declaration().tool_recipe);
    let original = fs::read_to_string(&tool_path).unwrap();
    let changed = original.replace(
        "$(GENCTBL_UCD_READY) : $(SRCDIR)/tools/genctbl/Makefile |",
        "$(GENCTBL_UCD_READY) : |",
    );
    assert_ne!(changed, original);
    write(&root, &delegated_declaration().tool_recipe, &changed);
    let error =
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("ready rule has changed source"), "{error}");
}

#[test]
fn rejects_computed_assignment_names_in_each_closed_make_scope() {
    let computed_assignment_forms = [
        "$(FETCH_NAME) = /tmp/evil",
        "$(if yes,FETCH,OTHER) = /tmp/evil",
        "$(if yes,$(if yes,FETCH,OTHER),OTHER) = /tmp/evil",
        "override $(if yes,FETCH,OTHER) := /tmp/evil",
        "$(if yes,FETCH,\\\n    OTHER) = /tmp/evil",
        "override $(FETCH_NAME) := /tmp/evil",
        "private $(FETCH_NAME) += /tmp/evil",
        "another-target: $(FETCH_NAME) ?= /tmp/evil",
        "export $(FETCH_NAME) != echo evil",
    ];
    let dynamic_directive_forms = [
        (
            "define $(FETCH_NAME)\n/tmp/evil\nendef",
            "define block outside the closed Make capability",
        ),
        ("export $(FETCH_NAME)", "protected or dynamic Make variable"),
        (
            "undefine $(FETCH_NAME)",
            "protected or dynamic Make variable",
        ),
    ];

    let (_temp, root) = source_root();
    for assignment in computed_assignment_forms {
        let content = format!("FETCH_NAME := FETCH\n{assignment}\n\n{MAKEFILE}");
        write(&root, &declaration().recipe, &content);
        let error = validate_fixture(&content, &root, &declaration(), None).unwrap_err();
        assert!(
            error.contains("computed Make assignment name"),
            "{assignment:?}: {error}"
        );
    }
    for (directive, expected) in dynamic_directive_forms {
        let content = format!("FETCH_NAME := FETCH\n{directive}\n\n{MAKEFILE}");
        write(&root, &declaration().recipe, &content);
        let error = validate_fixture(&content, &root, &declaration(), None).unwrap_err();
        assert!(error.contains(expected), "{directive:?}: {error}");
    }

    let (_temp, root) = delegated_source_root();
    for assignment in computed_assignment_forms {
        let content = format!("MAKE_NAME := MAKE\n{assignment}\n\n{DELEGATED_MAKEFILE}");
        write(&root, &delegated_declaration().recipe, &content);
        let error = validate_fixture(&content, &root, &delegated_declaration(), None).unwrap_err();
        assert!(
            error.contains("computed Make assignment name"),
            "{assignment:?}: {error}"
        );
    }
    for (directive, expected) in dynamic_directive_forms {
        let content = format!("MAKE_NAME := MAKE\n{directive}\n\n{DELEGATED_MAKEFILE}");
        write(&root, &delegated_declaration().recipe, &content);
        let error = validate_fixture(&content, &root, &delegated_declaration(), None).unwrap_err();
        assert!(error.contains(expected), "{directive:?}: {error}");
    }

    let (_temp, root) = delegated_source_root();
    let tool_path = root.join(delegated_declaration().tool_recipe);
    let original = fs::read_to_string(&tool_path).unwrap();
    let changed = format!("$(if yes,FETCH,OTHER) = /tmp/evil\n{original}");
    write(&root, &delegated_declaration().tool_recipe, &changed);
    assert!(
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None)
            .unwrap_err()
            .contains("computed Make assignment name")
    );

    let (_temp, root) = delegated_source_root();
    let makefile_path = root.join("Makefile.in");
    let original = fs::read_to_string(&makefile_path).unwrap();
    let changed = format!("$(if yes,SRCDIR,OTHER) = /tmp/evil\n{original}");
    write(&root, "Makefile.in", &changed);
    assert!(
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None)
            .unwrap_err()
            .contains("computed Make assignment name")
    );
}

#[test]
fn rejects_continued_make_directives_and_eval_in_each_closed_scope() {
    let probes = [
        (
            ".RECIPEPREFIX := >\n\toverride SRCDIR := /tmp/evil\n.RECIPEPREFIX :=",
            "locally assigns protected Make variable .RECIPEPREFIX",
        ),
        (
            ".RECIPEPREFIX \\\n:= >\n\tdefine SAFE\n\t$(eval FETCH := altered)\n\tendef\n.RECIPEPREFIX :=",
            "locally assigns protected Make variable .RECIPEPREFIX",
        ),
        (
            "export .RECIPEPREFIX",
            "uses export for a protected or dynamic Make variable",
        ),
        (
            "define \\\nFETCH\nchanged\nendef",
            "define block outside the closed Make capability",
        ),
        (
            "define SAFE\n\t$(eval FETCH := altered)\nendef\nX := $(SAFE)",
            "define block outside the closed Make capability",
        ),
        (
            "undefine \\\nFETCH",
            "uses undefine for a protected or dynamic Make variable",
        ),
        (
            "export \\\nFETCH",
            "uses export for a protected or dynamic Make variable",
        ),
        (
            "unexport \\\nFETCH",
            "uses unexport for a protected or dynamic Make variable",
        ),
        (
            "X = $( \\\neval FETCH := altered)",
            "contains an unbounded eval expansion",
        ),
        (
            "override SRCDIR \\\n:= /tmp/evil",
            "uses Make modifiers or target scope for protected variable SRCDIR",
        ),
        (
            "$(GENCTBL): SRCDIR \\\n:= /tmp/evil",
            "uses Make modifiers or target scope for protected variable SRCDIR",
        ),
    ];

    for (probe, expected) in probes {
        let (_temp, root) = delegated_source_root();
        let source = format!("# preceding comment\n{probe}\n{DELEGATED_MAKEFILE}");
        write(&root, &delegated_declaration().recipe, &source);
        let joined = crate::parser::join_continuations(&source);
        for candidate in [&source, &joined] {
            let error =
                validate_fixture(candidate, &root, &delegated_declaration(), None).unwrap_err();
            assert!(error.contains(expected), "source {probe:?}: {error}");
            assert!(error.contains("line 2"), "source line mapping: {error}");
        }

        let (_temp, root) = delegated_source_root();
        let tool_makefile = format!("# preceding comment\n{probe}\n{DELEGATED_TOOL_MAKEFILE}");
        write(&root, &delegated_declaration().tool_recipe, &tool_makefile);
        let error = validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None)
            .unwrap_err();
        assert!(error.contains(expected), "tool Makefile {probe:?}: {error}");
        assert!(error.contains("line 2"), "tool line mapping: {error}");

        let (_temp, root) = delegated_source_root();
        let original = fs::read_to_string(root.join("Makefile.in")).unwrap();
        let makefile_in = format!("# preceding comment\n{probe}\n{original}");
        write(&root, "Makefile.in", &makefile_in);
        let error = validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None)
            .unwrap_err();
        assert!(error.contains(expected), "Makefile.in {probe:?}: {error}");
        assert!(error.contains("line 2"), "top-level line mapping: {error}");
    }
}

#[test]
fn dynamic_scope_guard_ignores_make_comments_and_recipe_text() {
    let content = concat!(
        "# X = $(eval FETCH := altered)\n",
        "# continued comment $(eval FETCH := \\\n",
        "# altered)\n",
        "dummy-target:\n",
        "\t@echo $(eval FETCH := altered) \\\n",
        "\t    still recipe text\n",
    );
    reject_dynamic_make_rebindings(content, &["FETCH"], &[], "fixture").unwrap();
}

#[test]
fn delegated_archive_rejects_version_hash_and_command_changes() {
    let (_temp, root) = delegated_source_root();
    let tool_path = root.join(delegated_declaration().tool_recipe);
    let original = fs::read_to_string(&tool_path).unwrap();
    for (changed, expected) in [
        (
            original.replace(
                "GENCTBL_UCD_VERSION := 17.0.0",
                "GENCTBL_UCD_VERSION := 16.0.0",
            ),
            "differs from the sealed input URL version",
        ),
        (
            original.replace(
                "GENCTBL_UCD_SHA256 := ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "GENCTBL_UCD_SHA256 := not-a-sha256",
            ),
            "64-character hexadecimal digest",
        ),
        (
            original.replace("\t@touch \"$@\"", "\t@touch \"$@\"\n\t@echo extra"),
            "exact continued recipe",
        ),
    ] {
        assert_ne!(changed, original);
        write(&root, &delegated_declaration().tool_recipe, &changed);
        let error = validate_fixture(
            DELEGATED_MAKEFILE,
            &root,
            &delegated_declaration(),
            None,
        )
        .unwrap_err();
        assert!(error.contains(expected), "expected {expected:?}, got {error}");
    }

    let changed = format!("SRCDIR := /tmp/other-source\n{DELEGATED_TOOL_MAKEFILE}");
    write(&root, &delegated_declaration().tool_recipe, &changed);
    let error =
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("protected Make variable SRCDIR"), "{error}");
}

#[test]
fn canonical_tool_assignments_are_continuation_aware_and_unique() {
    let (_temp, root) = delegated_source_root();
    let tool_path = root.join(delegated_declaration().tool_recipe);
    let original = fs::read_to_string(&tool_path).unwrap();
    let continued = original
        .replace(
            "USER_CFLAGS := -Wall -Werror -Wunused -O2",
            "USER_CFLAGS \\\n:= -Wall -Werror -Wunused -O2",
        )
        .replace(
            "HOST_CFLAGS ?= $(USER_CFLAGS)",
            "HOST_CFLAGS \\\n?= $(USER_CFLAGS)",
        )
        .replace("GENCTBL ?= genctbl", "GENCTBL \\\n?= genctbl")
        .replace(
            "GENCTBL_UCD_VERSION := 17.0.0",
            "GENCTBL_UCD_VERSION \\\n:= 17.0.0",
        )
        .replace(
            "GENCTBL_UCD_SHA256 := ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "GENCTBL_UCD_SHA256 \\\n:= ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )
        .replace(
            "GENCTBL_UCD_READY := $(GENDIR)/ucd/.ucd-$(GENCTBL_UCD_VERSION)-ready",
            "GENCTBL_UCD_READY \\\n:= $(GENDIR)/ucd/.ucd-$(GENCTBL_UCD_VERSION)-ready",
        )
        .replace(
            "FETCH ?= $(SRCDIR)/scripts/fetch.sh",
            "FETCH \\\n?= $(SRCDIR)/scripts/fetch.sh",
        );
    assert_ne!(continued, original);
    write(&root, &delegated_declaration().tool_recipe, &continued);
    validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap();

    for (duplicate, name) in [
        (
            format!(
                "{original}\nFETCH \\\n?= $(SRCDIR)/scripts/fetch.sh\n"
            ),
            "FETCH",
        ),
        (
            format!(
                "{original}\nGENCTBL_UCD_SHA256 \\\n:= ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n"
            ),
            "GENCTBL_UCD_SHA256",
        ),
    ] {
        write(&root, &delegated_declaration().tool_recipe, &duplicate);
        let error = validate_fixture(
            DELEGATED_MAKEFILE,
            &root,
            &delegated_declaration(),
            None,
        )
        .unwrap_err();
        assert!(error.contains(&format!("duplicate {name} assignments")), "{error}");
    }
}

#[test]
fn top_level_config_assignments_are_continuation_aware_unique_and_canonical() {
    let (_temp, root) = delegated_source_root();
    let original = fs::read_to_string(root.join("Makefile.in")).unwrap();
    let continued = original
        .replace("TOP := @AROS_BUILDDIR@", "TOP \\\n:= @AROS_BUILDDIR@")
        .replace("SRCDIR := @SRCDIR@", "SRCDIR \\\n:= @SRCDIR@");
    assert_ne!(continued, original);
    write(&root, "Makefile.in", &continued);
    validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap();

    for (duplicate, name) in [
        (format!("{original}\nTOP \\\n:= @AROS_BUILDDIR@\n"), "TOP"),
        (format!("{original}\nSRCDIR \\\n:= @SRCDIR@\n"), "SRCDIR"),
    ] {
        write(&root, "Makefile.in", &duplicate);
        let error = validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None)
            .unwrap_err();
        assert!(
            error.contains(&format!("duplicate {name} assignments")),
            "{error}"
        );
    }
}

#[test]
fn delegated_source_rejects_changed_continuation_duplicate_and_missing_chain_edges() {
    let (_temp, root) = delegated_source_root();
    let guard_start = DELEGATED_MAKEFILE
        .find("\t@if ! test -s \"$@\"; then \\")
        .unwrap();
    let guard_end = DELEGATED_MAKEFILE[guard_start..]
        .find("\n\t@test -s \"$@\"")
        .unwrap()
        + guard_start;
    let single_line = format!(
        "{}\t@if ! test -s \"$@\"; then $(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP) all; fi{}",
        &DELEGATED_MAKEFILE[..guard_start],
        &DELEGATED_MAKEFILE[guard_end..]
    );
    write(&root, &delegated_declaration().recipe, &single_line);
    let error = validate_fixture(&single_line, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("exact continued recipe"), "{error}");

    let duplicate = format!(
        "{DELEGATED_MAKEFILE}\n$(GENDIR)/ucd/UnicodeData.txt: $(GENCTBL)\n\t@test -s \"$@\"\n"
    );
    write(&root, &delegated_declaration().recipe, &duplicate);
    let error = validate_fixture(&duplicate, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("duplicate Make rules"), "{error}");

    let extra_command = DELEGATED_MAKEFILE.replace(
        "\t@test -s \"$@\"\n\n$(GENDIR)/ucd/SpecialCasing.txt",
        "\t@test -s \"$@\"\n\t@echo unexpected\n\n$(GENDIR)/ucd/SpecialCasing.txt",
    );
    assert_ne!(extra_command, DELEGATED_MAKEFILE);
    write(&root, &delegated_declaration().recipe, &extra_command);
    let joined_extra = crate::parser::join_continuations(&extra_command);
    for candidate in [&extra_command, &joined_extra] {
        let error = validate_fixture(candidate, &root, &delegated_declaration(), None).unwrap_err();
        assert!(
            error.contains("commands outside the closed recipe"),
            "{error}"
        );
    }

    let conditional = DELEGATED_MAKEFILE.replace(
        "$(GENDIR)/ucd/UnicodeData.txt: $(GENCTBL)",
        "ifeq ($(ENABLE_UCD),yes)\n$(GENDIR)/ucd/UnicodeData.txt: $(GENCTBL)",
    );
    assert_ne!(conditional, DELEGATED_MAKEFILE);
    write(&root, &delegated_declaration().recipe, &conditional);
    let joined_conditional = crate::parser::join_continuations(&conditional);
    for candidate in [&conditional, &joined_conditional] {
        let error = validate_fixture(candidate, &root, &delegated_declaration(), None).unwrap_err();
        assert!(error.contains("conditional or unresolved"), "{error}");
    }

    for (override_line, expected) in [
        ("SRCDIR := /tmp/other-source", "SRCDIR"),
        ("OTHER := $(eval MKARGS := -f /tmp/evil)", "eval"),
    ] {
        let changed = format!("{override_line}\n{DELEGATED_MAKEFILE}");
        write(&root, &delegated_declaration().recipe, &changed);
        let error = validate_fixture(&changed, &root, &delegated_declaration(), None).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }

    let missing_edge = DELEGATED_TOOL_MAKEFILE.replace(
        "all : $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt",
        "all : $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt",
    );
    write(&root, &delegated_declaration().recipe, DELEGATED_MAKEFILE);
    write(&root, &delegated_declaration().tool_recipe, &missing_edge);
    let error =
        validate_fixture(DELEGATED_MAKEFILE, &root, &delegated_declaration(), None).unwrap_err();
    assert!(error.contains("every declared input"), "{error}");
}

#[test]
fn derives_non_ucd_directories_and_input_output_suffixes() {
    let (_temp, root) = source_root();
    let mut declaration = declaration();
    declaration.input_directory = "gen/reference/unicode".into();
    declaration.inputs = ["Primary.csv", "Secondary.csv"]
        .into_iter()
        .map(|filename| NativeHostFileInput {
            filename: filename.into(),
            url: format!("https://example.invalid/reference/{filename}"),
            sha256: Sha256Digest::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .unwrap(),
            size: 1,
        })
        .collect();
    declaration.output = format!("gen/{REL_DIR}/generated/locale_variant.h");
    declaration.arguments[2] = "locale_variant".into();

    let custom = MAKEFILE
        .replace(
            "$(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c",
            "$(GENDIR)/$(CURDIR)/generated/locale_variant.h",
        )
        .replace(
            "$(GENDIR)/$(CURDIR)/defaults/%.c",
            "$(GENDIR)/$(CURDIR)/generated/%.h",
        )
        .replace(
            "$(GENDIR)/$(CURDIR)/defaults",
            "$(GENDIR)/$(CURDIR)/generated",
        )
        .replace(
            "$(GENDIR)/ucd/UnicodeData.txt",
            "$(GENDIR)/reference/unicode/Primary.csv",
        )
        .replace(
            "$(GENDIR)/ucd/SpecialCasing.txt",
            "$(GENDIR)/reference/unicode/Secondary.csv",
        )
        .replace("$(GENDIR)/ucd/%.txt", "$(GENDIR)/reference/unicode/%.csv")
        .replace("$(PORTSSOURCEDIR)/%.txt", "$(PORTSSOURCEDIR)/%.csv")
        .replace("$(GENDIR)/ucd", "$(GENDIR)/reference/unicode")
        .replace("$*.c", "$*.h");
    write(&root, &declaration.recipe, &custom);
    validate_fixture(&custom, &root, &declaration, None).unwrap();
}

#[test]
fn keeps_every_declared_input_as_a_normal_prerequisite() {
    let (_temp, root) = source_root();
    let altered = MAKEFILE.replace(" $(GENDIR)/ucd/SpecialCasing.txt |", " |");
    write(&root, &declaration().recipe, &altered);
    let error = validate_fixture(&altered, &root, &declaration(), None).unwrap_err();
    assert!(error.contains("every sealed input"), "{error}");
}

#[test]
fn rejects_extra_shell_and_changed_generator_mode() {
    let (_temp, root) = source_root();
    let altered = MAKEFILE.replace(
        "\t@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c;",
        "\t@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c; touch bad",
    );
    write(&root, &declaration().recipe, &altered);
    assert!(validate_fixture(&altered, &root, &declaration(), None).is_err());

    let altered = MAKEFILE.replace("$* --emit-c;", "$* --emit-binary;");
    write(&root, &declaration().recipe, &altered);
    assert!(validate_fixture(&altered, &root, &declaration(), None).is_err());
}

#[test]
fn rejects_unresolved_conditions_and_owner_additions() {
    let (_temp, root) = source_root();
    let mut states = vec![ConditionalTruth::True; MAKEFILE.lines().count()];
    let owner_line = MAKEFILE
        .lines()
        .position(|line| line.starts_with("compiler-stdc-genwcharsupport"))
        .unwrap();
    states[owner_line] = ConditionalTruth::Unknown;
    assert!(validate_fixture(MAKEFILE, &root, &declaration(), Some(&states)).is_err());

    let altered = MAKEFILE.replace(
        "compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c",
        "compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c extra",
    );
    write(&root, &declaration().recipe, &altered);
    assert!(validate_fixture(&altered, &root, &declaration(), None).is_err());
}

#[test]
fn rejects_source_make_variable_rebinding_forms() {
    let (_temp, root) = source_root();
    let declarations = [
        "override GENCTBL := /tmp/evil",
        "export GENCTBL := /tmp/evil",
        "other-owner: private GENCTBL := /tmp/evil",
        "other-owner: override export private GENCTBL := /tmp/evil",
        "define GENCTBL =\n/tmp/evil\nendef",
        "undefine GENCTBL",
        "OTHER := $(eval GENCTBL := /tmp/evil)",
    ];
    for rebinding in declarations {
        let altered = format!("{rebinding}\n\n{MAKEFILE}");
        write(&root, &declaration().recipe, &altered);
        let error = validate_fixture(&altered, &root, &declaration(), None).unwrap_err();
        let expected = if rebinding.starts_with("define ") {
            "define"
        } else if rebinding.starts_with("undefine ") {
            "undefine"
        } else if rebinding.contains("$(eval") {
            "eval"
        } else {
            "GENCTBL"
        };
        assert!(error.contains(expected), "{rebinding:?}: {error}");
    }
}

#[test]
fn rejects_tool_flag_assignments_with_make_modifiers() {
    let (_temp, root) = source_root();
    let tool_makefile_path = root.join(declaration().tool_recipe);
    let original = fs::read_to_string(&tool_makefile_path).unwrap();
    for assignment in [
        "HOST_CFLAGS ?= $(USER_CFLAGS)",
        "USER_CFLAGS := -Wall -Werror -Wunused -O2",
    ] {
        let changed = original.replace(assignment, &format!("override {assignment}"));
        assert_ne!(changed, original);
        write(&root, &declaration().tool_recipe, &changed);
        let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
        assert!(error.contains("Make modifiers"), "{error}");
    }
}

#[test]
fn binds_host_flags_and_rejects_quoted_project_includes() {
    let (_temp, root) = source_root();
    let mut changed = declaration();
    changed.compile_flags.pop();
    assert!(validate_fixture(MAKEFILE, &root, &changed, None).is_err());

    write(
        &root,
        &changed.tool_source,
        "#include \"local.h\"\nint main(void) { return 0; }\n",
    );
    assert!(validate_fixture(MAKEFILE, &root, &declaration(), None).is_err());

    write(
        &root,
        &changed.tool_source,
        "#include <private_project_header.h>\nint main(void) { return 0; }\n",
    );
    assert!(validate_fixture(MAKEFILE, &root, &declaration(), None).is_err());
}

#[test]
fn rejects_alternate_and_spliced_preprocessor_include_syntax() {
    let (_temp, root) = source_root();
    for source in [
        "%:include <private_project_header.h>\n",
        "#inc\\\nlude <private_project_header.h>\n",
        "#??=include <private_project_header.h>\n",
    ] {
        write(&root, &declaration().tool_source, source);
        let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
        assert!(
            error.contains("include") || error.contains("trigraph"),
            "{error}"
        );
    }
}

#[test]
fn rejects_source_local_shadows_of_standard_headers() {
    let (_temp, root) = source_root();
    write(
        &root,
        &declaration().tool_source,
        "#include <stdio.h>\nint main(void) { return 0; }\n",
    );
    write(&root, "tools/genctbl/stdio.h", "/* local shadow */\n");
    let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
    assert!(
        error.contains("shadow for standard header <stdio.h>"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_tool_sources() {
    use std::os::unix::fs::symlink;

    let (_temp, root) = source_root();
    let real = root.join("tools/genctbl/real.c");
    fs::write(&real, "#include <stdio.h>\n").unwrap();
    let target = root.join("tools/genctbl/genctbl.c");
    fs::remove_file(&target).unwrap();
    symlink(real, target).unwrap();
    assert!(validate_fixture(MAKEFILE, &root, &declaration(), None).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_source_local_header_shadows() {
    use std::os::unix::fs::symlink;

    let (_temp, root) = source_root();
    let real = root.join("tools/genctbl/real.h");
    fs::write(&real, "/* shadow */\n").unwrap();
    symlink(real, root.join("tools/genctbl/stdio.h")).unwrap();
    let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
    assert!(
        error.contains("shadow for standard header <stdio.h>"),
        "{error}"
    );
}

#[test]
#[ignore = "requires AROS_TEST_P4_SOURCE"]
fn actual_source_genctbl_recipe_is_admitted() {
    let root = PathBuf::from(
        std::env::var_os("AROS_TEST_P4_SOURCE")
            .expect("set AROS_TEST_P4_SOURCE to the selected AROS source tree"),
    );
    let root = root.canonicalize().expect("source tree must exist");
    let contract_path = root.join("arch/riscv-esp32p4/native-sdk/consumer-v1.json");
    let contract: serde_json::Value =
        serde_json::from_slice(&fs::read(contract_path).unwrap()).unwrap();
    let generators: Vec<NativeHostFileGenerator> = serde_json::from_value(
        contract
            .get("host_file_generators")
            .cloned()
            .expect("source consumer contract must declare host file generators"),
    )
    .unwrap();
    let declaration = generators
        .into_iter()
        .find(|declaration| declaration.recipe == format!("{REL_DIR}/mmakefile.src"))
        .expect("source consumer must declare the stdc host generator");
    let content = fs::read_to_string(root.join(&declaration.recipe)).unwrap();
    validate_source_rule(&content, &root, Path::new(REL_DIR), &declaration, None).unwrap();
    let joined = crate::parser::join_continuations(&content);
    validate_source_rule(&joined, &root, Path::new(REL_DIR), &declaration, None).unwrap();
}
