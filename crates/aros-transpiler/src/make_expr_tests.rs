use super::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use crate::dirs::DirVars;
use crate::make_vars::collect_vars;
use crate::parser::join_continuations;
use aros_common::read_source;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("aros-make-expr-{}-{serial}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root() -> PathBuf {
    crate::testing::root()
}

fn evaluate(src: &str, expression: &str) -> Result<String, MakeExprError> {
    let scope = collect_vars(src);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("."),
        Path::new("fixture"),
    );
    evaluate_make_expr(expression, &context)
}

fn evaluate_without_filesystem(src: &str, expression: &str) -> Result<String, MakeExprError> {
    let scope = collect_vars(src);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("."),
        Path::new("fixture"),
    )
    .without_filesystem();
    evaluate_make_expr(expression, &context)
}

#[test]
fn filesystem_disabled_context_evaluates_nested_pure_functions() {
    assert_eq!(
        evaluate_without_filesystem(
            "MODE := enabled\nFILES := src/a.c src/b.h src/c.c\n",
            "$(if $(filter enabled,$(MODE)),$(strip $(if $(filter %.c,$(FILES)),yes $(filter %.c,$(FILES)),no)),disabled)",
        ),
        Ok("yes src/a.c src/c.c".to_owned())
    );
}

#[test]
fn filesystem_disabled_context_rejects_both_wildcard_forms() {
    let scope = collect_vars("");
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("."),
        Path::new("fixture"),
    )
    .without_filesystem();

    for expression in [
        "$(wildcard this-file-does-not-exist-*.c)",
        "$(call WILDCARD,this-file-does-not-exist-*.c)",
    ] {
        let error = evaluate_make_expr(expression, &context).unwrap_err();
        assert!(matches!(
            &error,
            MakeExprError::FilesystemAccessDisabled { .. }
        ));
        assert!(error.to_string().contains("filesystem access is disabled"));
    }
}

#[test]
fn filesystem_disabled_context_skips_unselected_wildcard_branch() {
    assert_eq!(
        evaluate_without_filesystem(
            "MODE := enabled\nFILES := src/a.c src/b.h\n",
            "$(if $(filter enabled,$(MODE)),$(strip $(filter %.c,$(FILES))),$(wildcard this-file-does-not-exist-*.c))",
        ),
        Ok("src/a.c".to_owned())
    );
}

#[test]
fn nested_prefix_and_suffix_match_compiler_startup() {
    let value = evaluate(
        "NIXFILES := startup crt\n",
        "$(addprefix $(GENDIR)/$(CURDIR)/nix/,$(addsuffix .o,$(NIXFILES)))",
    )
    .unwrap();
    assert_eq!(
        value,
        "${AROS_BUILD_DIR}/gen/fixture/nix/startup.o \
         ${AROS_BUILD_DIR}/gen/fixture/nix/crt.o"
    );
}

#[test]
fn filters_and_both_substitution_reference_forms_are_aligned() {
    let value = evaluate(
        "BASE := source/a.c source/b.cpp source/c.c\nSKIP := source/c\n",
        "$(filter-out $(SKIP),$(BASE:%.c=%)) $(BASE:.cpp=.cc)",
    )
    .unwrap();
    assert_eq!(
        value,
        "source/a source/b.cpp source/a.c source/b.cc source/c.c"
    );
}

#[test]
fn subst_handles_archive_version_spellings() {
    assert_eq!(
        evaluate(
            "VERSION := 2.14.3\n",
            "$(subst .,,$(VERSION)) $(subst .,_,$(VERSION))",
        ),
        Ok("2143 2_14_3".to_owned())
    );
}

#[test]
fn patsubst_filter_and_path_functions_cover_real_shapes() {
    let value = evaluate(
        "FILES := src/a.c src/b.cpp include/c.h archive.tar.gz plain\n",
        "$(patsubst src/%,gen/%,$(filter %.c %.cpp,$(FILES)))",
    )
    .unwrap();
    assert_eq!(value, "gen/a.c gen/b.cpp");
    assert_eq!(
        evaluate("", "$(notdir a/b.c plain tail/)"),
        Ok("b.c plain".to_owned())
    );
    assert_eq!(
        evaluate("", "$(dir a/b.c plain /root.c)"),
        Ok("a/ ./ /".to_owned())
    );
    assert_eq!(
        evaluate("", "$(basename a/b.c plain archive.tar.gz)"),
        Ok("a/b plain archive.tar".to_owned())
    );
    assert_eq!(
        evaluate("", "$(suffix a/b.c plain archive.tar.gz)"),
        Ok(".c .gz".to_owned())
    );
}

