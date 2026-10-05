//! Process-boundary probes for source-bound native graph selection.
//! These synthetic graphs verify selection, not a P4 kernel or hardware boot.

use aros_transpiler::parser::TargetContext;
use serde_json::{json, Value};
use std::{
    fmt::Write as _,
    fs,
    path::Path,
    process::{Command, Output},
};

fn bind_optional_meta_edge(fixture: &mut Fixture, recipe: &str) {
    let bytes = fs::read(fixture.root.path().join(recipe)).unwrap();
    fixture.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": recipe, "sha256": aros_common::sha256_bytes(&bytes),
        }));
    fixture.contract["optional_meta_dependencies"] = json!([{
        "recipe": recipe,
        "target": "optional-aggregate",
        "dependency": "optional-headers-${AROS_TARGET_PLATFORM}-${AROS_TARGET_VARIANT}",
    }]);
    fixture.write_contract();
}

fn rewrite_fixture_exec(fixture: &Fixture, declaration: &str) {
    let recipe = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&recipe).unwrap();
    let updated = original.replace(
        "%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe",
        declaration,
    );
    assert_ne!(
        updated, original,
        "fixture exec declaration was not replaced"
    );
    fs::write(recipe, updated).unwrap();
}

fn bind_fixture_input(fixture: &mut Fixture, path: &str) {
    let bytes = fs::read(fixture.root.path().join(path)).unwrap();
    fixture.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": path,
            "sha256": aros_common::sha256_bytes(&bytes),
        }));
}

fn fixture_native_selector_context() -> TargetContext {
    TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("fixture".into()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("gnu".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("0".into()),
        float_abi: Some("ilp32f".into()),
        ..TargetContext::default()
    }
}

fn fixture_native_selector_globals(context: &TargetContext) -> String {
    const SELECTORS: &[&str] = &[
        "AROS_TARGET_CPU",
        "CPU",
        "AROS_TARGET_ARCH",
        "ARCH",
        "AROS_TARGET_PLATFORM",
        "FAMILY",
        "AROS_TARGET_VARIANT",
        "AROS_TOOLCHAIN",
        "AROS_TARGET_CPU32",
    ];
    SELECTORS.iter().fold(String::new(), |mut globals, name| {
        writeln!(
            globals,
            "{name} := {}",
            context
                .value_of(name)
                .expect("fixture defines every required native selector")
        )
        .unwrap();
        globals
    })
}

#[test]
fn native_metamake_projection_prefers_src_and_keeps_direct_fragment_at_process_boundary() {
    let mut fixture = Fixture::new();
    let selector_context = fixture_native_selector_context();
    let globals = fixture_native_selector_globals(&selector_context);
    let source = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&source).unwrap();
    fs::write(
        &source,
        format!("{original}#MM- fixture-kernel : source-edge\n"),
    )
    .unwrap();

    fs::create_dir(fixture.root.path().join("direct")).unwrap();
    fs::write(
        fixture.root.path().join("direct/mmakefile"),
        "#MM fixture-kernel : direct-edge\n",
    )
    .unwrap();
    fs::write(
        fixture.root.path().join("mmakefile"),
        "#MM- fixture-kernel : stale-extraedge\n",
    )
    .unwrap();

    fs::create_dir(fixture.root.path().join("meta")).unwrap();
    fs::write(
        fixture.root.path().join("meta/project.conf"),
        "[fixture]\ndefaultmakefilename mmakefile\nglobalvarfile native.globals\n",
    )
    .unwrap();
    fs::write(
        fixture.root.path().join("meta/root.tmpl"),
        concat!(
            "%define build_module_simple mmake=/A modname=/A modtype=/A files=/A\n",
            "#MM %(mmake) :\n",
            "%end\n",
            "%define build_linklib mmake=/A libname=/A files=/A\n",
            "#MM %(mmake) :\n",
            "%end\n",
            "%define make_package mmake=/A file=/A res=/A libs=/A devs=/A\n",
            "#MM %(mmake) :\n",
            "%end\n",
        ),
    )
    .unwrap();
    fs::write(fixture.root.path().join("meta/globals.snapshot"), &globals).unwrap();

    let host = aros_common::target::native_host_key().unwrap_or("");
    let policy = json!({
        "schema_version": 1,
        "kind": "native-metamake-policy-v1",
        "profile": "fixture-native",
        "project": "fixture",
        "configuration_source": "meta/project.conf",
        "template": "meta/root.tmpl",
        "substitutions": [],
        "global_snapshots": [{
            "source": "meta/globals.snapshot",
            "configured_path": "native.globals",
            "text": globals,
        }],
        "environment": [],
        "host_environment": [{"host": host, "bindings": []}],
        "declared_absent": [],
        "evidence_sources": ["meta/globals.snapshot"],
        "closed_environment": true,
        "out_of_source": true,
    });
    fs::write(
        fixture.root.path().join("meta/policy.json"),
        serde_json::to_vec_pretty(&policy).unwrap(),
    )
    .unwrap();

    fixture.contract["metamake_projection"] = json!("meta/policy.json");
    for path in [
        "mmakefile.src",
        "direct/mmakefile",
        "meta/project.conf",
        "meta/root.tmpl",
        "meta/globals.snapshot",
        "meta/policy.json",
    ] {
        bind_fixture_input(&mut fixture, path);
    }
    fixture.write_contract();

    let report_path = fixture.root.path().join("metamake-audit.json");
    let result = fixture.invoke(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            report_path.to_str().unwrap(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    let source_owners = report["native_owner_projection"]["selected_source_files"]
        .as_array()
        .unwrap();
    for path in ["mmakefile.src", "direct/mmakefile"] {
        assert!(
            source_owners
                .iter()
                .any(|entry| entry.as_str() == Some(path)),
            "{report}"
        );
    }
    assert!(!source_owners
        .iter()
        .any(|entry| entry.as_str() == Some("mmakefile")));

    let projected_missing = report["native_owner_projection"]["missing_endpoints"]
        .as_array()
        .unwrap();
    for name in ["source-edge", "direct-edge"] {
        assert!(
            projected_missing
                .iter()
                .any(|entry| entry.as_str() == Some(name)),
            "{report}"
        );
    }
    assert!(!projected_missing
        .iter()
        .any(|entry| entry.as_str() == Some("stale-extraedge")));

    let audit_missing = report["audit"]["missing_endpoints"].as_array().unwrap();
    assert!(!audit_missing
        .iter()
        .any(|entry| entry["name"] == "stale-extraedge"));
    assert!(!report.to_string().contains("stale-extraedge"));
    assert!(!fixture.output().exists());
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn diagnostic_graph_audit_collects_siblings_without_publishing_or_qualifying() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : audit-missing-a audit-missing-b\n");
    let path = fixture.root.path().join("graph-audit.json");
    let result = fixture.invoke(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            path.to_str().unwrap(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        report["audit"]["qualification"],
        "diagnostic-only-not-build-proof"
    );
    let missing = report["audit"]["missing_endpoints"].as_array().unwrap();
    for name in ["audit-missing-a", "audit-missing-b"] {
        assert!(
            missing.iter().any(|entry| entry["name"] == name),
            "{report}"
        );
    }
    assert!(report["audit"]["strict_validation_error"].is_string());
    assert!(!fixture.output().exists());
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
    let strict = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(!strict.status.success());
    assert!(!fixture.output().exists());
}

#[test]
fn diagnostic_graph_audit_cannot_replace_the_graph_or_coverage_sidecar() {
    let fixture = Fixture::new();
    let graph = fixture.root.path().join("protected.json");
    let coverage = graph.with_extension("coverage.json");
    for path in [&graph, &coverage] {
        fs::write(path, "existing artifact\n").unwrap();
        let result = fixture.invoke_at(
            true,
            &[
                "--source-inventory-only",
                "--native-graph-audit",
                path.to_str().unwrap(),
            ],
            &graph,
        );
        assert!(!result.status.success());
        assert_eq!(fs::read_to_string(path).unwrap(), "existing artifact\n");
    }
}

#[cfg(unix)]
#[test]
fn diagnostic_graph_audit_rejects_symlink_aliases_of_protected_outputs() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let report = fixture.root.path().join("audit.json");
    fs::write(&report, "existing artifact\n").unwrap();
    let graph = fixture.root.path().join("generated.cmake");
    symlink(&report, &graph).unwrap();
    let result = fixture.invoke_at(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            report.to_str().unwrap(),
        ],
        &graph,
    );
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(&report).unwrap(), "existing artifact\n");
    assert!(fs::symlink_metadata(&graph)
        .unwrap()
        .file_type()
        .is_symlink());

    let alias = fixture.root.path().join("alias");
    symlink(fixture.root.path(), &alias).unwrap();
    let graph = fixture.root.path().join("new.json");
    let report = alias.join("new.json");
    let result = fixture.invoke_at(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            report.to_str().unwrap(),
        ],
        &graph,
    );
    assert!(!result.status.success());
    assert!(!graph.exists());

    // `..` applies after resolving a directory symlink, not before it.
    fs::create_dir_all(fixture.root.path().join("real/child")).unwrap();
    let alias = fixture.root.path().join("child-alias");
    symlink(fixture.root.path().join("real/child"), &alias).unwrap();
    let report = fixture.root.path().join("real/another.json");
    let graph = alias.join("../another.json");
    let result = fixture.invoke_at(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            report.to_str().unwrap(),
        ],
        &graph,
    );
    assert!(!result.status.success());
    assert!(!report.exists());
}

#[test]
fn diagnostic_graph_audit_records_root_failure_without_inventing_roots() {
    let fixture = Fixture::new();
    let recipe = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&recipe).unwrap();
    fs::write(
        recipe,
        original.replace("devs=timer", "devs=missing-device"),
    )
    .unwrap();
    let report = fixture.root.path().join("audit.json");
    let result = fixture.invoke(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            report.to_str().unwrap(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert!(
        report["root_resolution_error"]
            .as_str()
            .unwrap()
            .contains("unresolved members"),
        "{report}"
    );
    assert_eq!(report["audit"]["roots"], json!([]));
    assert!(report["audit"]["strict_validation_error"].is_string());
    assert!(!fixture.output().exists());
    assert!(!fixture
        .invoke(true, &["--source-inventory-only"])
        .status
        .success());
}

#[test]
fn diagnostic_graph_audit_respects_only_bound_source_literal_ignoredirs() {
    let mut fixture = Fixture::new();
    let config = "[AROS]\nignoredir .unmaintained\nignoredir active@UNKNOWN@\n";
    fs::write(fixture.root.path().join("mmake.config.in"), config).unwrap();
    for (directory, dependency) in [
        ("arch/.unmaintained/nested", "hidden-missing"),
        ("active", "live-missing"),
    ] {
        fs::create_dir_all(fixture.root.path().join(directory)).unwrap();
        fs::write(
            fixture.root.path().join(directory).join("mmakefile.src"),
            format!("#MM- fixture-kernel : {dependency}\n"),
        )
        .unwrap();
    }
    let report = fixture.root.path().join("audit.json");
    let invoke = |fixture: &Fixture| {
        fixture.invoke(
            true,
            &[
                "--source-inventory-only",
                "--native-graph-audit",
                report.to_str().unwrap(),
            ],
        )
    };
    assert!(
        !invoke(&fixture).status.success(),
        "unbound discovery policy was accepted"
    );
    assert!(!report.exists());
    fixture.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": "mmake.config.in", "sha256": aros_common::sha256_bytes(config.as_bytes()),
        }));
    fixture.write_contract();
    let result = invoke(&fixture);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report_value: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(
        report_value["source_literal_ignoredirs"],
        json!([".unmaintained"])
    );
    let missing = report_value["audit"]["missing_endpoints"]
        .as_array()
        .unwrap();
    assert!(missing.iter().any(|entry| entry["name"] == "live-missing"));
    assert!(!missing
        .iter()
        .any(|entry| entry["name"] == "hidden-missing"));
    fs::write(
        fixture.root.path().join("mmake.config.in"),
        "ignoredir active\n",
    )
    .unwrap();
    assert!(
        !invoke(&fixture).status.success(),
        "modified discovery policy was accepted"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(report).unwrap()).unwrap(),
        report_value
    );
    assert!(!fixture.output().exists());
}

#[test]
fn native_inventory_preparation_preserves_cold_sources_without_publishing_a_graph() {
    let fixture = Fixture::new();
    fixture.append("\
%fetch mmake=pending-fetch archive=probe-1 destination=$(PORTSDIR)/probe location=$(PORTSSOURCEDIR) archive_origins=https://example.invalid suffixes=tar.gz\n\
SOURCES := $(wildcard $(PORTSDIR)/probe/probe-1/lib/*.c)\n\
%build_linklib mmake=pending-lib libname=probe files=\"$(SOURCES:.c=)\"\n\
%fetch mmake=unselected-fetch archive=unselected-9 destination=$(PORTSDIR)/unselected location=$(PORTSSOURCEDIR) archive_origins=https://unselected.example.invalid suffixes=tar.gz\n\
UNSELECTED_SOURCES := $(wildcard $(PORTSDIR)/unselected/unselected-9/lib/*.c)\n\
%build_linklib mmake=unselected-lib libname=unselected files=\"$(UNSELECTED_SOURCES:.c=)\"\n\
#MM- fixture-kernel : pending-lib\n");
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !fixture.output().exists(),
        "inventory preparation fabricated a graph"
    );
    let inventory_path = fixture.output().with_extension("source-inventory.cmake");
    let inventory = fs::read_to_string(&inventory_path).unwrap();
    assert!(
        inventory.contains("set(AROS_SOURCE_INVENTORY_FETCH_COUNT 1)"),
        "{inventory}"
    );
    assert!(inventory.contains("pending-fetch"));
    assert!(inventory.contains("probe-1"));
    assert!(!inventory.contains("unselected-fetch"), "{inventory}");
    assert!(!inventory.contains("unselected-9"), "{inventory}");
    assert!(
        !inventory.contains("unselected.example.invalid"),
        "{inventory}"
    );

    let strict = fixture.invoke(true, &[]);
    assert!(
        !strict.status.success(),
        "cold graph unexpectedly qualified"
    );
    assert!(!fixture.output().exists());
    assert_eq!(fs::read_to_string(&inventory_path).unwrap(), inventory);

    let ports = fixture.root.path().join("ports");
    fs::create_dir_all(ports.join("probe/probe-1/lib")).unwrap();
    fs::write(
        ports.join("probe/probe-1/lib/probe.c"),
        "int probe(void) { return 1; }\n",
    )
    .unwrap();
    let warm = fixture.invoke(true, &["--ports-dir", ports.to_str().unwrap()]);
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    let graph = fs::read_to_string(fixture.output()).unwrap();
    assert!(graph.contains("pending-lib"));
    assert!(graph.contains("probe/probe-1/lib/probe"));
    assert!(fs::read_to_string(&inventory_path)
        .unwrap()
        .contains("FETCH_COUNT 0)"));
    assert!(
        !ports.join("unselected/unselected-9").exists(),
        "the warm strict pass must not require an unrelated source tree"
    );

    // Preparation is not a graph replacement, even on an already warm tree.
    let again = fixture.invoke(
        true,
        &[
            "--source-inventory-only",
            "--ports-dir",
            ports.to_str().unwrap(),
        ],
    );
    assert!(again.status.success());
    assert_eq!(fs::read_to_string(fixture.output()).unwrap(), graph);
}

