//! Explicit data/provenance replay; no compiler or Make process is executed.

use super::*;
use crate::dirs::DirVars;
use crate::genmf_projection::{expand_text, Limits};
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::collect_vars_with_context;
use crate::parser::{join_continuations, macro_arg, macro_invocations};
use aros_common::mesa_configuration_input::load_mesa_configuration_input;
use aros_common::sha256_bytes;

#[test]
#[ignore = "requires explicit real Source root and retained genmodule input; data replay only"]
fn actual_original_genmodule_output_enters_original_source_template_include() {
    replay_actual_programs(false);
}

#[test]
#[ignore = "requires explicit real Source/configuration; complete rule interpretation development gate"]
fn actual_whole_owner_program_enters_generic_rule_interpretation() {
    replay_actual_programs(true);
}

fn replay_actual_programs(interpret_rules: bool) {
    let source = std::path::PathBuf::from(std::env::var_os("AROS_TEST_SOURCE_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let path =
        std::path::PathBuf::from(std::env::var_os("AROS_TEST_MESA_GENMODULE_INPUT").unwrap())
            .canonicalize()
            .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let input = load_mesa_configuration_input(&path, &sha256_bytes(&bytes)).unwrap();
    assert_eq!(
        input
            .configuration()
            .recipe_configuration()
            .make_coordinates
            .source_root,
        source.to_str().unwrap()
    );
    let collection = input.genmodule_collection().unwrap();
    // This explicit replay fixture is the retained ordinary ARM selection,
    // not a guessed platform based on a producer name.
    assert_eq!(collection.selection().binding().profile, "arm-raspi");
    let target = crate::parser::TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        mesa_version: Some("26.0.0".into()),
        ..Default::default()
    };
    let coordinates = &input
        .configuration()
        .recipe_configuration()
        .make_coordinates;
    // Nominal preflight Ports need not exist. Use explicit existing inventory
    // for this Source-scope replay; never repair the completed preflight owner.
    let scope_build =
        std::path::PathBuf::from(std::env::var_os("AROS_TEST_MESA_SCOPE_BUILD").unwrap())
            .canonicalize()
            .unwrap();
    let scope_ports = scope_build.join("Ports").canonicalize().unwrap();
    let mut dirs = DirVars::load(&source);
    dirs.set_materialized_path("AROS_PORTS_DIR", scope_ports.clone());
    assert!(!collection.binding().outputs.is_empty());
    // Marker-owned writefiles recipes have no generated-Make include. Their
    // generated C sources require a separate recipe projection, not a fake
    // module invocation or an import appended at an arbitrary position.
    let module_owners = collection
        .selection()
        .inventory()
        .records
        .iter()
        .filter(|row| !row.consumer_path.starts_with("marker/"))
        .map(|row| row.owner.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!module_owners.is_empty());
    let mut replayed = std::collections::BTreeSet::new();
    for binding in &collection.binding().outputs {
        let producer = &binding.producer;
        let owner = producer.owner.as_str();
        if !module_owners.contains(owner) {
            continue;
        }
        assert!(replayed.insert(owner));
        let declaration = source
            .join(&producer.declaration_directory)
            .join("mmakefile.src");
        let text = std::fs::read_to_string(&declaration).unwrap();
        let calls = macro_invocations(&join_continuations(&text));
        let calls = calls
            .iter()
            .filter(|call| macro_arg(&call.args, "mmake").as_deref() == Some(owner))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1, "do not pick the first matching declaration");
        let invocation = format!("%{} {}\n", calls[0].name, calls[0].args);
        let expanded = expand_text(
            &invocation,
            &declaration,
            &source.join("config/make.tmpl"),
            Limits::default(),
        )
        .unwrap();
        let makefile = &collection.outputs()[owner];
        let relative = Path::new(&producer.declaration_directory);
        let source_scope = super::super::configured_scope::project(
            &source,
            relative,
            owner,
            &dirs,
            &target,
            input.configuration(),
            (&scope_build, &scope_ports),
        )
        .unwrap();
        let whole = crate::genmf_projection::expand_positioned_text(
            &source_scope.context_input.source_program,
            &declaration,
            &source_scope.context_input.source_locations,
            &source.join("config/make.tmpl"),
            Limits::default(),
        )
        .unwrap();
        let physical_call = source_scope.context_input.source_locations
            [source_scope.context_input.declaration_line]
            .as_ref()
            .unwrap();
        assert_eq!(physical_call.path, declaration);
        assert_eq!(
            join_continuations(&text)
                .lines()
                .filter(|line| line.contains(&format!("mmake={owner}")))
                .count(),
            1
        );
        assert!(whole
            .provenance
            .iter()
            .any(|span| span.top_level_source_invocation.as_ref() == Some(physical_call)));
        assert!(
            whole.text.len() > expanded.text.len(),
            "whole original owner program, not only its selected invocation"
        );
        let resolve = |expression: &str, origin: &ExpandedLineProvenance| {
            // Use original source-prefix/template text only, not the imported
            // fragment or any values from the expected producer tuple.
            let prefix = join_continuations(&format!(
                "{}\n{}",
                source_scope.context_input.prefix().unwrap(),
                &expanded.text[..origin.output_start_byte]
            ));
            let scope = collect_vars_with_context(&prefix, &source_scope.context_input.selected);
            let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &source, relative)
                .without_filesystem();
            evaluate_make_expr(expression, &context)
                .map(|value| value.replace("${AROS_SOURCE_DIR}", &coordinates.source_root))
                .map_err(|error| error.to_string())
        };
        let program = project(&source, &expanded, collection, &[owner], &resolve).unwrap();
        let imports = program
            .spans
            .iter()
            .filter(|span| matches!(span.origin, MakeProgramOrigin::Genmodule { .. }))
            .collect::<Vec<_>>();
        assert_eq!(imports.len(), 1);
        assert_eq!(
            &program.text[imports[0].start_byte..imports[0].end_byte],
            makefile.canonical_make_fragment()
        );
        assert!(program.text[imports[0].end_byte..].contains(&format!("{owner}_CPPFLAGS")));
        // TARGET is an explicit MetaMake execution selection, not a measured
        // configure value or an inferred compiler default. Replay this exact
        // selected endpoint; a foreign endpoint below must not import it.
        let mut execution_context = source_scope.context_input.selected.clone();
        execution_context
            .make_variables
            .insert("TARGET".into(), owner.to_owned());
        let resolve_whole = |expression: &str, _origin: &ExpandedLineProvenance, prefix: &str| {
            let prefix = join_continuations(prefix);
            let scope = collect_vars_with_context(&prefix, &execution_context);
            let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &source, relative)
                .without_filesystem();
            evaluate_make_expr(expression, &context)
                .map(|value| value.replace("${AROS_SOURCE_DIR}", &coordinates.source_root))
                .map_err(|error| error.to_string())
        };
        let whole_owners = collection
            .selection()
            .producers()
            .iter()
            .filter(|producer| producer.declaration_directory == relative.to_str().unwrap())
            .map(|producer| producer.owner.as_str())
            .collect::<Vec<_>>();
        let active = |_origin: &ExpandedLineProvenance, prefix: &str| {
            let (_, states) = crate::make_vars::collect_vars_impl(
                &format!("{prefix}# current generated include\n"),
                Some(&execution_context),
            );
            Ok(states.last().and_then(|state| match state {
                crate::make_vars::ConditionalTruth::True => Some(true),
                crate::make_vars::ConditionalTruth::False => Some(false),
                crate::make_vars::ConditionalTruth::Unknown => None,
            }))
        };
        let whole_program = project_with_prefix(
            &source,
            &whole,
            collection,
            &whole_owners,
            &active,
            &resolve_whole,
        )
        .unwrap_or_else(|error| panic!("whole original owner {owner}: {error}"));
        assert!(whole_program.text.len() > program.text.len());
        let (joined_program, source_ranges) =
            crate::parser::join_continuations_with_source_ranges(&whole_program.text);
        assert_eq!(source_ranges.len(), joined_program.lines().count());
        assert_eq!(source_ranges.first().unwrap().start, 0);
        assert_eq!(source_ranges.last().unwrap().end, whole_program.text.len());
        let mut span_index = 0;
        for range in &source_ranges {
            while whole_program.spans[span_index].end_byte <= range.start {
                span_index += 1;
            }
            let mut end = range.start;
            let mut covering = span_index;
            while end < range.end {
                let span = &whole_program.spans[covering];
                assert!(span.start_byte <= end);
                end = span.end_byte.min(range.end);
                covering += 1;
            }
            assert_eq!(
                end, range.end,
                "joined rule lines retain every contributing original/import span"
            );
        }
        let whole_imports = imported_owners(&whole_program);
        assert_eq!(
            whole_imports,
            whole_owners
                .iter()
                .map(|owner| (*owner).to_owned())
                .collect()
        );
        let mut foreign_context = execution_context.clone();
        foreign_context
            .make_variables
            .insert("TARGET".into(), "unselected-counter-target".into());
        let inactive = |_origin: &ExpandedLineProvenance, prefix: &str| {
            let (_, states) = crate::make_vars::collect_vars_impl(
                &format!("{prefix}# include\n"),
                Some(&foreign_context),
            );
            Ok(states.last().and_then(|state| match state {
                crate::make_vars::ConditionalTruth::True => Some(true),
                crate::make_vars::ConditionalTruth::False => Some(false),
                crate::make_vars::ConditionalTruth::Unknown => None,
            }))
        };
        assert!(project_with_prefix(
            &source,
            &whole,
            collection,
            &whole_owners,
            &inactive,
            &resolve_whole
        )
        .unwrap_err()
        .contains("does not cover every selected producer"));
        if interpret_rules {
            let owner_cwd = Path::new(&coordinates.build_root).join(relative);
            let paths = crate::make_vars::MakeProgramPaths {
                dirs: &dirs,
                source_root: &source,
                relative_dir: relative,
                top_directory: Path::new(&coordinates.build_root),
                working_directory: &owner_cwd,
            };
            let namespace = input.configuration().initial_namespace()
                .expect("whole owner replay requires newly captured initial namespace; legacy values cannot prove absence");
            // A global configure snapshot is not an owner snapshot. Changing
            // CURDIR/TARGET can change immediate values before this program
            // begins. Require future owner-bound capture/dependency replay;
            // do not transplant global simple values and relabel their paths.
            let mut variables = crate::make_vars::MakeVariableProgram::new_with_namespace(
                &execution_context,
                &[],
                &paths,
                namespace,
            )
            .unwrap_or_else(|error| panic!("whole owner {owner} initial state: {error}"));
            // Genmodule inputs above are independently bound and already
            // inlined. Do not silently omit another include or open ambient
            // files: its exact validated content must join this replay first.
            let includes = |requested: &str, _: &str| -> Result<Option<String>, String> {
                Err(format!(
                    "include has no independently bound replay input: {requested}"
                ))
            };
            let program = variables
                .read_file(owner, &whole_program.text, &paths, &includes)
                .unwrap_or_else(|error| panic!("whole owner {owner} source-order read: {error}"));
            assert!(program.events.iter().any(|event| matches!(
                event.kind,
                crate::make_vars::MakeFileEventKind::Rule(
                    crate::make_rule_header::RuleHeader::Rule { .. }
                )
            )));
        }
        aros_common::outputln!(
            "actual original Source/genmodule positioned import PASS: {owner}, {} bytes",
            makefile.canonical_make_fragment().len()
        );
    }
    assert_eq!(replayed, module_owners);
    input.revalidate().unwrap();
}

fn imported_owners(program: &PositionedMesaMakeProgram) -> std::collections::BTreeSet<String> {
    program
        .spans
        .iter()
        .filter_map(|span| match &span.origin {
            MakeProgramOrigin::Genmodule { owner, .. } => Some(owner.clone()),
            MakeProgramOrigin::Genmf(_) => None,
        })
        .collect()
}
