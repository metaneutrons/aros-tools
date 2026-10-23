use super::{
    project_selected as project_with_resolver, MakeProgramOrigin, PositionedMesaMakeProgram,
};
use crate::dirs::DirVars;
use crate::genmf_projection::{expand_text, ExpandedLineProvenance, ExpandedText, Limits};
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::collect_vars_with_context;
use crate::parser::TargetContext;
use aros_common::mesa_genmodule_inventory::MesaGenmoduleProducer;
use aros_common::mesa_genmodule_make::{MesaGenmoduleMakefile, MESA_GENMODULE_MAKE_SUFFIXES};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const MODULE: &str = "mesa3dgl26-0";
const DECLARATION_DIRECTORY: &str = "workbench/libs/mesa";
const OWNER_ALPHA: &str = "mesa-gl-alpha";
const OWNER_BRAVO: &str = "mesa-gl-bravo";

const TEMPLATE: &str = "\
%define build_module_core mmake=/A modname=/A modtype=/A flavour= conffile= confoverride=\n\
%(mmake)_BEFORE := $(USER_CPPFLAGS)\n\
-include $(%(mmake)%(flavour)-makefile)\n\
%(mmake)_RECIPE := $(%(modname)_LINKLIBCFLAGS) $(USER_CPPFLAGS)\n\
%end\n\
%define common\n\
# synthetic common terminator\n\
%end\n";

struct Fixture {
    _temporary: tempfile::TempDir,
    source_root: PathBuf,
    source_file: PathBuf,
    source: String,
    expanded: ExpandedText,
    alpha: MesaGenmoduleProducer,
    bravo: MesaGenmoduleProducer,
    alpha_makefile: MesaGenmoduleMakefile,
    bravo_makefile: MesaGenmoduleMakefile,
}

fn producer(owner: &str, module_type: &str) -> MesaGenmoduleProducer {
    producer_for(
        owner,
        MODULE,
        module_type,
        format!("{DECLARATION_DIRECTORY}/{MODULE}.conf"),
        None,
    )
}

fn producer_for(
    owner: &str,
    module: &str,
    module_type: &str,
    base_configuration: String,
    override_configuration: Option<String>,
) -> MesaGenmoduleProducer {
    MesaGenmoduleProducer {
        owner: owner.to_owned(),
        declaration_directory: DECLARATION_DIRECTORY.to_owned(),
        module: module.to_owned(),
        module_type: module_type.to_owned(),
        base_configuration,
        override_configuration,
    }
}

fn makefile(linklib_cflags: &str) -> MesaGenmoduleMakefile {
    makefile_for(MODULE, linklib_cflags)
}

fn makefile_for(module: &str, linklib_cflags: &str) -> MesaGenmoduleMakefile {
    let mut text = String::new();
    for suffix in MESA_GENMODULE_MAKE_SUFFIXES {
        let value = if suffix == "LINKLIBCFLAGS" {
            linklib_cflags
        } else {
            ""
        };
        writeln!(
            text,
            "{module}_{suffix} +={}",
            if value.is_empty() {
                String::new()
            } else {
                format!(" {value}")
            }
        )
        .unwrap();
    }
    MesaGenmoduleMakefile::parse(module, &text).unwrap()
}

fn expand_fixture(source: &str) -> (tempfile::TempDir, PathBuf, PathBuf, ExpandedText) {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().canonicalize().unwrap();
    let config = source_root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let template_file = config.join("make.tmpl");
    std::fs::write(&template_file, TEMPLATE).unwrap();
    let source_file = source_root
        .join(DECLARATION_DIRECTORY)
        .join("mmakefile.src");
    let expanded = expand_text(source, &source_file, &template_file, Limits::default()).unwrap();
    (temporary, source_root, source_file, expanded)
}