#[test]
fn native_inventory_preparation_rejects_an_unowned_ports_wildcard() {
    let fixture = Fixture::new();
    fixture.append("SOURCES := $(wildcard $(PORTSDIR)/unowned/*.c)\n%build_linklib mmake=pending-lib libname=probe files=\"$(SOURCES:.c=)\"\n#MM- fixture-kernel : pending-lib\n");
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure(&result, &fixture.output(), "no owning %fetch declaration");
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn native_inventory_ignores_an_unowned_ports_wildcard_outside_the_selected_closure() {
    let fixture = Fixture::new();
    fixture.append(
        "UNOWNED_SOURCES := $(wildcard $(PORTSDIR)/unowned/*.c)\n\
         %build_linklib mmake=unselected-unowned-lib libname=unowned files=\"$(UNOWNED_SOURCES:.c=)\"\n",
    );
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !fixture.output().exists(),
        "inventory preparation published a graph"
    );
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 0)"), "{inventory}");
}

#[test]
fn native_inventory_resolves_a_cold_direct_core_library_root_without_publishing_a_graph() {
    let fixture = Fixture::new();
    let recipe = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&recipe).unwrap();
    let replacement = "\
%fetch mmake=core-exec-fetch archive=exec-1 destination=$(PORTSDIR)/core-exec location=$(PORTSSOURCEDIR) archive_origins=https://core-exec.example.invalid suffixes=tar.gz\n\
EXEC_SOURCES := $(wildcard $(PORTSDIR)/core-exec/exec-1/lib/*.c)\n\
%build_module_simple mmake=fixture-exec modname=exec modtype=library files=\"$(EXEC_SOURCES:.c=)\"";
    let updated = original.replace(
        "%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe",
        replacement,
    );
    assert_ne!(
        updated, original,
        "fixture library declaration was not replaced"
    );
    fs::write(&recipe, updated).unwrap();

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !fixture.output().exists(),
        "inventory preparation published a graph"
    );
    let inventory_path = fixture.output().with_extension("source-inventory.cmake");
    let inventory = fs::read_to_string(&inventory_path).unwrap();
    assert!(inventory.contains("FETCH_COUNT 1)"), "{inventory}");
    assert!(inventory.contains("core-exec-fetch"), "{inventory}");
    assert!(inventory.contains("exec-1"), "{inventory}");

    let cold = fixture.invoke(true, &[]);
    assert!(
        !cold.status.success(),
        "cold direct root unexpectedly qualified"
    );
    assert!(!fixture.output().exists());

    let ports = fixture.root.path().join("ports");
    fs::create_dir_all(ports.join("core-exec/exec-1/lib")).unwrap();
    fs::write(
        ports.join("core-exec/exec-1/lib/exec_core.c"),
        "int exec_core(void) { return 1; }\n",
    )
    .unwrap();
    let warm = fixture.invoke(true, &["--ports-dir", ports.to_str().unwrap()]);
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    let graph = fs::read_to_string(fixture.output()).unwrap();
    assert!(graph.contains("fixture-exec"), "{graph}");
    assert!(graph.contains("core-exec/exec-1/lib/exec_core"), "{graph}");
    assert!(fs::read_to_string(inventory_path)
        .unwrap()
        .contains("FETCH_COUNT 0)"));
}

#[test]
fn native_inventory_rejects_a_selected_missing_archive_despite_an_empty_generic_endpoint() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe uselibs=missing-proof",
    );
    fixture.append("#MM- linklibs-missing-proof :\n");

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure(&result, &fixture.output(), "preparation archive request");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("missing-proof"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn native_inventory_rejects_a_selected_ambiguous_archive_request() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe uselibs=ambiguous-proof",
    );
    fixture.append(
        "%build_linklib mmake=ambiguous-provider-a libname=ambiguous-proof files=probe\n\
         %build_linklib mmake=ambiguous-provider-b libname=ambiguous-proof files=probe\n\
         #MM- linklibs-ambiguous-proof :\n",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure(&result, &fixture.output(), "preparation archive request");
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("ambiguous-proof"), "{diagnostic}");
    assert!(diagnostic.contains("found 2"), "{diagnostic}");
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn native_inventory_ignores_unselected_missing_and_ambiguous_archive_requests() {
    let fixture = Fixture::new();
    fixture.append(
        "%build_linklib mmake=unselected-missing-consumer libname=unselected-missing-consumer uselibs=missing-unselected-proof files=probe\n\
         %build_linklib mmake=unselected-ambiguous-consumer libname=unselected-ambiguous-consumer uselibs=ambiguous-unselected-proof files=probe\n\
         %build_linklib mmake=unselected-provider-a libname=ambiguous-unselected-proof files=probe\n\
         %build_linklib mmake=unselected-provider-b libname=ambiguous-unselected-proof files=probe\n",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 0)"), "{inventory}");
    assert!(!inventory.contains("unselected-"), "{inventory}");
}

#[test]
fn native_inventory_preserves_an_explicit_source_owned_generic_linklibs_endpoint() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe uselibs=typed-library",
    );
    fixture.append(
        "%fetch mmake=source-owned-fetch archive=source-owned-1 destination=$(PORTSDIR)/source-owned location=$(PORTSSOURCEDIR) archive_origins=https://source-owned.example.invalid suffixes=tar.gz\n\
         SOURCE_OWNED_SOURCES := $(wildcard $(PORTSDIR)/source-owned/source-owned-1/lib/*.c)\n\
         %build_linklib mmake=source-owned-producer libname=source-owned files=\"$(SOURCE_OWNED_SOURCES:.c=)\"\n\
         %build_linklib mmake=typed-library-provider libname=typed-library files=probe\n\
         #MM- fixture-exec : linklibs-typed-library\n\
         #MM- linklibs-typed-library : source-owned-producer\n\
         #MM- source-owned-producer : source-owned-fetch\n",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 1)"), "{inventory}");
    assert!(inventory.contains("source-owned-fetch"), "{inventory}");
    assert!(inventory.contains("source-owned-1"), "{inventory}");
}

#[test]
fn native_inventory_preserves_concrete_linklibs_producer_without_an_explicit_consumer_edge() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "%build_module mmake=fixture-exec modname=exec modtype=library files=probe uselibs=foo conffile=exec.conf",
    );
    fs::write(
        fixture.root.path().join("exec.conf"),
        "##begin config\nversion 1.0\noptions noautoinit\n##end config\n##begin functionlist\nvoid Fixture() ()\n##end functionlist\n",
    )
    .unwrap();
    fixture.append(
        "%fetch mmake=linklibs-foo-fetch archive=foo-1 destination=$(PORTSDIR)/foo location=$(PORTSSOURCEDIR) archive_origins=https://foo.example.invalid suffixes=tar.gz\n\
         FOO_SOURCES := $(wildcard $(PORTSDIR)/foo/foo-1/lib/*.c)\n\
         %build_linklib mmake=linklibs-foo libname=foo-static files=\"$(FOO_SOURCES:.c=)\"\n\
         %build_linklib mmake=foo-provider libname=foo files=probe\n\
         #MM- linklibs-foo : linklibs-foo-fetch\n\
         #MM- includes-generate-deps :\n\
         #MM- core-linklibs :\n",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 1)"), "{inventory}");
    assert!(inventory.contains("linklibs-foo-fetch"), "{inventory}");
    assert!(inventory.contains("foo-1"), "{inventory}");
}

#[test]
fn native_inventory_does_not_treat_exact_archive_link_options_as_typed_uselibs() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "USER_LDFLAGS := -l:exact-name.a\n%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 0)"), "{inventory}");
}

#[test]
fn native_inventory_does_not_resolve_raw_libraries_from_static_linklib_flags() {
    let fixture = Fixture::new();
    let recipe = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&recipe).unwrap();
    let updated = original.replace(
        "%build_linklib mmake=fixture-helper libname=helper files=probe",
        "USER_LDFLAGS := -lmissing-static-linklib-flag\n%build_linklib mmake=fixture-helper libname=helper files=probe",
    );
    assert_ne!(
        updated, original,
        "fixture helper declaration was not replaced"
    );
    fs::write(recipe, updated).unwrap();

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 0)"), "{inventory}");
}

#[test]
fn native_inventory_requires_an_exact_search_path_for_a_private_raw_library() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "USER_LDFLAGS := -lprivate-proof\n%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe uselibs=private-proof",
    );
    fixture.append(
        "%build_linklib mmake=private-provider libname=private-proof libdir=$(GENDIR)/private files=probe\n",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure(&result, &fixture.output(), "preparation archive request");
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("private-proof"), "{diagnostic}");
    assert!(diagnostic.contains("found 0"), "{diagnostic}");
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn native_inventory_accepts_a_private_raw_library_with_its_exact_search_path() {
    let fixture = Fixture::new();
    rewrite_fixture_exec(
        &fixture,
        "USER_LDFLAGS := -L$(GENDIR)/private -lprivate-proof\n%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe uselibs=private-proof",
    );
    fixture.append(
        "%build_linklib mmake=private-provider libname=private-proof libdir=$(GENDIR)/private files=probe\n",
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory =
        fs::read_to_string(fixture.output().with_extension("source-inventory.cmake")).unwrap();
    assert!(inventory.contains("FETCH_COUNT 0)"), "{inventory}");
}

#[test]
fn native_source_bound_optional_selector_fails_without_erasing_the_source_edge() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-headers-$(AROS_TARGET_ARCH)-$(AROS_TARGET_VARIANT)\n");
    bind_optional_meta_edge(&mut fixture, "mmakefile.src");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("fix upstream"), "{stderr}");
    assert!(stderr.contains("optional-headers-fixture-"), "{stderr}");
    assert!(!fixture.output().exists());
}

fn bind_disabled_owner(fixture: &mut Fixture, recipe: &str) {
    bind_optional_meta_edge(fixture, recipe);
    fixture.contract["optional_meta_dependencies"][0]["dependency"] = json!("optional-pkgconfig");
    fixture.contract["optional_meta_dependencies"][0]["absence"] = json!("disabled-owner");
    fixture.write_contract();
}

#[test]
fn native_disabled_metadata_dependency_fails_upstream_without_publishing() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-pkgconfig required-sibling\n##MM\n#optional-pkgconfig : $(AROS_LIB)/pkgconfig/optional.pc\n#MM- required-sibling :\n");
    bind_disabled_owner(&mut fixture, "mmakefile.src");
    for arguments in [vec![], vec!["--source-inventory-only"]] {
        let result = fixture.invoke(true, &arguments);
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        for expected in [
            "fix upstream",
            "mmakefile.src",
            "optional-pkgconfig",
            "commented out",
        ] {
            assert!(stderr.contains(expected), "{stderr}");
        }
        assert!(!fixture.output().exists());
        assert!(!fixture
            .output()
            .with_extension("source-inventory.cmake")
            .exists());
    }
}

#[test]
fn diagnostic_graph_audit_retains_disabled_owner_failure_and_sibling_evidence() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-pkgconfig audit-required-sibling\n##MM\n#optional-pkgconfig : $(AROS_LIB)/pkgconfig/optional.pc\n");
    bind_disabled_owner(&mut fixture, "mmakefile.src");
    let path = fixture.root.path().join("disabled-owner-audit.json");
    let result = fixture.invoke(
        true,
        &[
            "--source-inventory-only",
            "--native-graph-audit",
            path.to_str().unwrap(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        report["audit"]["qualification"],
        "diagnostic-only-not-build-proof"
    );
    let error = report["source_dependency_validation_error"]
        .as_str()
        .unwrap();
    for expected in [
        "fix upstream",
        "optional-pkgconfig",
        "commented out",
        "mmakefile.src",
    ] {
        assert!(error.contains(expected), "{report}");
    }
    assert!(report["audit"]["strict_validation_error"]
        .as_str()
        .unwrap()
        .contains(error));
    let missing = report["audit"]["missing_endpoints"].as_array().unwrap();
    for expected in ["optional-pkgconfig", "audit-required-sibling"] {
        assert!(
            missing.iter().any(|entry| entry["name"] == expected),
            "{report}"
        );
    }
    assert!(!fixture.output().exists());
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
    assert!(!fixture
        .invoke(true, &["--source-inventory-only"])
        .status
        .success());
    assert!(!fixture.output().exists());
}

#[test]
fn native_disabled_owner_requires_exact_bound_comment_not_a_plain_missing_dependency() {
    for comment in [
        "",
        "#optional-pkgconfig :\n",
        "##MM\n#other-pkgconfig :\n",
        " ##MM optional-pkgconfig :\n",
        "##MM\n\n#optional-pkgconfig :\n",
    ] {
        let mut fixture = Fixture::new();
        fixture.append(&format!("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-pkgconfig\n{comment}"));
        bind_disabled_owner(&mut fixture, "mmakefile.src");
        let result = fixture.invoke(true, &[]);
        assert!(!result.status.success(), "accepted {comment:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("not explicitly commented"));
        assert!(!fixture.output().exists());
    }
}

#[test]
fn native_disabled_comment_cannot_hide_an_active_unsupported_owner_elsewhere() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-pkgconfig\n##MM optional-pkgconfig :\n");
    fs::create_dir(fixture.root.path().join("other")).unwrap();
    fs::write(
        fixture.root.path().join("other/mmakefile.src"),
        "#MM optional-pkgconfig :\noptional-pkgconfig :\n\t@$(ECHO) unmodelled\n",
    )
    .unwrap();
    bind_disabled_owner(&mut fixture, "mmakefile.src");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("optional-pkgconfig"));
    assert!(!fixture.output().exists());
}

