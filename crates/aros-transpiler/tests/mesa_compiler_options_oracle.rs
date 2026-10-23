//! Explicit local configured-source option comparison, not SDK qualification.

use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas,
    mesa_configuration::ValidatedMesaConfiguration, run_output_with_control, sha256_bytes,
    CancellationToken,
};
use aros_transpiler::{
    capability::mesa::ordered_contract::evaluate_ordered_mesa_compiler_options, dirs::DirVars,
    make_vars::collect_vars_with_context, parser::join_continuations, MakeExprContext,
    TargetContext,
};
use std::{
    collections::BTreeMap, fmt::Write as _, fs, path::Path, process::Command, time::Duration,
};

#[path = "support/mesa_compile_projection.rs"]
mod mesa_compile_projection;

const VC4: &str = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium";
const GCA: &str = "GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)";
const MACRO: &str = "%build_linklib mmake=linklibs-gallium_vc4 libname=gallium_vc4 \\\n    libdir=$(top_libdir) objdir=$(top_builddir)/$(CUR_MESADIR)/$(CUR_GALLIUMDIR) \\\n    files=\"$(VC4_DRIVER_SOURCES)\"";
const KEYS: &[(&str, &str)] = &[
    ("C", "$(CFLAGS) $(CPPFLAGS)"),
    ("CXX", "$(CXXFLAGS) $(CPPFLAGS)"),
    (
        "BRIDGE",
        "$(CFLAGS) $(USER_CFLAGS) $(CPPFLAGS) $(USER_CPPFLAGS) $(USER_INCLUDES)",
    ),
    ("GCA", "$(GCA_ABI_FLAGS)"),
    ("GENDIR", "$(GENDIR)"),
    ("PORTSDIR", "$(PORTSDIR)"),
];

