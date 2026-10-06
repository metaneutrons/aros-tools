use super::{expand_bytes, expand_files, expand_text, Limits};
use std::fs;
use std::path::{Path, PathBuf};

struct TestTree {
    root: tempfile::TempDir,
}

impl TestTree {
    fn new() -> Self {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("temporary directory resolves without aliases");
        let root = tempfile::Builder::new()
            .prefix("genmf-projection-")
            .tempdir_in(base)
            .expect("create unique temporary tree");
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(&path, bytes).expect("write fixture");
        path
    }
}

#[test]
fn expands_captured_bytes_without_rereading_source_path() {
    let tree = TestTree::new();
    let template = tree.write("root.tmpl", b"%define common\n%end\n");
    let source = tree.write("input.src", b"changed-path-content\n");
    let captured = b"captured-\xA4\n";
    let result = expand_bytes(captured, &source, &template, Limits::default()).unwrap();
    assert!(result.text.contains("captured-\u{20ac}"));
    assert!(!result.text.contains("changed-path-content"));
    let limits = Limits {
        max_source_bytes: captured.len() - 1,
        ..Limits::default()
    };
    assert!(expand_bytes(captured, &source, &template, limits).is_err());
}

#[test]
fn expands_relative_includes_defaults_required_multi_prefix_suffix_and_common() {
    let tree = TestTree::new();
    let nested = "%define nested value=/A\nnested = [%(value)]\n%end\n";
    let root = concat!(
        "%include nested/children.tmpl\n",
        "%define emit required=/A items=/M label=default-value\n",
        "expanded = %(required)\n",
        "words = <%(items)>\n",
        "%nested value=%(required)\n",
        "defaulted = %(label)\n",
        "%end\n",
        "%define common\nCOMMON-LINE\n%end\n",
    );
    let root_path = tree.write("root.tmpl", root.as_bytes());
    let child_path = tree.write("nested/children.tmpl", nested.as_bytes());
    let source = concat!(
        "# %emit required=ignored items=comment\n",
        "%emit required=ready \\\n",
        "    red blue\n",
    );

    let output = expand_text(source, Path::new("input.mm"), &root_path, Limits::default()).unwrap();
    assert_eq!(
        output.text,
        concat!(
            "# %emit required=ignored items=comment\n",
            "expanded = ready\n",
            "words = <red blue>\n",
            "nested = [ready]\n",
            "defaulted = default-value\n",
            "\n",
            "COMMON-LINE\n",
        )
    );
    assert_eq!(output.template_snapshots.len(), 2);
    assert_eq!(
        output.template_snapshots[0].path,
        root_path.canonicalize().unwrap()
    );
    assert_eq!(output.template_snapshots[0].bytes, root.as_bytes());
    assert_eq!(
        output.template_snapshots[1].path,
        child_path.canonicalize().unwrap()
    );
    assert_eq!(output.template_snapshots[1].bytes, nested.as_bytes());
}

#[test]
fn supports_multiline_calls_inline_comment_expansion_and_explicit_common() {
    let tree = TestTree::new();
    let template = concat!(
        "%define decorate value=default/M\n",
        "decorated = (%(value))\n",
        "%end\n",
        "%define common\n",
        "EXPLICIT-COMMON\n",
        "%end\n",
    );
    let template_path = tree.write("root.tmpl", template.as_bytes());
    let source = concat!(
        "before = untouched\n",
        "%decorate \\\n",
        "    multiline\n",
        "after = untouched\n",
        "X = keep # %decorate value=comment\n",
        "%common\n",
        "keep = %not_defined value=x\n",
    );
    let output = expand_text(
        source,
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        output.text,
        concat!(
            "before = untouched\n",
            "decorated = (multiline)\n",
            "after = untouched\n",
            "decorated = (comment)\n",
            "EXPLICIT-COMMON\n",
            "keep = %not_defined value=x\n",
        )
    );
}

#[test]
fn expands_build_module_and_hidd_stubs_fixture() {
    let tree = TestTree::new();
    let template = concat!(
        "%define build_module modname=/A modtype=/A files=/A\n",
        "module=%(modname) type=%(modtype) files=%(files)\n",
        "%end\n",
        "%define make_hidd_stubs hidd=/A\n",
        "hidd-%(hidd)-stubs\n",
        "%end\n",
        "%define common\n",
        "COMMON\n",
        "%end\n",
    );
    let template_path = tree.write("make.tmpl", template.as_bytes());
    let source = concat!(
        "%build_module modname=projection modtype=resource files=sample\n",
        "%make_hidd_stubs hidd=fixture\n",
    );
    let output = expand_text(
        source,
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        output.text,
        "module=projection type=resource files=sample\nhidd-fixture-stubs\n\nCOMMON\n"
    );
    assert!(!output.text.contains("%build_module "));
    assert!(!output.text.contains("%make_hidd_stubs "));
}