fn fixture() -> Fixture {
    let source = format!(
        concat!(
            "USER_CPPFLAGS := -DEARLY\n",
            "GCA_ABI_FLAGS := $(USER_CPPFLAGS)\n",
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library\n",
            "USER_CPPFLAGS += -DLATE\n",
            "%build_module_core mmake={OWNER_BRAVO} modname={MODULE} modtype=hidd\n",
            "%common\n",
        ),
        OWNER_ALPHA = OWNER_ALPHA,
        OWNER_BRAVO = OWNER_BRAVO,
        MODULE = MODULE,
    );
    fixture_from_source(source)
}

fn fixture_from_source(source: String) -> Fixture {
    let (temporary, source_root, source_file, expanded) = expand_fixture(&source);
    Fixture {
        _temporary: temporary,
        source_root,
        source_file,
        source,
        expanded,
        alpha: producer(OWNER_ALPHA, "library"),
        bravo: producer(OWNER_BRAVO, "hidd"),
        alpha_makefile: makefile("-DALPHA"),
        bravo_makefile: makefile("-DBRAVO"),
    }
}

#[test]
fn later_include_resolution_sees_prior_imports_but_never_future_assignments() {
    let source = format!(
        "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library\n\
         %build_module_core mmake={OWNER_BRAVO} modname={MODULE} modtype=hidd conffile=\"$(if $({MODULE}_LINKLIBCFLAGS),{MODULE}.conf,wrong.conf)\"\n\
         {MODULE}_LINKLIBCFLAGS :=\n"
    );
    let fixture = fixture_from_source(source);
    let dirs = DirVars::default();
    let resolve = |expression: &str, _origin: &ExpandedLineProvenance, prefix: &str| {
        let scope = collect_vars_with_context(prefix, &TargetContext::default());
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            usize::MAX,
            &fixture.source_root,
            Path::new(DECLARATION_DIRECTORY),
        )
        .without_filesystem();
        evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
    };
    let result = super::project_selected_with_prefix(
        &fixture.source_root,
        &fixture.expanded,
        &selected(&fixture),
        &|_, _| Ok(Some(true)),
        &resolve,
    )
    .unwrap();
    assert_full_span_coverage(&result);
    assert_eq!(imported_spans(&result).len(), 2);
    let stale = |expression: &str, origin: &ExpandedLineProvenance, prefix: &str| {
        resolve(expression, origin, &prefix.replace("-DALPHA", ""))
    };
    assert!(super::project_selected_with_prefix(
        &fixture.source_root,
        &fixture.expanded,
        &selected(&fixture),
        &|_, _| Ok(Some(true)),
        &stale
    )
    .unwrap_err()
    .contains("configuration differs"));
    let future = |expression: &str, origin: &ExpandedLineProvenance, prefix: &str| {
        resolve(
            expression,
            origin,
            &format!("{prefix}\n{MODULE}_LINKLIBCFLAGS :=\n"),
        )
    };
    assert!(super::project_selected_with_prefix(
        &fixture.source_root,
        &fixture.expanded,
        &selected(&fixture),
        &|_, _| Ok(Some(true)),
        &future
    )
    .unwrap_err()
    .contains("configuration differs"));
}

#[test]
fn include_branch_selection_precedes_lookup_and_unknown_selection_refuses() {
    let run = |condition: &str| {
        let fixture = fixture_from_source(format!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library\n\
             {condition}\n%build_module_core mmake={OWNER_BRAVO} modname={MODULE} modtype=hidd\nendif\n"
        ));
        let active = |_origin: &ExpandedLineProvenance, prefix: &str| {
            let (_, states) = crate::make_vars::collect_vars_impl(
                &format!("{prefix}# include position\n"),
                Some(&TargetContext::default()),
            );
            Ok(states.last().and_then(|state| match state {
                crate::make_vars::ConditionalTruth::True => Some(true),
                crate::make_vars::ConditionalTruth::False => Some(false),
                crate::make_vars::ConditionalTruth::Unknown => None,
            }))
        };
        super::project_selected_with_prefix(
            &fixture.source_root,
            &fixture.expanded,
            &[(&fixture.alpha, &fixture.alpha_makefile)],
            &active,
            &|expression, _, _| Ok(expression.to_owned()),
        )
    };
    let inactive = run("ifeq (x,y)").unwrap();
    assert_eq!(imported_spans(&inactive).len(), 1);
    assert!(inactive
        .text
        .contains(&format!("-include $({OWNER_BRAVO}-makefile)")));
    assert!(run("ifeq (x,x)")
        .unwrap_err()
        .contains("no selected producer"));
    assert!(run("ifeq ($(UNMEASURED),x)")
        .unwrap_err()
        .contains("unresolved branch"));
}

