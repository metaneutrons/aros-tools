//! Optional replay against the user's original Source template.
//!
//! This verifies the bounded text and Make-expression projection only. It
//! neither starts GNU Make nor invokes a compiler, and makes no product or ABI
//! claim.

use crate::dirs::DirVars;
use crate::genmf_projection::{expand_text, Limits};
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_target_capture::{capture, TargetCapture};
use crate::make_vars::{
    collect_vars_impl, collect_vars_with_immediate_evaluator, ConditionalTruth,
};
use crate::parser::{join_continuations, TargetContext};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const SYNTHETIC_GLOBALS: &str = concat!(
    "Q := @\n",
    "ECHO := echo\n",
    "IF := if\n",
    "TEST := test\n",
    "NOP := :\n",
    "MKDIR := mkdir -p\n",
    "MKDEPEND := /selected/mkdepend\n",
    "GENDIR := /synthetic/generated\n",
    "CPPFLAGS := -DGLOBAL_CPP=1 -DREPEAT=1\n",
    "CFLAGS := -O2 -DREPEAT=1\n",
    "TARGET_CC := /selected/clang\n",
    "TARGET_SYSROOT := --sysroot=/selected/sysroot\n",
    "SAFETY_CFLAGS := -DSAFE=1 -DREPEAT=1\n",
    "CFLAGS_IQUOTE := -iquote\n",
    "CFLAGS_IQUOTE_END :=\n",
    "oracle_MC_EXTRA_CPPFLAGS := -DEXTRA=1 -DREPEAT=1 -DEXTRA=1\n",
);

fn original_source_root() -> PathBuf {
    PathBuf::from(std::env::var_os("AROS_TEST_SOURCE_ROOT").expect("AROS_TEST_SOURCE_ROOT"))
        .canonicalize()
        .expect("canonical Source root")
}

fn compile_multi_fragment(source: &Path) -> String {
    compile_multi_fragment_with(source, "a b", "")
}

fn compile_multi_fragment_with(source: &Path, basenames: &str, targetdir: &str) -> String {
    let template = source.join("config/make.tmpl");
    let recipe = source.join("workbench/libs/mesa/libgalliumaux/mmakefile.src");
    let invocation = format!(
        "%rule_compile_multi mmake=oracle basenames=\"{basenames}\" targetdir={targetdir} compiler=target\n"
    );
    let expanded = expand_text(&invocation, &recipe, &template, Limits::default())
        .expect("expand the original rule_compile_multi macro");

    let mut fragment = String::new();
    for span in &expanded.provenance {
        let from_compile_multi = span
            .macro_stack
            .iter()
            .any(|frame| frame.name == "rule_compile_multi");
        let from_common = span.macro_stack.iter().any(|frame| frame.name == "common");
        if !from_compile_multi || from_common {
            continue;
        }
        let text = &expanded.text[span.output_start_byte..span.output_end_byte];
        if !fragment.is_empty() && !fragment.ends_with('\n') && !text.starts_with('\n') {
            fragment.push('\n');
        }
        fragment.push_str(text);
    }
    assert!(fragment.contains("oracle_MC_TARGETS"), "{fragment}");
    assert!(fragment.contains("CPPFLAGS :="), "{fragment}");
    assert!(fragment.contains("CFLAGS :="), "{fragment}");
    fragment
}

fn observe_vpath_candidate(candidate: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(candidate) {
        Ok(_) => {
            aros_common::open_regular_file_nofollow(candidate).map_err(|error| {
                format!(
                    "Make vpath candidate is not a regular no-follow file: {}: {error}",
                    candidate.display()
                )
            })?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Make vpath candidate cannot be inspected: {}: {error}",
            candidate.display()
        )),
    }
}

fn resolve_vpath_file(
    search_paths: &crate::make_vpath::SearchPaths,
    file: &str,
    build_directory: &Path,
    global_vpath: &str,
) -> PathBuf {
    let candidates = search_paths
        .candidates(file, build_directory, global_vpath)
        .unwrap_or_else(|error| panic!("build Make vpath candidates for {file}: {error}"));
    crate::make_vpath::first_present(&candidates, &observe_vpath_candidate)
        .unwrap_or_else(|error| panic!("observe Make vpath candidates for {file}: {error}"))
        .unwrap_or_else(|| panic!("Make vpath did not resolve {file}"))
}