#[test]
fn sort_and_strip_use_make_word_semantics() {
    assert_eq!(
        evaluate("LIST := z a z b\n", "$(sort $(strip   $(LIST)   c  ))"),
        Ok("a b c z".to_owned())
    );
}

#[test]
fn computed_variable_names_and_source_order_are_supported() {
    let joined = "ID := 2\nFILES_2 := old.c\nuse\nFILES_2 := new.c\n";
    let scope = collect_vars(joined);
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(&scope, &dirs, 2, Path::new("."), Path::new("fixture"));
    assert_eq!(
        evaluate_make_list("$($(addprefix FILES_,$(ID)))", &context).unwrap(),
        vec!["old.c"]
    );
}

#[test]
fn simple_and_recursive_assignments_observe_different_times() {
    let source = "BASE = old\n\
                  RECURSIVE_BASE = $(BASE)\n\
                  SIMPLE := $(RECURSIVE_BASE)\n\
                  RECURSIVE = $(BASE)\n\
                  BASE = new\n";
    assert_eq!(evaluate(source, "$(SIMPLE)"), Ok("old".to_owned()));
    assert_eq!(evaluate(source, "$(RECURSIVE)"), Ok("new".to_owned()));

    let appended = "BASE = old\n\
                    SIMPLE := first\n\
                    SIMPLE += $(BASE)\n\
                    RECURSIVE = first\n\
                    RECURSIVE += $(BASE)\n\
                    BASE = new\n";
    assert_eq!(
        evaluate(appended, "$(SIMPLE) $(RECURSIVE)"),
        Ok("first old first new".to_owned())
    );
}

#[test]
fn collector_lookup_values_fall_back_to_global_directory_variables() {
    let source = root();
    let scope = collect_vars("");
    let dirs = DirVars::load(&source);
    let lookup =
        |name: &str| (name == "LOCAL_PORT_DIR").then(|| "$(PORTSDIR)/Example/source".to_owned());
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        &source,
        Path::new("external/example"),
    )
    .with_lookup(&lookup);

    assert_eq!(
        evaluate_make_expr("$(LOCAL_PORT_DIR)/file.c", &context).unwrap(),
        "${AROS_PORTS_DIR}/Example/source/file.c"
    );
    assert_eq!(
        evaluate_make_expr("$(TOP)/generated", &context).unwrap(),
        "${AROS_BUILD_DIR}/generated"
    );
}

#[test]
fn a_simple_local_directory_does_not_reshadow_its_global_base() {
    let source = root();
    let scope = collect_vars(
        "TARGETDIR := $(AROS_TESTS)/Library\n\
         CUNITEXEDIR := $(AROS_TESTS)/cunit/genmodule/library\n",
    );
    let dirs = DirVars::load(&source);
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        &source,
        Path::new("developer/debug/test/library"),
    );

    assert_eq!(
        evaluate_make_expr("$(TARGETDIR)", &context).unwrap(),
        "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/Library"
    );
    assert_eq!(
        evaluate_make_expr("$(CUNITEXEDIR)", &context).unwrap(),
        "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/cunit/genmodule/library"
    );
}

#[test]
fn wildcard_is_sorted_and_call_wildcard_keeps_only_regular_files() {
    let tree = TempTree::new();
    let rel = Path::new("locale");
    fs::create_dir_all(tree.0.join(rel).join("directory.po")).unwrap();
    fs::write(tree.0.join(rel).join("z.po"), "").unwrap();
    fs::write(tree.0.join(rel).join("a.po"), "").unwrap();
    let scope = collect_vars("");
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &tree.0, rel);

    let rendered_source = evaluate_make_expr("$(SRCDIR)/$(CURDIR)/a.po", &context).unwrap();
    assert_eq!(rendered_source, "${AROS_SOURCE_DIR}/locale/a.po");
    assert!(!rendered_source.contains(&tree.0.display().to_string()));

    assert_eq!(
        evaluate_make_list("$(wildcard *.po)", &context).unwrap(),
        vec!["a.po", "directory.po", "z.po"]
    );
    assert_eq!(
        evaluate_make_list("$(call WILDCARD,*.po)", &context).unwrap(),
        vec!["a.po", "z.po"]
    );
    let source_matches =
        evaluate_make_list("$(call WILDCARD,$(SRCDIR)/$(CURDIR)/*.po)", &context).unwrap();
    assert_eq!(
        source_matches,
        vec![
            "${AROS_SOURCE_DIR}/locale/a.po",
            "${AROS_SOURCE_DIR}/locale/z.po"
        ]
    );
    assert!(!source_matches
        .join(" ")
        .contains(&tree.0.display().to_string()));
    assert_eq!(
        evaluate_make_list(
            "$(basename $(notdir $(call WILDCARD,$(SRCDIR)/$(CURDIR)/*.po)))",
            &context,
        )
        .unwrap(),
        vec!["a", "z"]
    );
}

