use super::*;
use crate::make_vars::collect_vars_impl;
use crate::testing::TempTree;
use std::path::PathBuf;

const CONFIG: &str = "AROS_DIR_AROS := SYS\n\
AROS_DIR_DEVELOPER := Developer\n\
AROS_DIR_INCLUDE := include\n\
AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n\
AROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\n\
AROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\n\
GENINCDIR := $(GENDIR)/include\n";

struct Fixture {
    root: TempTree,
    relative_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempTree::new();
        let relative_dir = PathBuf::from("compiler/include");
        fs::create_dir_all(root.0.join("config")).unwrap();
        fs::write(root.0.join("config/make.cfg.in"), CONFIG).unwrap();
        let input = root.0.join(&relative_dir).join("exec/execbase.inc");
        fs::create_dir_all(input.parent().unwrap()).unwrap();
        fs::write(
            &input,
            b"ThisTask;\nQuantum;\nElapsed;\nIDNestCnt;\nTDNestCnt;\n",
        )
        .unwrap();
        Self { root, relative_dir }
    }

    fn write_source(&self, relative: &str) {
        let path = self.root.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"source header\n").unwrap();
    }

    fn scan(
        &self,
        content: &str,
        states: Option<&[ConditionalTruth]>,
    ) -> (Vec<SourceHeaderPipelineDecl>, Vec<Rejection>) {
        let (scope, computed_states) =
            collect_vars_impl(content, Some(&crate::parser::TargetContext::default()));
        let dirs = DirVars::load(&self.root.0);
        collect(
            content,
            &scope,
            &dirs,
            &self.root.0,
            &self.relative_dir,
            Some(states.unwrap_or(computed_states.as_slice())),
        )
    }
}

fn fixture_text(exec_smp: &str, with_sed: bool) -> String {
    let sed = if with_sed {
        "ifneq ($(strip $(EXECSMP)),\"\")\n\
\t$(SED) -i -e 's/.*ThisTask;.*/    IPTR         SMPPrivate1;/' -e 's/.*Quantum;.*/    WORD         SMPPrivate2;/' -e 's/.*Elapsed;.*/    WORD         SMPPrivate3;/' -e 's/.*IDNestCnt;.*/    UBYTE        SMPPrivate4;/' -e 's/.*TDNestCnt;.*/    UBYTE        SMPPrivate5;/' $@\n\
endif\n"
    } else {
        ""
    };
    format!(
        "EXECSMP := {exec_smp}\n\
#MM\n\
includes-execbase_h : $(AROS_INCLUDES)/exec/execbase.h\n\
$(GENINCDIR)/exec/execbase.h : $(SRCDIR)/$(CURDIR)/exec/execbase.inc\n\
\t@$(ECHO) \"Copying generated header to $(GENINCDIR)...\"\n\
\t%mkdir_q dir=$(GENINCDIR)/exec\n\
\t@$(CP) $< $@\n\
{sed}\
$(AROS_INCLUDES)/exec/execbase.h : $(GENINCDIR)/exec/execbase.h\n\
\t@$(ECHO) \"Copying generated header to $(AROS_INCLUDES)...\"\n\
\t@$(CP) $< $@\n"
    )
}

#[test]
fn exact_source_copy_and_sdk_mirror_form_two_ordered_steps() {
    let fixture = Fixture::new();
    let (declarations, rejected) = fixture.scan(&fixture_text("\"\"", false), None);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(declarations.len(), 1);
    let declaration = &declarations[0];
    assert_eq!(declaration.owner, "includes-execbase_h");
    assert_eq!(
        declaration.generated_output,
        "${AROS_GENINC_DIR}/exec/execbase.h"
    );
    assert_eq!(
        declaration.sdk_output,
        "${AROS_SDK_INCLUDE_DIR}/exec/execbase.h"
    );
    assert_eq!(
        declaration.source_prerequisite,
        "${AROS_SOURCE_DIR}/compiler/include/exec/execbase.inc"
    );
    assert_eq!(declaration.steps.len(), 2);
    assert!(matches!(
        &declaration.steps[0],
        SourceHeaderPipelineStep::Copy { input, output, .. }
            if input == &declaration.source_prerequisite && output == &declaration.generated_output
    ));
    assert!(matches!(
        &declaration.steps[1],
        SourceHeaderPipelineStep::Copy { input, output, .. }
            if input == &declaration.generated_output && output == &declaration.sdk_output
    ));
}