fn selected_variables() -> BTreeSet<String> {
    [
        "CMD",
        "CPPFLAGS",
        "CFLAGS",
        "oracle_MC_CPPFLAGS",
        "oracle_CFLAGS",
        "oracle_DFLAGS",
        "oracle_IQUOTE",
        "oracle_IQUOTE_END",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn capture_fragment(source: &Path, fragment: &str) -> TargetCapture {
    let unjoined = format!("{SYNTHETIC_GLOBALS}{fragment}");
    let program = join_continuations(&unjoined);
    let target = TargetContext::default();
    let (scope, line_states) = collect_vars_impl(&program, Some(&target));
    let states = line_states
        .into_iter()
        .map(|state| match state {
            ConditionalTruth::True => Some(true),
            ConditionalTruth::False => Some(false),
            ConditionalTruth::Unknown => None,
        })
        .collect::<Vec<_>>();
    let dirs = DirVars::default();
    let relative = Path::new("workbench/libs/mesa/libgalliumaux");
    capture(
        &program,
        &selected_variables(),
        &states,
        &|expression, line, overlay| {
            let escaped_overlay = overlay
                .iter()
                .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
                .collect::<BTreeMap<_, _>>();
            let lookup = |name: &str| escaped_overlay.get(name).cloned();
            let context = MakeExprContext::new(&scope, &dirs, line, source, relative)
                .with_lookup(&lookup)
                .without_filesystem();
            evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
        },
    )
    .expect("capture original target-specific assignments")
}

#[test]
#[ignore = "requires explicit AROS_TEST_SOURCE_ROOT; original-template text replay only"]
fn original_rule_compile_multi_captures_synthetic_c_flags_per_object() {
    let source = original_source_root();
    crate::capability::mesa::configured_source_context::verify_templates(&source)
        .expect("verify original recipe templates");
    let fragment = compile_multi_fragment(&source);
    let captured = capture_fragment(&source, &fragment);

    let expected = [
        ("CMD", "/selected/clang --sysroot=/selected/sysroot"),
        (
            "CPPFLAGS",
            "-DGLOBAL_CPP=1 -DREPEAT=1 -DEXTRA=1 -DREPEAT=1 -DEXTRA=1",
        ),
        ("CFLAGS", "-O2 -DREPEAT=1 -DSAFE=1 -DREPEAT=1"),
    ];
    for object in ["a.o", "b.o"] {
        for (variable, value) in expected {
            assert_eq!(
                captured.targets[object][variable], value,
                "{object} {variable}"
            );
        }
    }
    assert_eq!(
        captured
            .assignments
            .iter()
            .filter(|assignment| {
                assignment.target == "a.o" && assignment.variable == "CPPFLAGS"
            })
            .map(|assignment| assignment.value.as_str())
            .collect::<Vec<_>>(),
        ["-DGLOBAL_CPP=1 -DREPEAT=1 -DEXTRA=1 -DREPEAT=1 -DEXTRA=1"]
    );
}

#[test]
#[ignore = "requires explicit AROS_TEST_SOURCE_ROOT; original static-pattern recipe text replay only"]
fn original_c_static_patterns_feed_frozen_target_values_and_automatic_recipe_values() {
    let source = original_source_root();
    crate::capability::mesa::configured_source_context::verify_templates(&source).unwrap();
    let fragment = compile_multi_fragment(&source);
    let program = join_continuations(&format!(
        "{SYNTHETIC_GLOBALS}{fragment}\nCFLAGS := -DFINAL_CFLAGS\nCPPFLAGS := -DFINAL_CPPFLAGS\n"
    ));
    let (scope, states) = collect_vars_impl(&program, Some(&TargetContext::default()));
    let states = states
        .into_iter()
        .map(|state| match state {
            ConditionalTruth::True => Some(true),
            ConditionalTruth::False => Some(false),
            ConditionalTruth::Unknown => None,
        })
        .collect::<Vec<_>>();
    let dirs = DirVars::default();
    let relative = Path::new("workbench/libs/mesa/libgalliumaux");
    let projected =
        crate::make_rule_program::expand(&program, &states, &|raw, line, definitions| {
            let context = MakeExprContext::new(&scope, &dirs, line, &source, relative)
                .with_definitions(definitions)
                .without_filesystem();
            crate::make_expr::evaluate_make_rule_expansion(raw, &context)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let projected_states = projected
        .lines
        .iter()
        .map(|line| line.active)
        .collect::<Vec<_>>();
    let captured = capture(
        &projected.text,
        &selected_variables(),
        &projected_states,
        &|raw, line, overlay| {
            let escaped = overlay
                .iter()
                .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
                .collect::<BTreeMap<_, _>>();
            let lookup = |name: &str| escaped.get(name).cloned();
            let context = MakeExprContext::new(
                &scope,
                &dirs,
                projected.lines[line].source_line,
                &source,
                relative,
            )
            .with_lookup(&lookup)
            .without_filesystem();
            evaluate_make_expr(raw, &context).map_err(|error| error.to_string())
        },
    )
    .unwrap();
    let events = crate::make_rule_events::collect(&projected).unwrap();
    let mut instances = Vec::new();
    for rule in &events.rules {
        instances.extend(
            crate::make_rule_events::instantiate(rule, &|raw, line| {
                let context = MakeExprContext::new(&scope, &dirs, line, &source, relative)
                    .without_filesystem();
                evaluate_make_expr(raw, &context).map_err(|error| error.to_string())
            })
            .unwrap(),
        );
    }
    assert_eq!(instances.len(), 4);
    assert_eq!(
        instances
            .iter()
            .map(|row| row.target.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["a.o", "b.o", "a.d", "b.d"])
    );
    for instance in &instances {
        let extension = Path::new(&instance.target)
            .extension()
            .and_then(std::ffi::OsStr::to_str);
        assert!(matches!(extension, Some("o" | "d")));
        let stem = instance.stem.as_deref().unwrap();
        assert_eq!(instance.prerequisites, [format!("{stem}.c")]);
        let overlay = &captured.targets[&instance.target];
        let escaped = overlay
            .iter()
            .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
            .collect::<BTreeMap<_, _>>();
        let lookup = |name: &str| escaped.get(name).cloned();
        let automatic = instance.automatic_variables();
        let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &source, relative)
            .with_lookup(&lookup)
            .with_automatic_variables(&automatic)
            .with_working_directory(Path::new("/synthetic/build"))
            .without_filesystem();
        let recipes = instance
            .recipes
            .iter()
            .map(|recipe| evaluate_make_expr(&recipe.text, &context).unwrap())
            .collect::<Vec<_>>();
        let text = recipes.join("\n");
        assert!(
            !text.contains("FINAL_CFLAGS") && !text.contains("FINAL_CPPFLAGS"),
            "{text}"
        );
        if extension == Some("o") {
            assert!(
                text.contains("/selected/clang --sysroot=/selected/sysroot"),
                "{text}"
            );
            assert!(text.contains("-O2 -DREPEAT=1 -DSAFE=1 -DREPEAT=1 -DGLOBAL_CPP=1 -DREPEAT=1 -DEXTRA=1 -DREPEAT=1 -DEXTRA=1"), "{text}");
            assert!(
                text.contains(&format!("-c {stem}.c -o {} >", instance.target)),
                "{text}"
            );
        } else {
            assert!(text.contains(&format!("/selected/mkdepend -O2 -DREPEAT=1 -DGLOBAL_CPP=1 -DREPEAT=1 -DEXTRA=1 -DREPEAT=1 -DEXTRA=1 -I${{AROS_BUILD_DIR}}/workbench/libs/mesa/libgalliumaux -I${{AROS_SOURCE_DIR}}/workbench/libs/mesa/libgalliumaux {stem}.c -o {}", instance.target)), "{text}");
        }
    }
}

#[test]
#[ignore = "requires explicit AROS_TEST_SOURCE_ROOT; original-template synthetic text replay only"]
fn original_c_nonempty_targetdir_uses_generic_search_paths_and_recipe_values() {
    let source = original_source_root();
    crate::capability::mesa::configured_source_context::verify_templates(&source)
        .expect("verify the selected original recipe templates before replay");

    let fixture = tempfile::tempdir().expect("create temporary source fixture");
    let source_root = fixture
        .path()
        .canonicalize()
        .expect("canonical fixture source root");
    let expected_a = source_root.join("unit/left/a.c");
    let expected_b = source_root.join("unit/right/b.c");
    for path in [&expected_a, &expected_b] {
        fs::create_dir_all(path.parent().expect("fixture source parent"))
            .expect("create fixture source directory");
        fs::write(path, b"").expect("write empty fixture source");
    }
    let build_fixture = tempfile::tempdir().expect("create separate temporary build directory");
    let build_directory = build_fixture
        .path()
        .canonicalize()
        .expect("canonical fixture build directory");

    let relative = Path::new("unit");
    let global_vpath = source_root
        .join(relative)
        .to_str()
        .expect("UTF-8 fixture VPATH")
        .to_owned();
    let globals = format!(
        "{SYNTHETIC_GLOBALS}SRCDIR := {}\nCURDIR := unit\nVPATH := {global_vpath}\n",
        source_root.display()
    );
    let fragment = compile_multi_fragment_with(&source, "left/a right/b", "objects");
    let program = join_continuations(&format!(
        "{globals}\n{fragment}\nCFLAGS := -DFINAL_CFLAGS\nCPPFLAGS := -DFINAL_CPPFLAGS\n"
    ));
    let dirs = DirVars::load(&source);
    let (scope, states) = collect_vars_with_immediate_evaluator(
        &program,
        &TargetContext::default(),
        &|expression, partial_scope, line| {
            let context = MakeExprContext::new(partial_scope, &dirs, line, &source_root, relative)
                .with_working_directory(&build_directory)
                .with_pure_text_pipeline()
                .without_filesystem();
            evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
        },
    )
    .expect("evaluate original immediate globals without filesystem or process access");

    let projected = crate::make_rule_program::expand(
        &program,
        &states,
        &|expression, source_line, definitions| {
            let context = MakeExprContext::new(&scope, &dirs, source_line, &source_root, relative)
                .with_definitions(definitions)
                .with_working_directory(&build_directory)
                .with_pure_text_pipeline()
                .without_filesystem();
            crate::make_expr::evaluate_make_rule_expansion(expression, &context)
                .map_err(|error| error.to_string())
        },
    )
    .expect("project original Make rule text without running recipes");

    let projected_states = projected
        .lines
        .iter()
        .map(|line| line.active)
        .collect::<Vec<_>>();
    let captured = capture(
        &projected.text,
        &selected_variables(),
        &projected_states,
        &|expression, output_line, overlay| {
            let source_line = projected.lines[output_line].source_line;
            let escaped_overlay = overlay
                .iter()
                .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
                .collect::<BTreeMap<_, _>>();
            let lookup = |name: &str| escaped_overlay.get(name).cloned();
            let context = MakeExprContext::new(&scope, &dirs, source_line, &source_root, relative)
                .with_lookup(&lookup)
                .with_working_directory(&build_directory)
                .with_pure_text_pipeline()
                .without_filesystem();
            evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
        },
    )
    .expect("capture original target-specific C values at their projected source positions");

    let events = crate::make_rule_events::collect(&projected)
        .expect("collect generic original rules, recipes and search declarations");
    assert!(!events.search_paths.is_empty(), "{events:#?}");
    let search_paths =
        crate::make_vpath::project(&events.search_paths, &|expression, source_line| {
            let context = MakeExprContext::new(&scope, &dirs, source_line, &source_root, relative)
                .with_working_directory(&build_directory)
                .with_pure_text_pipeline()
                .without_filesystem();
            evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
        })
        .expect("project selective Make searches at their original source positions");

    let mut instances = Vec::new();
    for rule in &events.rules {
        instances.extend(
            crate::make_rule_events::instantiate(rule, &|expression, source_line| {
                let context =
                    MakeExprContext::new(&scope, &dirs, source_line, &source_root, relative)
                        .with_working_directory(&build_directory)
                        .with_pure_text_pipeline()
                        .without_filesystem();
                evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
            })
            .unwrap_or_else(|error| panic!("instantiate original C rule {rule:?}: {error}")),
        );
    }
    let graph = crate::make_rule_graph::compose(instances)
        .expect("compose all original C rule instances without dropping prerequisites");
    assert_eq!(
        graph
            .targets
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["objects/a.o", "objects/a.d", "objects/b.o", "objects/b.d"])
    );

    let expected_paths = [
        ("a.c", expected_a.clone()),
        ("b.c", expected_b.clone()),
        ("left/a.c", expected_a.clone()),
        ("right/b.c", expected_b.clone()),
    ];
    let mut resolved_paths = BTreeMap::new();
    for (logical, expected) in expected_paths {
        let resolved = resolve_vpath_file(&search_paths, logical, &build_directory, &global_vpath);
        assert_eq!(resolved, expected, "Make vpath mapping for {logical}");
        resolved_paths.insert(logical.to_owned(), resolved);
    }

    let expected_cppflags = "-DGLOBAL_CPP=1 -DREPEAT=1 -DEXTRA=1 -DREPEAT=1 -DEXTRA=1";
    let expected_compile_flags = format!("-O2 -DREPEAT=1 -DSAFE=1 -DREPEAT=1 {expected_cppflags}");
    let expected_dependency_flags = format!("-O2 -DREPEAT=1 {expected_cppflags}");
    let expected_target_set =
        BTreeSet::from(["objects/a.o", "objects/a.d", "objects/b.o", "objects/b.d"]);
    assert_eq!(
        captured
            .targets
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        expected_target_set
    );

    for (target, target_rules) in &graph.targets {
        let crate::make_rule_graph::TargetRules::Ordinary(ordinary) = target_rules else {
            panic!("original C target became double-colon: {target}");
        };
        let mut effective = ordinary.effective_instance();
        let base = Path::new(target)
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .expect("object target basename");
        let logical_source = format!("{base}.c");
        assert_eq!(
            effective.prerequisites,
            std::slice::from_ref(&logical_source),
            "{target}"
        );
        assert_eq!(
            effective.order_only,
            ["left/a.c".to_owned(), "right/b.c".to_owned()],
            "supplemental order-only inputs for {target}"
        );
        for prerequisite in &mut effective.prerequisites {
            *prerequisite = resolved_paths[prerequisite]
                .to_str()
                .expect("UTF-8 resolved normal prerequisite")
                .to_owned();
        }
        for prerequisite in &mut effective.order_only {
            *prerequisite = resolved_paths[prerequisite]
                .to_str()
                .expect("UTF-8 resolved order-only prerequisite")
                .to_owned();
        }
        for prerequisite in &mut effective.prerequisite_order {
            *prerequisite = resolved_paths[prerequisite]
                .to_str()
                .expect("UTF-8 resolved declaration-order prerequisite")
                .to_owned();
        }

        let expected_source = if base == "a" {
            &expected_a
        } else {
            &expected_b
        };
        let expected_source = expected_source
            .to_str()
            .expect("UTF-8 expected source path");
        assert_eq!(effective.prerequisites, [expected_source], "{target}");
        assert_eq!(
            effective.order_only,
            [
                expected_a
                    .to_str()
                    .expect("UTF-8 expected a.c path")
                    .to_owned(),
                expected_b
                    .to_str()
                    .expect("UTF-8 expected b.c path")
                    .to_owned(),
            ],
            "resolved supplemental inputs for {target}"
        );

        let automatic = effective.automatic_variables();
        assert_eq!(automatic["<"], expected_source, "{target}");
        let other_source = if base == "a" {
            expected_b.to_str().expect("UTF-8 expected b.c path")
        } else {
            expected_a.to_str().expect("UTF-8 expected a.c path")
        };
        assert_eq!(automatic["|"], other_source, "{target}");

        let overlay = &captured.targets[target];
        let escaped_overlay = overlay
            .iter()
            .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
            .collect::<BTreeMap<_, _>>();
        let lookup = |name: &str| escaped_overlay.get(name).cloned();
        let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &source_root, relative)
            .with_lookup(&lookup)
            .with_automatic_variables(&automatic)
            .with_working_directory(&build_directory)
            .with_pure_text_pipeline()
            .without_filesystem();
        let recipe_text = effective
            .recipes
            .iter()
            .map(|recipe| {
                evaluate_make_expr(&recipe.text, &context)
                    .unwrap_or_else(|error| panic!("expand original recipe for {target}: {error}"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !recipe_text.contains("FINAL_CFLAGS") && !recipe_text.contains("FINAL_CPPFLAGS"),
            "final poisoned globals leaked into {target}: {recipe_text}"
        );
        if Path::new(target)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            == Some("o")
        {
            assert!(
                recipe_text.contains("/selected/clang --sysroot=/selected/sysroot"),
                "{recipe_text}"
            );
            assert!(
                recipe_text.contains(&expected_compile_flags),
                "{recipe_text}"
            );
            assert!(
                recipe_text.contains(&format!("-c {expected_source} -o {target} >")),
                "{recipe_text}"
            );
        } else {
            assert!(recipe_text.contains("/selected/mkdepend"), "{recipe_text}");
            assert!(
                recipe_text.contains(&expected_dependency_flags),
                "{recipe_text}"
            );
            assert!(
                recipe_text.contains(&format!("{expected_source} -o {target}")),
                "{recipe_text}"
            );
        }
    }

    // The earlier positive results bind to the fixture sources. A later local
    // hit demonstrates first-present Make search order only; it makes no input
    // authentication claim.
    let shadow = build_directory.join("a.c");
    fs::write(&shadow, b"shadow").expect("write build-directory shadow source");
    let candidates = search_paths
        .candidates("a.c", &build_directory, &global_vpath)
        .expect("build shadow-source candidates");
    let inspected = Cell::new(0);
    let selected = crate::make_vpath::first_present(&candidates, &|candidate| {
        inspected.set(inspected.get() + 1);
        observe_vpath_candidate(candidate)
    })
    .expect("observe shadow-source candidates");
    assert_eq!(selected, Some(shadow));
    assert_ne!(selected, Some(expected_a));
    assert_eq!(inspected.get(), 1, "first-present inspection count");
}