#[test]
fn repeated_include_callbacks_share_a_cumulative_prefix_work_budget() {
    let run = |copies| {
        let mut source = format!("#{}\n", "x".repeat(512 * 1024));
        for _ in 0..copies {
            writeln!(
                source,
                "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library"
            )
            .unwrap();
        }
        let fixture = fixture_from_source(source);
        super::project_selected_with_prefix(
            &fixture.source_root,
            &fixture.expanded,
            &[(&fixture.alpha, &fixture.alpha_makefile)],
            &|_, _| Ok(Some(true)),
            &|expression, _, _| Ok(expression.to_owned()),
        )
    };
    assert!(run(4).is_ok());
    assert!(run(16)
        .unwrap_err()
        .contains("cumulative prefix work limit"));
}

/// A compact adapter for the older literal-only cases. Identity resolution is
/// sufficient there; tests of variable binding use `source_resolver` below.
fn project(
    source_root: &Path,
    expanded: &ExpandedText,
    selected: &[(&MesaGenmoduleProducer, &MesaGenmoduleMakefile)],
) -> Result<PositionedMesaMakeProgram, String> {
    project_with_resolver(source_root, expanded, selected, &|expression, _| {
        Ok(expression.to_owned())
    })
}

/// Evaluate Source Make expressions from their original positional scope.
fn source_resolver<'a>(
    source_root: &'a Path,
    source: &str,
) -> impl Fn(&str, &ExpandedLineProvenance) -> Result<String, String> + 'a {
    let scope = collect_vars_with_context(source, &TargetContext::default());
    let dirs = DirVars::default();
    move |expression, origin| {
        let line = origin
            .top_level_source_invocation
            .as_ref()
            .ok_or("Source expression has no original invocation")?
            .line
            .saturating_sub(1);
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            line,
            source_root,
            Path::new(DECLARATION_DIRECTORY),
        )
        .without_filesystem();
        evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
    }
}

fn selected(fixture: &Fixture) -> [(&MesaGenmoduleProducer, &MesaGenmoduleMakefile); 2] {
    [
        (&fixture.alpha, &fixture.alpha_makefile),
        (&fixture.bravo, &fixture.bravo_makefile),
    ]
}

fn imported_spans(program: &PositionedMesaMakeProgram) -> Vec<(String, String, usize, usize)> {
    program
        .spans
        .iter()
        .filter_map(|span| match &span.origin {
            MakeProgramOrigin::Genmodule { owner, module, .. } => Some((
                owner.clone(),
                module.clone(),
                span.start_byte,
                span.end_byte,
            )),
            MakeProgramOrigin::Genmf(_) => None,
        })
        .collect()
}

fn assert_full_span_coverage(program: &PositionedMesaMakeProgram) {
    let mut next_byte = 0;
    for span in &program.spans {
        assert_eq!(
            span.start_byte, next_byte,
            "origin spans have a gap/overlap"
        );
        assert!(span.end_byte > span.start_byte, "origin span is empty");
        assert!(program.text.get(span.start_byte..span.end_byte).is_some());
        next_byte = span.end_byte;
    }
    assert_eq!(next_byte, program.text.len());
}

fn make_values(program: &PositionedMesaMakeProgram, expression: &str) -> Vec<String> {
    let scope = collect_vars_with_context(&program.text, &TargetContext::default());
    let dirs = DirVars::default();
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        program.text.lines().count(),
        Path::new("/selected/source"),
        Path::new(DECLARATION_DIRECTORY),
    );
    evaluate_make_list(expression, &context).unwrap()
}

