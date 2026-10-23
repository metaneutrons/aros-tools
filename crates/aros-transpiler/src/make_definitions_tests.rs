use super::MakeDefinitions;
use crate::dirs::DirVars;
use crate::make_expr::{
    evaluate_make_expr, evaluate_make_rule_expansion, MakeExprContext, MakeExprError,
};
use crate::make_vars::collect_vars;
use std::path::Path;

fn active_states(program: &str) -> Vec<Option<bool>> {
    vec![Some(true); program.lines().count()]
}

#[test]
fn source_mode_never_falls_back_to_configured_directory_aliases() {
    let dirs = DirVars::from_config_text("ORDINARY := configured-only\n");
    let scope = collect_vars("");
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .without_source_projection();
    for expression in ["$(TOP)", "${SRCDIR}", "$(ORDINARY)"] {
        assert!(matches!(
            evaluate_make_expr(expression, &context),
            Err(MakeExprError::UnresolvedVariables { .. })
        ));
    }
    let source = collect_vars("TOP := measured-source\nORDINARY := source-value\n");
    let context = MakeExprContext::new(
        &source,
        &dirs,
        usize::MAX,
        Path::new("/source"),
        Path::new("unit"),
    )
    .without_source_projection();
    assert_eq!(
        evaluate_make_expr("$(TOP)|$(ORDINARY)", &context).unwrap(),
        "measured-source|source-value"
    );
}

fn line_containing(program: &str, expected: &str) -> usize {
    program
        .lines()
        .position(|line| line == expected)
        .unwrap_or_else(|| panic!("missing fixture line: {expected}"))
}

fn evaluate_rules_at(
    program: &str,
    line: usize,
    expression: &str,
) -> Result<crate::make_expr::MakeRuleExpansion, MakeExprError> {
    let states = active_states(program);
    let definitions = MakeDefinitions::collect_at(program, line, &states)
        .unwrap_or_else(|error| panic!("invalid definitions fixture: {error}"));
    let scope = collect_vars(program);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(&scope, &dirs, line, Path::new("."), Path::new("fixture"))
        .with_definitions(&definitions);
    evaluate_make_rule_expansion(expression, &context)
}

fn evaluate_expr_at(program: &str, line: usize, expression: &str) -> Result<String, MakeExprError> {
    let states = active_states(program);
    let definitions = MakeDefinitions::collect_at(program, line, &states)
        .unwrap_or_else(|error| panic!("invalid definitions fixture: {error}"));
    let scope = collect_vars(program);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(&scope, &dirs, line, Path::new("."), Path::new("fixture"))
        .with_definitions(&definitions);
    evaluate_make_expr(expression, &context)
}

#[test]
fn recursive_defines_call_foreach_and_eval_emit_ordered_generic_object_rules() {
    let program = "define compile_unit\n\
$(1)_$(2).o: $(1)_$(2).c\n\
\t@compile $$< -o $$@\n\
endef\n\
$(foreach unit,solid wire,$(eval $(call compile_unit,renderer,$(unit))))\n";
    let eval_line = line_containing(
        program,
        "$(foreach unit,solid wire,$(eval $(call compile_unit,renderer,$(unit))))",
    );

    let expansion = evaluate_rules_at(
        program,
        eval_line,
        "$(foreach unit,solid wire,$(eval $(call compile_unit,renderer,$(unit))))",
    )
    .unwrap();

    assert_eq!(expansion.rules.len(), 2);
    assert!(expansion.rules[0].contains("renderer_solid.o: renderer_solid.c"));
    assert!(expansion.rules[1].contains("renderer_wire.o: renderer_wire.c"));
    let emitted = expansion.rules.join("\n");
    assert!(
        emitted.contains("$<"),
        "recipe input reference was not deferred: {emitted}"
    );
    assert!(
        emitted.contains("$@"),
        "recipe target reference was not deferred: {emitted}"
    );
}

#[test]
fn recursive_define_expands_global_values_at_the_call_site() {
    let program = "BASE := early\n\
define emit_unit\n\
$(BASE)_$(1).o: $(BASE)_$(1).c\n\
endef\n\
BASE := current\n\
$(eval $(call emit_unit,worker))\n";
    let eval_line = line_containing(program, "$(eval $(call emit_unit,worker))");

    let expansion =
        evaluate_rules_at(program, eval_line, "$(eval $(call emit_unit,worker))").unwrap();

    assert_eq!(expansion.rules.len(), 1);
    assert!(expansion.rules[0].contains("current_worker.o: current_worker.c"));
    assert!(!expansion.rules[0].contains("early_worker"));
}