#[test]
fn native_disabled_owner_cannot_borrow_another_recipes_comment_or_erase_its_edge() {
    for duplicate_edge in [false, true] {
        let mut fixture = Fixture::new();
        fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-pkgconfig\n");
        fs::create_dir(fixture.root.path().join("other")).unwrap();
        fs::write(
            fixture.root.path().join("other/mmakefile.src"),
            "##MM optional-pkgconfig :\n#MM- optional-aggregate : optional-pkgconfig\n",
        )
        .unwrap();
        if duplicate_edge {
            fixture.append("##MM optional-pkgconfig :\n");
        }
        bind_disabled_owner(&mut fixture, "mmakefile.src");
        let result = fixture.invoke(true, &[]);
        assert!(!result.status.success());
        assert!(!fixture.output().exists());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains(if duplicate_edge {
                "source declaration without explicit optionality"
            } else {
                "not explicitly commented"
            }),
            "{stderr}"
        );
    }
}

#[test]
fn native_disabled_owner_cannot_erase_a_macro_generated_dependency() {
    let mut fixture = Fixture::new();
    fixture.append("%build_module mmake=implicit-provider modname=implicit modtype=library files=probe\n##MM core-linklibs :\n");
    bind_disabled_owner(&mut fixture, "mmakefile.src");
    fixture.contract["optional_meta_dependencies"][0]["target"] = json!("implicit-provider");
    fixture.contract["optional_meta_dependencies"][0]["dependency"] = json!("core-linklibs");
    fixture.write_contract();
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("not declared by this bound source recipe"));
    assert!(!fixture.output().exists());
}

#[test]
fn native_architecture_edge_with_a_source_declared_producer_keeps_its_module_owner() {
    let mut fixture = Fixture::new();
    fixture.append(
        "#MM fixture-kernel : optional-headers-$(AROS_TARGET_ARCH)-$(AROS_TARGET_VARIANT)\n#MM- optional-headers-fixture- :\n",
    );
    bind_optional_meta_edge(&mut fixture, "mmakefile.src");
    fixture.contract["optional_meta_dependencies"][0]["target"] = json!("fixture-kernel");
    fixture.write_contract();
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(!generated.contains("# Source-declared absent optional MetaMake edge:"));
    assert!(generated.contains("aros_add_module_simple("));
    assert!(generated.contains("add_custom_target(\"optional-headers-fixture-\")"));
}

#[test]
fn native_parser_retains_the_exact_legacy_byte_snapshot_identity() {
    let fixture = Fixture::new();
    let path = fixture.root.path().join("mmakefile.src");
    let mut bytes = b"# Copyright \xa9\n".to_vec();
    bytes.extend(fs::read(&path).unwrap());
    fs::write(&path, &bytes).unwrap();
    let parsed = aros_transpiler::parse_mmakefile_with_context(
        &path,
        fixture.root.path(),
        &aros_transpiler::TargetContext::default(),
    )
    .unwrap();
    let expected = aros_common::sha256_bytes(&bytes);
    assert_eq!(parsed.source_sha256.as_deref(), Some(expected.as_str()));
    fs::write(&path, b"#MM- replacement :\n").unwrap();
    assert_eq!(parsed.source_sha256.as_deref(), Some(expected.as_str()));
    assert_ne!(
        expected,
        aros_common::sha256_bytes(&fs::read(path).unwrap())
    );
}

#[test]
fn native_optional_selector_cannot_borrow_a_declaration_from_another_recipe() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-headers-$(AROS_TARGET_ARCH)-$(AROS_TARGET_VARIANT)\n");
    fs::create_dir(fixture.root.path().join("other")).unwrap();
    fs::write(
        fixture.root.path().join("other/mmakefile.src"),
        "#MM- unrelated :\n",
    )
    .unwrap();
    bind_optional_meta_edge(&mut fixture, "other/mmakefile.src");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("not declared by this bound source recipe"));
    assert!(!fixture.output().exists());
}

#[test]
fn native_optional_selector_cannot_erase_another_recipes_mandatory_edge() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-headers-$(AROS_TARGET_ARCH)-$(AROS_TARGET_VARIANT)\n");
    fs::create_dir(fixture.root.path().join("other")).unwrap();
    fs::write(
        fixture.root.path().join("other/mmakefile.src"),
        "#MM- optional-aggregate : optional-headers-$(AROS_TARGET_ARCH)-$(AROS_TARGET_VARIANT)\n",
    )
    .unwrap();
    bind_optional_meta_edge(&mut fixture, "mmakefile.src");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("source declaration without explicit optionality"));
    assert!(!fixture.output().exists());
}

#[test]
fn native_optional_selector_does_not_hide_a_real_unimplemented_provider() {
    let mut fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : optional-aggregate\n#MM- optional-aggregate : optional-headers-$(AROS_TARGET_ARCH)-$(AROS_TARGET_VARIANT)\n#MM optional-headers-fixture- :\noptional-headers-fixture- :\n\t@$(ECHO) unmodelled\n");
    bind_optional_meta_edge(&mut fixture, "mmakefile.src");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("optional-headers-fixture-"));
    assert!(!fixture.output().exists());
}

#[test]
fn native_explicit_empty_virtual_target_is_a_proven_noop() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : fixture-empty\n#MM- fixture-empty :\n");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(
        generated.contains("add_custom_target(\"fixture-empty\")"),
        "{generated}"
    );
}

const fn sdk_text_recipe() -> &'static str {
    "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/fixture/archive\n\
     %fetch mmake=fixture-sdk-fetch archive=archive destination=$(PORTSDIR)/fixture\n\
     #MM- fixture-kernel : fixture-sdk-pkgc\n\
     #MM fixture-sdk-pkgc : fixture-sdk-fetch\n\
     fixture-sdk-pkgc : $(AROS_LIB)/pkgconfig/fixture.pc\n\
     $(AROS_LIB)/pkgconfig/fixture.pc : $(ARCHSRCDIR)/fixture.pc.in\n\
     \t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\
     \t@$(SED) -e 's|@VERSION@|$(VERSION)|g' -e 's|^exec_prefix=.*|exec_prefix=$${prefix}|' $< > $@\n"
}

fn literal_object_recipe(flags: &str) -> String {
    format!(
        "#MM- fixture-kernel : literal-entry\n\
        ENTRY_FLAGS = {flags}\n\
        $(GENDIR)/entry.o : $(SRCDIR)/entry.c | $(GENDIR)\n\
        \t@$(ECHO) \"Compiling  $<\"\n\
        \t@$(TARGET_CC) -c $(ENTRY_FLAGS) $< -o $@\n\
        #MM literal-entry :\n\
        literal-entry : $(GENDIR)/entry.o\n"
    )
}

#[test]
fn native_literal_compile_has_a_real_owner_and_preserves_order() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("entry.c"), "int literal_entry;\n").unwrap();
    fixture.append(&literal_object_recipe("-DX=1 -UX -DX=2 -DX=2"));
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(generated.contains("aros_compile_literal_object("));
    assert!(generated.contains("aros_literal_object_group(\n    NAME \"literal-entry\""));
    assert!(generated.contains("\"-DX=1\"\n        \"-UX\"\n        \"-DX=2\"\n        \"-DX=2\""));
    assert!(!generated.contains("aros_compile_sdk_object("));
}

#[test]
fn native_literal_compile_refuses_unknown_and_unsafe_flags_before_output() {
    for flags in [
        "$(UNKNOWN_FLAGS)",
        "-fplugin=outside.so",
        "@outside.rsp",
        "-o outside.o",
        "-DX=1;touch marker",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.root.path().join("entry.c"), "int literal_entry;\n").unwrap();
        fixture.append(&literal_object_recipe(flags));
        let result = fixture.invoke(true, &[]);
        assert!(!result.status.success(), "accepted {flags}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("literal object producer"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!fixture.output().exists());
    }
}

#[test]
fn native_sdk_text_selects_real_fetch_and_preserves_literal_pkgconfig_variables() {
    let fixture = Fixture::new();
    fixture.append(sdk_text_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(
        generated.contains("aros_transform_sdk_text("),
        "{generated}"
    );
    assert!(
        generated.contains("FETCH \"fixture-sdk-fetch\""),
        "{generated}"
    );
    assert!(generated.contains("exec_prefix=\\${prefix}"), "{generated}");
    let source_sha =
        aros_common::sha256_bytes(&fs::read(fixture.root.path().join("mmakefile.src")).unwrap());
    assert!(generated.contains("FILE \"mmakefile.src\""), "{generated}");
    assert!(
        generated.contains(&format!("FILE_SHA256 \"{source_sha}\"")),
        "{generated}"
    );
    assert!(
        !generated.contains("add_custom_target(\"fixture-sdk-pkgc\")"),
        "{generated}"
    );
}

fn fetched_sdk_text_with_own_recipe() -> String {
    sdk_text_recipe()
        .replace(
            "#MM fixture-sdk-pkgc : fixture-sdk-fetch",
            "#MM",
        )
        .replace(
            "$(ARCHSRCDIR)/fixture.pc.in\n",
            "$(ARCHSRCDIR)/fixture.pc.in $(SRCDIR)/$(CURDIR)/mmakefile.src\n",
        )
        .replace(
            "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi",
            "\t@$(ECHO) \"Generating /Developer/lib/pkgconfig/fixture.pc ...\"\n\t%mkdir_q dir=$(AROS_LIB)/pkgconfig",
        )
        .replace(
            "-e 's|@VERSION@|$(VERSION)|g' -e 's|^exec_prefix=.*|exec_prefix=$${prefix}|' $< > $@",
            "-e 's|@VERSION@|$(VERSION)|' -e 's| -I$${includedir}||' $< >$@",
        )
}

#[test]
fn native_sdk_text_derives_fetch_order_and_first_match_from_the_source_recipe() {
    let fixture = Fixture::new();
    fixture.append(&fetched_sdk_text_with_own_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(
        generated.contains("FETCH \"fixture-sdk-fetch\""),
        "{generated}"
    );
    assert!(
        generated.contains("REPLACE_FIRST_PER_LINE|@VERSION@|1.2.3"),
        "{generated}"
    );
    assert!(
        generated.contains("REPLACE_FIRST_PER_LINE| -I\\${includedir}|"),
        "{generated}"
    );
    let source_sha =
        aros_common::sha256_bytes(&fs::read(fixture.root.path().join("mmakefile.src")).unwrap());
    assert!(
        generated.contains(&format!("FILE_SHA256 \"{source_sha}\"")),
        "{generated}"
    );
}

#[test]
fn native_sdk_text_refuses_a_foreign_second_prerequisite_before_output() {
    let fixture = Fixture::new();
    fixture.append(&fetched_sdk_text_with_own_recipe().replace(
        "$(SRCDIR)/$(CURDIR)/mmakefile.src",
        "$(SRCDIR)/other/mmakefile.src",
    ));
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "SDK text rule is outside its closed capability",
        "capability_validation",
    );
}

#[test]
fn native_sdk_text_refuses_unbound_deferred_path_variables_before_output() {
    for recipe in [
        sdk_text_recipe().replace(
            "$(ARCHSRCDIR)/fixture.pc.in",
            "$(ARCHSRCDIR)/$${EVIL}/fixture.pc.in",
        ),
        sdk_text_recipe().replace(
            "$(AROS_LIB)/pkgconfig/fixture.pc",
            "$(AROS_LIB)/pkgconfig/$${EVIL}/fixture.pc",
        ),
        sdk_text_recipe().replace(
            "destination=$(PORTSDIR)/fixture",
            "destination=$(PORTSDIR)/fixture/$${EVIL}",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.append(&recipe);
        let result = fixture.invoke(true, &[]);
        assert!(
            !result.status.success(),
            "unbound CMake path variable accepted: {recipe}"
        );
        assert!(
            !fixture.output().exists(),
            "unbound path published partial output"
        );
    }
}

#[test]
fn native_sdk_text_cannot_ignore_conflicting_virtual_or_tabbed_fetch_edges() {
    for marker in ["#MM-", "#MM\t", "#MM-\t"] {
        let fixture = Fixture::new();
        let recipe = sdk_text_recipe().replace(
            "#MM fixture-sdk-pkgc : fixture-sdk-fetch",
            &format!("{marker} fixture-sdk-pkgc : fixture-other-fetch"),
        );
        fixture.append(&recipe);
        fixture.append(
            "%fetch mmake=fixture-other-fetch archive=other destination=$(PORTSDIR)/other\n",
        );
        let result = fixture.invoke(true, &[]);
        assert!(
            !result.status.success(),
            "conflicting {marker} edge accepted"
        );
        assert!(
            !fixture.output().exists(),
            "conflicting edge published partial output"
        );
    }
}

#[test]
fn native_sdk_text_cannot_hide_an_extra_shell_command() {
    let fixture = Fixture::new();
    fixture.append(&sdk_text_recipe().replace("\t@$(SED)", "\t@$(ECHO) unmodelled\n\t@$(SED)"));
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "SDK text rule is outside its closed capability",
        "capability_validation",
    );
}

#[test]
fn native_sdk_text_cannot_borrow_a_fetch_from_another_makefile() {
    let fixture = Fixture::new();
    fixture.append(&sdk_text_recipe().replace(
        "%fetch mmake=fixture-sdk-fetch archive=archive destination=$(PORTSDIR)/fixture\n",
        "",
    ));
    let other = fixture.root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        "%fetch mmake=fixture-sdk-fetch archive=archive destination=$(PORTSDIR)/fixture\n",
    )
    .unwrap();
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "SDK text rule is outside its closed capability",
        "capability_validation",
    );
}

const fn whole_line_header_recipe() -> &'static str {
    "ARCHSRCDIR := $(PORTSDIR)/fixture/archive\n\
     %fetch mmake=fixture-header-fetch archive=archive destination=$(PORTSDIR)/fixture\n\
     #MM- fixture-kernel : fixture-header-generated\n\
     #MM fixture-header-generated : fixture-header-fetch\n\
     fixture-header-generated : $(AROS_INCLUDES)/fixtureconfig.h\n\
     $(AROS_INCLUDES)/fixtureconfig.h : $(ARCHSRCDIR)/fixtureconfig.h.prebuilt\n\
     \t$(SED) \"s|.*FEATURE_TOKEN.*|#if defined(PLATFORM)\\\\n#define FEATURE_TOKEN\\\\n#else\\\\n/*#undef FEATURE_TOKEN*/\\\\n#endif|g\" $< > $@\n"
}

