use super::*;
use crate::captured_programs::{ProgramRootSelection, ProgramSelection, ProgramSetSelection};
use crate::{measure_tree_content_cas, sha256_bytes};
use serde_json::{json, Value};
use std::os::unix::fs::{symlink, PermissionsExt};
use tempfile::TempDir;

struct CompilerFixture {
    _temporary: TempDir,
    root: PathBuf,
    clang_sha256: Sha256Digest,
}

impl CompilerFixture {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("compiler");
        fs::create_dir_all(root.join("bin")).unwrap();
        let clang = root.join("bin/clang");
        let clang_bytes = b"compiler role fixture; never executed\n";
        fs::write(&clang, clang_bytes).unwrap();
        fs::set_permissions(&clang, fs::Permissions::from_mode(0o700)).unwrap();
        symlink("clang", root.join("bin/clang++")).unwrap();

        Self {
            _temporary: temporary,
            root: root.canonicalize().unwrap(),
            clang_sha256: sha256_bytes(clang_bytes),
        }
    }

    fn tree_sha256(&self) -> Sha256Digest {
        measure_tree_content_cas(&self.root)
            .unwrap()
            .payload_digest_excluding(None)
    }

    fn selection(&self) -> ProgramSetSelection {
        let clang = self.root.join("bin/clang");
        let clangxx = self.root.join("bin/clang++");
        ProgramSetSelection {
            roots: vec![ProgramRootSelection {
                root: self.root.clone(),
                content_sha256: self.tree_sha256(),
            }],
            programs: vec![
                ProgramSelection {
                    role: "target-c".into(),
                    root_index: 0,
                    invocation_path: clang,
                    executable_sha256: self.clang_sha256.clone(),
                },
                ProgramSelection {
                    role: "target-cxx".into(),
                    root_index: 0,
                    invocation_path: clangxx,
                    executable_sha256: self.clang_sha256.clone(),
                },
            ],
        }
    }
}

fn write_compiler_program_receipt(
    fixture: &mut Fixture,
    compiler: &CompilerFixture,
    compiler_programs: Value,
) -> Sha256Digest {
    fixture.identity.compiler_tree_sha256 = compiler.tree_sha256();
    let clang = compiler.root.join("bin/clang");
    let clangxx = compiler.root.join("bin/clang++");
    let mut configuration: Value = serde_json::from_slice(&fixture.configuration_bytes).unwrap();
    configuration["identity"] = json!(fixture.identity);
    configuration["recipe_configuration"]["target_cc"] = json!(clang);
    configuration["recipe_configuration"]["target_cxx"] = json!(clangxx);
    fixture.configuration_bytes = serde_json::to_vec(&configuration).unwrap();
    fs::write(
        fixture.root.join("mesa-configuration.json"),
        &fixture.configuration_bytes,
    )
    .unwrap();

    let parsed =
        ValidatedMesaConfiguration::parse_bound(&fixture.configuration_bytes, &fixture.identity)
            .unwrap();
    let mut receipt = receipt_value(fixture);
    receipt["identity"] = json!(fixture.identity);
    receipt["configuration_sha256"] = json!(sha256_bytes(&fixture.configuration_bytes));
    receipt["make_values"] = serde_json::to_value(parsed.make_variables()).unwrap();
    receipt["compiler_programs"] = compiler_programs;
    fixture.write_receipt_and_refresh_input(&receipt)
}

#[test]
fn captures_compiler_root_and_keeps_clang_alias_invocation_distinct() {
    let mut fixture = Fixture::new();
    let compiler = CompilerFixture::new();
    let selection = compiler.selection();
    let expected = write_compiler_program_receipt(
        &mut fixture,
        &compiler,
        serde_json::to_value(selection).unwrap(),
    );

    let loaded = load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap();
    let programs = loaded.compiler_programs().unwrap();
    let clang = programs.identity_for_role("target-c").unwrap();
    let clangxx = programs.identity_for_role("target-cxx").unwrap();

    assert_eq!(clang.invocation_path(), compiler.root.join("bin/clang"));
    assert_eq!(clang.resolved_path(), compiler.root.join("bin/clang"));
    assert_eq!(clang.executable_sha256(), &compiler.clang_sha256);
    assert_eq!(clangxx.invocation_path(), compiler.root.join("bin/clang++"));
    assert_eq!(clangxx.resolved_path(), compiler.root.join("bin/clang"));
    assert_eq!(clangxx.executable_sha256(), &compiler.clang_sha256);
    assert_ne!(clangxx.invocation_path(), clangxx.resolved_path());
}