#[test]
fn missing_cycles_and_unsupported_syntax_are_never_empty_successes() {
    let missing = evaluate("", "$(DOES_NOT_EXIST)").unwrap_err();
    assert!(matches!(
        missing,
        MakeExprError::UnresolvedVariables { names, .. }
            if names == vec!["DOES_NOT_EXIST"]
    ));

    let cycle = evaluate("A := $(B)\nB := $(A)\n", "$(A)").unwrap_err();
    assert!(matches!(cycle, MakeExprError::VariableCycle { .. }));

    let unsupported = evaluate("", "$(eval SOMETHING := x)").unwrap_err();
    assert!(matches!(
        unsupported,
        MakeExprError::UnsupportedFunction { ref name } if name == "eval"
    ));
    assert!(unsupported
        .to_string()
        .contains("transpiler must be updated"));
    assert!(matches!(
        evaluate("", "$(call SOMETHING,x)"),
        Err(MakeExprError::UnresolvedVariables { names, .. })
            if names == vec!["SOMETHING"]
    ));
    assert!(matches!(
        evaluate("", "$(notdir $@)"),
        Err(MakeExprError::UnsupportedReference { reference }) if reference == "$@"
    ));
    assert!(matches!(
        evaluate("", "$(BROKEN"),
        Err(MakeExprError::InvalidSyntax { .. })
    ));
    assert_eq!(evaluate("A := foo\n", "${A} $$x"), Ok("foo $x".to_owned()));
}

#[test]
fn conditional_variable_guard_wins_over_all_value_lookups() {
    let conditional_scope =
        collect_vars("ifeq ($(ARCH),pc)\nFILES := pc.c\nelse\nFILES := other.c\nendif\n");
    let dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    let conditional_context = MakeExprContext::new(
        &conditional_scope,
        &dirs,
        usize::MAX,
        Path::new("."),
        Path::new("fixture"),
    );
    assert!(matches!(
        evaluate_make_expr("$(FILES)", &conditional_context),
        Err(MakeExprError::UnsafeVariable { name, detail, .. })
            if name == "FILES" && detail.contains("unevaluated Make conditional")
    ));

    let scope = collect_vars("FILES := last-branch.c\n");
    let lookup = |name: &str| (name == "FILES").then(|| "collector.c".to_owned());
    let guard = |name: &str| {
        (name == "FILES").then(|| "assigned in both sides of an undecidable ifeq".to_owned())
    };
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("."),
        Path::new("fixture"),
    )
    .with_lookup(&lookup)
    .with_guard(&guard);

    assert!(matches!(
        evaluate_make_expr("$(FILES)", &context),
        Err(MakeExprError::UnsafeVariable { name, detail, .. })
            if name == "FILES" && detail.contains("undecidable ifeq")
    ));
}

#[test]
fn deferred_cmake_paths_cannot_silently_become_empty_wildcards() {
    assert!(matches!(
        evaluate("FILES := $(wildcard $(GENDIR)/*.c)\n", "$(FILES)"),
        Err(MakeExprError::DeferredWildcard { pattern })
            if pattern == "${AROS_BUILD_DIR}/gen/*.c"
    ));
}

