use super::{expand_bytes, expand_files, expand_text, Limits};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn native_genmf_encoding_round_trips_all_bytes_without_lossy_fallback() {
    let original: Vec<_> = (0..=u8::MAX).collect();
    let mut expanded = super::ExpandedText {
        text: super::decode_iso_8859_15(&original),
        template_snapshots: Vec::new(),
        provenance: Vec::new(),
    };
    assert_eq!(expanded.native_bytes().unwrap(), original);
    for character in [
        '\u{a4}',
        '\u{a6}',
        '\u{a8}',
        '\u{b4}',
        '\u{b8}',
        '\u{bc}',
        '\u{bd}',
        '\u{be}',
        '\u{1f600}',
    ] {
        expanded.text = character.to_string();
        assert!(expanded.native_bytes().is_err());
    }
    // The native tool copies these bytes; it does not decode this UTF-8 dash.
    let utf8_bytes = "a — b\n".as_bytes();
    expanded.text = super::decode_iso_8859_15(utf8_bytes);
    assert_ne!(expanded.text.as_bytes(), utf8_bytes);
    assert_eq!(expanded.native_bytes().unwrap(), utf8_bytes);
}

#[test]
#[ignore = "requires selected real Mesa26 Source and retained native generated Makefile roots"]
fn actual_mesa26_entry_program_matches_retained_native_genmf_bytes() {
    use aros_common::sha256_bytes;
    let source_root = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA26_SOURCE_ROOT").expect("select actual Mesa26 Source"),
    );
    let generated_root = PathBuf::from(
        std::env::var_os("AROS_TEST_GENERATED_MMAKE_ROOT").expect("select retained generated tree"),
    );
    let owner = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/mmakefile";
    let source = source_root.join(format!("{owner}.src"));
    let template = source_root.join("config/make.tmpl");
    let native = generated_root.join(owner);
    let before = [&source, &template, &native]
        .map(|path| sha256_bytes(&fs::read(path).expect("read selected original input")));
    let expanded = expand_files(&source, &template, Limits::default())
        .expect("expand real selected Source without execution");
    let expected_bytes = expanded.native_bytes().expect("emit native GenMF encoding");
    let expected = expected_bytes.as_slice();
    let actual = fs::read(&native).expect("read retained native GenMF output");
    for snapshot in &expanded.template_snapshots {
        assert_eq!(
            fs::read(&snapshot.path).unwrap(),
            snapshot.bytes,
            "template input changed during expansion"
        );
    }
    assert_eq!(
        [&source, &template, &native]
            .map(|path| sha256_bytes(&fs::read(path).expect("recheck selected original input"))),
        before
    );
    if let Some(directory) = std::env::var_os("AROS_TEST_GENMF_PARITY_EVIDENCE") {
        let directory = PathBuf::from(directory);
        fs::create_dir(&directory).expect("create fresh private parity evidence directory");
        fs::write(directory.join("projected-mmakefile"), expected).unwrap();
        fs::write(directory.join("native-mmakefile"), &actual).unwrap();
        let metadata = serde_json::json!({
            "source": source, "template": template, "native": native,
            "before_sha256": before,
            "projected_sha256": sha256_bytes(expected),
            "native_sha256": sha256_bytes(&actual),
            "projected_bytes": expected.len(), "native_bytes": actual.len(),
            "template_inputs": expanded.template_snapshots.iter().map(|input| {
                serde_json::json!({"path":input.path,"sha256":sha256_bytes(&input.bytes)})
            }).collect::<Vec<_>>()
        });
        fs::write(
            directory.join("comparison.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(
        sha256_bytes(&actual),
        sha256_bytes(expected),
        "retained native bytes differ from selected Source expansion"
    );
    assert_eq!(actual, expected);
    assert!(!actual.is_empty());
    let counter_tree = TestTree::new();
    let mut changed = actual.clone();
    changed[0] ^= 1;
    let counter = counter_tree.write("changed-mmakefile", &changed);
    assert_ne!(
        sha256_bytes(&fs::read(counter).unwrap()),
        sha256_bytes(expected)
    );
    assert_eq!(
        [&source, &template, &native]
            .map(|path| sha256_bytes(&fs::read(path).expect("recheck selected original input"))),
        before
    );
    eprintln!(
        "retained native entry program: {} bytes, SHA256 {}, original inputs unchanged",
        actual.len(),
        sha256_bytes(&actual)
    );
}

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
fn assembled_input_maps_physical_lines_and_nested_invocations_without_changing_text() {
    use super::{expand_positioned_text, SourceLineLocation};
    let tree = TestTree::new();
    let template = tree.write(
        "root.tmpl",
        b"%define inner\nINNER\n%end\n%define outer\n%inner\n%end\n%define common\n%end\n",
    );
    let assembly = tree.path("assembly");
    let fragment = tree.path("included.cfg");
    let source = "SYNTHETIC\r\nVALUE = present\r\n%outer\r\n";
    let locations = vec![
        None,
        Some(SourceLineLocation {
            path: fragment.clone(),
            line: 17,
        }),
        Some(SourceLineLocation {
            path: fragment.clone(),
            line: 23,
        }),
    ];
    let plain = expand_text(source, &assembly, &template, Limits::default()).unwrap();
    let mapped =
        expand_positioned_text(source, &assembly, &locations, &template, Limits::default())
            .unwrap();
    assert_eq!(plain.text, mapped.text);
    assert_eq!(plain.template_snapshots, mapped.template_snapshots);
    let synthetic = mapped
        .provenance
        .iter()
        .find(|span| &mapped.text[span.output_start_byte..span.output_end_byte] == "SYNTHETIC\n")
        .unwrap();
    assert_eq!(
        (synthetic.source_path.as_ref(), synthetic.source_line),
        (None, None)
    );
    let value = mapped
        .provenance
        .iter()
        .find(|span| {
            &mapped.text[span.output_start_byte..span.output_end_byte] == "VALUE = present\n"
        })
        .unwrap();
    assert_eq!(
        (value.source_path.as_ref(), value.source_line),
        (Some(&fragment), Some(17))
    );
    let inner = mapped
        .provenance
        .iter()
        .find(|span| &mapped.text[span.output_start_byte..span.output_end_byte] == "INNER\n")
        .unwrap();
    assert_eq!(inner.source_path.as_ref(), Some(&template));
    assert_eq!(inner.source_line, Some(2));
    assert_eq!(inner.top_level_source_invocation, locations[2]);
    assert_eq!(inner.macro_stack[0].invocation, locations[2]);
    assert_eq!(
        inner.macro_stack[1].invocation,
        Some(SourceLineLocation {
            path: template.clone(),
            line: 5
        })
    );
    let synthetic_calls = vec![None; 3];
    let mapped = expand_positioned_text(
        source,
        &assembly,
        &synthetic_calls,
        &template,
        Limits::default(),
    )
    .unwrap();
    assert!(mapped
        .provenance
        .iter()
        .all(|span| span.top_level_source_invocation.is_none()));
    assert!(mapped
        .provenance
        .iter()
        .flat_map(|span| &span.macro_stack)
        .filter(|frame| frame.name == "outer")
        .all(|frame| frame.invocation.is_none()));
}

#[test]
fn assembled_input_refuses_invalid_maps_collisions_and_coordinate_amplification() {
    use super::{expand_positioned_text, SourceLineLocation};
    let tree = TestTree::new();
    let template = tree.write("root.tmpl", b"%define common\n%end\n");
    let assembly = tree.path("assembly");
    let valid = vec![Some(SourceLineLocation {
        path: tree.path("input"),
        line: 2,
    })];
    assert!(
        expand_positioned_text("VALUE\n", &assembly, &valid, &template, Limits::default()).is_ok()
    );
    assert!(
        expand_positioned_text("VALUE\n", &assembly, &[], &template, Limits::default())
            .unwrap_err()
            .detail
            .contains("origin map")
    );
    let mut invalid = valid.clone();
    invalid[0].as_mut().unwrap().line = 0;
    assert!(
        expand_positioned_text("VALUE\n", &assembly, &invalid, &template, Limits::default())
            .is_err()
    );
    invalid[0].as_mut().unwrap().line = 1;
    invalid[0].as_mut().unwrap().path = PathBuf::new();
    assert!(
        expand_positioned_text("VALUE\n", &assembly, &invalid, &template, Limits::default())
            .is_err()
    );
    assert!(
        expand_positioned_text("VALUE\n", &template, &valid, &template, Limits::default())
            .unwrap_err()
            .detail
            .contains("collides")
    );
    let limits = Limits {
        max_work_bytes: 8,
        ..Limits::default()
    };
    assert!(
        expand_positioned_text("VALUE\n", &assembly, &valid, &template, limits)
            .unwrap_err()
            .detail
            .contains("budget")
    );
    // The extra map and every copied physical coordinate share the expansion's
    // work budget, rather than obtaining a fresh budget after expansion.
    let mut low = 0;
    let mut high = Limits::default().max_work_bytes;
    while low < high {
        let middle = low + (high - low) / 2;
        if expand_text(
            "VALUE\n",
            &assembly,
            &template,
            Limits {
                max_work_bytes: middle,
                ..Limits::default()
            },
        )
        .is_ok()
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    assert!(expand_positioned_text(
        "VALUE\n",
        &assembly,
        &valid,
        &template,
        Limits {
            max_work_bytes: low,
            ..Limits::default()
        }
    )
    .unwrap_err()
    .detail
    .contains("budget"));
    let path_bytes = valid[0].as_ref().unwrap().path.to_string_lossy().len();
    let map_cost = std::mem::size_of::<Option<SourceLineLocation>>() + path_bytes;
    let copy_cost = std::mem::size_of::<SourceLineLocation>() + path_bytes;
    let run = |budget| {
        expand_positioned_text(
            "VALUE\n",
            &assembly,
            &valid,
            &template,
            Limits {
                max_work_bytes: budget,
                ..Limits::default()
            },
        )
    };
    assert!(run(low + map_cost).unwrap_err().detail.contains("budget"));
    assert!(run(low + map_cost + copy_cost - 1)
        .unwrap_err()
        .detail
        .contains("budget"));
    assert!(run(low + map_cost + copy_cost).is_ok());
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