#[test]
fn metadata_only_origins_preserve_text_and_physical_line_identity_with_bounded_work() {
    let tree = TestTree::new();
    let template = format!(
        "%define module payload=\n{}#MM- module : child\n%end\n",
        "ordinary-recipe-text\n".repeat(100)
    );
    let template_path = tree.write("config/make.tmpl", template.as_bytes());
    let source = format!(
        "%module payload={}\n#MM- handwritten : child\n",
        "x".repeat(1024)
    );
    let full = expand_text(
        &source,
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap();
    let limits = Limits {
        max_work_bytes: 50_000,
        ..Limits::default()
    };
    assert!(expand_text(&source, Path::new("input.mm"), &template_path, limits).is_err());
    let limited = expand_text(
        &source,
        Path::new("input.mm"),
        &template_path,
        Limits {
            metadata_provenance_only: true,
            ..limits
        },
    )
    .unwrap();
    assert_eq!(limited.text, full.text);
    assert_eq!(limited.provenance.len(), 2);
    assert_eq!(limited.provenance[0].output_line, 101);
    assert_eq!(limited.provenance[1].output_line, 102);
    assert_eq!(limited.provenance[0].macro_stack.len(), 1);
    assert!(limited.provenance[1].macro_stack.is_empty());
}

#[test]
fn records_nested_macro_provenance_without_rewriting_emitted_text() {
    let tree = TestTree::new();
    let template = concat!(
        "%define gen_archspecificrules target= subtarget=\n",
        "#MM- hook-%(target)%(subtarget) : missing-%(target)%(subtarget)\n",
        "%end\n",
        "%define build_module target= subtarget=\n",
        "%gen_archspecificrules target=%(target) subtarget=%(subtarget)\n",
        "%end\n",
        "%define common\n",
        "#MM- common-extension : no-owner\n",
        "%end\n",
    );
    let template_path = tree.write("config/make.tmpl", template.as_bytes());
    let canonical_template = template_path.canonicalize().unwrap();
    let generated = "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n";
    let source = concat!(
        "%build_module target=-set-archincludes \\\n",
        "    subtarget=-variant\n",
        "%build_module target=-set-archincludes subtarget=-variant\n",
        "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
    );
    let source_path = Path::new("input.mm");
    let output = expand_text(source, source_path, &template_path, Limits::default()).unwrap();
    let expected = concat!(
        "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
        "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
        "#MM- hook--set-archincludes-variant : missing--set-archincludes-variant\n",
        "\n",
        "#MM- common-extension : no-owner\n",
    );
    assert_eq!(output.text, expected);

    let matching = output
        .provenance
        .iter()
        .filter(|span| {
            output
                .text
                .get(span.output_start_byte..span.output_end_byte)
                == Some(generated)
        })
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 3);
    for (span, invocation_line) in matching.iter().take(2).zip([1, 3]) {
        assert_eq!(
            span.template_path.as_deref(),
            Some(canonical_template.as_path())
        );
        assert_eq!(span.template_definition_line, Some(1));
        assert_eq!(span.source_line, Some(2));
        assert_eq!(span.macro_stack.len(), 2);
        assert_eq!(span.macro_stack[0].name, "build_module");
        assert_eq!(span.macro_stack[1].name, "gen_archspecificrules");
        assert_eq!(span.macro_stack[1].definition_line, 1);
        assert_eq!(span.macro_stack[1].template_path, canonical_template);
        assert_eq!(
            span.top_level_source_invocation
                .as_ref()
                .unwrap()
                .path
                .as_path(),
            source_path
        );
        assert_eq!(
            span.top_level_source_invocation.as_ref().unwrap().line,
            invocation_line
        );
        assert_eq!(
            span.macro_stack[0].resolved_arguments["target"],
            "-set-archincludes"
        );
        assert_eq!(
            span.macro_stack[1].resolved_arguments["target"],
            "-set-archincludes"
        );
        assert_eq!(
            span.macro_stack[1].resolved_arguments["subtarget"],
            "-variant"
        );
    }
    assert_eq!(matching[0].output_line, 1);
    assert_eq!(matching[1].output_line, 2);

    // Same emitted #MM bytes, but this line is a handwritten source claim,
    // not output from a template hook.
    let handwritten = matching[2];
    assert_eq!(handwritten.output_line, 3);
    assert_eq!(handwritten.source_path.as_deref(), Some(source_path));
    assert_eq!(handwritten.source_line, Some(4));
    assert!(handwritten.template_path.is_none());
    assert!(handwritten.template_definition_line.is_none());
    assert!(handwritten.macro_stack.is_empty());
    assert!(handwritten.top_level_source_invocation.is_none());

    let common = output
        .provenance
        .iter()
        .find(|span| {
            output
                .text
                .get(span.output_start_byte..span.output_end_byte)
                == Some("#MM- common-extension : no-owner\n")
        })
        .unwrap();
    assert_eq!(
        common.template_path.as_deref(),
        Some(canonical_template.as_path())
    );
    assert_eq!(common.template_definition_line, Some(7));
    assert_eq!(common.source_line, Some(8));
    assert_eq!(common.macro_stack.len(), 1);
    assert_eq!(common.macro_stack[0].name, "common");
    assert!(common.macro_stack[0].invocation.is_none());
    assert!(common.top_level_source_invocation.is_none());
}