#[test]
fn actual_execbase_owner_and_phony_shape_keeps_non_smp_pipeline() {
    let fixture = Fixture::new();
    let text = fixture_text("\"\"", true);
    let chain = text.strip_prefix("EXECSMP := \"\"\n").unwrap();
    let content = format!(
        "EXECSMP = \"\"\n\
#MM\n\
compiler-includes : setup $(DEST_INCLUDES) $(GEN_INCLUDES) includes-execbase_h\n\
{chain}.PHONY : includes-execbase_h\n"
    );

    let (declarations, rejected) = fixture.scan(&content, None);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].owner, "includes-execbase_h");
    assert_eq!(declarations[0].steps.len(), 2);
    assert!(declarations[0]
        .steps
        .iter()
        .all(|step| matches!(step, SourceHeaderPipelineStep::Copy { .. })));

    let unrelated_copy = content.replace(
        "$(AROS_INCLUDES)/exec/execbase.h : $(GENINCDIR)/exec/execbase.h",
        "$(AROS_INCLUDES)/exec/execbase.h : $(SRCDIR)/$(CURDIR)/exec/execbase.inc",
    );
    let (declarations, rejected) = fixture.scan(&unrelated_copy, None);
    assert!(declarations.is_empty());
    assert!(rejected.is_empty(), "{rejected:#?}");
}

#[test]
fn independent_source_to_sdk_and_gen_copies_are_not_a_pipeline() {
    let fixture = Fixture::new();
    let text = fixture_text("\"\"", false).replace(
        "$(AROS_INCLUDES)/exec/execbase.h : $(GENINCDIR)/exec/execbase.h",
        "$(AROS_INCLUDES)/exec/execbase.h : $(SRCDIR)/$(CURDIR)/exec/execbase.inc",
    );
    let (declarations, rejected) = fixture.scan(&text, None);
    assert!(declarations.is_empty());
    assert!(rejected.is_empty(), "{rejected:#?}");
}

#[test]
fn candidate_requires_source_cp_and_sdk_cp_pipeline_shape() {
    let fixture = Fixture::new();
    let normal = fixture_text("\"\"", false);

    let missing_generated_rule = normal.replace(
        "$(GENINCDIR)/exec/execbase.h : $(SRCDIR)/$(CURDIR)/exec/execbase.inc\n\
\t@$(ECHO) \"Copying generated header to $(GENINCDIR)...\"\n\
\t%mkdir_q dir=$(GENINCDIR)/exec\n\
\t@$(CP) $< $@\n",
        "",
    );
    let (declarations, rejected) = fixture.scan(&missing_generated_rule, None);
    assert!(declarations.is_empty());
    assert!(rejected.is_empty(), "{rejected:#?}");

    let host_generated = normal.replace(
        "$(GENINCDIR)/exec/execbase.h : $(SRCDIR)/$(CURDIR)/exec/execbase.inc\n\
\t@$(ECHO) \"Copying generated header to $(GENINCDIR)...\"\n\
\t%mkdir_q dir=$(GENINCDIR)/exec\n\
\t@$(CP) $< $@\n",
        "$(GENINCDIR)/exec/execbase.h : $(HOSTGENDIR)/tools/gen_execbase\n\
\t$(HOSTGENDIR)/tools/gen_execbase >$@\n",
    );
    let (declarations, rejected) = fixture.scan(&host_generated, None);
    assert!(declarations.is_empty());
    assert!(rejected.is_empty(), "{rejected:#?}");

    let source_copy_without_mirror = normal.replace(
        "$(AROS_INCLUDES)/exec/execbase.h : $(GENINCDIR)/exec/execbase.h\n\
\t@$(ECHO) \"Copying generated header to $(AROS_INCLUDES)...\"\n\
\t@$(CP) $< $@\n",
        "$(AROS_INCLUDES)/exec/execbase.h : $(GENINCDIR)/exec/execbase.h\n\
\t$(HOSTGENDIR)/tools/stage_execbase $< $@\n",
    );
    let (declarations, rejected) = fixture.scan(&source_copy_without_mirror, None);
    assert!(declarations.is_empty());
    assert!(rejected.is_empty(), "{rejected:#?}");
}

