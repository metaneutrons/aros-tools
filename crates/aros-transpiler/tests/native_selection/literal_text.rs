use super::*;

#[test]
fn unsupported_writer_with_extra_prerequisite_is_not_silently_omitted() {
    let mut fixture = fixture();
    let file = fixture.root.path().join("literal/mmakefile.src");
    let source = fs::read_to_string(&file)
        .unwrap()
        .replace(
            ": $(SRCDIR)/$(CURDIR)/mmakefile.src",
            ": $(SRCDIR)/$(CURDIR)/mmakefile.src dep.in",
        )
        .replace(
            "@$(ECHO) \"#define VALUE $(VALUE)\" > $@",
            "@printf unsafe > $@",
        );
    fs::write(&file, source).unwrap();
    rebind_native_fixture_input(&mut fixture, "literal/mmakefile.src");
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "exactly one source prerequisite",
        "capability_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

fn fixture() -> Fixture {
    let mut fixture = Fixture::new();
    let directory = fixture.root.path().join("literal");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("mmakefile.src"),
        "ECHO := echo\nVALUE := 17\n#MM\nfixture-literal : $(GENDIR)/$(CURDIR)/platform.h\n$(GENDIR)/$(CURDIR)/platform.h : $(SRCDIR)/$(CURDIR)/mmakefile.src\n\t%mkdir_q dir=$(GENDIR)/$(CURDIR)\n\t@$(ECHO) \"#define VALUE $(VALUE)\" > $@\n").unwrap();
    fixture.append("#MM- fixture-kernel : fixture-literal\n");
    install_native_metamake_projection(&mut fixture, &["mmakefile.src", "literal/mmakefile.src"]);
    fixture
}

#[test]
fn physical_literal_text_provider_survives_strict_selection_and_graph_emission() {
    let fixture = fixture();
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let graph = fs::read_to_string(fixture.output()).unwrap();
    assert!(graph.contains("aros_write_source_literal_text("));
    assert!(graph.contains("NAME \"fixture-literal\""));
    assert!(graph.contains("literal/mmakefile.src"));
    assert!(graph.contains("#define VALUE 17"));
    let receipt: Value = serde_json::from_slice(
        &fs::read(fixture.output().with_extension("native-invocation.json")).unwrap(),
    )
    .unwrap();
    assert!(receipt["native_owner_projection"]["selected_source_files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "literal/mmakefile.src"));
}

#[test]
fn unsupported_literal_command_cannot_become_a_selected_empty_provider() {
    let fixture = fixture();
    let file = fixture.root.path().join("literal/mmakefile.src");
    let source = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        source.replace(
            "@$(ECHO) \"#define VALUE $(VALUE)\" > $@",
            "@printf unsafe > $@",
        ),
    )
    .unwrap();
    let mut fixture = fixture;
    rebind_native_fixture_input(&mut fixture, "literal/mmakefile.src");
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "literal text recipe",
        "capability_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn selected_literal_owner_cannot_shadow_a_concrete_library_provider() {
    let mut fixture = fixture();
    fixture.append("%build_linklib mmake=fixture-literal libname=shadow files=probe\n");
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "conflicting concrete producers",
        "graph_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}