const fn source_text_recipe() -> &'static str {
    "ARCHSRCDIR := $(PORTSDIR)/fixture/archive\n\
     %fetch mmake=fixture-text-fetch archive=archive destination=$(PORTSDIR)/fixture\n\
     #MM- fixture-kernel : fixture-text-generated\n\
     #MM fixture-text-generated : fixture-text-fetch\n\
     fixture-text-generated : $(AROS_INCLUDES)/fixtureconfig.h $(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)/fixture-config\n\
     \t@$(NOP)\n\
     $(AROS_INCLUDES)/fixtureconfig.h : $(ARCHSRCDIR)/fixtureconfig.h.prebuilt\n\
     \t@$(ECHO) \"Generating build options ...\"\n\
     \t%mkdir_q dir=\"$(AROS_INCLUDES)\"\n\
     \t@$(SED) -e \"s|.*FEATURE_TOKEN.*|#define FEATURE_TOKEN\\n|g\" -e \"s|.*SECOND_TOKEN.*|#define SECOND_TOKEN\\n|g\" $< > $@\n\
     $(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)/fixture-config : $(ARCHSRCDIR)/fixture-config.in\n\
     \t@$(ECHO) \"Generating cross configuration ...\"\n\
     \t%mkdir_q dir=\"$(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)\"\n\
     \t@$(SED) -e \"s|%PKG_CONFIG%|false|g\" -e \"s|%prefix%|$(AROS_DEVELOPER)|g\" $< > $@\n\
     \t@chmod 744 $@\n"
}

const fn sdk_file_recipe() -> &'static str {
    "AROS_DIR_SDK := SDK\nAROS_DIR_FD := fd\n\
     AROS_SDK_FD := $(AROS_DEVELOPER)/$(AROS_DIR_SDK)/$(AROS_DIR_FD)\n\
     ARCHSRCDIR := $(PORTSDIR)/fixture/archive\n\
     %fetch mmake=fixture-file-fetch archive=archive destination=$(PORTSDIR)/fixture\n\
     #MM- fixture-kernel : fixture-sdk-files\n\
     #MM fixture-sdk-files : fixture-file-fetch\n\
     %copy_files_q mmake=fixture-sdk-files files=fixture_lib.fd src=$(ARCHSRCDIR)/developer/fd dst=$(AROS_SDK_FD)\n"
}

#[test]
fn native_developer_library_copy_uses_the_source_macro_defaults_and_real_producer() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.path().join("auto"),
        b"*autolib:\r\n-lfixture\r\n",
    )
    .unwrap();
    fixture.append("AROS_DIR_LIB := lib\nAUTOFILE := auto\n#MM- fixture-kernel : fixture-autofile\n%copy_files_q mmake=fixture-autofile files=$(AUTOFILE) dst=$(AROS_LIB)\n");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(
        generated.contains("NAME \"fixture-autofile\""),
        "{generated}"
    );
    assert!(
        generated.contains("DESTINATION \"${AROS_DEVELOPER_LIB_DIR}\""),
        "{generated}"
    );
    assert!(generated.contains("        \"auto\""), "{generated}");
    assert!(
        !generated.contains("add_custom_target(\"fixture-autofile\")"),
        "{generated}"
    );
}

#[test]
fn native_developer_library_copy_refuses_an_unresolved_file_list_before_publication() {
    let fixture = Fixture::new();
    fixture.append("AROS_DIR_LIB := lib\n#MM- fixture-kernel : fixture-autofile\n%copy_files_q mmake=fixture-autofile files=$(UNKNOWN_AUTOFILE) dst=$(AROS_LIB)\n");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(!fixture.output().exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("fixture-autofile"));
}

#[test]
fn native_sdk_file_copy_preserves_exact_file_and_fetch() {
    let fixture = Fixture::new();
    fixture.append(sdk_file_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(
        generated.matches("aros_stage_sdk_files(").count(),
        1,
        "{generated}"
    );
    for expected in [
        "FETCH \"fixture-file-fetch\"",
        "DESTINATION \"${AROS_DEVELOPER_FD_DIR}\"",
        "\"fixture_lib.fd\"",
    ] {
        assert!(generated.contains(expected), "{generated}");
    }
    assert!(!generated.contains("add_custom_target(\"fixture-sdk-files\")"));
}

#[test]
fn native_sdk_file_copy_rejects_unsafe_or_unowned_inputs_before_output() {
    for text in [
        sdk_file_recipe().replace("files=fixture_lib.fd", "files=../fixture_lib.fd"),
        sdk_file_recipe().replace("files=fixture_lib.fd", "files=*.fd"),
        sdk_file_recipe().replace(
            "src=$(ARCHSRCDIR)/developer/fd",
            "src=$(PORTSDIR)/foreign/fd",
        ),
        sdk_file_recipe().replace(
            "#MM fixture-sdk-files : fixture-file-fetch",
            "#MM fixture-sdk-files : missing-fetch",
        ),
        sdk_file_recipe().replace(
            "files=fixture_lib.fd",
            "files=fixture_lib.fd files=other.fd",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.append(&text);
        assert_failure_at_stage(
            &fixture.invoke(true, &[]),
            &fixture.output(),
            "SDK file copy",
            "capability_validation",
        );
    }
}

#[test]
fn native_sdk_file_copy_cannot_borrow_foreign_makefile_fetch() {
    let fixture = Fixture::new();
    fixture.append(&sdk_file_recipe().replace(
        "%fetch mmake=fixture-file-fetch archive=archive destination=$(PORTSDIR)/fixture\n",
        "",
    ));
    let other = fixture.root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        "%fetch mmake=fixture-file-fetch archive=archive destination=$(PORTSDIR)/fixture\n",
    )
    .unwrap();
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "SDK file copy",
        "capability_validation",
    );
}

