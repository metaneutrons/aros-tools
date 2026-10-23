//! Optional text-only replay of the original C++ multi-rule template.
//!
//! This expands source-owned GenMF and Make definitions against synthetic
//! flags and a temporary source tree. It never runs Make or a compiler and
//! never writes into the selected Source checkout.

use super::configured_source_context;
use crate::dirs::DirVars;
use crate::genmf_projection::{expand_text, Limits};
use crate::make_expr::{evaluate_make_expr, evaluate_make_rule_expansion, MakeExprContext};
use crate::make_vars::collect_vars_with_immediate_evaluator;
use crate::parser::{join_continuations, TargetContext};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const OWNER: &str = "generic-unit";

const SYNTHETIC_GLOBALS: &str = concat!(
    "Q := @\n",
    "ECHO := echo\n",
    "IF := if\n",
    "TEST := test\n",
    "NOP := :\n",
    "MKDEPEND := /synthetic/bin/mkdepend\n",
    "MKDIR := mkdir -p\n",
    "GENDIR := /synthetic/generated\n",
    "CXXFLAGS := -O2 -DGLOBAL_CXX=1\n",
    "CPPFLAGS := -DGLOBAL_CPP=1\n",
    "AROS_CXX := /synthetic/bin/cxx\n",
    "TARGET_CC := /synthetic/bin/cc\n",
    "TARGET_SYSROOT := --sysroot=/synthetic/sysroot\n",
    "SAFETY_CFLAGS := -DSAFETY=1\n",
    "CFLAGS_IQUOTE := -iquote\n",
    "CFLAGS_IQUOTE_END := -I-\n",
    "TARGET_CXX_LIBS := -lsynthetic\n",
    "AROS_CXXEXTS := cpp\n",
    "generic-unit_MCXX_EXTRA_CPPFLAGS := -DEXTRA=1 -DREPEAT=1\n",
);

fn original_source_root() -> PathBuf {
    PathBuf::from(std::env::var_os("AROS_TEST_SOURCE_ROOT").expect("AROS_TEST_SOURCE_ROOT"))
        .canonicalize()
        .expect("canonical Source root")
}

fn cxx_multi_fragment(source: &Path) -> String {
    let template = source.join("config/make.tmpl");
    let invocation_file = source.join("workbench/libs/mesa/libgalliumaux/mmakefile.src");
    let invocation = format!(
        "%rule_compile_cxx_multi mmake={OWNER} basenames=\"a b\" cppflags=\"$(CPPFLAGS)\" cxxflags=\"$(CXXFLAGS)\" dxxflags= targetdir=objects compiler=target incextra=\"$(TOP)/$(CURDIR)\" usetree=no\n"
    );
    let expanded = expand_text(&invocation, &invocation_file, &template, Limits::default())
        .expect("expand the original rule_compile_cxx_multi macro");

    let mut fragment = String::new();
    for span in &expanded.provenance {
        let from_target_rule = span
            .macro_stack
            .iter()
            .any(|frame| frame.name == "rule_compile_cxx_multi");
        let from_common = span.macro_stack.iter().any(|frame| frame.name == "common");
        if !from_target_rule || from_common {
            continue;
        }
        let text = &expanded.text[span.output_start_byte..span.output_end_byte];
        if !fragment.is_empty() && !fragment.ends_with('\n') && !text.starts_with('\n') {
            fragment.push('\n');
        }
        fragment.push_str(text);
    }
    assert!(
        fragment.contains("generic-unit_CXXABSBASENAMES"),
        "{fragment}"
    );
    assert!(fragment.contains("cxx_multi_recipe_template"), "{fragment}");
    fragment
}