#[test]
fn imports_each_owner_at_its_provenance_site_and_freezes_immediate_values() {
    let fixture = fixture();
    assert_eq!(
        fixture.source_file,
        fixture
            .source_root
            .join(DECLARATION_DIRECTORY)
            .join("mmakefile.src")
    );
    let program = project(&fixture.source_root, &fixture.expanded, &selected(&fixture)).unwrap();

    assert_full_span_coverage(&program);
    assert!(program
        .spans
        .iter()
        .any(|span| matches!(&span.origin, MakeProgramOrigin::Genmf(_))));
    let imported = imported_spans(&program);
    assert_eq!(imported.len(), 2);
    assert_eq!(imported[0].0, OWNER_ALPHA);
    assert_eq!(imported[1].0, OWNER_BRAVO);
    assert_eq!(imported[0].1, MODULE);
    assert_eq!(imported[1].1, MODULE);
    assert!(imported[0].2 < imported[1].2);

    for (span, expected) in program
        .spans
        .iter()
        .filter(|span| matches!(&span.origin, MakeProgramOrigin::Genmodule { .. }))
        .zip([
            fixture.alpha_makefile.canonical_make_fragment(),
            fixture.bravo_makefile.canonical_make_fragment(),
        ])
    {
        assert_eq!(
            program.text.get(span.start_byte..span.end_byte),
            Some(expected.as_str())
        );
        let MakeProgramOrigin::Genmodule { include, .. } = &span.origin else {
            unreachable!();
        };
        assert_eq!(
            include.top_level_source_invocation.as_ref().unwrap().path,
            fixture.source_file
        );
        assert!(include
            .macro_stack
            .iter()
            .any(|frame| frame.name == "build_module_core"));
    }

    assert_eq!(make_values(&program, "$(GCA_ABI_FLAGS)"), vec!["-DEARLY"]);
    assert_eq!(
        make_values(&program, "$(mesa-gl-alpha_BEFORE)"),
        vec!["-DEARLY"]
    );
    assert_eq!(
        make_values(&program, "$(mesa-gl-alpha_RECIPE)"),
        vec!["-DALPHA", "-DEARLY"]
    );
    assert_eq!(
        make_values(&program, "$(mesa-gl-bravo_RECIPE)"),
        vec!["-DALPHA", "-DBRAVO", "-DEARLY", "-DLATE"]
    );
}

#[test]
fn repeated_physical_imports_preserve_make_condition_and_order() {
    let source = format!(
        concat!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library\n",
            "ifeq ($(IMPORT_AGAIN),yes)\n",
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library\n",
            "endif\n",
            "%common\n",
        ),
        OWNER_ALPHA = OWNER_ALPHA,
        MODULE = MODULE,
    );
    let (temporary, source_root, _source_file, expanded) = expand_fixture(&source);
    let producer = producer(OWNER_ALPHA, "library");
    let generated = makefile("-DALPHA");
    let program = project(&source_root, &expanded, &[(&producer, &generated)]).unwrap();

    assert_full_span_coverage(&program);
    let imported = imported_spans(&program);
    assert_eq!(imported.len(), 2);
    assert!(imported
        .iter()
        .all(|(owner, module, _, _)| owner == OWNER_ALPHA && module == MODULE));
    assert!(imported[0].2 < imported[1].2);
    let first_end = imported[0].3;
    let second_start = imported[1].2;
    let between = &program.text[first_end..second_start];
    assert!(between.contains("ifeq ($(IMPORT_AGAIN),yes)"));
    assert_eq!(
        program
            .text
            .matches(&generated.canonical_make_fragment())
            .count(),
        2
    );
    drop(temporary);
}