#[test]
#[ignore = "requires explicit retained real source/preflight and fresh oracle output"]
fn actual_mesa_compiler_options_match_make_and_configuration_counters() {
    let case_path = std::env::var_os("AROS_TEST_MESA_COMPILER_ORACLE").unwrap();
    let case: serde_json::Value = serde_json::from_slice(&fs::read(case_path).unwrap()).unwrap();
    let preflight = Path::new(case["preflight_root"].as_str().unwrap());
    let output = Path::new(case["output_root"].as_str().unwrap());
    assert!(preflight.is_absolute() && output.is_absolute());
    fs::create_dir(output).unwrap();
    let input: serde_json::Value =
        serde_json::from_slice(&fs::read(preflight.join("case.json")).unwrap()).unwrap();
    let source = Path::new(input["source_root"].as_str().unwrap());
    let compiler = Path::new(input["compiler_root"].as_str().unwrap());
    let run = fs::canonicalize(preflight.join("run")).unwrap();
    let build = fs::canonicalize(run.join("configure")).unwrap();
    // The case pins the producer receipt independently of the JSON being
    // consumed. This is retained local evidence, not publisher authentication.
    let run_before = measure_tree_content_cas(&run).unwrap();
    let receipt_bytes = pinned_document(
        &run.join("receipt.json"),
        case["receipt_sha256"].as_str().unwrap(),
    )
    .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&receipt_bytes).unwrap();
    assert_eq!(receipt["schema"], "aros-mesa-preflight-v3");
    assert_eq!(receipt["qualification"], "local-configure-only");
    let bytes = pinned_document(
        &run.join("mesa-configuration.json"),
        receipt["configuration_sha256"].as_str().unwrap(),
    )
    .unwrap();
    let identity = serde_json::from_value(receipt["identity"].clone()).unwrap();
    let configuration = ValidatedMesaConfiguration::parse_bound(&bytes, &identity).unwrap();
    let generated = Path::new(receipt["configured_paths"]["GENDIR"].as_str().unwrap());
    assert!(generated.is_absolute() && generated.starts_with(&build));
    assert_eq!(fs::canonicalize(generated).unwrap(), generated);
    pinned_document(
        &generated.join("config/compiler.cfg"),
        receipt["compiler_cfg_sha256"].as_str().unwrap(),
    )
    .unwrap();
    let measured: BTreeMap<String, String> =
        serde_json::from_value(receipt["make_values"].clone()).unwrap();
    for (key, value) in configuration.make_variables() {
        assert_eq!(measured.get(&key), Some(&value), "receipt/{key}");
    }
    let source_before = measure_tree_content_cas(source).unwrap();
    let compiler_before = measure_tree_content_cas(compiler).unwrap();
    assert_eq!(
        source_before.payload_digest_excluding(None),
        configuration.identity().source_inputs_sha256
    );
    assert_eq!(
        compiler_before.payload_digest_excluding(None),
        configuration.identity().compiler_tree_sha256
    );
    let mesa = fs::read_to_string(source.join("workbench/libs/mesa/mesa.cfg")).unwrap();
    let vc4 = fs::read_to_string(source.join(VC4).join("mmakefile.src")).unwrap();
    assert_eq!(vc4.matches(GCA).count(), 1);
    assert_eq!(vc4.matches(MACRO).count(), 1);
    let prefix = format!("{}{GCA}\n", vc4.split_once(GCA).unwrap().0);
    let generated = fs::read_to_string(build.join("Makefile")).unwrap();
    let include = "include $(TOP)/config/make.cfg\n";
    assert_eq!(generated.matches(include).count(), 1);
    let header = format!("{}{include}", generated.split_once(include).unwrap().0);
    let mut make = format!("{header}\nCURDIR := {VC4}\n{}\n", prefix.replace(MACRO, ""));
    for (key, expression) in KEYS {
        writeln!(make, "$(info {key}=$(strip {expression}))").unwrap();
    }
    make.push_str(".PHONY: measured-inputs\nmeasured-inputs:\n");
    let make_path = output.join("measure.mk");
    fs::write(&make_path, &make).unwrap();

    let original = make_values(preflight, &build, &make_path, output, "original", &[]);
    check_configured_paths(&original, &receipt).unwrap();
    let c_counter = "-DCOUNTER=1 -UCOUNTER -DCOUNTER=1";
    let changed = make_values(
        preflight,
        &build,
        &make_path,
        output,
        "changed-c",
        &[format!("CONFIG_CFLAGS={c_counter}")],
    );
    assert_ne!(original["C"], changed["C"]);
    assert_ne!(original["BRIDGE"], changed["BRIDGE"]);
    assert_eq!(original["CXX"], changed["CXX"]);
    assert_eq!(original["GCA"], changed["GCA"]);
    let cxx_counter = "-DCXX_ONLY=1";
    let changed_cxx = make_values(
        preflight,
        &build,
        &make_path,
        output,
        "changed-cxx",
        &[format!("TARGET_ISA_CXXFLAGS={cxx_counter}")],
    );
    check_configured_paths(&changed, &receipt).unwrap();
    check_configured_paths(&changed_cxx, &receipt).unwrap();
    assert_ne!(original["CXX"], changed_cxx["CXX"]);
    assert_eq!(original["C"], changed_cxx["C"]);
    assert_eq!(original["GCA"], changed_cxx["GCA"]);

    // Bind the six actual source recursive definitions, not globally expanded
    // TARGET_CFLAGS values that would erase later USER assignments.
    let mut definitions = String::new();
    for (file, names) in [
        (
            "config/target.cfg.in",
            &["TARGET_CFLAGS", "TARGET_CXXFLAGS", "TARGET_CPPFLAGS"][..],
        ),
        (
            "config/make.cfg.in",
            &["CFLAGS", "CXXFLAGS", "CPPFLAGS"][..],
        ),
    ] {
        // Older source templates have Latin-1 copyright comments. Decode only
        // for line selection; every selected executable formula must be ASCII
        // so replacement characters can never change an evaluated option.
        let template_bytes = fs::read(source.join(file)).unwrap();
        let text = String::from_utf8_lossy(&template_bytes);
        for name in names {
            let lines = text
                .lines()
                .filter(|line| {
                    line.split_once('=')
                        .is_some_and(|(left, _)| left.trim() == *name)
                })
                .collect::<Vec<_>>();
            assert_eq!(lines.len(), 1, "ambiguous recursive definition {name}");
            assert!(lines[0].is_ascii(), "non-ASCII compiler formula {name}");
            definitions.push_str(lines[0]);
            definitions.push('\n');
        }
    }
    let joined = join_continuations(&format!("{definitions}\n{mesa}\n{prefix}"));
    let declaration = joined
        .lines()
        .position(|line| line.starts_with("%build_linklib "))
        .unwrap();
    let assignment = joined.lines().position(|line| line == GCA).unwrap();
    let paths = BTreeMap::from([
        ("SRCDIR".to_owned(), source.display().to_string()),
        ("CURDIR".to_owned(), VC4.into()),
        ("GENDIR".to_owned(), original["GENDIR"].clone()),
        ("PORTSDIR".to_owned(), original["PORTSDIR"].clone()),
    ]);
    let cpu = configuration
        .identity()
        .target_triple
        .split_once('-')
        .unwrap()
        .0;
    for (label, oracle, changes) in [
        ("original", &original, Vec::new()),
        ("changed-c", &changed, vec![("CONFIG_CFLAGS", c_counter)]),
        (
            "changed-cxx",
            &changed_cxx,
            vec![("TARGET_ISA_CXXFLAGS", cxx_counter)],
        ),
    ] {
        let mut variables = configuration.make_variables();
        variables.extend(paths.clone());
        for (name, value) in changes {
            variables.insert(name.into(), value.into());
        }
        let target = TargetContext {
            cpu: Some(cpu.into()),
            platform: Some("raspi".into()),
            toolchain: Some("llvm".into()),
            mesa_version: Some("26.0.0".into()),
            make_variables: variables,
            ..TargetContext::default()
        };
        let scope = collect_vars_with_context(&joined, &target);
        let dirs = DirVars::load(source);
        let lookup = |name: &str| paths.get(name).cloned();
        let at_declaration =
            MakeExprContext::new(&scope, &dirs, declaration, source, Path::new(VC4))
                .with_lookup(&lookup);
        let at_assignment =
            MakeExprContext::new(&scope, &dirs, assignment + 1, source, Path::new(VC4))
                .with_lookup(&lookup);
        let actual =
            evaluate_ordered_mesa_compiler_options(&at_declaration, &at_assignment).unwrap();
        if case["compile_projection"].as_bool() == Some(true) {
            let projection_root = output.join(format!("compiled-{label}"));
            fs::create_dir(&projection_root).unwrap();
            mesa_compile_projection::verify_projection(
                &projection_root,
                &preflight.join("host-tools"),
                Path::new(case["cmake"].as_str().unwrap()),
                Path::new(case["ninja"].as_str().unwrap()),
                compiler,
                &configuration.identity().target_triple,
                &actual,
            )
            .expect("compile actual source option projection");
        }
        for (key, value) in [
            ("C", actual.c),
            ("CXX", actual.cxx),
            ("BRIDGE", actual.vc4_bridge),
        ] {
            assert_eq!(
                value,
                oracle[key].split_whitespace().collect::<Vec<_>>(),
                "{label}/{key}"
            );
        }
        assert_eq!(actual.gca_abi_flags, oracle["GCA"], "{label}/GCA");
    }
    assert_eq!(measure_tree_content_cas(source).unwrap(), source_before);
    assert_eq!(measure_tree_content_cas(compiler).unwrap(), compiler_before);
    assert_eq!(measure_tree_content_cas(&run).unwrap(), run_before);
    fs::write(output.join("result.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "scope":"actual source option sequences and GCA assignment; optional tiny object fixtures, not Mesa product or SDK qualification",
        "tiny_object_projection_executed":case["compile_projection"].as_bool() == Some(true),
        "retained_output_scope":"pinned receipt consistency and complete run unchanged during oracle; no reconstructed original producer output snapshot",
        "receipt_sha256":sha256_bytes(&receipt_bytes),
        "run_before_sha256":run_before.payload_digest_excluding(None),
        "configuration_sha256":sha256_bytes(&bytes), "wrapper_sha256":sha256_bytes(make.as_bytes()),
        "mesa_sha256":sha256_bytes(mesa.as_bytes()),"vc4_sha256":sha256_bytes(vc4.as_bytes()),
        "original":original,"c_counter":changed,"cxx_counter":changed_cxx
    })).unwrap()).unwrap();
}