#[test]
fn replacing_assignments_undefine_and_recursive_append_update_define_macros() {
    let replaced = "define emit_unit\n\
$(1)_old.o: $(1)_old.c\n\
endef\n\
emit_unit = $(1)_new.o: $(1)_new.c\n\
$(eval $(call emit_unit,worker))\n";
    let line = line_containing(replaced, "$(eval $(call emit_unit,worker))");
    let expansion = evaluate_rules_at(replaced, line, "$(eval $(call emit_unit,worker))").unwrap();
    assert_eq!(expansion.rules.len(), 1);
    assert!(expansion.rules[0].contains("worker_new.o: worker_new.c"));
    assert!(!expansion.rules[0].contains("worker_old"));

    let appended = "define emit_unit\n\
$(1)_source.o: $(1)_source.c\n\
endef\n\
emit_unit += $(1)_header.o: $(1)_header.h\n\
$(eval $(call emit_unit,worker))\n";
    let line = line_containing(appended, "$(eval $(call emit_unit,worker))");
    let expansion = evaluate_rules_at(appended, line, "$(eval $(call emit_unit,worker))").unwrap();
    // += appends text separated by one space. It does not introduce a second
    // eval call or a newline; parsing the resulting rules is a later phase.
    assert_eq!(expansion.rules.len(), 1);
    assert_eq!(
        expansion.rules[0],
        "worker_source.o: worker_source.c worker_header.o: worker_header.h"
    );

    let undefined = "define emit_unit\n\
$(1)_old.o: $(1)_old.c\n\
endef\n\
undefine emit_unit\n\
$(eval $(call emit_unit,worker))\n";
    let line = line_containing(undefined, "$(eval $(call emit_unit,worker))");
    let undefined = evaluate_rules_at(undefined, line, "$(eval $(call emit_unit,worker))").unwrap();
    assert!(undefined.value.is_empty());
    assert!(undefined.rules.iter().all(String::is_empty));
}

#[test]
fn nested_define_body_text_is_preserved() {
    let program = "define outer_template\n\
start\n\
define inner_template\n\
inner payload\n\
endef\n\
finish\n\
endef\n\
$(outer_template)\n";
    let use_line = line_containing(program, "$(outer_template)");

    let value = evaluate_expr_at(program, use_line, "$(outer_template)").unwrap();

    assert!(value.contains("define inner_template\ninner payload\nendef"));
    assert!(value.ends_with("finish"));
}

#[test]
fn inactive_definitions_are_ignored_and_unknown_definitions_are_rejected() {
    let program = "define selected_rule\n\
$(1).o: $(1).c\n\
endef\n\
$(eval $(call selected_rule,worker))\n";
    let eval_line = line_containing(program, "$(eval $(call selected_rule,worker))");
    let mut states = active_states(program);
    states[0] = Some(false);
    let definitions = MakeDefinitions::collect_at(program, eval_line, &states).unwrap();
    assert!(definitions.bodies.is_empty());
    let scope = collect_vars(program);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        eval_line,
        Path::new("."),
        Path::new("fixture"),
    )
    .with_definitions(&definitions);
    assert!(matches!(
        evaluate_make_rule_expansion("$(eval $(call selected_rule,worker))", &context),
        Err(MakeExprError::UnsafeVariable { name, .. }) if name == "selected_rule"
    ));

    let mut states = active_states(program);
    states[0] = None;
    let error = MakeDefinitions::collect_at(program, eval_line, &states).unwrap_err();
    assert_eq!(error, "Make definition has unresolved selection");
}

#[test]
fn malformed_dynamic_and_simple_define_forms_fail_closed() {
    for program in [
        "define incomplete\nbody\n",
        "define $(DYNAMIC_NAME)\nbody\nendef\n",
        "define immediate :=\nbody\nendef\n",
    ] {
        let states = active_states(program);
        let error = MakeDefinitions::collect_at(program, program.lines().count(), &states)
            .expect_err("unsupported define syntax must not be accepted");
        assert!(error.contains("Make definition"), "{error}");
    }
}

#[test]
fn eval_rejects_global_mutation_and_non_rule_output() {
    let fixtures = [
        (
            "define generated_text\nVALUE := changed\nendef\n",
            "assignment",
        ),
        (
            "define generated_text\ninclude other.mk\nendef\n",
            "include",
        ),
        ("define generated_text\nordinary text\nendef\n", "rule"),
    ];

    for (definition, reason) in fixtures {
        let program = format!("{definition}$(eval $(call generated_text))\n");
        let expression = "$(eval $(call generated_text))";
        let line = line_containing(&program, expression);
        let error = evaluate_rules_at(&program, line, expression).unwrap_err();
        let detail = error.to_string().to_lowercase();
        assert!(
            detail.contains("eval") || detail.contains(reason),
            "{error}"
        );
    }
}

