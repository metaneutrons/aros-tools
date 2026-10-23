use super::super::super::{MakeProgramPaths, MakeVariableProgram, MAX_STATEMENTS, MAX_STATE_WORK};
use crate::{dirs::DirVars, make_expr::MakeExprError, parser::TargetContext};
use std::path::Path;

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build/unit"),
    }
}

fn program(source: &str, paths: &MakeProgramPaths<'_>) -> MakeVariableProgram {
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[])
        .expect("synthetic initial Make state is valid");
    program
        .read_file("synthetic.mk", source, paths, &|_, _| Ok(None))
        .expect("synthetic Make source parses without filesystem access");
    program
}

fn global_snapshot(program: &mut MakeVariableProgram, paths: &MakeProgramPaths<'_>) -> String {
    program
        .evaluate(
            "$(value X)|$(X)|$(flavor X)|$(origin X);$(value Y)|$(Y)|$(flavor Y)|$(origin Y)",
            paths,
        )
        .expect("global bindings are measurable")
}

#[test]
fn readonly_contexts_preserve_transitive_flavor_refusal_and_known_shadowing() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let target = TargetContext {
        make_variables: [("Y".into(), "configured".into())].into(),
        ..TargetContext::default()
    };
    let make = || {
        let mut p = MakeVariableProgram::new(&target, &[]).unwrap();
        p.read_file(
            "uncertain.mk",
            "Z := early\nt: X = $(Y)\nu: Y = known\nGLOBAL = $(Y)\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
        // Native file reading already refuses unknown-flavor appends. Exercise
        // this defensive guard using the legacy configuration collector's real
        // uncertainty marker, not an assertion that such a source read passed.
        let legacy = crate::make_vars::collect_vars_with_context("Y += $(Z)\n", &target);
        p.state.scope.flavor_uncertainties = legacy.flavor_uncertainties;
        p
    };
    for expression in ["$(value X)", "$(X)"] {
        assert!(make()
            .evaluate_target_expressions(&["t"], &[expression], &paths)
            .is_err());
        assert!(make()
            .evaluate_target_contexts(&[vec!["t"]], &[expression], &paths)
            .is_err());
    }
    // Origin/flavor describe the known target binding itself, not its unsafe
    // expanded value; the legacy inspector deliberately keeps them available.
    let metadata = ["$(origin X)", "$(flavor X)"];
    let expected = make()
        .evaluate_target_expressions(&["t"], &metadata, &paths)
        .unwrap();
    assert_eq!(expected, ["file", "recursive"]);
    assert_eq!(
        make()
            .evaluate_target_contexts(&[vec!["t"]], &metadata, &paths)
            .unwrap(),
        [expected]
    );
    let expressions = [
        "$(GLOBAL)",
        "$(value GLOBAL)",
        "$(flavor GLOBAL)",
        "$(origin GLOBAL)",
    ];
    let expected = make()
        .evaluate_target_expressions(&["u"], &expressions, &paths)
        .unwrap();
    assert_eq!(
        make()
            .evaluate_target_contexts(&[vec!["u"]], &expressions, &paths)
            .unwrap(),
        [expected]
    );
}

#[test]
fn readonly_contexts_match_literal_dollars_and_equal_specificity_pattern_order() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let source = "X := $$literal\nroot: X := $$parent\nchild: X += $$child\na%.o: X += first\n%b.o: X += second\na%.o: X += third\n";
    let expressions = ["$(X)", "$(value X)", "$(flavor X)", "$(origin X)"];
    let chains = [vec!["root", "child"], vec!["ab.o"]];
    let mut scalar = program(source, &paths);
    let expected = chains
        .iter()
        .map(|chain| {
            scalar
                .evaluate_target_expressions(chain, &expressions, &paths)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        program(source, &paths)
            .evaluate_target_contexts(&chains, &expressions, &paths)
            .unwrap(),
        expected
    );
}