fn pinned_document(path: &Path, expected: &str) -> std::io::Result<Vec<u8>> {
    let (_, bytes) = measure_regular_file_bounded(path, 1024 * 1024)?
        .ok_or_else(|| std::io::Error::other("missing regular oracle document"))?;
    if sha256_bytes(&bytes).as_str() != expected {
        return Err(std::io::Error::other("oracle document digest mismatch"));
    }
    Ok(bytes)
}

fn check_configured_paths(
    measured: &BTreeMap<String, String>,
    receipt: &serde_json::Value,
) -> Result<(), &'static str> {
    for key in ["GENDIR", "PORTSDIR"] {
        let expected = receipt["configured_paths"][key]
            .as_str()
            .ok_or("missing receipt configured path")?;
        if measured.get(key).map(String::as_str) != Some(expected) {
            return Err("Make configured path differs from pinned receipt");
        }
    }
    Ok(())
}

#[test]
fn retained_oracle_rejects_changed_or_missing_make_configuration_paths() {
    let receipt = serde_json::json!({"configured_paths":{
        "GENDIR":"/run/configure/gen", "PORTSDIR":"/run/configure/Ports"
    }});
    let original = BTreeMap::from([
        ("GENDIR".into(), "/run/configure/gen".into()),
        ("PORTSDIR".into(), "/run/configure/Ports".into()),
    ]);
    assert!(check_configured_paths(&original, &receipt).is_ok());
    for key in ["GENDIR", "PORTSDIR"] {
        let mut changed = original.clone();
        changed.insert(key.into(), "/other-tree".into());
        assert!(check_configured_paths(&changed, &receipt).is_err());
        changed.remove(key);
        assert!(check_configured_paths(&changed, &receipt).is_err());
    }
    assert!(check_configured_paths(&original, &serde_json::json!({})).is_err());
}

