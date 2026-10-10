//! Opt-in checks of the actual source-owned P4 compiler-consumer declaration.
//! Contract validation is not native build, package, media or boot evidence.

use aros_common::native_consumer_contract::load_bound_native_consumer_contract;
use aros_common::TargetProfile;
use std::{fs, path::Path};

#[test]
#[ignore = "requires AROS_TEST_P4_SOURCE with the current native SDK declaration"]
fn source_owned_p4_sdk_contract_is_bound_and_rejects_drift() {
    let source = std::env::var_os("AROS_TEST_P4_SOURCE")
        .expect("AROS_TEST_P4_SOURCE selects the source checkout");
    let root = Path::new(&source);
    let profile_text = fs::read_to_string(root.join("aros-targets.toml")).unwrap();
    let profiles = TargetProfile::parse_config(&profile_text, "aros-targets.toml").unwrap();
    let selected = profiles
        .targets
        .iter()
        .filter(|profile| profile.name == "esp32p4-riscv-gnu")
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 1, "P4 SDK profile must be unique");
    let profile = selected[0];
    let relative = Path::new(profile.native_consumer_contract.as_deref().unwrap());
    let binding = load_bound_native_consumer_contract(root, relative, profile).unwrap();
    let contract = &binding.contract;
    assert_eq!(contract.roots, ["includes", "linklibs"]);
    assert_eq!(contract.abi.source_cpu, "riscv");
    assert_eq!(contract.abi.target_triple, "riscv-aros");
    assert_eq!(contract.abi.isa, "rv32imafc_zicsr_zifencei_zaamo_zalrsc");
    assert_eq!(contract.abi.abi, "ilp32f");
    assert_eq!(contract.abi.code_model, "medany");
    assert!(contract.abi.platform_smp);
    assert!(!contract.abi.use_mmu);
    assert_eq!(contract.make_variables["AROS_TOOLCHAIN_RELEASE"], "0");
    assert_eq!(
        contract.make_variables["CROSSTOOLS_PORTS_INCLUDES"],
        "ports-includes"
    );
    assert_eq!(
        contract.generated_make_templates["compiler/include/geninc.cfg"].substitutions
            ["@ENABLE_EXECSMP@"],
        "#define __AROSEXEC_SMP__"
    );
    assert!(contract
        .inputs
        .iter()
        .any(|input| input.path == "config/kobj-romtag.ld"));

    // Copy only the explicitly measured regular inputs into an isolated root.
    // Counterprobes never alter the original source or any build tree.
    let isolated = tempfile::tempdir().unwrap();
    for input in &contract.inputs {
        let destination = isolated.path().join(&input.path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(root.join(&input.path), destination).unwrap();
    }
    let contract_path = isolated.path().join(relative);
    fs::create_dir_all(contract_path.parent().unwrap()).unwrap();
    let original = fs::read(root.join(relative)).unwrap();
    fs::write(&contract_path, &original).unwrap();
    load_bound_native_consumer_contract(isolated.path(), relative, profile).unwrap();

    for (field, value, reason) in [
        (
            "use_mmu",
            serde_json::json!(true),
            "abi.use_mmu true does not match profile transpiler.use_mmu false",
        ),
        (
            "platform_smp",
            serde_json::json!(false),
            "abi.platform_smp false does not match profile bootstrap ABI platform_smp true",
        ),
        (
            "isa",
            serde_json::json!("rv64imafdc_zicsr_zifencei"),
            "abi.isa width does not match selected target architecture",
        ),
    ] {
        let mut incompatible: serde_json::Value = serde_json::from_slice(&original).unwrap();
        incompatible["abi"][field] = value;
        fs::write(&contract_path, serde_json::to_vec(&incompatible).unwrap()).unwrap();
        let failure = load_bound_native_consumer_contract(isolated.path(), relative, profile)
            .unwrap_err()
            .to_string();
        assert!(failure.contains(reason), "{field}: {failure}");
    }

    fs::write(&contract_path, &original).unwrap();
    let profile_path = isolated.path().join("aros-targets.toml");
    let old_profile = fs::read(&profile_path).unwrap();
    let mut changed_profile = old_profile.clone();
    changed_profile.extend_from_slice(b"\n# isolated stale-seal counterprobe\n");
    fs::write(&profile_path, changed_profile).unwrap();
    let failure = load_bound_native_consumer_contract(isolated.path(), relative, profile)
        .unwrap_err()
        .to_string();
    assert!(failure.contains("requires the selected source profile document"));
    fs::write(&profile_path, old_profile).unwrap();
    load_bound_native_consumer_contract(isolated.path(), relative, profile).unwrap();

    let script = isolated.path().join("config/kobj-romtag.ld");
    let old_script = fs::read(&script).unwrap();
    fs::write(&script, b"altered isolated linker contract\n").unwrap();
    let failure = load_bound_native_consumer_contract(isolated.path(), relative, profile)
        .unwrap_err()
        .to_string();
    assert!(failure.contains("measured source input digest differs"));
    fs::write(&script, old_script).unwrap();
    load_bound_native_consumer_contract(isolated.path(), relative, profile).unwrap();
}