#[test]
fn discovers_nested_invocations_before_argument_substitution() {
    let tree = TestTree::new();
    let template = concat!(
        "%define inner\n",
        "#MM injected\n",
        "injected :\n",
        "%end\n",
        "%define outer value=/A\n",
        "%(value)\n",
        "%end\n",
        "%define common\n",
        "#MM clean\n",
        "clean :\n",
        "%end\n",
    );
    let template_path = tree.write("root.tmpl", template.as_bytes());
    let output = expand_text(
        "%outer value=\"%inner\"\n",
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "%inner\n\n#MM clean\nclean :\n");
}

#[test]
fn uses_original_reference_character_offsets_after_substitution() {
    let tree = TestTree::new();
    let template = concat!(
        "%define inner arg=/M\n",
        "inner=[%(arg)]\n",
        "%end\n",
        "%define outer value=/A\n",
        "%(value)%inner arg=hello\n",
        "%end\n",
        "%define common\n",
        "#MM clean\n",
        "clean :\n",
        "%end\n",
    );
    let template_path = tree.write("root.tmpl", template.as_bytes());
    let output = expand_text(
        "%outer value=\"x\"\n",
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "inner=[llo]\n\n#MM clean\nclean :\n");
}

#[test]
fn continuation_consumption_does_not_erase_precomputed_physical_calls() {
    let tree = TestTree::new();
    let template = tree.write(
        "root.tmpl",
        b"%define outer rest=/M\nOUTER\n%end\n%define inner\nINNER\n%end\n",
    );
    let output = expand_text(
        "%outer ignored \\\n%inner\n",
        Path::new("input.mm"),
        &template,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "OUTER\nINNER\n\n");
}

#[test]
fn bare_cr_source_lines_match_the_classic_reference_without_argument_merging() {
    let tree = TestTree::new();
    let template = tree.write("root.tmpl", b"%define common args=/M\nSEEN=%(args)\n%end\n");
    let output = expand_text(
        "%common\rX\r",
        Path::new("input.mm"),
        &template,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "SEEN=\nX\n");
}