#[test]
fn source_copy_outside_declaring_directory_is_allowed_only_inside_source_root() {
    let fixture = Fixture::new();
    let source_relative = "rom/hidds/pci/include/pci_hidd.h";
    fixture.write_source(source_relative);
    let content = fixture_text("\"\"", false).replace(
        "$(SRCDIR)/$(CURDIR)/exec/execbase.inc",
        "$(SRCDIR)/rom/hidds/pci/include/pci_hidd.h",
    );
    let (declarations, rejected) = fixture.scan(&content, None);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        declarations[0].source_prerequisite,
        "${AROS_SOURCE_DIR}/rom/hidds/pci/include/pci_hidd.h"
    );

    let external_input = fixture_text("\"\"", false).replace(
        "$(SRCDIR)/$(CURDIR)/exec/execbase.inc",
        "$(GENDIR)/downloaded/execbase.inc",
    );
    let (declarations, rejected) = fixture.scan(&external_input, None);
    assert!(declarations.is_empty());
    assert!(
        rejected.is_empty(),
        "outside/fetched inputs are not candidates: {rejected:#?}"
    );
}

#[cfg(unix)]
#[test]
fn source_copy_symlinks_are_rejected_even_inside_selected_source_root() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let alias = fixture
        .root
        .0
        .join(&fixture.relative_dir)
        .join("exec/alias.inc");
    symlink("execbase.inc", &alias).unwrap();
    let content = fixture_text("\"\"", false).replace(
        "$(SRCDIR)/$(CURDIR)/exec/execbase.inc",
        "$(SRCDIR)/$(CURDIR)/exec/alias.inc",
    );
    let (declarations, rejected) = fixture.scan(&content, None);
    assert!(declarations.is_empty());
    assert!(
        rejected
            .iter()
            .any(|item| item.reason.contains("crosses a symlink")),
        "{rejected:#?}"
    );
}

#[test]
fn malformed_cp_and_unknown_copy_branch_remain_candidate_rejections() {
    let fixture = Fixture::new();
    let normal = fixture_text("\"\"", false);
    let wrong_cp = normal.replace("@$(CP) $< $@", "@$(CP) --force $< $@");
    let (declarations, rejected) = fixture.scan(&wrong_cp, None);
    assert!(declarations.is_empty());
    assert!(
        rejected
            .iter()
            .any(|item| item.reason.contains("unsupported recipe command")),
        "{rejected:#?}"
    );
    let generated_line = wrong_cp
        .lines()
        .position(|line| line.starts_with("$(GENINCDIR)/exec/execbase.h :"))
        .unwrap()
        + 1;
    assert!(rejected.iter().any(|item| item.line == generated_line));

    let guarded = normal
        .replacen(
            "$(GENINCDIR)/exec/execbase.h :",
            "ifeq ($(UNKNOWN_SWITCH),yes)\n$(GENINCDIR)/exec/execbase.h :",
            1,
        )
        .replacen("\t@$(CP) $< $@\n", "\t@$(CP) $< $@\nendif\n", 1);
    let (declarations, rejected) = fixture.scan(&guarded, None);
    assert!(declarations.is_empty());
    assert!(
        rejected
            .iter()
            .any(|item| item.reason.contains("unresolved Make conditional")),
        "{rejected:#?}"
    );
    let guarded_generated_line = guarded
        .lines()
        .position(|line| line.starts_with("$(GENINCDIR)/exec/execbase.h :"))
        .unwrap()
        + 1;
    assert!(rejected
        .iter()
        .any(|item| item.line == guarded_generated_line));
}

#[test]
fn configured_non_smp_literal_quote_empty_omits_the_conditional_sed_recipe() {
    let fixture = Fixture::new();
    let content = fixture_text("\"\"", true);
    let (scope, states) =
        collect_vars_impl(&content, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(&fixture.root.0);
    let (declarations, rejected) = collect(
        &content,
        &scope,
        &dirs,
        &fixture.root.0,
        &fixture.relative_dir,
        Some(&states),
    );
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].steps.len(), 2);
    assert!(declarations[0]
        .steps
        .iter()
        .all(|step| matches!(step, SourceHeaderPipelineStep::Copy { .. })));
}