#[test]
fn native_sdk_file_copy_generated_cmake_preserves_bytes_and_rebuilds() {
    use std::fmt::Write as _;
    let fixture = Fixture::new();
    fixture.append(sdk_file_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    let copies = generated.split("aros_stage_sdk_files(").skip(1).fold(
        String::new(),
        |mut result, block| {
            let (arguments, _) = block.split_once(")\n\n").unwrap();
            writeln!(result, "aros_stage_sdk_files({arguments})").unwrap();
            result
        },
    );
    assert_eq!(copies.matches("aros_stage_sdk_files(").count(), 1);
    let project = fixture.root.path().join("file project");
    let build = fixture.root.path().join("file build");
    let source = build.join("Ports/fixture/archive/developer/fd/fixture_lib.fd");
    fs::create_dir(&project).unwrap();
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let original = b"##base FixtureBase\r\n##bias 30\n\xff\x00";
    fs::write(&source, original).unwrap();
    fs::write(
        build.join("Ports/fixture/.complete"),
        b"fixture fetch receipt\n",
    )
    .unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/SdkFileCopies.cmake");
    fs::write(project.join("CMakeLists.txt"), format!(r#"
cmake_minimum_required(VERSION 3.22)
project(native_sdk_files NONE)
include("{}")
set(AROS_SOURCE_DIR "{}")
set(AROS_BUILD_DIR "${{CMAKE_BINARY_DIR}}")
set(AROS_PORTS_DIR "${{CMAKE_BINARY_DIR}}/Ports")
set(AROS_DEVELOPER_FD_DIR "${{CMAKE_BINARY_DIR}}/SYS/Developer/SDK/fd")
add_custom_target(fixture-file-fetch)
set_property(GLOBAL PROPERTY AROS_FETCH_TARGETS fixture-file-fetch)
set_property(TARGET fixture-file-fetch PROPERTY AROS_FETCH_DESTINATION "${{AROS_PORTS_DIR}}/fixture")
set_property(TARGET fixture-file-fetch PROPERTY AROS_FETCH_COMPLETION_STAMP "${{AROS_PORTS_DIR}}/fixture/.complete")
{copies}
"#, helper.display(), fixture.root.path().display())).unwrap();
    let configure = Command::new("cmake")
        .arg("-S")
        .arg(&project)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .unwrap();
    assert!(
        configure.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&configure.stdout),
        String::from_utf8_lossy(&configure.stderr)
    );
    let output = build.join("SYS/Developer/SDK/fd/fixture_lib.fd");
    for bytes in [original.as_slice(), b"updated SDK bytes\n".as_slice()] {
        fs::write(&source, bytes).unwrap();
        let result = Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--target", "fixture-sdk-files"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read(&output).unwrap(), bytes);
    }
}

#[test]
fn native_source_text_preserves_all_products_and_fetch_identity() {
    let fixture = Fixture::new();
    fixture.append(source_text_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(
        generated.matches("aros_transform_source_text(").count(),
        2,
        "{generated}"
    );
    assert_eq!(generated.matches("FETCH \"fixture-text-fetch\"").count(), 2);
    assert!(generated.contains("MODE \"744\""), "{generated}");
    assert!(
        generated.contains("replace_whole_line_containing"),
        "{generated}"
    );
    assert!(!generated.contains("add_custom_target(\"fixture-text-generated\")"));
}

#[test]
fn native_source_text_rejects_incomplete_or_unsafe_aggregate_before_output() {
    for content in [
        source_text_recipe().replace("\t@chmod 744 $@", "\t@chmod 777 $@"),
        source_text_recipe().replace("\t@chmod 744 $@", "\t@chmod 744 $@\n\t@touch stolen"),
        source_text_recipe().replace("$(ARCHSRCDIR)/fixture-config.in", "$(SRCDIR)/foreign.in"),
        source_text_recipe()
            .split("\n$(TOOLDIR)")
            .next()
            .unwrap()
            .to_owned(),
    ] {
        let fixture = Fixture::new();
        fixture.append(&content);
        assert_failure_at_stage(
            &fixture.invoke(true, &[]),
            &fixture.output(),
            "source text",
            "capability_validation",
        );
    }
}

#[cfg(unix)]
#[test]
fn native_source_text_generated_cmake_builds_all_products() {
    use std::fmt::Write as _;
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.append(source_text_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    let transforms = generated.split("aros_transform_source_text(").skip(1).fold(
        String::new(),
        |mut result, block| {
            writeln!(
                result,
                "aros_transform_source_text({})",
                block.split(")\n\n").next().unwrap()
            )
            .unwrap();
            result
        },
    );
    let project = fixture.root.path().join("text-project");
    let build = fixture.root.path().join("text-build");
    fs::create_dir(&project).unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/SourceTextRules.cmake");
    fs::write(project.join("CMakeLists.txt"), format!(r#"
cmake_minimum_required(VERSION 3.22)
project(native_text NONE)
include("{}")
set(AROS_BUILD_DIR "${{CMAKE_BINARY_DIR}}")
set(AROS_PORTS_DIR "${{CMAKE_BINARY_DIR}}/Ports")
set(AROS_DEVELOPER_INCLUDE_DIR "${{CMAKE_BINARY_DIR}}/SYS/Developer/include")
set(AROS_SDK_INCLUDE_DIR "${{CMAKE_BINARY_DIR}}/SDK/include")
set(AROS_GENINC_DIR "${{CMAKE_BINARY_DIR}}/gen/include")
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM fixture)
file(MAKE_DIRECTORY "${{AROS_PORTS_DIR}}/fixture/archive" "${{AROS_DEVELOPER_INCLUDE_DIR}}"
    "${{AROS_GENINC_DIR}}" "${{CMAKE_BINARY_DIR}}/hosttools")
file(WRITE "${{AROS_PORTS_DIR}}/fixture/archive/fixtureconfig.h.prebuilt" "/* FEATURE_TOKEN */\n/* SECOND_TOKEN */\n")
file(WRITE "${{AROS_PORTS_DIR}}/fixture/archive/fixture-config.in" "pkg=%PKG_CONFIG%\nprefix=%prefix%\n")
file(WRITE "${{AROS_PORTS_DIR}}/fixture/.complete" "verified fixture\n")
add_custom_target(fixture-text-fetch)
set_property(TARGET fixture-text-fetch PROPERTY AROS_FETCH_DESTINATION "${{AROS_PORTS_DIR}}/fixture")
set_property(TARGET fixture-text-fetch PROPERTY AROS_FETCH_COMPLETION_STAMP "${{AROS_PORTS_DIR}}/fixture/.complete")
{transforms}
"#, helper.display())).unwrap();
    for arguments in [
        vec![
            "-S".to_owned(),
            project.display().to_string(),
            "-B".to_owned(),
            build.display().to_string(),
            "-G".to_owned(),
            "Ninja".to_owned(),
        ],
        vec![
            "--build".to_owned(),
            build.display().to_string(),
            "--target".to_owned(),
            "fixture-text-generated".to_owned(),
        ],
    ] {
        let result = Command::new("cmake").args(arguments).output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(
        fs::read_to_string(build.join("SDK/include/fixtureconfig.h")).unwrap(),
        "#define FEATURE_TOKEN\n\n#define SECOND_TOKEN\n\n"
    );
    let script = build.join("hosttools/riscv-fixture/fixture-config");
    assert_eq!(
        fs::read_to_string(&script).unwrap(),
        format!("pkg=false\nprefix={}/SYS/Developer\n", build.display())
    );
    assert_eq!(
        fs::metadata(script).unwrap().permissions().mode() & 0o777,
        0o744
    );
}

#[test]
fn native_whole_line_header_selects_the_exact_producer_and_fetch() {
    let fixture = Fixture::new();
    fixture.append(whole_line_header_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(generated.contains("WHOLE_LINE_CONTAINING"), "{generated}");
    assert!(
        generated.contains("DEPENDS \"fixture-header-fetch\""),
        "{generated}"
    );
    assert!(
        generated.contains("#if defined(PLATFORM)\n#define FEATURE_TOKEN\n#else"),
        "{generated}"
    );
    assert!(
        !generated.contains("add_custom_target(\"fixture-header-generated\")"),
        "{generated}"
    );
}

#[test]
fn native_whole_line_header_cannot_hide_an_extra_command() {
    let fixture = Fixture::new();
    fixture
        .append(&whole_line_header_recipe().replace("\t$(SED)", "\t$(ECHO) unmodelled\n\t$(SED)"));
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "nonvirtual Make provider",
        "capability_validation",
    );
}

#[test]
fn native_package_reports_unknown_membership_without_publishing_a_graph() {
    let fixture = Fixture::new();
    let makefile = fixture.root.path().join("mmakefile.src");
    let content = fs::read_to_string(&makefile).unwrap();
    fs::write(&makefile, content.replace(
        "%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer",
        "SELECTED_DEVS := timer\nifeq ($(UNREVIEWED_SWITCH),1)\nSELECTED_DEVS += unproven\nendif\n%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=\"$(SELECTED_DEVS)\"",
    )).unwrap();
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "SELECTED_DEVS",
        "capability_validation",
    );
}

#[test]
fn native_source_configuration_resolves_conditions_without_board_special_cases() {
    for (value, local, passes) in [
        ("", "", true),
        ("0", "", true),
        ("1", "", false),
        ("1", "FEATURE_SWITCH := 0\n", true),
    ] {
        let mut fixture = Fixture::new();
        fixture.contract["make_variables"] = json!({"FEATURE_SWITCH": value});
        fixture.write_contract();
        let makefile = fixture.root.path().join("mmakefile.src");
        let content = fs::read_to_string(&makefile).unwrap();
        fs::write(&makefile, content.replace(
            "%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer",
            &format!("{local}SELECTED_DEVS := timer\nifeq ($(FEATURE_SWITCH),1)\nSELECTED_DEVS += unproven\nendif\n%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=\"$(SELECTED_DEVS)\""),
        )).unwrap();
        let result = fixture.invoke(true, &[]);
        assert_eq!(
            result.status.success(),
            passes,
            "value={value:?} local={local:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fixture.output().exists(), passes);
    }
}

#[test]
fn native_source_configuration_is_visible_to_direct_make_expression_and_question_assignment() {
    let mut fixture = Fixture::new();
    fixture.contract["make_variables"] = json!({"DEVICE_SET": "timer", "EMPTY_SWITCH": ""});
    fixture.write_contract();
    let makefile = fixture.root.path().join("mmakefile.src");
    let content = fs::read_to_string(&makefile).unwrap();
    fs::write(&makefile, content.replace(
        "%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer",
        "DEVICE_SET ?= unproven\nifdef EMPTY_SWITCH\nDEVICE_SET += unproven\nendif\n%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=\"$(DEVICE_SET)\"",
    )).unwrap();
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn native_host_configuration_uses_only_the_actual_declared_host() {
    let host = aros_common::target::native_host_key().expect("supported native test host");
    for (table_host, local, passes) in [
        (host, "", true),
        ("unselected-host", "", false),
        (host, "DEVICE_SET := unproven\n", false),
    ] {
        let mut fixture = Fixture::new();
        fixture.contract["host_make_variables"] = json!({table_host: {"DEVICE_SET": "timer"}});
        fixture.write_contract();
        let makefile = fixture.root.path().join("mmakefile.src");
        let content = fs::read_to_string(&makefile).unwrap();
        fs::write(&makefile, content.replace(
            "%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer",
            &format!("{local}%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=\"$(DEVICE_SET)\""),
        )).unwrap();
        let result = fixture.invoke(true, &[]);
        assert_eq!(
            result.status.success(),
            passes,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fixture.output().exists(), passes);
    }
}

#[test]
fn native_source_defaults_do_not_hide_uncertain_ifdef_writes() {
    for reset in ["", "FEATURE :=\n"] {
        let mut fixture = Fixture::new();
        fixture.contract["make_variables"] = json!({"FEATURE":""});
        fixture.write_contract();
        let makefile = fixture.root.path().join("mmakefile.src");
        let content = fs::read_to_string(&makefile).unwrap();
        fs::write(&makefile, content.replace(
            "%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer",
            &format!("ifeq ($(UNKNOWN_CONFIG),x)\nFEATURE := 1\nendif\n{reset}SELECTED_DEVS := timer\nifdef FEATURE\nSELECTED_DEVS += unproven\nendif\n%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=\"$(SELECTED_DEVS)\""),
        )).unwrap();
        let result = fixture.invoke(true, &[]);
        assert_eq!(
            result.status.success(),
            !reset.is_empty(),
            "reset={reset:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fixture.output().exists(), !reset.is_empty());
    }
}

#[test]
fn native_configured_append_and_immediate_assignments_preserve_make_order() {
    let mut fixture = Fixture::new();
    fixture.contract["make_variables"] = json!({"DEVICE_SET":"other", "CURRENT_DEVICE":"timer"});
    fixture.write_contract();
    let makefile = fixture.root.path().join("mmakefile.src");
    let content = fs::read_to_string(&makefile).unwrap();
    fs::write(&makefile, format!("FROZEN_DEVICE := $(CURRENT_DEVICE)\nCURRENT_DEVICE := unproven\n%build_module_simple mmake=fixture-other modname=other modtype=device files=probe\n{}", content.replace(
        "%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer",
        "DEVICE_SET += $(FROZEN_DEVICE)\n%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=\"$(DEVICE_SET)\"",
    ))).unwrap();
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fs::read_to_string(fixture.output())
        .unwrap()
        .contains("fixture-other"));
}

#[test]
fn native_source_configuration_cannot_override_target_identity_or_execute_syntax() {
    for variables in [
        json!({"CPU":"riscv"}),
        json!({"FEATURE":"$(shell touch bad)"}),
        json!({"FEATURE":"a;b"}),
    ] {
        let mut fixture = Fixture::new();
        fixture.contract["make_variables"] = variables;
        fixture.write_contract();
        assert_failure_at_stage(
            &fixture.invoke(true, &[]),
            &fixture.output(),
            "make_variables",
            "graph_validation",
        );
    }
}

#[test]
fn native_config_projection_reaches_scoped_kobj_metadata_at_the_process_boundary() {
    let mut fixture = Fixture::new();
    let original = "include $(TOP)/config/make.cfg\n";
    let projection = "undefine KOBJ_LDFLAGS\nKERNEL_KOBJ_LDSCRIPT :=\nFUNCINSTR_LIBS = instrfunc\nTARGET_FUNCINSTR := no\n";
    for (path, bytes) in [
        ("config/aros.cfg", original.as_bytes()),
        ("native-config.mk", projection.as_bytes()),
        ("source-native.mk", b"KERNEL_KOBJ_LDSCRIPT :=\n".as_slice()),
    ] {
        fs::write(fixture.root.path().join(path), bytes).unwrap();
        fixture.contract["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "path": path,
                "sha256": aros_common::sha256_bytes(bytes),
            }));
    }
    fixture.contract["make_include_bindings"] = json!({
        "config/aros.cfg": "native-config.mk",
        "source-native.mk": "source-native.mk"
    });
    fixture.write_contract();
    let makefile = fixture.root.path().join("mmakefile.src");
    let content = fs::read_to_string(&makefile).unwrap();
    fs::write(
        &makefile,
        format!("include $(SRCDIR)/config/aros.cfg\ninclude $(SRCDIR)/source-native.mk\n{content}"),
    )
    .unwrap();
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    let mut checked = 0;
    for line in generated
        .lines()
        .filter(|line| line.trim().starts_with("JSON "))
    {
        let Some(quoted) = line
            .trim()
            .strip_prefix("JSON \"")
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        // Decode the generated CMake quoted argument, not its printed escape
        // syntax. Preserve the inner JSON escapes through exactly one layer.
        let mut decoded = String::new();
        let mut chars = quoted.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let escaped = chars.next().expect("complete CMake escape");
                assert!(matches!(escaped, '\\' | '"' | '$' | ';'));
                decoded.push(escaped);
            } else {
                decoded.push(ch);
            }
        }
        let value: Value = serde_json::from_str(&decoded).unwrap();
        if value.get("defname").is_none() {
            continue;
        }
        assert_eq!(value["kobj_ldflags"]["state"], "known_empty");
        assert_eq!(value["kernel_kobj_ldscript"]["state"], "known_empty");
        assert_eq!(
            value["kernel_kobj_ldscript"]["source"][0]["path"],
            "source-native.mk"
        );
        assert_eq!(value["funcinstr_libs"]["words"], json!(["instrfunc"]));
        assert_eq!(value["function_instrumentation"]["words"], json!(["no"]));
        assert_eq!(
            value["included_configuration_files"][0]["path"],
            "native-config.mk"
        );
        assert_eq!(
            value["included_configuration_files"][1]["path"],
            "source-native.mk"
        );
        checked += 1;
    }
    assert_eq!(checked, 3, "{generated}");

    // A changed source projection fails before graph publication, not merely
    // during a later native link. Remove only this fixture's generated output.
    fs::remove_file(fixture.output()).unwrap();
    fs::write(
        fixture.root.path().join("native-config.mk"),
        "TARGET_FUNCINSTR := yes\n",
    )
    .unwrap();
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "SHA-256",
        "graph_validation",
    );
}

#[test]
fn cmake_forwards_validated_native_identity_to_the_real_transpiler_process() {
    let mut fixture = Fixture::new();
    fixture.contract["make_variables"] = json!({"DEVICE_SET":"timer"});
    fixture.write_contract();
    let makefile = fixture.root.path().join("mmakefile.src");
    let content = fs::read_to_string(&makefile).unwrap();
    fs::write(
        &makefile,
        content.replace("devs=timer", "devs=\"$(DEVICE_SET)\""),
    )
    .unwrap();
    let source = fixture.root.path().canonicalize().unwrap();
    let module = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/NativeBuildContract.cmake")
        .canonicalize()
        .unwrap();
    let script = fixture.root.path().join("invoke.cmake");
    fs::write(
        &script,
        r#"
cmake_minimum_required(VERSION 3.22)
include("${MODULE}")
set(AROS_SOURCE_DIR "${SOURCE}")
set(AROS_NATIVE_BUILD_CONTRACT "${SOURCE}/native.json")
file(SHA256 "${AROS_NATIVE_BUILD_CONTRACT}" AROS_NATIVE_BUILD_CONTRACT_SHA256)
set(AROS_TARGET_PROFILE fixture-native)
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_TRIPLE riscv-aros)
set(AROS_TOOLCHAIN gnu)
set(GCC_CONFIG_FLOAT_ABI ilp32f)
set(AROS_ABI_FLAVOUR standalone)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)
aros_validate_native_build_contract()
aros_native_transpiler_arguments(native_args)
execute_process(COMMAND "${TRANSPILER}" ${native_args}
    --source-dir "${SOURCE}" --output "${OUTPUT}"
    --cpu riscv --platform fixture --family "" --variant "" --cpu32 ""
    --toolchain gnu --use-mmu 0 --float-abi ilp32f --diagnostic-format json
    RESULT_VARIABLE result ERROR_VARIABLE error)
if(NOT result EQUAL 0)
    message(FATAL_ERROR "Native transpiler forwarding failed: ${error}")
endif()
"#,
    )
    .unwrap();
    let result = Command::new("cmake")
        .arg(format!("-DMODULE={}", module.display()))
        .arg(format!("-DSOURCE={}", source.display()))
        .arg(format!("-DOUTPUT={}", fixture.output().display()))
        .arg(format!(
            "-DTRANSPILER={}",
            env!("CARGO_BIN_EXE_aros-transpiler")
        ))
        .arg("-P")
        .arg(script)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(!generated.contains("unrelated-archive"));
    let engine = include_str!("../../aros-cmake-engine/engine/CMakeLists.txt");
    assert_eq!(
        engine.matches("${_aros_native_transpiler_args}").count(),
        3,
        "both inventory calls and replay record must forward the validated selection"
    );
}

#[test]
fn native_absent_virtual_target_is_not_inferred() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : fixture-empty\n");
    assert_failure(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "has no proven endpoint",
    );
}

#[test]
fn native_virtual_empty_cannot_mask_a_nonvirtual_make_provider() {
    for declarations in [
        "#MM- fixture-empty :\n#MM fixture-empty :\n",
        "#MM fixture-empty :\n#MM- fixture-empty :\n",
        "#MM- fixture-empty :\n#MM\nfixture-empty :\n\techo unmodelled\n",
        "#MM- fixture-empty :\n#MM fixture-empty : fixture-helper\nfixture-empty:\n\techo unmodelled\n",
    ] {
        let fixture = Fixture::new();
        fixture.append(&format!(
            "#MM- fixture-kernel : fixture-empty\n{declarations}"
        ));
        assert_failure_at_stage(
            &fixture.invoke(true, &[]),
            &fixture.output(),
            "nonvirtual Make provider",
            "capability_validation",
        );
    }
}

#[test]
fn native_virtual_empty_retains_a_real_concrete_provider() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : fixture-prepare\n#MM- fixture-prepare :\n#MM fixture-prepare :\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/include/proto\n");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fs::read_to_string(fixture.output())
        .unwrap()
        .contains("aros_prepare_directories("));
}

#[test]
fn native_virtual_empty_cannot_mask_a_provider_in_another_mmakefile() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : fixture-empty\n#MM- fixture-empty :\n");
    let other = fixture.root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        "#MM fixture-empty : fixture-helper\nfixture-empty:\n\techo unmodelled\n",
    )
    .unwrap();
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "nonvirtual Make provider",
        "capability_validation",
    );
}

#[test]
fn native_virtual_unresolved_prerequisite_is_not_an_empty_target() {
    let fixture = Fixture::new();
    fixture
        .append("#MM- fixture-kernel : fixture-empty\n#MM- fixture-empty : $(UNPROVEN_TARGET)\n");
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "unresolved prerequisites",
        "capability_validation",
    );
}

#[test]
fn native_concrete_provider_cannot_hide_an_unresolved_virtual_prerequisite() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : fixture-prepare\n#MM- fixture-prepare : $(UNPROVEN_TARGET)\n#MM fixture-prepare :\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/include/proto\n");
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "unresolved prerequisites",
        "capability_validation",
    );
}

#[test]
fn native_unselected_unresolved_meta_rule_does_not_poison_the_slice() {
    let fixture = Fixture::new();
    fixture.append("#MM- unrelated-archive : $(UNPROVEN_TARGET)\n");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn native_nonvirtual_provider_cannot_borrow_a_same_named_concrete_producer() {
    let fixture = Fixture::new();
    fixture.append("#MM- fixture-kernel : fixture-prepare\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/include/proto\n");
    let other = fixture.root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        "#MM fixture-prepare :\nfixture-prepare:\n\techo unmodelled\n",
    )
    .unwrap();
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "declaring mmakefile",
        "capability_validation",
    );
}

#[test]
fn native_directory_preparation_is_a_real_selected_endpoint() {
    let fixture = Fixture::new();
    fixture.append("#MM fixture-kernel : fixture-prepare\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/$(CURDIR)/include/proto\n");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cmake = fs::read_to_string(fixture.output()).unwrap();
    assert!(cmake.contains("aros_prepare_directories("));
    assert!(cmake.contains("${AROS_BUILD_DIR}/gen/include/proto"));
    assert!(!cmake.contains("add_custom_target(fixture-prepare)"));
}