#[test]
fn retained_oracle_documents_reject_drift_missing_files_and_links() {
    let root = tempfile::tempdir().unwrap();
    let document = root.path().join("receipt.json");
    let digest = sha256_bytes(b"original");
    fs::write(&document, b"original").unwrap();
    assert_eq!(
        pinned_document(&document, digest.as_str()).unwrap(),
        b"original"
    );
    fs::write(&document, b"changed").unwrap();
    assert!(pinned_document(&document, digest.as_str()).is_err());
    assert!(pinned_document(&root.path().join("missing"), digest.as_str()).is_err());
    #[cfg(unix)]
    {
        let link = root.path().join("linked.json");
        std::os::unix::fs::symlink(&document, &link).unwrap();
        assert!(pinned_document(&link, sha256_bytes(b"changed").as_str()).is_err());
    }
}

fn make_values(
    preflight: &Path,
    build: &Path,
    wrapper: &Path,
    output: &Path,
    label: &str,
    overrides: &[String],
) -> BTreeMap<String, String> {
    let host = preflight.join("host-tools");
    let mut command = Command::new(host.join("make"));
    command
        .env_clear()
        .env("PATH", &host)
        .env("LC_ALL", "C")
        .current_dir(build)
        .args([
            "--silent",
            "--no-builtin-rules",
            "--no-builtin-variables",
            "-f",
        ])
        .arg(wrapper)
        .arg("measured-inputs")
        .args(overrides);
    let result = run_output_with_control(
        &mut command,
        128 * 1024,
        Duration::from_secs(30),
        &CancellationToken::default(),
    )
    .unwrap();
    fs::write(
        output.join(format!("{label}.stdout")),
        result.stdout.exact_bytes().unwrap(),
    )
    .unwrap();
    fs::write(
        output.join(format!("{label}.stderr")),
        result.stderr.exact_bytes().unwrap(),
    )
    .unwrap();
    assert!(
        result.status.success() && !result.timed_out && !result.cancelled,
        "GNU Make {label} failed: {}",
        result.stderr.rendered_lossy()
    );
    let mut values = BTreeMap::new();
    for line in std::str::from_utf8(result.stdout.exact_bytes().unwrap())
        .unwrap()
        .lines()
    {
        let (key, value) = line.split_once('=').unwrap();
        assert!(KEYS.iter().any(|(wanted, _)| *wanted == key));
        assert!(values.insert(key.into(), value.into()).is_none());
    }
    assert_eq!(values.len(), KEYS.len());
    values
}