#[test]
fn active_smp_branch_preserves_five_ordered_whole_line_replacements() {
    let fixture = Fixture::new();
    let content = fixture_text("smp", true);
    let (scope, states) =
        collect_vars_impl(&content, Some(&crate::parser::TargetContext::default()));
    let dirs = DirVars::load(&fixture.root.0);
    let (declarations, rejected) = collect(
        &content,
        &scope,
        &dirs,
        &fixture.root.0,
        &fixture.relative_dir,
        Some(&states),
    );
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].steps.len(), 7);
    let substitutions = declarations[0].steps[1..6]
        .iter()
        .map(|step| match step {
            SourceHeaderPipelineStep::ReplaceWholeLineInPlace {
                token, replacement, ..
            } => (token.as_str(), replacement.as_str()),
            other @ SourceHeaderPipelineStep::Copy { .. } => {
                panic!("expected in-place SED operation, got {other:?}");
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        substitutions,
        [
            ("ThisTask;", "    IPTR         SMPPrivate1;"),
            ("Quantum;", "    WORD         SMPPrivate2;"),
            ("Elapsed;", "    WORD         SMPPrivate3;"),
            ("IDNestCnt;", "    UBYTE        SMPPrivate4;"),
            ("TDNestCnt;", "    UBYTE        SMPPrivate5;"),
        ]
    );
    assert!(matches!(
        declarations[0].steps.last(),
        Some(SourceHeaderPipelineStep::Copy { input, output, .. })
            if input == &declarations[0].generated_output && output == &declarations[0].sdk_output
    ));
}

#[test]
fn unknown_sed_recipe_state_rejects_instead_of_becoming_non_smp() {
    let fixture = Fixture::new();
    let content = fixture_text("$(UNKNOWN_SWITCH)", true);
    let (scope, mut states) = collect_vars_impl(&content, None);
    let sed_line = content
        .lines()
        .position(|line| line.contains("$(SED)"))
        .unwrap();
    states[sed_line] = ConditionalTruth::Unknown;
    let dirs = DirVars::load(&fixture.root.0);
    let (declarations, rejected) = collect(
        &content,
        &scope,
        &dirs,
        &fixture.root.0,
        &fixture.relative_dir,
        Some(&states),
    );
    assert!(declarations.is_empty());
    assert_eq!(rejected.len(), 1);
    assert!(
        rejected[0].reason.contains("unknown conditional state"),
        "{rejected:#?}"
    );
}

#[test]
fn alternate_recipe_duplicate_owner_and_output_collision_are_refused() {
    let fixture = Fixture::new();
    let normal = fixture_text("\"\"", false);

    let alternate = normal.replace(
        "\t@$(CP) $< $@\n$(AROS_INCLUDES)",
        "\t@$(CP) $< $@\n\t@$(CP) foreign.h $@\n$(AROS_INCLUDES)",
    );
    let (declarations, rejected) = fixture.scan(&alternate, None);
    assert!(declarations.is_empty());
    assert!(
        rejected
            .iter()
            .any(|item| item.reason.contains("unsupported recipe command")),
        "{rejected:#?}"
    );

    let duplicate_owner =
        format!("{normal}\n#MM\nincludes-execbase_alias : $(AROS_INCLUDES)/exec/execbase.h\n");
    let (declarations, rejected) = fixture.scan(&duplicate_owner, None);
    assert!(declarations.is_empty());
    assert!(
        rejected
            .iter()
            .any(|item| item.reason.contains("claimed more than once")),
        "{rejected:#?}"
    );

    let duplicate_output = format!(
        "{normal}\n$(GENINCDIR)/exec/execbase.h : $(SRCDIR)/$(CURDIR)/exec/execbase.inc\n\t@$(CP) $< $@\n"
    );
    let (declarations, rejected) = fixture.scan(&duplicate_output, None);
    assert!(declarations.is_empty());
    assert!(
        rejected
            .iter()
            .any(|item| item.reason.contains("multiple Make producers")),
        "{rejected:#?}"
    );
}