#[test]
fn readonly_contexts_preserve_measured_command_line_priority() {
    use super::super::super::super::{MakeInitialFlavor, MakeInitialValue};
    use aros_common::make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    };
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION", "X"]
            .into_iter()
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: if name == "X" {
                    MakeVariableOrigin::CommandLine
                } else {
                    MakeVariableOrigin::Default
                },
            })
            .collect(),
    })
    .unwrap();
    let mut p = MakeVariableProgram::new_source_program(
        &paths,
        &namespace,
        &[MakeInitialValue {
            name: "X",
            value: "command",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::CommandLine,
        }],
    )
    .unwrap();
    p.read_file(
        "priority.mk",
        "X = global\nt: X := local\nt: Y := $(X)\n",
        &paths,
        &|_, _| Ok(None),
    )
    .unwrap();
    assert_eq!(
        p.evaluate_target_contexts(&[vec!["t"]], &["$(X)|$(Y)|$(origin X)"], &paths)
            .unwrap(),
        [vec!["command|command|command line"]]
    );
}

#[test]
fn target_contexts_match_scalar_queries_for_inheritance_patterns_and_siblings() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let source = concat!(
        "X = global-x\n",
        "Y = global-y\n",
        "root: X += +root\n",
        "root: Y = parent-y\n",
        "child.o: X += +child\n",
        "%.o: X += +pattern\n",
        "c%.o: X += +narrow\n",
        "%.o: Y += +pattern-y\n",
        "left.o: X := left-x\n",
        "left.o: Y := left-y\n",
        "right.o: X += +right\n",
        "right.o: Y = right-y\n",
    );
    let expressions = [
        "$(value X)",
        "$(X)",
        "$(flavor X)",
        "$(origin X)",
        "$(value Y)",
        "$(Y)",
        "$(flavor Y)",
        "$(origin Y)",
    ];
    let chains = vec![
        vec!["root", "child.o"],
        vec!["root", "left.o"],
        vec!["root", "right.o"],
        vec!["root", "left.o"],
        vec!["root", "child.o"],
    ];

    let mut batched = program(source, &paths);
    let mut scalar = program(source, &paths);
    let before = global_snapshot(&mut batched, &paths);
    let expected = chains
        .iter()
        .map(|chain| {
            scalar
                .evaluate_target_expressions(chain, &expressions, &paths)
                .expect("legacy scalar-target batch evaluates")
        })
        .collect::<Vec<_>>();

    let actual = batched
        .evaluate_target_contexts(&chains, &expressions, &paths)
        .expect("target contexts evaluate together");

    assert_eq!(actual, expected);
    assert_eq!(global_snapshot(&mut batched, &paths), before);
    assert_eq!(
        global_snapshot(&mut scalar, &paths),
        global_snapshot(&mut batched, &paths)
    );
}

#[test]
fn target_contexts_refuse_eval_without_publishing_global_changes() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program("X := before\nt: LOCAL := local\n", &paths);
    let events = program.events().to_vec();

    assert!(program
        .evaluate_target_contexts(&[vec!["t"]], &["$(LOCAL)|$(eval X := changed)"], &paths,)
        .is_err());

    assert_eq!(program.events(), events.as_slice());
    assert_eq!(
        program.evaluate("$(X)|$(origin X)", &paths).unwrap(),
        "before|file"
    );
}

#[test]
fn target_contexts_allow_empty_context_and_expression_batches() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program("t: LOCAL := local\n", &paths);

    assert_eq!(
        program
            .evaluate_target_contexts(&[], &["literal"], &paths)
            .unwrap(),
        Vec::<Vec<String>>::new()
    );
    assert_eq!(
        program
            .evaluate_target_contexts(&[vec!["t"]], &[], &paths)
            .unwrap(),
        vec![Vec::<String>::new()]
    );
}