#[test]
fn ordinary_expression_evaluation_continues_to_reject_eval() {
    let program = "define emit_unit\n$(1).o: $(1).c\nendef\n";
    let line = program.lines().count();
    let states = active_states(program);
    let definitions = MakeDefinitions::collect_at(program, line, &states).unwrap();
    let scope = collect_vars(program);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(&scope, &dirs, line, Path::new("."), Path::new("fixture"))
        .with_definitions(&definitions);

    assert!(matches!(
        evaluate_make_expr("$(eval $(call emit_unit,worker))", &context),
        Err(MakeExprError::UnsupportedFunction { name }) if name == "eval"
    ));
}

#[test]
fn eval_does_not_mistake_colon_named_include_files_for_rules() {
    for expression in [
        "$(eval include missing.mk:other.mk)",
        "$(eval -include missing.mk:other.mk)",
        "$(eval sinclude missing.mk:other.mk)",
        "$(eval export NAME:=value)",
        "$(eval load plugin.so:alias.so)",
        "$(eval -load plugin.so:alias.so)",
        "$(eval ifeq(a:b,c))",
    ] {
        let error = evaluate_rules_at("", 0, expression).unwrap_err();
        assert!(error.to_string().contains("eval supports rule data only"));
    }
}

#[test]
fn local_lookup_shadows_defined_global_but_custom_guard_still_applies() {
    let program = "define NAME\nglobal\nendef\n";
    let definitions =
        MakeDefinitions::collect_at(program, usize::MAX, &active_states(program)).unwrap();
    let scope = collect_vars(program);
    let dirs = DirVars::default();
    let lookup = |name: &str| (name == "NAME").then(|| "local".to_owned());
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("/source"),
        Path::new(""),
    )
    .with_definitions(&definitions)
    .with_lookup(&lookup);
    assert_eq!(evaluate_make_expr("$(NAME)", &context).unwrap(), "local");
    let guard = |name: &str| (name == "NAME").then(|| "independent guard".to_owned());
    assert!(evaluate_make_expr("$(NAME)", &context.with_guard(&guard))
        .unwrap_err()
        .to_string()
        .contains("independent guard"));
}

#[test]
fn abspath_is_pure_and_uses_only_explicit_coordinates() {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("sub"))
        .without_filesystem();
    assert_eq!(
        evaluate_make_expr("$(abspath /x/./y/../z /../../root)", &context).unwrap(),
        "/x/z /root"
    );
    assert!(evaluate_make_expr("$(abspath ../relative)", &context)
        .unwrap_err()
        .to_string()
        .contains("explicit Make working directory"));
    let explicit = context.with_working_directory(Path::new("/nominal/build"));
    assert_eq!(
        evaluate_make_expr("$(abspath ../relative ./other)", &explicit).unwrap(),
        "/nominal/relative /nominal/build/other"
    );
    assert_eq!(
        evaluate_make_expr("$(abspath $(SRCDIR)/sub/../unit.cpp)", &context).unwrap(),
        "/source/unit.cpp"
    );
    assert!(evaluate_make_expr("$(abspath $(TOP)/sub)", &explicit).is_err());
    let build = explicit.with_build_directory(Path::new("/independent/build"));
    assert_eq!(
        evaluate_make_expr("$(abspath $(TOP)/sub/.. . ../peer)", &build).unwrap(),
        "/independent/build /nominal/build /nominal/peer"
    );
    assert_eq!(
        evaluate_make_expr("$(TOP)", &build).unwrap(),
        "${AROS_BUILD_DIR}"
    );
    assert!(evaluate_make_expr(
        "$(abspath $(TOP))",
        &explicit.with_build_directory(Path::new("relative/build"))
    )
    .is_err());
    assert!(evaluate_make_expr("$(abspath $(UNKNOWN_ROOT)/sub)", &build).is_err());
}

#[test]
fn abspath_build_materialization_is_bounded_for_public_context_callers() {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let oversized = format!("/{}", "x".repeat(4 * 1024 * 1024));
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new(""))
        .without_filesystem()
        .with_build_directory(Path::new(&oversized));
    assert!(matches!(
        evaluate_make_expr("$(abspath $(TOP))", &context),
        Err(MakeExprError::ResourceLimit { .. })
    ));
    // A root not reached by the path expression must not be expanded.
    assert_eq!(
        evaluate_make_expr("$(abspath /short)", &context).unwrap(),
        "/short"
    );
    let cwd = context.with_working_directory(Path::new(&oversized));
    assert!(matches!(
        evaluate_make_expr("$(abspath relative)", &cwd),
        Err(MakeExprError::ResourceLimit { .. })
    ));
    assert_eq!(
        evaluate_make_expr("$(abspath /short)", &cwd).unwrap(),
        "/short"
    );
}