#[test]
fn legacy_receipt_without_compiler_programs_loads_as_none() {
    let fixture = Fixture::new();
    let loaded =
        load_mesa_configuration_input(&fixture.input_path(), &fixture.expected_input_sha256())
            .unwrap();

    assert!(loaded.compiler_programs().is_none());
}

#[test]
fn revalidate_detects_compiler_prefix_drift_outside_the_input_root() {
    let mut fixture = Fixture::new();
    let compiler = CompilerFixture::new();
    let expected = write_compiler_program_receipt(
        &mut fixture,
        &compiler,
        serde_json::to_value(compiler.selection()).unwrap(),
    );
    let loaded = load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap();
    let input_bytes = fs::read(fixture.input_path()).unwrap();
    let receipt_bytes = fs::read(fixture.root().join("receipt.json")).unwrap();

    fs::write(compiler.root.join("bin/clang"), b"changed compiler bytes\n").unwrap();

    let error = loaded.revalidate().unwrap_err().to_string();
    assert!(error.contains("compiler program roots changed after loading"));
    assert_eq!(fs::read(fixture.input_path()).unwrap(), input_bytes);
    assert_eq!(
        fs::read(fixture.root().join("receipt.json")).unwrap(),
        receipt_bytes
    );
}

#[test]
fn refuses_mismatched_compiler_program_selection_data() {
    let compiler = CompilerFixture::new();
    let alternate = compiler.root.join("bin/clang-alternate");
    fs::write(&alternate, b"alternate compiler role fixture\n").unwrap();
    fs::set_permissions(&alternate, fs::Permissions::from_mode(0o700)).unwrap();
    let valid = compiler.selection();
    let mut cases = Vec::new();

    let mut wrong_root = valid.clone();
    wrong_root.roots[0].content_sha256 = digest('f');
    cases.push((
        "wrong root digest",
        serde_json::to_value(wrong_root).unwrap(),
    ));

    let mut wrong_file = valid.clone();
    wrong_file.programs[1].executable_sha256 = digest('f');
    cases.push((
        "wrong executable digest",
        serde_json::to_value(wrong_file).unwrap(),
    ));

    let mut wrong_role_path = valid.clone();
    wrong_role_path.programs[0].invocation_path = alternate.clone();
    wrong_role_path.programs[0].executable_sha256 = sha256_bytes(&fs::read(&alternate).unwrap());
    cases.push((
        "role path differs from configured driver",
        serde_json::to_value(wrong_role_path).unwrap(),
    ));

    let mut missing_role = valid.clone();
    missing_role.programs.pop();
    cases.push((
        "missing C++ role",
        serde_json::to_value(missing_role).unwrap(),
    ));

    let mut extra_role = valid.clone();
    let mut extra = extra_role.programs[0].clone();
    extra.role = "unselected-role".to_owned();
    extra.invocation_path = alternate;
    extra.executable_sha256 = sha256_bytes(&fs::read(&extra.invocation_path).unwrap());
    extra_role.programs.push(extra);
    cases.push(("extra role", serde_json::to_value(extra_role).unwrap()));

    let mut unknown_nested = serde_json::to_value(valid).unwrap();
    unknown_nested["programs"][0]["unexpected"] = json!(true);
    cases.push(("unknown nested field", unknown_nested));

    for (label, selection) in cases {
        let mut fixture = Fixture::new();
        let expected = write_compiler_program_receipt(&mut fixture, &compiler, selection);
        let result = load_mesa_configuration_input(&fixture.input_path(), &expected);
        assert!(result.is_err(), "accepted {label}");
    }

    let mut fixture = Fixture::new();
    let expected = write_compiler_program_receipt(&mut fixture, &compiler, Value::Null);
    assert!(
        load_mesa_configuration_input(&fixture.input_path(), &expected).is_err(),
        "accepted explicit null compiler_programs"
    );
}

#[test]
fn refuses_compiler_alias_that_escapes_selected_root() {
    let mut fixture = Fixture::new();
    let compiler = CompilerFixture::new();
    let outside = TempDir::new().unwrap();
    let outside_program = outside.path().join("clang++");
    fs::write(&outside_program, b"outside compiler alias target\n").unwrap();
    fs::set_permissions(&outside_program, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_file(compiler.root.join("bin/clang++")).unwrap();
    symlink(&outside_program, compiler.root.join("bin/clang++")).unwrap();

    let expected = write_compiler_program_receipt(
        &mut fixture,
        &compiler,
        serde_json::to_value(compiler.selection()).unwrap(),
    );
    assert!(load_mesa_configuration_input(&fixture.input_path(), &expected).is_err());
}