#[test]
fn fetched_port_wildcards_use_physical_files_but_keep_logical_paths() {
    let tree = TempTree::new();
    let components = tree.0.join("acpica/source/components/executer");
    fs::create_dir_all(&components).unwrap();
    fs::write(components.join("second.c"), "").unwrap();
    fs::write(components.join("first.c"), "").unwrap();

    let scope = collect_vars("");
    let mut dirs = DirVars::load(Path::new("/path/which/does/not/exist"));
    dirs.set_materialized_path("AROS_PORTS_DIR", tree.0.clone());
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("."),
        Path::new("fixture"),
    );
    assert_eq!(
        evaluate_make_list(
            "$(wildcard ${AROS_PORTS_DIR}/acpica/source/components/executer/*.c)",
            &context
        )
        .unwrap(),
        vec![
            "${AROS_PORTS_DIR}/acpica/source/components/executer/first.c",
            "${AROS_PORTS_DIR}/acpica/source/components/executer/second.c",
        ]
    );

    assert!(matches!(
        evaluate_make_expr(
            "$(wildcard ${AROS_PORTS_DIR}/missing/components/*.c)",
            &context
        ),
        Err(MakeExprError::DeferredWildcard { pattern })
            if pattern == "${AROS_PORTS_DIR}/missing/components/*.c"
    ));
}

#[test]
fn real_language_module_expression_matches_the_source_tree() {
    let source = root();
    let relative = Path::new("workbench/locale/languages");
    let text = read_source(&source.join(relative).join("mmakefile.src")).unwrap();
    let joined = join_continuations(&text);
    let scope = collect_vars(&joined);
    let dirs = DirVars::load(&source);
    let context = MakeExprContext::new(&scope, &dirs, usize::MAX, &source, relative);

    let languages = evaluate_make_list("$(LANGUAGES)", &context).unwrap();
    assert_eq!(languages.len(), 30);
    assert_eq!(languages.first().map(String::as_str), Some("albanian"));
    assert!(languages.iter().any(|name| name == "portuguese-brazil"));

    let modules = evaluate_make_list("$(MODULES)", &context).unwrap();
    assert_eq!(modules.len(), languages.len());
    assert_eq!(
        modules.first().map(String::as_str),
        Some("${AROS_BUILD_DIR}/SYS/Locale/Languages/albanian.language")
    );
}

#[test]
fn foreach_binds_its_loop_variable_and_shadows_a_global() {
    // rom/dos:42 verbatim: without this, dos.library has no ELF loader.
    assert_eq!(
        evaluate("", "$(foreach img, aos elf, internalloadseg_$(img))").unwrap(),
        " internalloadseg_aos  internalloadseg_elf"
    );
    // The binding is temporary: a global of the same name is shadowed
    // inside the body and intact outside it.
    assert_eq!(
        evaluate("f := global\n", "$(foreach f,one two,classes/$(f)) $(f)").unwrap(),
        "classes/one classes/two global"
    );
    // Nesting, and an empty list yielding nothing.
    assert_eq!(
        evaluate("", "$(foreach a,x y,$(foreach b,1 2,$(a)$(b)))").unwrap(),
        "x1 x2 y1 y2"
    );
    assert_eq!(evaluate("", "[$(foreach a,,body)]").unwrap(), "[]");
}

#[test]
fn foreach_preserves_body_whitespace_and_separates_empty_iterations() {
    assert_eq!(
        evaluate("", "$(foreach i,a b,left  right)"),
        Ok("left  right left  right".to_owned())
    );
    assert_eq!(
        evaluate("", "$(foreach i,a b,  x )"),
        Ok("  x    x ".to_owned())
    );
    assert_eq!(evaluate("", "$(foreach i,a b,)"), Ok(" ".to_owned()));
    assert_eq!(evaluate("", "$(foreach i,,body)"), Ok(String::new()));
}

#[test]
fn remaining_aros_word_and_conditional_functions_match_make_semantics() {
    let source = "LIST := alpha beta gamma\n\
                  OTHER := 1 2\n\
                  mapper = $(addprefix $(1)-,$(2))\n";
    assert_eq!(
        evaluate(
            source,
            "$(findstring et,$(LIST))|$(word 2,$(LIST))|\
             $(wordlist 2,9,$(LIST))|$(words $(LIST))|\
             $(firstword $(LIST))|$(lastword $(LIST))|\
             $(join $(LIST),$(OTHER))",
        )
        .unwrap(),
        "et|beta|beta gamma|3|alpha|gamma|alpha1 beta2 gamma"
    );
    assert_eq!(
        evaluate(source, "$(if ,bad,good) $(or ,first,second) $(and yes,ok)").unwrap(),
        "good first ok"
    );
    assert_eq!(
        evaluate(source, "$(call mapper,item,a b)").unwrap(),
        "item-a item-b"
    );
    assert_eq!(
        evaluate("RAW = literal\n", "$(value RAW)"),
        Ok("literal".to_owned())
    );
}