#[test]
fn native_directory_preparation_rejects_an_unmodelled_recipe() {
    let fixture = Fixture::new();
    fixture.append("#MM fixture-kernel : fixture-prepare\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/include/proto\n\techo unexpected\n");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(!fixture.output().exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("fixture-prepare"));
}

#[test]
fn full_tree_keeps_prior_coverage_policy_for_unmodelled_handwritten_recipes() {
    let fixture = Fixture::new();
    fixture.append("#MM fixture-kernel : fixture-prepare\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/include/proto\n\techo unexpected\n");
    let result = fixture.invoke(false, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    // The native model must not turn this into a directory-only producer.
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(!generated.contains("aros_prepare_directories("));
}

#[test]
fn native_directory_preparation_rejects_duplicate_cross_file_producers() {
    let fixture = Fixture::new();
    fixture.append(
        "#MM fixture-kernel : fixture-prepare\nfixture-prepare:\n\t%mkdirs_q $(GENDIR)/first\n",
    );
    let other = fixture.root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        "fixture-prepare:\n\t%mkdirs_q $(GENDIR)/second\n",
    )
    .unwrap();
    let result = fixture.invoke(true, &[]);
    assert_failure(&result, &fixture.output(), "conflicting concrete producers");
}

fn add_genmodule_stamp(fixture: &Fixture) {
    let directory = fixture.root.path().join("api");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("sample.conf"), "##begin config\nversion 1.0\nlibbase SampleBase\n##end config\n##begin functionlist\nvoid Sample(void) (D0)\n##end functionlist\n").unwrap();
    fs::write(directory.join("mmakefile.src"), r#"
#MM- fixture-kernel : fixture-api
#MM fixture-api : fixture-api-prepare
fixture-api: $(GENDIR)/$(CURDIR)/.includes-generated
$(GENDIR)/$(CURDIR)/.includes-generated: $(GENMODULE)
	@$(ECHO) "Generating API headers..."
	@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/sample.conf -d $(GENDIR)/$(CURDIR)/include writeincludes sample resource
	@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/sample.conf -d $(GENDIR)/$(CURDIR)/include writelibdefs sample resource
	@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/sample.conf -d $(GENDIR)/include writeincludes sample resource
	@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/sample.conf -d $(AROS_INCLUDES) writeincludes sample resource
	@$(TOUCH) $@
fixture-api-prepare:
	%mkdirs_q $(GENDIR)/$(CURDIR)/include/proto
"#).unwrap();
}

#[test]
fn native_genmodule_stamp_materializes_a_header_producer_not_an_empty_alias() {
    let fixture = Fixture::new();
    add_genmodule_stamp(&fixture);
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(generated.contains("aros_genmodule_header_stamp("));
    assert!(generated.contains("api/sample.conf"));
    assert!(!generated.contains("add_custom_target(fixture-api)"));
}

#[test]
fn native_genmodule_stamp_rejects_undeclared_commands_before_output() {
    let fixture = Fixture::new();
    add_genmodule_stamp(&fixture);
    let path = fixture.root.path().join("api/mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        original.replace("\t@$(TOUCH) $@", "\t@$(TOUCH) $@\n\trm anything"),
    )
    .unwrap();
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(!fixture.output().exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("fixture-api"));
}

fn add_host_header_rule(fixture: &Fixture) {
    let directory = fixture.root.path().join("headers");
    fs::create_dir(&directory).unwrap();
    for name in ["template.h", "defs.h", "first.h", "second.h"] {
        fs::write(directory.join(name), "/* source-owned fixture */\n").unwrap();
    }
    fs::write(directory.join("emit.c"), "int main(void) { return 0; }\n").unwrap();
    fs::write(
        directory.join("mmakefile.src"),
        r#"
CLASSES := first second
CLASSINCLUDES := $(foreach f, $(CLASSES), $(SRCDIR)/$(CURDIR)/$(f).h)
BUILDINCTOOL := $(GENDIR)/$(CURDIR)/emit
INCLUDEFILES := $(AROS_INCLUDES)/libraries/sample.h $(GENINCDIR)/libraries/sample.h
#MM- fixture-kernel : fixture-generated-header
fixture-generated-header : fixture-header-setup $(INCLUDEFILES)
	@$(NOP)
fixture-header-setup : $(GENINCDIR)/libraries $(AROS_INCLUDES)/libraries
$(GENINCDIR)/libraries $(AROS_INCLUDES)/libraries :
	%mkdir_q dir=$@
$(AROS_INCLUDES)/libraries/sample.h : $(GENINCDIR)/libraries/sample.h
	@$(ECHO) "Copying $< to $@"
	@$(CP) $< $@
$(GENINCDIR)/libraries/sample.h : $(BUILDINCTOOL) template.h defs.h $(CLASSINCLUDES)
	@$(ECHO) Rebuilding $@
	@cd $(SRCDIR)/$(CURDIR); $(BUILDINCTOOL) > $@
$(BUILDINCTOOL) : emit.c
	%mkdirs_q $(dir $(BUILDINCTOOL))
	@$(HOST_CC) $(HOST_CFLAGS) $< -o $@
"#,
    )
    .unwrap();
}

fn seal_native_fixture_input(fixture: &mut Fixture, path: &str) {
    let bytes = fs::read(fixture.root.path().join(path)).unwrap();
    fixture.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": path,
            "sha256": aros_common::sha256_bytes(&bytes),
        }));
}

fn rebind_native_fixture_input(fixture: &mut Fixture, path: &str) {
    let bytes = fs::read(fixture.root.path().join(path)).unwrap();
    let digest = aros_common::sha256_bytes(&bytes);
    let input = fixture.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|input| input["path"] == path)
        .unwrap();
    input["sha256"] = json!(digest);
    fixture.write_contract();
}

fn add_host_c_file_generator_fixture(fixture: &mut Fixture) {
    let generator_inputs = [
        ("UnicodeData.txt", b"unicode data fixture\n".as_slice()),
        ("SpecialCasing.txt", b"special casing fixture\n".as_slice()),
    ];
    fs::create_dir_all(fixture.root.path().join("tools/genctbl")).unwrap();
    fs::write(
        fixture.root.path().join("tools/genctbl/Makefile"),
        r#"USER_CFLAGS := -Wall -Werror -Wunused -O2

-include $(TOP)/config/make.cfg
-include Makefile.deps

HOST_CC ?= gcc
HOST_CFLAGS ?= $(USER_CFLAGS)
GENCTBL ?= genctbl

$(GENCTBL) : genctbl.c $(GENMODULE_DEPS)
	@$(ECHO) "Compiling $(notdir $@)..."
	@$(HOST_CC) -g $(HOST_CFLAGS) -I$(GENINCDIR) -I$(TOP)/$(CURDIR) genctbl.c -o $@
"#,
    )
    .unwrap();
    fs::write(
        fixture.root.path().join("tools/genctbl/genctbl.c"),
        "int main(void) { return 0; }\n",
    )
    .unwrap();
    fs::write(
        fixture.root.path().join("Makefile.in"),
        r"$(GENCTBL): $(SRCDIR)/tools/genctbl/genctbl.c
	@$(ECHO) Building $(notdir $@)...
	@$(CALL) $(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP)
",
    )
    .unwrap();
    fs::write(
        fixture.root.path().join("configure.in"),
        "make_extra_commands=\"$make_extra_commands$export_newline\"\"GENCTBL\t:= $\"\"(TOOLDIR)/genctbl$\"\"(HOST_EXE_SUFFIX)$export_newline\"\n",
    )
    .unwrap();

    fixture.append(
        "#MM- fixture-kernel : fixture-host-c-file-generator\n\
         #MM fixture-host-c-file-generator :\n\
         fixture-host-c-file-generator : $(GENDIR)/$(CURDIR)/defaults/sample.c\n\
         $(GENDIR)/$(CURDIR)/defaults:\n\
         \t%mkdirs_q $@\n\
         $(GENDIR)/ucd:\n\
         \t%mkdirs_q $@\n\
         $(GENDIR)/ucd/%.txt: $(PORTSSOURCEDIR)/%.txt | $(GENDIR)/ucd\n\
         \t@$(CP) $< $@\n\
         $(GENDIR)/$(CURDIR)/defaults/%.c: $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt | $(GENDIR)/$(CURDIR)/defaults\n\
         \t@$(ECHO) \"Generating $*.c\";\n\
         \t@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c;\n",
    );

    let source_inputs: Vec<_> = generator_inputs
        .iter()
        .map(|(name, bytes)| {
            json!({
                "filename": name,
                "url": format!("https://www.unicode.org/Public/17.0.0/ucd/{name}"),
                "sha256": aros_common::sha256_bytes(bytes),
                "size": bytes.len(),
            })
        })
        .collect();
    fixture.contract["host_file_generators"] = json!([{
        "owner": "fixture-host-c-file-generator",
        "recipe": "mmakefile.src",
        "tool_recipe": "tools/genctbl/Makefile",
        "tool_source": "tools/genctbl/genctbl.c",
        "tool_variable": "GENCTBL",
        "output": "gen/defaults/sample.c",
        "input_directory": "gen/ucd",
        "compile_flags": ["-g", "-Wall", "-Werror", "-Wunused", "-O2"],
        "arguments": ["@INPUT_DIRECTORY@", "@OUTPUT_DIRECTORY@", "sample", "--emit-c"],
        "inputs": source_inputs,
    }]);
    for path in [
        "mmakefile.src",
        "tools/genctbl/Makefile",
        "tools/genctbl/genctbl.c",
        "Makefile.in",
        "configure.in",
    ] {
        seal_native_fixture_input(fixture, path);
    }
    fixture.write_contract();
}

#[test]
fn native_host_c_file_generator_exports_sealed_inputs_without_inventing_a_make_target() {
    let mut fixture = Fixture::new();
    add_host_c_file_generator_fixture(&mut fixture);
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(
        generated.matches("aros_host_c_file_generator(\n").count(),
        1
    );
    assert!(generated.contains("NAME \"fixture-host-c-file-generator\""));
    assert!(generated.contains("COMPILE_FLAGS \"-g\" \"-Wall\" \"-Werror\" \"-Wunused\" \"-O2\""));
    assert!(generated.contains(
        "ARGUMENTS \"@INPUT_DIRECTORY@\" \"@OUTPUT_DIRECTORY@\" \"sample\" \"--emit-c\""
    ));
    for (name, bytes) in [
        ("UnicodeData.txt", b"unicode data fixture\n".as_slice()),
        ("SpecialCasing.txt", b"special casing fixture\n".as_slice()),
    ] {
        let digest = aros_common::sha256_bytes(bytes).to_string();
        assert!(generated.contains(name), "{generated}");
        assert!(generated.contains(&digest), "{generated}");
    }
    assert!(generated.contains("${AROS_SOURCE_DIR}/tools/genctbl/genctbl.c"));
    let tool_digest = aros_common::sha256_bytes(
        &fs::read(fixture.root.path().join("tools/genctbl/genctbl.c")).unwrap(),
    );
    assert!(generated.contains(&format!("TOOL_SHA256 \"{tool_digest}\"")));
    assert!(!generated.contains("add_custom_target(\"fixture-host-c-file-generator\")"));
    assert!(!generated.contains("aros_build_target(\"fixture-host-c-file-generator\")"));
}

#[test]
fn native_host_c_file_generator_rejects_changed_invocation_or_input_dependency() {
    for (old, new) in [
        ("$* --emit-c;", "$* --emit-c --extra;"),
        (
            "$(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt",
            "$(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/CaseFolding.txt",
        ),
    ] {
        let mut fixture = Fixture::new();
        add_host_c_file_generator_fixture(&mut fixture);
        let recipe = fixture.root.path().join("mmakefile.src");
        let original = fs::read_to_string(&recipe).unwrap();
        let changed = original.replace(old, new);
        assert_ne!(changed, original, "test mutation did not match fixture");
        fs::write(recipe, changed).unwrap();
        rebind_native_fixture_input(&mut fixture, "mmakefile.src");
        assert_failure_at_stage(
            &fixture.invoke(true, &[]),
            &fixture.output(),
            "source-owned host-C file generator is outside its closed capability",
            "capability_validation",
        );
    }
}

#[test]
fn native_host_c_file_generator_rejects_a_conflicting_concrete_output_producer() {
    let mut fixture = Fixture::new();
    add_host_c_file_generator_fixture(&mut fixture);
    fixture.append(
        "$(GENDIR)/$(CURDIR)/defaults/sample.c: competing-source.c\n\
         \t@$(ECHO) competing producer\n",
    );
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "concrete output also has a separate explicit producer rule",
        "capability_validation",
    );
}

#[test]
fn native_host_c_file_generator_rejects_an_unbound_recipe_before_publication() {
    let mut fixture = Fixture::new();
    add_host_c_file_generator_fixture(&mut fixture);
    fixture.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .retain(|input| input["path"] != "mmakefile.src");
    fixture.write_contract();

    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "is not inventoried",
        "graph_validation",
    );
}

#[test]
fn native_host_header_rule_is_source_owned_with_complete_dependencies() {
    let fixture = Fixture::new();
    add_host_header_rule(&fixture);
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert!(generated.contains("aros_host_header_rule("));
    assert!(generated.contains("headers/first.h"));
    assert!(generated.contains("headers/second.h"));
    assert!(generated.contains("USE_CONFIGURED_HOST_CFLAGS"));
    assert!(!generated.contains("add_custom_target(fixture-generated-header)"));
    assert!(!generated.contains("aros_transform_header("));
}

#[test]
fn native_host_header_rule_refuses_an_unknown_compile_flag_variable() {
    let fixture = Fixture::new();
    add_host_header_rule(&fixture);
    let path = fixture.root.path().join("headers/mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        original.replace("$(HOST_CFLAGS)", "$(HOST_CFLAGS) $(UNPROVEN_FLAGS)"),
    )
    .unwrap();
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(!fixture.output().exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("fixture-generated-header"));
}

fn add_literal_header_copy(fixture: &Fixture, files: &str) {
    let directory = fixture.root.path().join("published/devices");
    fs::create_dir_all(&directory).unwrap();
    for file in ["first.h", "second.h"] {
        fs::write(directory.join(file), "/* named source header */\n").unwrap();
    }
    fs::write(fixture.root.path().join("published/mmakefile.src"), format!(
        "FILES2 := {files}\n#MM- fixture-kernel : fixture-copy\n%copy_files_q mmake=fixture-copy files=$(FILES2) src=devices dst=$(AROS_INCLUDES)/devices\n"
    )).unwrap();
}

