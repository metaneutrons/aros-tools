use crate::{parse_mmakefile_with_context, TargetContext};
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

const WRITER: &str = "#MM\nfixture-text : $(GENDIR)/$(CURDIR)/platform.h\n$(GENDIR)/$(CURDIR)/platform.h : $(SRCDIR)/$(CURDIR)/mmakefile.src\n\t%mkdir_q dir=$(GENDIR)/$(CURDIR)\n\t@$(ECHO) \"#define VALUE $(VALUE)\" > $@\n";

fn fixture(physical: &str, configuration: &str) -> (TempDir, TargetContext) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::create_dir_all(root.path().join("compiler/fixture")).unwrap();
    fs::write(root.path().join("config/make.cfg.in"), "ECHO := echo\n").unwrap();
    fs::write(root.path().join("config/aros.cfg"), "").unwrap();
    fs::write(root.path().join("config/sdk.mk"), configuration).unwrap();
    fs::write(root.path().join("compiler/fixture/mmakefile.src"), physical).unwrap();
    let context = TargetContext {
        cpu: Some("arm".into()),
        platform: Some("fixture".into()),
        family: Some(String::new()),
        variant: Some(String::new()),
        make_include_bindings: BTreeMap::from([("config/aros.cfg".into(), "config/sdk.mk".into())]),
        ..TargetContext::default()
    };
    (root, context)
}

#[test]
fn physical_literal_rule_uses_native_values_and_exact_source_digest_location() {
    let source = format!("include $(SRCDIR)/config/aros.cfg\n{WRITER}");
    let (root, context) = fixture(&source, "VALUE := 41\n");
    let parsed = parse_mmakefile_with_context(
        &root.path().join("compiler/fixture/mmakefile.src"),
        root.path(),
        &context,
    )
    .unwrap();
    let [rule] = parsed.source_literal_text_rules.as_slice() else {
        panic!("{:#?}", parsed.native_graph_errors);
    };
    assert_eq!(rule.text, "#define VALUE 41\n");
    assert_eq!(rule.line, 2);
    assert_eq!(
        rule.file_sha256,
        aros_common::sha256_bytes(source.as_bytes()).as_str()
    );
    assert!(
        parsed.native_graph_errors.iter().all(|error| error
            .context
            .as_ref()
            .and_then(|ctx| ctx.target.as_deref())
            != Some("fixture-text")),
        "{:#?}",
        parsed.native_graph_errors
    );
}

#[test]
fn configuration_cannot_supply_an_executable_literal_owner() {
    let (root, context) = fixture(
        "include $(SRCDIR)/config/aros.cfg\n",
        &format!("VALUE := 41\n{WRITER}"),
    );
    let parsed = parse_mmakefile_with_context(
        &root.path().join("compiler/fixture/mmakefile.src"),
        root.path(),
        &context,
    )
    .unwrap();
    assert!(parsed.source_literal_text_rules.is_empty());
}

#[test]
fn failed_native_scope_cannot_reuse_a_legacy_literal_producer() {
    let source = format!("VALUE := 9\ninclude $(SRCDIR)/config/aros.cfg\n{WRITER}");
    let (root, mut context) = fixture(&source, "VALUE := 41\n");
    context
        .make_include_bindings
        .insert("config/aros.cfg".into(), "config/missing.mk".into());
    let parsed = parse_mmakefile_with_context(
        &root.path().join("compiler/fixture/mmakefile.src"),
        root.path(),
        &context,
    )
    .unwrap();
    assert!(parsed.source_literal_text_rules.is_empty());
    assert!(parsed.native_graph_errors.iter().any(|error| error
        .context
        .as_ref()
        .and_then(|ctx| ctx.target.as_deref())
        == Some("fixture-text")));
}