#[test]
#[ignore = "requires explicit AROS_TEST_SOURCE_ROOT; original-template text replay only"]
fn original_cxx_multi_eval_emits_frozen_object_and_dependency_rules() {
    let source = original_source_root();
    configured_source_context::verify_templates(&source)
        .expect("verify original recipe templates before replay");

    let fixture = tempfile::tempdir().expect("temporary source fixture");
    let source_dir = fixture.path();
    let relative_dir = Path::new("fixture");
    fs::create_dir(source_dir.join(relative_dir)).expect("create fixture source directory");
    fs::write(source_dir.join(relative_dir).join("a.cpp"), "").expect("write empty a.cpp");
    fs::write(source_dir.join(relative_dir).join("b.cpp"), "").expect("write empty b.cpp");

    let fragment = cxx_multi_fragment(&source);
    let program = join_continuations(&format!("{SYNTHETIC_GLOBALS}\n{fragment}"));
    let target = TargetContext {
        make_variables: BTreeMap::from([
            ("SRCDIR".to_owned(), "${AROS_SOURCE_DIR}".to_owned()),
            ("CURDIR".to_owned(), "fixture".to_owned()),
        ]),
        ..TargetContext::default()
    };
    let dirs = DirVars::load(&source);
    let (scope, states) =
        collect_vars_with_immediate_evaluator(&program, &target, &|raw, scope, line| {
            let context = MakeExprContext::new(scope, &dirs, line, source_dir, relative_dir)
                .with_working_directory(source_dir);
            evaluate_make_expr(raw, &context).map_err(|error| error.to_string())
        })
        .expect("evaluate active immediate globals generically at their original position");

    let projected = crate::make_rule_program::expand(
        &program,
        &states,
        &|expression, source_line, definitions| {
            let context =
                MakeExprContext::new(&scope, &dirs, source_line, source_dir, relative_dir)
                    .with_definitions(definitions)
                    .with_working_directory(source_dir);
            evaluate_make_rule_expansion(expression, &context).map_err(|error| error.to_string())
        },
    )
    .expect("project original C++ program without selecting template-specific eval lines");
    assert_eq!(projected.fragments.len(), 2, "{:#?}", projected.fragments);
    assert_eq!(
        projected.fragments[0].source_line,
        projected.fragments[1].source_line
    );
    let events = crate::make_rule_events::collect(&projected)
        .expect("parse every generated C++ rule with the common rule collector");
    let mut instances = BTreeMap::new();
    let mut prerequisite_rules = BTreeMap::new();
    let mut all_instances = Vec::new();
    for rule in &events.rules {
        let resolved = crate::make_rule_events::instantiate(rule, &|raw, line| {
            let context = MakeExprContext::new(&scope, &dirs, line, source_dir, relative_dir)
                .with_working_directory(source_dir);
            evaluate_make_expr(raw, &context).map_err(|error| error.to_string())
        })
        .unwrap_or_else(|error| panic!("instantiate original C++ rule {rule:?}: {error}"));
        all_instances.extend(resolved.iter().cloned());
        for instance in resolved {
            let destination = if instance.recipes.is_empty() {
                &mut prerequisite_rules
            } else {
                &mut instances
            };
            assert!(destination
                .insert(instance.target.clone(), instance)
                .is_none());
        }
    }
    assert_eq!(instances.len(), 4);
    let graph = crate::make_rule_graph::compose(all_instances)
        .expect("compose original supplemental and recipe rules without dropping either");
    assert_eq!(graph.targets.len(), 4);
    assert_eq!(prerequisite_rules.len(), 4);
    assert_eq!(
        instances.keys().collect::<Vec<_>>(),
        prerequisite_rules.keys().collect::<Vec<_>>()
    );
    let expected_order_only = ["a.cpp", "b.cpp"].map(|name| {
        source_dir
            .join(relative_dir)
            .join(name)
            .to_str()
            .unwrap()
            .to_owned()
    });
    for instance in prerequisite_rules.values() {
        assert!(instance.prerequisites.is_empty());
        assert_eq!(instance.order_only, expected_order_only);
        assert!(instance.source_line < projected.fragments[0].source_line);
        assert_eq!(
            program.lines().nth(instance.source_line).unwrap().trim(),
            format!("$({OWNER}_CXXTARGETS) $({OWNER}_CXXDTARGETS) : | $({OWNER}_MCXX_FILES)")
        );
    }
    let expected_compile_flags =
        "-O2 -DGLOBAL_CXX=1 -DSAFETY=1 -DGLOBAL_CPP=1 -DEXTRA=1 -DREPEAT=1";
    let expected_dependency_flags = "-O2 -DGLOBAL_CXX=1 -DGLOBAL_CPP=1 -DEXTRA=1 -DREPEAT=1";

    for (index, (base, source_name)) in [("a", "a.cpp"), ("b", "b.cpp")].into_iter().enumerate() {
        let fragment = &projected.fragments[index];
        let rule = &projected.text[fragment.start_byte..fragment.end_byte];
        let physical_source = source_dir.join(relative_dir).join(source_name);
        let physical_source = physical_source.to_str().expect("UTF-8 fixture path");
        for extension in ["o", "d"] {
            let target = format!("objects/{base}.{extension}");
            let instance = &instances[&target];
            assert_eq!(instance.source_line, fragment.source_line);
            assert_eq!(instance.prerequisites, [physical_source]);
            assert!(!instance.recipes.is_empty());
            assert!(instance.order_only.is_empty());
            assert!(!instance.double_colon);
            assert_eq!(instance.stem, None);
            for recipe in &instance.recipes {
                assert_eq!(recipe.source_line, fragment.source_line);
                assert!(rule.contains(&format!("\t{}", recipe.text)));
            }
            let crate::make_rule_graph::TargetRules::Ordinary(composed) = &graph.targets[&target]
            else {
                panic!("original ordinary C++ target became double-colon");
            };
            assert_eq!(composed.declarations().len(), 2);
            let effective = composed.effective_instance();
            assert_eq!(effective.prerequisites, [physical_source]);
            assert_eq!(effective.order_only, expected_order_only);
            assert_eq!(effective.source_line, fragment.source_line);
            assert_eq!(effective.recipes, instance.recipes);
            let automatic = effective.automatic_variables();
            assert_eq!(automatic["<"], physical_source);
            let other_source = expected_order_only
                .iter()
                .filter(|path| *path != physical_source)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(automatic["|"], other_source);
        }
        assert!(
            rule.contains(&format!("objects/{base}.o : {physical_source}")),
            "{rule}"
        );
        assert!(
            rule.contains(&format!("objects/{base}.d : {physical_source}")),
            "{rule}"
        );
        assert!(
            rule.contains(&format!(
                "\t@if /synthetic/bin/cxx --sysroot=/synthetic/sysroot -iquote {}/ -iquote ${{AROS_BUILD_DIR}}/fixture -iquote ${{AROS_SOURCE_DIR}}/fixture -iquote . -I- {expected_compile_flags} -D__SRCFILENAME__=",
                source_dir.join(relative_dir).display()
            )),
            "{rule}"
        );
        assert!(
            rule.contains(&format!(
                "-c {physical_source} -o objects/{base}.o > /synthetic/generated/cerrors"
            )),
            "{rule}"
        );
        assert!(
            rule.contains(&format!(
                "\t@AROS_CC=\"/synthetic/bin/cxx --sysroot=/synthetic/sysroot\" /synthetic/bin/mkdepend {expected_dependency_flags} -I${{AROS_BUILD_DIR}}/fixture -I${{AROS_SOURCE_DIR}}/fixture {physical_source} -o objects/{base}.d"
            )),
            "{rule}"
        );
        assert!(
            !rule.contains("$(generic-unit_"),
            "flags were not frozen: {rule}"
        );
        assert!(
            !rule.contains("SAFETY_CFLAGS"),
            "flags were not frozen: {rule}"
        );
    }
}