#[test]
fn native_literal_file_copy_preserves_exact_files_and_single_destination() {
    let fixture = Fixture::new();
    add_literal_header_copy(&fixture, "first.h second.h");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(generated.matches("aros_transform_header(").count(), 2);
    assert!(generated.contains("${AROS_SOURCE_DIR}/published/devices/first.h"));
    assert!(generated.contains("${AROS_SDK_INCLUDE_DIR}/devices/second.h"));
    assert!(!generated.contains("${AROS_GENINC_DIR}/devices/second.h"));
    assert!(!generated.contains("add_custom_target(fixture-copy)"));
}

#[test]
fn native_literal_file_copy_refuses_traversal_or_empty_file_lists() {
    for (files, expected) in [
        ("../first.h", "unique literal header basename"),
        ("", "file list is empty"),
        ("first.h first.h", "unique literal header basename"),
    ] {
        let fixture = Fixture::new();
        add_literal_header_copy(&fixture, files);
        let result = fixture.invoke(true, &[]);
        assert!(!result.status.success());
        assert!(!fixture.output().exists());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("fixture-copy"), "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[cfg(unix)]
#[test]
fn native_literal_file_copy_refuses_symlinked_source_inputs() {
    let fixture = Fixture::new();
    add_literal_header_copy(&fixture, "first.h");
    let path = fixture.root.path().join("published/devices/first.h");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("second.h", path).unwrap();
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    assert!(!fixture.output().exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
}

fn add_source_value_fixture(fixture: &mut Fixture, tail: &str) {
    fs::create_dir_all(fixture.root.path().join("config")).unwrap();
    fs::write(fixture.root.path().join("config/make.cfg.in"),
        "AROS_DIR_PREFS := Prefs\nAROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\nAROS_PREFS := $(AROSDIR)/$(AROS_DIR_PREFS)\nAROS_DEVELOPER := $(AROSDIR)/Developer\nAROS_LIB := $(AROS_DEVELOPER)/lib\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n").unwrap();
    fs::write(
        fixture.root.path().join("value.in"),
        "#define SOURCE_VERSION  -7 /* source-owned */\n",
    )
    .unwrap();
    fixture.append(&format!(r"#MM- fixture-kernel : fixture-source-value
#MM
fixture-source-value :
	@$(MKDIR) $(AROS_PREFS)/Env-Archive
	@$(SED) -n 's/#define SOURCE_VERSION// p' < $(SRCDIR)/value.in | $(SED) -n 's/^ *//;s/[ */*].*//p' > $(AROS_PREFS)/Env-Archive/Value
{tail}"));
    for path in ["config/make.cfg.in", "value.in", "mmakefile.src"] {
        fixture.contract["inputs"].as_array_mut().unwrap().push(json!({
            "path":path, "sha256":aros_common::sha256_bytes(&fs::read(fixture.root.path().join(path)).unwrap())
        }));
    }
    fixture.write_contract();
}

#[test]
fn native_source_value_has_a_real_producer_and_exact_recipe_snapshot() {
    let mut fixture = Fixture::new();
    add_source_value_fixture(&mut fixture, "");
    let output = fixture.invoke(true, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(generated.matches("aros_extract_source_value(").count(), 1);
    assert!(
        generated.contains("NAME \"fixture-source-value\""),
        "{generated}"
    );
    assert!(generated.contains("${AROS_BUILD_DIR}/SYS/Prefs/Env-Archive/Value"));
    let source_sha =
        aros_common::sha256_bytes(&fs::read(fixture.root.path().join("mmakefile.src")).unwrap());
    assert!(
        generated.contains(&format!("FILE_SHA256 \"{source_sha}\"")),
        "{generated}"
    );
    assert!(
        !generated.contains("OUTPUT_VALUE -7"),
        "source value must not be hardcoded"
    );
}

#[test]
fn native_source_value_rejects_extra_recipe_commands_before_graph_publication() {
    let mut fixture = Fixture::new();
    add_source_value_fixture(&mut fixture, "\t@touch outside");
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "source value rule is outside its closed capability",
        "capability_validation",
    );
}

struct Fixture {
    root: tempfile::TempDir,
    contract: Value,
}

fn add_resource_abi_fixture(fixture: &Fixture, macro_form: &str, with_config: bool) {
    let path = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace(
            "%build_module_simple mmake=fixture-kernel",
            &format!("%{macro_form} mmake=fixture-kernel"),
        ),
    )
    .unwrap();
    if with_config {
        fs::write(fixture.root.path().join("kernel.conf"),
            "##begin config\nversion 1.0\noptions noautoinit\n##end config\n##begin functionlist\nvoid Fixture() ()\n##end functionlist\n").unwrap();
    }
    fixture.append(
        "#MM- fixture-kernel : fixture-kernel-includes\n\
        #MM fixture-kernel-includes :\n\
        #MM- core-linklibs :\n\
        #MM- includes-generate-deps :\n",
    );
}

#[test]
fn native_full_resource_abi_is_source_proven_and_registered_before_its_builder() {
    let fixture = Fixture::new();
    add_resource_abi_fixture(&fixture, "build_module", true);
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    let registration = generated
        .find("aros_set_module_abi(\"fixture-kernel\")")
        .unwrap();
    assert!(registration < generated.find("aros_add_resource(").unwrap());
    assert!(!generated.contains("add_custom_target(\"fixture-kernel-includes\")"));
}

#[test]
fn native_runtime_only_resource_cannot_borrow_an_abi_provider() {
    let fixture = Fixture::new();
    add_resource_abi_fixture(&fixture, "build_module_library", true);
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "nonvirtual Make provider fixture-kernel-includes",
        "capability_validation",
    );
}

#[test]
fn native_missing_resource_config_cannot_claim_an_abi_provider() {
    let fixture = Fixture::new();
    add_resource_abi_fixture(&fixture, "build_module", false);
    assert_failure_at_stage(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "nonvirtual Make provider fixture-kernel-includes",
        "capability_validation",
    );
}

const fn static_header_recipe() -> &'static str {
    "HEADERS := $(call WILDCARD, *.h sub/*.h)\n\
     DEST_HEADERS := $(foreach f,$(HEADERS),$(AROS_INCLUDES)/$(f))\n\
     #MM- fixture-kernel : fixture-static-headers\n\
     #MM fixture-static-headers : fixture-static-setup\n\
     fixture-static-headers : $(DEST_HEADERS)\n\
     fixture-static-setup :\n\
     \t%mkdirs_q $(AROS_INCLUDES) $(AROS_INCLUDES)/sub\n\
     $(DEST_HEADERS) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\
     \t@$(CP) $< $@\n"
}

fn add_static_header_inputs(fixture: &Fixture) {
    fs::create_dir(fixture.root.path().join("sub")).unwrap();
    fs::write(fixture.root.path().join("first.h"), b"first\r\n\0\xff").unwrap();
    fs::write(fixture.root.path().join("sub/second.h"), b"second\n").unwrap();
}

#[test]
fn native_static_header_copy_selects_exact_nested_files_and_setup() {
    let fixture = Fixture::new();
    add_static_header_inputs(&fixture);
    fixture.append(static_header_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(
        generated.matches("aros_transform_header(").count(),
        2,
        "{generated}"
    );
    assert!(generated.contains("fixture-static-setup"), "{generated}");
    assert!(
        generated.contains("${AROS_SDK_INCLUDE_DIR}/sub/second.h"),
        "{generated}"
    );
    assert!(
        !generated.contains("${AROS_GENINC_DIR}/sub/second.h"),
        "{generated}"
    );
    assert!(
        !generated.contains("add_custom_target(\"fixture-static-headers\")"),
        "{generated}"
    );
}

#[test]
fn native_static_header_copy_refuses_unsafe_or_ambiguous_rules_before_publication() {
    for text in [
        static_header_recipe().replace("@$(CP) $< $@", "@$(CP) $< $@\n\t@$(ECHO) ignored"),
        static_header_recipe().replace("$(SRCDIR)/$(CURDIR)/%", "$(SRCDIR)/../%"),
        static_header_recipe().replace("$(AROS_INCLUDES)/%", "$(AROS_INCLUDES)/extra/%"),
        static_header_recipe().replace("*.h sub/*.h", "first.h first.h"),
        static_header_recipe().replace(
            "fixture-static-headers : $(DEST_HEADERS)",
            "fixture-static-headers : $(DEST_HEADERS)\nother-owner : $(DEST_HEADERS)",
        ),
        static_header_recipe().replace("\t@$(CP) $< $@", "\t@$(CP) $< $@ ; ignored"),
    ] {
        let fixture = Fixture::new();
        add_static_header_inputs(&fixture);
        fixture.append(&text);
        assert_failure_at_stage(
            &fixture.invoke(true, &[]),
            &fixture.output(),
            "static header copy",
            "capability_validation",
        );
    }
}

#[test]
fn native_static_header_copy_generated_cmake_preserves_single_root_and_bytes() {
    use std::fmt::Write as _;
    let fixture = Fixture::new();
    add_static_header_inputs(&fixture);
    fixture.append(static_header_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    let mut invocations = String::new();
    for block in generated.split("aros_transform_header(").skip(1) {
        let (arguments, _) = block.split_once(")\n\n").unwrap();
        writeln!(invocations, "aros_transform_header({arguments})").unwrap();
    }
    assert_eq!(invocations.matches("aros_transform_header(").count(), 2);
    let directory_block = generated
        .split_once("aros_prepare_directories(")
        .expect("source-owned directory preparation")
        .1;
    let (directory_arguments, _) = directory_block.split_once(")\n\n").unwrap();
    writeln!(
        invocations,
        "aros_prepare_directories({directory_arguments})"
    )
    .unwrap();
    writeln!(
        invocations,
        "add_dependencies(fixture-static-headers fixture-static-setup)"
    )
    .unwrap();
    let project = fixture.root.path().join("copy project");
    let build = fixture.root.path().join("copy build");
    fs::create_dir(&project).unwrap();
    fs::create_dir_all(fixture.root.path().join("compiler/startup")).unwrap();
    fs::write(
        fixture.root.path().join("compiler/startup/startup.c"),
        "int fixture_startup(void) { return 0; }\n",
    )
    .unwrap();
    fs::write(
        fixture.root.path().join("compiler/startup/detach.c"),
        "int fixture_detach(void) { return 0; }\n",
    )
    .unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aros-cmake-engine/engine");
    fs::write(
        project.join("CMakeLists.txt"),
        format!(
            r#"
cmake_minimum_required(VERSION 3.22)
project(static_header_copy C)
set(_stub "${{CMAKE_BINARY_DIR}}/stub")
file(MAKE_DIRECTORY "${{_stub}}")
file(WRITE "${{_stub}}/BootstrapSDK.cmake" "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(PREPEND CMAKE_MODULE_PATH "${{_stub}}")
set(AROS_SOURCE_DIR "{}")
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM fixture)
include("{}/AROS.cmake")
{invocations}
"#,
            fixture.root.path().display(),
            engine.display()
        ),
    )
    .unwrap();
    let configure = Command::new("cmake")
        .arg("-S")
        .arg(&project)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .unwrap();
    assert!(
        configure.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&configure.stdout),
        String::from_utf8_lossy(&configure.stderr)
    );
    let build_headers = || {
        let result = Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--target", "fixture-static-headers"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        result
    };
    build_headers();
    for path in ["first.h", "sub/second.h"] {
        assert_eq!(
            fs::read(build.join("SDK/include").join(path)).unwrap(),
            fs::read(fixture.root.path().join(path)).unwrap()
        );
        assert!(!build.join("GENINCDIR").join(path).exists());
    }
    assert!(build.join("SDK/include/sub").is_dir());
    assert!(!build.join("SYS/Developer/include/sub").exists());
    let before = ["first.h", "sub/second.h"].map(|path| {
        fs::metadata(build.join("SDK/include").join(path))
            .unwrap()
            .modified()
            .unwrap()
    });
    // The source-owned directory setup is an idempotent phony target. It
    // runs again, but must not retrigger or rewrite either copy output.
    build_headers();
    let after = ["first.h", "sub/second.h"].map(|path| {
        fs::metadata(build.join("SDK/include").join(path))
            .unwrap()
            .modified()
            .unwrap()
    });
    assert_eq!(before, after);
    let output = build.join("SDK/include/sub/second.h");
    fs::remove_file(&output).unwrap();
    build_headers();
    assert_eq!(fs::read(&output).unwrap(), b"second\n");
    fs::write(
        fixture.root.path().join("sub/second.h"),
        b"changed\0\xff\r\n",
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(fixture.root.path().join("sub/second.h"))
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2)),
        )
        .unwrap();
    build_headers();
    assert_eq!(fs::read(&output).unwrap(), b"changed\0\xff\r\n");
}

const fn host_aggregate_recipe() -> &'static str {
    "#MM- fixture-kernel : fixture-header-aggregate\n\
     #MM fixture-header-aggregate\n\
     fixture-header-aggregate: $(AROS_INCLUDES)/fixture/first.h $(AROS_INCLUDES)/fixture/second.h $(GENINCDIR)/fixture/second.h\n\
     $(AROS_INCLUDES)/fixture/first.h: $(HOSTGENDIR)/tools/fixture_header_generator | $(AROS_INCLUDES)/fixture\n\
     \t$(HOSTGENDIR)/tools/fixture_header_generator first >$@\n\
     $(AROS_INCLUDES)/fixture/second.h: $(HOSTGENDIR)/tools/fixture_header_generator | $(AROS_INCLUDES)/fixture\n\
     \t$(HOSTGENDIR)/tools/fixture_header_generator second >$@\n\
     $(GENINCDIR)/fixture/second.h: $(AROS_INCLUDES)/fixture/second.h | $(GENINCDIR)/fixture\n\
     \t$(CP) $< $@\n\
     $(HOSTGENDIR)/tools/fixture_header_generator: $(SRCDIR)/$(CURDIR)/header_generator.c\n\
     \t@$(HOST_CC) -Wall -Werror -o $@ $<\n"
}

fn add_host_aggregate_source(fixture: &Fixture) {
    fs::write(fixture.root.path().join("header_generator.c"),
        "#include <stdio.h>\nint main(int argc, char **argv) { if (argc != 2) return 1; puts(argv[1]); return 0; }\n").unwrap();
}