#[test]
fn target_context_batch_count_is_checked_before_staging() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let initial_work = program.state_work.get();
    let too_many_contexts = vec![vec!["t"]; MAX_STATEMENTS + 1];
    let too_many_expressions = vec![""; MAX_STATEMENTS + 1];
    let excessive_product = vec![vec!["t"]; 257];
    let product_expressions = vec![""; 256];

    assert!(program
        .evaluate_target_contexts(&too_many_contexts, &[""], &paths)
        .is_err());
    assert!(program
        .evaluate_target_contexts(&[vec!["t"]], &too_many_expressions, &paths)
        .is_err());
    assert!(program
        .evaluate_target_contexts(&excessive_product, &product_expressions, &paths)
        .is_err());
    assert_eq!(program.state_work.get(), initial_work);
}

#[test]
fn target_contexts_share_the_persistent_state_work_counter() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    program.state_work.set(MAX_STATE_WORK - 1);

    let first = program
        .evaluate_target_contexts(&[vec!["t"]], &["literal"], &paths)
        .unwrap_err();
    assert!(matches!(
        first,
        MakeExprError::ResourceLimit {
            resource: "Make state work",
            limit: MAX_STATE_WORK,
        }
    ));
    assert_eq!(program.state_work.get(), MAX_STATE_WORK);

    let second = program
        .evaluate_target_contexts(&[vec!["t"]], &["literal"], &paths)
        .unwrap_err();
    assert!(matches!(
        second,
        MakeExprError::ResourceLimit {
            resource: "Make state work",
            limit: MAX_STATE_WORK,
        }
    ));
    assert_eq!(program.state_work.get(), MAX_STATE_WORK);
}

#[test]
fn many_contexts_stage_large_unrelated_globals_once() {
    use std::fmt::Write as _;

    const TARGETS: usize = 128;
    const GLOBAL_BYTES: usize = 64 * 1024;

    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let large_value = "s".repeat(GLOBAL_BYTES);
    let targets = (0..TARGETS)
        .map(|index| format!("target{index}"))
        .collect::<Vec<_>>();
    let mut source = format!(
        "LARGE_A := {large_value}\nLARGE_B := {large_value}\nLARGE_C := {large_value}\nLARGE_D := {large_value}\n"
    );
    for (index, target) in targets.iter().enumerate() {
        writeln!(source, "{target}: LOCAL := item{index}").unwrap();
    }
    let chains = targets
        .iter()
        .map(|target| vec![target.as_str()])
        .collect::<Vec<_>>();
    let expressions = ["$(LOCAL)|$(value LOCAL)|$(flavor LOCAL)|$(origin LOCAL)"];

    let mut shared = program(&source, &paths);
    let output = shared
        .evaluate_target_contexts(&chains, &expressions, &paths)
        .expect("one shared stage fits the unchanged state-work bound");
    assert_eq!(output.len(), TARGETS);
    for (index, values) in output.iter().enumerate() {
        assert_eq!(
            values.as_slice(),
            &[format!("item{index}|item{index}|simple|file")]
        );
    }
    assert!(shared.state_work.get() < MAX_STATE_WORK);

    let mut scalar = program(&source, &paths);
    let mut failure = None;
    for chain in &chains {
        match scalar.evaluate_target_expressions(chain, &expressions, &paths) {
            Ok(_) => {}
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    assert!(matches!(
        failure,
        Some(MakeExprError::ResourceLimit {
            resource: "Make state work",
            limit: MAX_STATE_WORK,
        })
    ));

    let work_at_limit = scalar.state_work.get();
    assert!(work_at_limit <= MAX_STATE_WORK);
    assert!(matches!(
        scalar.evaluate_target_expressions(&chains[0], &expressions, &paths),
        Err(MakeExprError::ResourceLimit {
            resource: "Make state work",
            limit: MAX_STATE_WORK,
        })
    ));
    assert_eq!(scalar.state_work.get(), work_at_limit);
}