#[test]
fn refuses_owner_module_type_location_and_selection_mismatches() {
    let fixture = fixture();
    let selected = selected(&fixture);

    assert!(project(&fixture.source_root, &fixture.expanded, &selected[..1]).is_err());

    let duplicate_owner = [
        (&fixture.alpha, &fixture.alpha_makefile),
        (&fixture.alpha, &fixture.alpha_makefile),
    ];
    assert!(project(&fixture.source_root, &fixture.expanded, &duplicate_owner).is_err());

    let extra = producer("mesa-gl-extra", "library");
    let extra_makefile = makefile("-DEXTRA");
    let with_extra = [selected[0], selected[1], (&extra, &extra_makefile)];
    assert!(project(&fixture.source_root, &fixture.expanded, &with_extra).is_err());

    let mut wrong_module = producer(OWNER_ALPHA, "library");
    let wrong_module_makefile = makefile("-DALPHA");
    wrong_module.module = "different-module".to_owned();
    assert!(project(
        &fixture.source_root,
        &fixture.expanded,
        &[(&wrong_module, &wrong_module_makefile), selected[1]],
    )
    .is_err());

    let mut wrong_type = fixture.bravo.clone();
    wrong_type.module_type = "library".to_owned();
    assert!(project(
        &fixture.source_root,
        &fixture.expanded,
        &[selected[0], (&wrong_type, &fixture.bravo_makefile)],
    )
    .is_err());

    let mut wrong_directory = fixture.bravo.clone();
    wrong_directory.declaration_directory = "workbench/libs/other".to_owned();
    assert!(project(
        &fixture.source_root,
        &fixture.expanded,
        &[selected[0], (&wrong_directory, &fixture.bravo_makefile)],
    )
    .is_err());
}

#[test]
fn refuses_changed_include_form_and_provenance_gaps_or_overlaps() {
    let fixture = fixture();
    let selected = selected(&fixture);

    let changed_line = format!("-include $({OWNER_ALPHA}-makefile)");
    let replacement = "-include $(other-owner-x-makefile)".to_string();
    assert_eq!(changed_line.len(), replacement.len());
    let changed_text = fixture
        .expanded
        .text
        .replacen(&changed_line, &replacement, 1);
    assert_ne!(changed_text, fixture.expanded.text);
    let mut changed = fixture.expanded.clone();
    changed.text = changed_text;
    assert!(project(&fixture.source_root, &changed, &selected).is_err());

    let mut gap = fixture.expanded.clone();
    gap.provenance[1].output_start_byte += 1;
    assert!(project(&fixture.source_root, &gap, &selected).is_err());

    let mut overlap = fixture.expanded.clone();
    overlap.provenance[1].output_start_byte -= 1;
    assert!(project(&fixture.source_root, &overlap, &selected).is_err());
}

#[test]
fn source_identity_rejects_changed_module_base_and_override_independently() {
    let baseline = fixture();
    let selected = selected(&baseline);
    let invocation =
        format!("%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library");

    let changed_module = baseline.source.replacen(
        &invocation,
        &format!("%build_module_core mmake={OWNER_ALPHA} modname=mesa3dgl26-9 modtype=library"),
        1,
    );
    let changed_module = fixture_from_source(changed_module);
    let resolve = source_resolver(&changed_module.source_root, &changed_module.source);
    assert_eq!(
        project_with_resolver(
            &changed_module.source_root,
            &changed_module.expanded,
            &selected,
            &resolve,
        )
        .unwrap_err(),
        "Mesa Source module name differs from selected genmodule producer"
    );

    let changed_base = baseline.source.replacen(
        &invocation,
        &format!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library conffile=other.conf"
        ),
        1,
    );
    let changed_base = fixture_from_source(changed_base);
    let resolve = source_resolver(&changed_base.source_root, &changed_base.source);
    assert_eq!(
        project_with_resolver(
            &changed_base.source_root,
            &changed_base.expanded,
            &selected,
            &resolve,
        )
        .unwrap_err(),
        "Mesa Source configuration differs from selected genmodule producer"
    );

    let changed_override = baseline.source.replacen(
        &invocation,
        &format!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library confoverride=other.override"
        ),
        1,
    );
    let changed_override = fixture_from_source(changed_override);
    let resolve = source_resolver(&changed_override.source_root, &changed_override.source);
    assert_eq!(
        project_with_resolver(
            &changed_override.source_root,
            &changed_override.expanded,
            &selected,
            &resolve,
        )
        .unwrap_err(),
        "Mesa Source configuration differs from selected genmodule producer"
    );
}