#[test]
fn native_named_host_header_aggregate_retains_both_leaves_and_exact_mirror_policy() {
    let fixture = Fixture::new();
    add_host_aggregate_source(&fixture);
    fixture.append(host_aggregate_recipe());
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    assert_eq!(
        generated.matches("aros_host_header_aggregate(\n").count(),
        1
    );
    assert_eq!(
        generated
            .matches("aros_host_header_aggregate_output(\n")
            .count(),
        2
    );
    assert_eq!(generated.matches("    GENERATED_MIRROR\n").count(), 1);
    assert!(!generated.contains("aros_host_generated_header("));
    assert!(!generated.contains("add_custom_target(\"fixture-header-aggregate\")"));
}

#[test]
fn native_host_header_aggregate_rejects_partial_or_unmodelled_producers_before_output() {
    for recipe in [
        host_aggregate_recipe().replace("first >$@", "first >$@\n\t@echo unmodelled"),
        host_aggregate_recipe().replace("$(HOST_CC) -Wall -Werror", "$(HOST_CC) $(UNKNOWN_FLAGS)"),
        host_aggregate_recipe().replace("/header_generator.c", "/../header_generator.c"),
        host_aggregate_recipe().replace(
            "fixture/first.h $(AROS_INCLUDES)",
            "fixture/first.h $(AROS_INCLUDES)/fixture/missing.h $(AROS_INCLUDES)",
        ),
        host_aggregate_recipe().replace("\t$(CP) $< $@", "\t$(CP) $< $@\n\t@echo unmodelled"),
        format!(
            "AROS_DIR_INCLUDE := redirected\n{}",
            host_aggregate_recipe()
        ),
    ] {
        let fixture = Fixture::new();
        add_host_aggregate_source(&fixture);
        fixture.append(&recipe);
        let result = fixture.invoke(true, &[]);
        assert!(
            !result.status.success(),
            "accepted invalid aggregate: {recipe}"
        );
        assert!(
            !fixture.output().exists(),
            "published partial graph: {recipe}"
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("fixture-header-aggregate"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

impl Fixture {
    fn new() -> Self {
        // macOS exposes its temporary directory through /var -> /private/var.
        // Native projection deliberately rejects aliases in source input paths.
        let temporary_root = std::env::temp_dir().canonicalize().unwrap();
        let root = tempfile::tempdir_in(temporary_root).unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/make.cfg.in"),
            "AROS_DIR_INCLUDE := include\nAROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nGENINCDIR := $(GENDIR)/include\nHOSTGENDIR := $(HOSTDIR)/gen/host\nAROS_LIB := $(AROS_DEVELOPER)/lib\n",
        )
        .unwrap();
        let input = b"native selection fixture\n";
        fs::write(root.path().join("input.txt"), input).unwrap();
        let mut contract: Value = serde_json::from_str(include_str!(
            "../../aros-cmake-engine/engine/tests/native-build-contract/source/native-build-v1.json"
        ))
        .unwrap();
        contract["profile"] = json!("fixture-native");
        contract["board"] = json!("fixture-board");
        contract["inputs"] =
            json!([{"path": "input.txt", "sha256": aros_common::sha256_bytes(input)}]);
        for field in ["recipe", "linker_script", "residency_check"] {
            contract["core"][field] = json!("input.txt");
        }
        contract["core"]["resources"] = json!(["kernel"]);
        contract["core"]["libraries"] = json!(["exec"]);
        contract["core"]["devices"] = json!(["timer"]);
        contract["core"]["link_libraries"] = json!(["helper"]);
        contract["package"]["recipe"] = json!("input.txt");
        contract["package"]["target"] = json!("fixture-package");
        for field in [
            "board_rules",
            "partition_table",
            "bootloader_configuration",
            "bootloader_patch",
        ] {
            contract["media"][field] = json!("input.txt");
        }
        fs::write(
            root.path().join("aros-targets.toml"),
            r#"
[[targets]]
name = "fixture-native"
arch = "riscv32"
platform = "fixture"
bsp = "fixture-board"
float_abi = "ilp32f"
native_build_contract = "native.json"
[targets.transpiler]
family = ""
variant = ""
toolchain = "gnu"
cpu32 = ""
use_mmu = false
[targets.bootstrap_abi]
flavour = "standalone"
platform_smp = false
"#,
        )
        .unwrap();
        fs::write(
            root.path().join("probe.c"),
            "int fixture(void) { return 0; }\n",
        )
        .unwrap();
        fs::write(
            root.path().join("mmakefile.src"),
            r"
%build_module_simple mmake=fixture-kernel modname=kernel modtype=resource files=probe
%build_module_simple mmake=fixture-exec modname=exec modtype=library files=probe
%build_module_simple mmake=fixture-timer modname=timer modtype=device files=probe
%build_linklib mmake=fixture-helper libname=helper files=probe
%build_linklib mmake=unrelated-archive libname=unrelated files=probe
%make_package mmake=fixture-package file=$(AROS_BOOT)/fixture.pkg res=kernel libs=exec devs=timer
",
        )
        .unwrap();
        let fixture = Self { root, contract };
        fixture.write_contract();
        fixture
    }

    fn write_contract(&self) {
        fs::write(
            self.root.path().join("native.json"),
            serde_json::to_vec_pretty(&self.contract).unwrap(),
        )
        .unwrap();
    }

    fn append(&self, text: &str) {
        let path = self.root.path().join("mmakefile.src");
        let existing = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("{existing}\n{text}\n")).unwrap();
    }

    fn rejected_capability(&self, declaration: &str) {
        // This path is an existing closed external-CMake capability. An
        // arbitrary unknown macro elsewhere is only a skipped declaration,
        // which would not prove rejection of a capability diagnostic.
        let directory = self.root.path().join("compiler/cunit");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("mmakefile.src"), declaration).unwrap();
    }

    fn invoke(&self, native: bool, overrides: &[&str]) -> Output {
        self.invoke_at(native, overrides, &self.output())
    }

    fn invoke_at(&self, native: bool, overrides: &[&str], output: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"));
        command
            .arg("--source-dir")
            .arg(self.root.path())
            .arg("--output")
            .arg(output)
            .args([
                "--diagnostic-format",
                "json",
                "--cpu",
                "riscv",
                "--platform",
                "fixture",
                "--family",
                "",
                "--variant",
                "",
                "--toolchain",
                "gnu",
                "--cpu32",
                "",
                "--use-mmu",
                "0",
                "--float-abi",
                "ilp32f",
            ]);
        if native {
            command.args(["--native-profile", "fixture-native"]);
        }
        command.args(overrides).output().unwrap()
    }

    fn output(&self) -> std::path::PathBuf {
        self.root.path().join("generated.cmake")
    }
}

fn assert_failure(result: &Output, output: &Path, message: &str) {
    assert_failure_at_stage(result, output, message, "graph_validation");
}

fn assert_failure_at_stage(result: &Output, output: &Path, message: &str, stage: &str) {
    assert!(
        !result.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let diagnostic: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"][0]["stage"], stage, "{diagnostic}");
    assert!(
        diagnostic["diagnostics"].to_string().contains(message),
        "{diagnostic}"
    );
    assert!(
        !output.exists(),
        "a failed selection must not publish generated CMake"
    );
}

#[test]
fn native_selection_publishes_only_a_proven_contract_closure() {
    let fixture = Fixture::new();
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    for owner in [
        "fixture-kernel",
        "fixture-exec",
        "fixture-timer",
        "fixture-helper",
        "fixture-package",
        "native-contract-selection",
    ] {
        assert!(generated.contains(owner), "missing {owner}: {generated}");
    }
    assert!(!generated.contains("unrelated-archive"), "{generated}");
}

#[test]
fn native_selection_rejects_missing_edges_with_a_source_dependency_path() {
    let fixture = Fixture::new();
    fixture.append("#MM fixture-kernel : missing-header-producer");
    assert_failure(
        &fixture.invoke(true, &[]),
        &fixture.output(),
        "fixture-kernel -> missing-header-producer",
    );
}

#[test]
fn native_selection_requires_an_explicit_header_staging_producer_for_selected_edges() {
    let fixture = Fixture::new();
    let include_dir = fixture.root.path().join("include");
    fs::create_dir_all(&include_dir).unwrap();
    fs::write(include_dir.join("fdt.h"), "#define FIXTURE_FDT 1\n").unwrap();
    fixture.append(
        "#MM fixture-kernel : fixture-fdt-includes\n\
         %copy_includes mmake=fixture-fdt-includes includes=\"fdt.h\" path=. dir=include",
    );

    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let generated = fs::read_to_string(fixture.output()).unwrap();
    let staging = generated
        .lines()
        .find(|line| line.contains("aros_copy_includes("))
        .expect("generated header staging declaration");
    assert!(
        staging.contains("NAME \"fixture-fdt-includes\""),
        "{staging}"
    );
    assert!(staging.contains("PATTERNS \"fdt.h\""), "{staging}");

    let old_fixture = Fixture::new();
    let include_dir = old_fixture.root.path().join("include");
    fs::create_dir_all(&include_dir).unwrap();
    fs::write(include_dir.join("fdt.h"), "#define FIXTURE_FDT 1\n").unwrap();
    old_fixture.append(
        "#MM fixture-kernel : fixture-fdt-includes\n\
         %copy_includes path=. dir=include",
    );
    assert_failure(
        &old_fixture.invoke(true, &[]),
        &old_fixture.output(),
        "fixture-kernel -> fixture-fdt-includes",
    );
}

#[test]
fn native_selection_preserves_named_header_copy_in_the_global_include_route() {
    for consumer in ["fixture-gl-includes", "includes-copy"] {
        let fixture = Fixture::new();
        fs::write(fixture.root.path().join("gla.h"), "#define FIXTURE_GL 1\n").unwrap();
        fixture.append(&format!(
            "#MM- fixture-kernel : {consumer}\n\
             #MM- fixture-gl-includes : fixture-gl-includes-copy\n\
             #MM- includes-copy : fixture-gl-includes-copy\n\
             %copy_includes mmake=fixture-gl-includes-copy path=GL includes=gla.h\n"
        ));
        let result = fixture.invoke(true, &[]);
        assert!(
            result.status.success(),
            "{consumer}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let generated = fs::read_to_string(fixture.output()).unwrap();
        let staging = generated
            .lines()
            .find(|line| line.contains("aros_copy_includes("))
            .expect("both routes must select the real header copier");
        assert!(
            staging.contains("NAME \"fixture-gl-includes-copy\""),
            "{staging}"
        );
        assert!(staging.contains("PATTERNS \"gla.h\""), "{staging}");
        assert!(staging.contains("GL"), "{staging}");
    }
}

#[test]
fn native_selection_binds_contract_bytes_and_actual_source_inputs() {
    let fixture = Fixture::new();
    assert_failure(
        &fixture.invoke(
            true,
            &[
                "--native-contract-sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ],
        ),
        &fixture.output(),
        "digest differs",
    );
    fs::write(fixture.root.path().join("input.txt"), "altered input\n").unwrap();
    assert_failure(&fixture.invoke(true, &[]), &fixture.output(), "digest");
}

#[test]
fn native_selection_requires_a_concrete_source_owned_writefiles_producer() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("client.conf"),
        "##begin config\nversion 1.0\noptions rellinklib\n##end config\n##begin functionlist\nvoid Probe(void)\n##end functionlist\n").unwrap();
    fixture.append(
        "\
#MM fixture-client-stubs :\n\
#MM- fixture-kernel : fixture-client-stubs\n\
#MM\n\
fixture-client-stubs: $(GENDIR)/$(CURDIR)/.stubs-generated\n\
$(GENDIR)/$(CURDIR)/.stubs-generated :\n\
\t@$(ECHO) \"Generating client stubs...\"\n\
\t@$(GENMODULE) -c $(SRCDIR)/client.conf -d $(GENDIR)/$(CURDIR) writefiles client library\n\
\t@$(TOUCH) $@\n",
    );
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let graph = fs::read_to_string(fixture.output()).unwrap();
    assert!(
        graph.contains("aros_genmodule_writefiles_stamp("),
        "{graph}"
    );
    assert!(graph.contains("NAME \"fixture-client-stubs\""), "{graph}");
    assert!(graph.contains("CONFIG \"client.conf\""), "{graph}");

    let recipe = fixture.root.path().join("mmakefile.src");
    let original = fs::read_to_string(&recipe).unwrap();
    fs::write(
        &recipe,
        original.replace(
            "writefiles client library",
            "writefiles client library ; touch unowned",
        ),
    )
    .unwrap();
    let bad = fixture.invoke(true, &[]);
    assert!(!bad.status.success(), "unsafe recipe was admitted");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("fixture-client-stubs"));
    assert_eq!(
        fs::read_to_string(fixture.output()).unwrap(),
        graph,
        "a failed translation must preserve the previous good graph"
    );
}

#[test]
fn selected_capability_failure_is_fatal_but_unrelated_failure_does_not_poison_the_slice() {
    let fixture = Fixture::new();
    fixture.rejected_capability("%build_with_cmake mmake=unsupported-cmake");
    let result = fixture.invoke(true, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fs::read_to_string(fixture.output())
        .unwrap()
        .contains("unsupported-cmake"));
    fs::remove_file(fixture.output()).unwrap();
    fixture.append("#MM fixture-kernel : unsupported-cmake");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    let diagnostic: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert!(
        diagnostic["diagnostics"]
            .to_string()
            .contains("unsupported-cmake"),
        "{diagnostic}"
    );
    assert_eq!(
        diagnostic["diagnostics"][0]["stage"], "capability_validation",
        "{diagnostic}"
    );
    assert!(!fixture.output().exists());
}

#[test]
fn unselected_mode_retains_the_full_tree_failure_contract() {
    let fixture = Fixture::new();
    fixture.rejected_capability("%build_with_cmake mmake=unsupported-cmake");
    let result = fixture.invoke(false, &[]);
    assert!(!result.status.success());
    let diagnostic: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert!(
        diagnostic["diagnostics"]
            .to_string()
            .contains("unsupported-cmake"),
        "{diagnostic}"
    );
    assert!(!fixture.output().exists());
}

#[test]
fn unowned_capability_failure_cannot_be_hidden_by_native_selection() {
    let fixture = Fixture::new();
    fixture.rejected_capability("%build_with_cmake");
    let result = fixture.invoke(true, &[]);
    assert!(!result.status.success());
    let diagnostic: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(
        diagnostic["diagnostics"][0]["stage"], "capability_validation",
        "{diagnostic}"
    );
    assert!(!fixture.output().exists());
}