#[test]
fn source_mode_wildcard_uses_explicit_cwd_not_the_projection_source_root() {
    use std::{
        io::Write as _,
        process::{Command, Stdio},
    };
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let cwd = temp.path().join("build");
    std::fs::create_dir_all(source.join("unit")).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(source.join("unit/source_only.txt"), "source").unwrap();
    std::fs::write(cwd.join("child_only.txt"), "child").unwrap();
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let projected = MakeExprContext::new(&scope, &dirs, 0, &source, Path::new("unit"))
        .with_working_directory(&cwd);
    let native_context = projected.without_source_projection();
    let expression = "$(wildcard *.txt)";
    assert_eq!(
        evaluate_make_expr(expression, &projected).unwrap(),
        "source_only.txt"
    );
    let observed = evaluate_make_expr(expression, &native_context).unwrap();
    assert_eq!(observed, "child_only.txt");
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut child = Command::new(make)
        .args(["-rR", "--no-print-directory", "-s", "-f", "-", "probe"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("$(info AROS_TEST_GLOB={expression})\nprobe:\n").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "synthetic wildcard query failed");
    let text = String::from_utf8(output.stdout).unwrap();
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix("AROS_TEST_GLOB="))
        .collect::<Vec<_>>();
    assert_eq!(rows, [observed]);
    let no_cwd = MakeExprContext::new(&scope, &dirs, 0, &source, Path::new("unit"))
        .without_source_projection();
    assert!(evaluate_make_expr(expression, &no_cwd).is_err());
}

#[test]
fn modifier_prefixed_definition_shadows_never_reuse_stale_bodies() {
    for shadow in [
        "override emit = replacement",
        "private emit := replacement",
        "export emit = replacement",
        "override undefine emit",
        "override export emit = replacement",
    ] {
        let program = format!("define emit\nold-body\nendef\n{shadow}\n");
        let error = MakeDefinitions::collect_at(&program, usize::MAX, &active_states(&program))
            .unwrap_err();
        assert!(error.contains("modifier directive"), "{shadow}: {error}");
    }
}

#[test]
fn eval_refuses_second_parse_effects_outside_recipes() {
    for expression in [
        "$(eval target: $$(eval SIDE := changed))",
        "$(eval target: $$(shell forbidden))",
        "$(eval target: LOCAL := $$(eval SIDE := changed))",
    ] {
        let error = evaluate_rules_at("", 0, expression).unwrap_err();
        assert!(
            error.to_string().contains("eval supports rule data only"),
            "{error}"
        );
    }
    // Recipe expansion belongs to the later execution boundary, not eval's
    // parse-time prerequisite expansion. Keep automatic variables deferred.
    let expansion =
        evaluate_rules_at("", 0, "$(eval target: input\n\tcompile $$< -o $$@)").unwrap();
    assert_eq!(expansion.rules, ["target: input\n\tcompile $< -o $@"]);
}

#[test]
fn call_never_substitutes_user_body_for_a_make_builtin() {
    for builtin in ["strip", "foreach", "shell", "eval"] {
        let program = format!("define {builtin}\nnot-the-built-in\nendef\n");
        let expression = format!("$(call {builtin},argument)");
        let error = evaluate_expr_at(&program, usize::MAX, &expression).unwrap_err();
        assert!(
            matches!(error, MakeExprError::UnsupportedFunction { name } if name == format!("call built-in {builtin}"))
        );
    }
}

#[test]
fn definitions_never_admit_names_the_assignment_reader_cannot_shadow() {
    for later in ["emit.unit := replacement", "undefine emit.unit"] {
        let program = format!("define emit.unit\nstale\nendef\n{later}\n");
        let error = MakeDefinitions::collect_at(&program, usize::MAX, &active_states(&program))
            .unwrap_err();
        assert!(error.contains("definition name or flavor"), "{error}");
    }
}

#[test]
fn computed_shadows_and_unsupported_undefine_never_reuse_old_body() {
    for (later, reason) in [
        ("$(name) := new.o: new.c", "shadow assignment"),
        ("${name} += replacement", "shadow assignment"),
        ("ifeq(x=y,x=y)", "shadow assignment"),
        ("undefine $(name)", "definition undefine"),
        ("undefine emit another", "definition undefine"),
    ] {
        let program = format!("define emit\nold.o: old.c\nendef\nname := emit\n{later}\n");
        let error = MakeDefinitions::collect_at(&program, usize::MAX, &active_states(&program))
            .unwrap_err();
        assert!(error.contains(reason), "{later}: {error}");
    }
    let program = "define emit\nobject.o: object.c\nendef\n$(OBJECTS): LOCAL := harmless\n";
    assert!(MakeDefinitions::collect_at(program, usize::MAX, &active_states(program)).is_ok());
    let conditional = "define emit\nobject.o: object.c\nendef\nifeq (x=y,x=y)\nendif\n";
    assert!(
        MakeDefinitions::collect_at(conditional, usize::MAX, &active_states(conditional)).is_ok()
    );
}