#[test]
fn source_identity_propagates_resolver_errors_and_rejects_unresolved_values() {
    let fixture = fixture();
    let selected = selected(&fixture);
    let resolver_error =
        |_: &str, _: &ExpandedLineProvenance| Err("synthetic source resolver failure".to_owned());
    assert_eq!(
        project_with_resolver(
            &fixture.source_root,
            &fixture.expanded,
            &selected,
            &resolver_error,
        )
        .unwrap_err(),
        "synthetic source resolver failure"
    );

    let invocation =
        format!("%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library");
    let source = fixture.source.replacen(
        &invocation,
        &format!(
            "%build_module_core mmake={OWNER_ALPHA} modname=$(MISSING_MODULE) modtype=library"
        ),
        1,
    );
    let unresolved = fixture_from_source(source);
    let resolve = source_resolver(&unresolved.source_root, &unresolved.source);
    assert!(project_with_resolver(
        &unresolved.source_root,
        &unresolved.expanded,
        &selected,
        &resolve,
    )
    .unwrap_err()
    .contains("MISSING_MODULE"));
}

#[test]
fn source_identity_rejects_wrapper_substitution_of_another_owner() {
    let fixture = fixture();
    let mut changed = fixture.expanded.clone();
    let mut changed_sites = 0;
    for origin in &mut changed.provenance {
        if origin.macro_stack.first().is_some_and(|frame| {
            frame.resolved_arguments.get("mmake").map(String::as_str) == Some(OWNER_ALPHA)
        }) {
            // Preserve the selected inner core and generated text, but model
            // an outer Source call naming another owner in the same file.
            let mut source_frame = origin.macro_stack[0].clone();
            source_frame.name = "build_module".into();
            source_frame
                .resolved_arguments
                .insert("mmake".into(), "another-owner".into());
            origin.macro_stack.insert(0, source_frame);
            changed_sites += 1;
        }
    }
    assert!(changed_sites > 0);
    assert_eq!(
        project(&fixture.source_root, &changed, &selected(&fixture)).unwrap_err(),
        "Mesa Source macro owner or type differs from selected genmodule producer"
    );
}

#[test]
fn source_identity_resolves_variables_at_each_original_source_position() {
    const SECOND_MODULE: &str = "mesa3dgl26-1";
    let source = format!(
        concat!(
            "CURRENT_MODULE := {MODULE}\n",
            "CURRENT_CONFIG := alpha.conf\n",
            "%build_module_core mmake={OWNER_ALPHA} modname=$(CURRENT_MODULE) modtype=library conffile=$(CURRENT_CONFIG)\n",
            "CURRENT_MODULE := {SECOND_MODULE}\n",
            "CURRENT_CONFIG := bravo.conf\n",
            "CURRENT_OVERRIDE := bravo.override\n",
            "%build_module_core mmake={OWNER_BRAVO} modname=$(CURRENT_MODULE) modtype=hidd conffile=$(CURRENT_CONFIG) confoverride=$(CURRENT_OVERRIDE)\n",
            "%common\n",
        ),
        MODULE = MODULE,
        SECOND_MODULE = SECOND_MODULE,
        OWNER_ALPHA = OWNER_ALPHA,
        OWNER_BRAVO = OWNER_BRAVO,
    );
    let mut fixture = fixture_from_source(source);
    fixture.alpha = producer_for(
        OWNER_ALPHA,
        MODULE,
        "library",
        format!("{DECLARATION_DIRECTORY}/alpha.conf"),
        None,
    );
    fixture.bravo = producer_for(
        OWNER_BRAVO,
        SECOND_MODULE,
        "hidd",
        format!("{DECLARATION_DIRECTORY}/bravo.conf"),
        Some(format!("{DECLARATION_DIRECTORY}/bravo.override")),
    );
    fixture.alpha_makefile = makefile_for(MODULE, "-DALPHA");
    fixture.bravo_makefile = makefile_for(SECOND_MODULE, "-DBRAVO");

    let resolve = source_resolver(&fixture.source_root, &fixture.source);
    let program = project_with_resolver(
        &fixture.source_root,
        &fixture.expanded,
        &selected(&fixture),
        &resolve,
    )
    .unwrap();
    assert_full_span_coverage(&program);
    let imported = imported_spans(&program);
    assert_eq!(imported.len(), 2);
    assert_eq!(imported[0].1, MODULE);
    assert_eq!(imported[1].1, SECOND_MODULE);
}