#[test]
fn bounded_evaluator_preserves_ordinary_nested_word_functions() {
    assert_eq!(
        evaluate(
            "FILES := alpha.c beta.c gamma.h\n",
            "$(foreach file,$(filter %.c,$(FILES)),\
             $(addprefix out/,$(patsubst %.c,%.o,$(file)))) \
             $(subst .,_,$(firstword $(FILES)))",
        ),
        Ok("out/alpha.o out/beta.o alpha_c".to_owned())
    );
}

#[test]
fn rejects_exponential_alias_expansion_before_unbounded_output() {
    let mut source = String::from("A0 = x\n");
    for index in 1..=28 {
        writeln!(source, "A{index} = $(A{})$(A{})", index - 1, index - 1).unwrap();
    }

    assert!(matches!(
        evaluate(&source, "$(A28)"),
        Err(MakeExprError::ResourceLimit {
            resource: "value bytes" | "aggregate emitted bytes" | "work units" | "scanned bytes",
            ..
        })
    ));
}

#[test]
fn rejects_subst_output_amplification_before_replace() {
    let source = format!("TEXT = {}\n", "x".repeat(40_000));
    let replacement = "y".repeat(128);
    let expression = format!("$(subst x,{replacement},$(TEXT))");

    assert!(matches!(
        evaluate(&source, &expression),
        Err(MakeExprError::ResourceLimit {
            resource: "value bytes",
            limit: 4_194_304,
        })
    ));
}

#[test]
fn rejects_foreach_output_at_the_shared_list_item_budget() {
    let source = format!("ITEMS = {}\n", vec!["x"; 65_533].join(" "));

    assert!(matches!(
        evaluate(&source, "$(foreach item,$(ITEMS),$(item))"),
        Err(MakeExprError::ResourceLimit {
            resource: "list items",
            limit: 65_536,
        })
    ));
}

#[test]
fn rejects_quadratic_filter_comparisons_before_matching() {
    let patterns = vec!["p%"; 1_025].join(" ");
    let words = vec!["value"; 1_025].join(" ");
    let source = format!("PATTERNS = {patterns}\nWORDS = {words}\n");

    assert!(matches!(
        evaluate(&source, "$(filter $(PATTERNS),$(WORDS))"),
        Err(MakeExprError::ResourceLimit {
            resource: "work units",
            limit: 1_048_576,
        })
    ));
}

#[test]
fn pure_function_corpus_is_differentially_checked_against_gnu_make() {
    let executable = ["gmake", "make"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("GNU Make")
            })
    });
    let Some(executable) = executable else {
        return;
    };

    let source = "LIST := gamma alpha beta alpha\n\
                  OTHER := 1 2\n\
                  mapper = $(addsuffix -$(1),$(2))\n";
    let expressions = [
        "$(sort $(LIST))",
        "$(patsubst %a,X%,$(LIST))",
        "$(filter %a,$(LIST))",
        "$(filter-out %a,$(LIST))",
        "$(wordlist 2,7,$(LIST))",
        "$(join $(LIST),$(OTHER))",
        "$(if $(findstring beta,$(LIST)),yes,no)",
        "$(or ,,$(firstword $(LIST)))",
        "$(and one,two,$(lastword $(LIST)))",
        "$(call mapper,tag,a b)",
        "$(foreach item,a b,prefix-$(item))",
        "<$(foreach i,a b,left  right)>",
        "<$(foreach i,a b,  x )>",
        "<$(foreach i,a b,)>",
    ];
    let tree = TempTree::new();
    for (index, expression) in expressions.iter().enumerate() {
        let expected = evaluate(source, expression).unwrap();
        let makefile = tree.0.join(format!("oracle-{index}.mk"));
        fs::write(
            &makefile,
            format!("{source}RESULT := {expression}\nall:\n\t@printf '%s\\n' '$(RESULT)'\n"),
        )
        .unwrap();
        let output = Command::new(executable)
            .arg("--no-print-directory")
            .arg("-f")
            .arg(&makefile)
            .current_dir(&tree.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "GNU Make oracle failed for {expression}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            expected,
            String::from_utf8_lossy(&output.stdout).trim_end(),
            "expression: {expression}"
        );
    }
}