#[test]
fn rejects_missing_required_arguments_and_recursive_templates() {
    let tree = TestTree::new();
    let template = concat!(
        "%define required value=/A\n",
        "%(value)\n",
        "%end\n",
        "%define recurse\n",
        "%recurse\n",
        "%end\n",
    );
    let template_path = tree.write("root.tmpl", template.as_bytes());

    let missing = expand_text(
        "%required\n",
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap_err();
    assert!(missing.detail.contains("required argument"));
    let recursive = expand_text(
        "%recurse\n",
        Path::new("input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap_err();
    assert!(recursive.detail.contains("recursively"));
}

#[test]
fn matches_python_whitespace_and_latin9_argument_boundaries() {
    let tree = TestTree::new();
    let mut template = b"%define common\nCOMMON\n%end\n%define probe value=".to_vec();
    template.push(0xe9);
    template.extend_from_slice(
        b"\n%(value)\n%end\n%define quoted value=\"literal/A\"\n%(value)\n%end\n%define words value=\"default\"\n<%(value)>\n%end\n",
    );
    let template_path = tree.write("root.tmpl", &template);

    for separator in ['\u{00a0}', '\u{001c}'] {
        let source = format!("%common{separator}\n");
        let output = expand_text(
            &source,
            Path::new("unicode-input.mm"),
            &template_path,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(output.text, "COMMON\n");
    }

    let source = "%probe\n%quoted\n%words value=\"red blue\"\n";
    let output = expand_text(
        source,
        Path::new("unicode-input.mm"),
        &template_path,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "é\nliteral/A\n<red blue>\n\nCOMMON\n");
}

#[test]
fn normalizes_crlf_and_cr_in_source_and_template_text() {
    let tree = TestTree::new();
    let template = tree.write("root.tmpl", b"%define emit\r\nA\r\nB\rC\n%end\r\n");
    let output = expand_text(
        "%emit\r\n",
        Path::new("newline-input.mm"),
        &template,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "A\nB\nC\n\n");
}

#[cfg(unix)]
#[test]
fn refuses_symlink_import_aliases_and_source_paths() {
    use std::os::unix::fs::symlink;

    let tree = TestTree::new();
    let child = tree.write("child.tmpl", b"%define common\ncommon\n%end\n");
    symlink(&child, tree.path("alias.tmpl")).unwrap();
    let root = tree.write("root.tmpl", b"%include child.tmpl\n%include alias.tmpl\n");
    let alias_error =
        expand_text("source\n", Path::new("input.mm"), &root, Limits::default()).unwrap_err();
    assert!(alias_error.detail.contains("symbolic link"));

    let source = tree.write("source.mm", b"source\n");
    symlink(&source, tree.path("source-link.mm")).unwrap();
    let source_error =
        expand_files(&tree.path("source-link.mm"), &root, Limits::default()).unwrap_err();
    assert!(source_error.detail.contains("symbolic link"));
}

#[test]
fn enforces_output_and_template_root_budgets_and_regular_files() {
    let tree = TestTree::new();
    fs::create_dir_all(tree.path("templates")).unwrap();
    fs::create_dir_all(tree.path("outside")).unwrap();
    let repeated = tree.write(
        "templates/root.tmpl",
        b"%define repeat value=/M\n%(value)%(value)%(value)\n%end\n",
    );
    let limits = Limits {
        max_output_bytes: 8,
        ..Limits::default()
    };
    let result = expand_text(
        "%repeat abcdefghijklmnop\n",
        Path::new("input.mm"),
        &repeated,
        limits,
    );
    assert!(result.unwrap_err().detail.contains("budget"));

    let oversized_source = tree.write("oversized.mm", b"12345");
    let source_limit = Limits {
        max_source_bytes: 4,
        ..Limits::default()
    };
    let result = expand_files(&oversized_source, &repeated, source_limit);
    assert!(result.unwrap_err().detail.contains("byte budget"));

    let small_source = tree.write("small.mm", b"source\n");
    let template_limit = Limits {
        max_template_bytes: 4,
        ..Limits::default()
    };
    let result = expand_files(&small_source, &repeated, template_limit);
    assert!(result.unwrap_err().detail.contains("byte budget"));

    tree.write("outside/child.tmpl", b"%define child\nok\n%end\n");
    let escape = tree.write("templates/root.tmpl", b"%include ../outside/child.tmpl\n");
    let result = expand_text(
        "source\n",
        Path::new("input.mm"),
        &escape,
        Limits::default(),
    );
    assert!(result
        .unwrap_err()
        .detail
        .contains("outside the template root"));

    fs::create_dir(tree.path("templates/directory.tmpl")).unwrap();
    let directory = tree.write("templates/root.tmpl", b"%include directory.tmpl\n");
    let result = expand_text(
        "source\n",
        Path::new("input.mm"),
        &directory,
        Limits::default(),
    );
    assert!(result.unwrap_err().detail.contains("regular file"));
}

#[test]
fn keeps_unknown_reference_text_and_adds_implicit_common() {
    let tree = TestTree::new();
    let template = tree.write("root.tmpl", b"%define common\nCOMMON\n%end\n");
    let output = expand_text(
        "ordinary\n%unknown value=x\n",
        Path::new("input.mm"),
        &template,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(output.text, "ordinary\n%unknown value=x\n\nCOMMON\n");
}