#[test]
fn source_identity_rejects_explicit_empty_override_literal_and_variable() {
    let baseline = fixture();
    let selected = selected(&baseline);
    let invocation =
        format!("%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library");

    let literal_source = baseline.source.replacen(
        &invocation,
        &format!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library confoverride="
        ),
        1,
    );
    let literal_empty = fixture_from_source(literal_source);
    let resolve = source_resolver(&literal_empty.source_root, &literal_empty.source);
    assert_eq!(
        project_with_resolver(
            &literal_empty.source_root,
            &literal_empty.expanded,
            &selected,
            &resolve,
        )
        .unwrap_err(),
        "Mesa Source explicitly supplied override resolves empty"
    );

    let variable_source = baseline.source.replacen(
        &invocation,
        &format!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library confoverride=$(EMPTY_OVERRIDE)"
        ),
        1,
    );
    let variable_source = format!("EMPTY_OVERRIDE :=\n{variable_source}");
    let variable_empty = fixture_from_source(variable_source);
    let resolve = source_resolver(&variable_empty.source_root, &variable_empty.source);
    assert_eq!(
        project_with_resolver(
            &variable_empty.source_root,
            &variable_empty.expanded,
            &selected,
            &resolve,
        )
        .unwrap_err(),
        "Mesa Source explicitly supplied override resolves empty"
    );
}

#[test]
fn source_identity_accepts_root_absolute_and_empty_default_configuration_paths() {
    let baseline = fixture();
    assert_eq!(
        baseline.alpha.base_configuration,
        format!("{DECLARATION_DIRECTORY}/{MODULE}.conf")
    );
    assert!(project(
        &baseline.source_root,
        &baseline.expanded,
        &selected(&baseline)
    )
    .is_ok());

    let invocation =
        format!("%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library");
    let source = baseline.source.replacen(
        &invocation,
        &format!(
            "%build_module_core mmake={OWNER_ALPHA} modname={MODULE} modtype=library conffile=$(ABS_CONFIG)"
        ),
        1,
    );
    let absolute = fixture_from_source(source);
    let absolute_config = absolute
        .source_root
        .join(DECLARATION_DIRECTORY)
        .join(format!("{MODULE}.conf"))
        .to_string_lossy()
        .into_owned();
    let resolve = |expression: &str, _origin: &ExpandedLineProvenance| {
        if expression == "$(ABS_CONFIG)" {
            Ok(absolute_config.clone())
        } else {
            Ok(expression.to_owned())
        }
    };
    assert!(project_with_resolver(
        &absolute.source_root,
        &absolute.expanded,
        &selected(&absolute),
        &resolve,
    )
    .is_ok());

    let outside = absolute.source_root.parent().unwrap().join("outside.conf");
    let outside = outside.to_string_lossy().into_owned();
    let resolve = |expression: &str, _origin: &ExpandedLineProvenance| {
        if expression == "$(ABS_CONFIG)" {
            Ok(outside.clone())
        } else {
            Ok(expression.to_owned())
        }
    };
    assert_eq!(
        project_with_resolver(
            &absolute.source_root,
            &absolute.expanded,
            &selected(&absolute),
            &resolve,
        )
        .unwrap_err(),
        "Mesa Source configuration leaves selected source root"
    );
}
