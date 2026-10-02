use aros_toolchain::profiles::Profiles;
use aros_toolchain::source_lock::CompilerFamily;
use serde_json::{json, Value};

fn target(abi: &str, architecture: &str, isa: &str) -> Value {
    json!({
        "schema": "aros-riscv-target-v1",
        "isa": isa,
        "abi": abi,
        "code_model": "medany",
        "architecture": architecture,
        "unaligned_access": false,
        "atomic_abi": 0,
        "x3_reg_usage": 0
    })
}

fn gnu_profile() -> Value {
    json!({
        "name": "rv64-aros",
        "configure_target": "riscv-aros",
        "upstream_output_target": "riscv64-aros",
        "target_triple": "riscv64-unknown-aros",
        "cpu": "riscv64",
        "platform": "riscv64",
        "float_abi": "lp64d",
        "capabilities": ["c", "libgcc", "standalone-collector"],
        "target": target(
            "lp64d",
            "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            "rva22u64"
        )
    })
}

fn document(family: &str) -> Value {
    let mut profile = gnu_profile();
    if family == "llvm" {
        profile.as_object_mut().unwrap().remove("target");
        profile["name"] = json!("pc-x86_64");
        profile["configure_target"] = json!("pc-x86_64");
        profile["upstream_output_target"] = json!("pc-x86_64");
        profile["target_triple"] = json!("x86_64-unknown-aros");
        profile["cpu"] = json!("x86_64");
        profile["platform"] = json!("pc");
        profile["float_abi"] = json!("");
        profile["capabilities"] = json!(["c", "cxx", "standalone-collector"]);
    }
    json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": family,
        "upstream_commit": "a".repeat(40),
        "profiles": [profile]
    })
}

fn parse(value: &Value) -> Result<Profiles, aros_toolchain::ContractError> {
    Profiles::parse(&serde_json::to_vec(value).unwrap())
}

#[test]
fn parses_family_aware_v2_and_preserves_v1_as_llvm() {
    let gnu = parse(&document("gnu")).unwrap();
    assert_eq!(gnu.family(), CompilerFamily::Gnu);
    let gnu_profile = gnu.select("rv64-aros").unwrap();
    assert_eq!(gnu_profile.family(), CompilerFamily::Gnu);
    assert_eq!(gnu_profile.target().unwrap().abi(), "lp64d");

    let llvm = parse(&document("llvm")).unwrap();
    assert_eq!(llvm.family(), CompilerFamily::Llvm);
    let llvm_profile = llvm.select("pc-x86_64").unwrap();
    assert_eq!(llvm_profile.family(), CompilerFamily::Llvm);
    assert!(llvm_profile.target().is_none());

    let v1 = json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": "b".repeat(40),
        "profiles": [{
            "name": "pc-x86_64", "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64", "platform": "pc", "float_abi": "",
            "capabilities": ["c", "standalone-collector"]
        }]
    });
    let parsed_v1 = parse(&v1).unwrap();
    assert_eq!(parsed_v1.family(), CompilerFamily::Llvm);
    assert_eq!(parsed_v1.entries()[0].family(), CompilerFamily::Llvm);
    assert!(parsed_v1.entries()[0].target().is_none());

    let mut target_in_v1 = v1;
    target_in_v1["profiles"][0]["target"] =
        target("lp64d", "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0", "rva22u64");
    assert!(parse(&target_in_v1).is_err());
}

#[test]
fn rejects_unknown_duplicate_and_null_v2_fields() {
    let mut unknown_top = document("gnu");
    unknown_top["future"] = json!(true);
    assert!(parse(&unknown_top).is_err());

    let mut unknown_profile = document("gnu");
    unknown_profile["profiles"][0]["future"] = json!(true);
    assert!(parse(&unknown_profile).is_err());

    let mut null_target = document("gnu");
    null_target["profiles"][0]["target"] = Value::Null;
    assert!(parse(&null_target).is_err());

    let mut null_family = document("gnu");
    null_family["family"] = Value::Null;
    assert!(parse(&null_family).is_err());

    let duplicate_family = format!(
        r#"{{"schema":"aros-toolchain-profiles-v2","family":"gnu","family":"llvm","upstream_commit":"{}","profiles":[]}}"#,
        "a".repeat(40)
    );
    assert!(Profiles::parse(duplicate_family.as_bytes()).is_err());

    let mut duplicate_target = serde_json::to_string(&document("gnu")).unwrap();
    duplicate_target = duplicate_target.replacen("\"target\":{", "\"target\":{},\"target\":{", 1);
    assert!(Profiles::parse(duplicate_target.as_bytes()).is_err());
}

#[test]
fn family_selects_disjoint_runtime_capability_sets() {
    let mut llvm_with_gnu_runtime = document("llvm");
    llvm_with_gnu_runtime["profiles"][0]["capabilities"] = json!(["c", "libgcc"]);
    assert!(parse(&llvm_with_gnu_runtime).is_err());

    for rejected in [
        "objc",
        "compiler-rt",
        "compiler-rt32",
        "libcxx",
        "libcxxabi",
        "libunwind",
        "multilib-collector",
    ] {
        let mut gnu = document("gnu");
        gnu["profiles"][0]["capabilities"] =
            json!(["c", "libgcc", "standalone-collector", rejected]);
        assert!(parse(&gnu).is_err(), "accepted GNU capability {rejected}");
    }

    for missing in ["c", "libgcc", "standalone-collector"] {
        let mut gnu = document("gnu");
        gnu["profiles"][0]["capabilities"] = json!(["c", "libgcc", "standalone-collector"]);
        gnu["profiles"][0]["capabilities"]
            .as_array_mut()
            .unwrap()
            .retain(|value| value != missing);
        assert!(parse(&gnu).is_err(), "accepted missing {missing}");
    }

    for capabilities in [
        json!(["c", "libgcc", "standalone-collector", "cxx"]),
        json!(["c", "libgcc", "standalone-collector", "libstdcxx"]),
        json!(["c", "libgcc", "standalone-collector", "libsupcxx"]),
        json!([
            "c",
            "libgcc",
            "standalone-collector",
            "libstdcxx",
            "libsupcxx"
        ]),
    ] {
        let mut gnu = document("gnu");
        gnu["profiles"][0]["capabilities"] = capabilities;
        assert!(parse(&gnu).is_err());
    }

    let mut gnu_cxx = document("gnu");
    gnu_cxx["profiles"][0]["capabilities"] = json!([
        "c",
        "cxx",
        "libgcc",
        "libstdcxx",
        "libsupcxx",
        "standalone-collector"
    ]);
    assert!(parse(&gnu_cxx).is_ok());
}

#[test]
fn enforces_gnu_target_cpu_width_float_abi_and_aros_triple() {
    let mut wrong_cpu = document("gnu");
    wrong_cpu["profiles"][0]["cpu"] = json!("riscv");
    assert!(parse(&wrong_cpu).is_err());

    let mut wrong_float_abi = document("gnu");
    wrong_float_abi["profiles"][0]["float_abi"] = json!("lp64");
    assert!(parse(&wrong_float_abi).is_err());

    let mut wrong_triple = document("gnu");
    wrong_triple["profiles"][0]["target_triple"] = json!("riscv64-unknown-linux-gnu");
    assert!(parse(&wrong_triple).is_err());

    for triple in ["x86_64-aros", "riscv-aros"] {
        let mut cpu_mismatch = document("gnu");
        cpu_mismatch["profiles"][0]["target_triple"] = json!(triple);
        assert!(parse(&cpu_mismatch).is_err());
    }

    let mut rv32 = document("gnu");
    rv32["profiles"][0]["cpu"] = json!("riscv");
    rv32["profiles"][0]["float_abi"] = json!("ilp32");
    rv32["profiles"][0]["target_triple"] = json!("riscv-unknown-aros");
    rv32["profiles"][0]["target"] = target("ilp32", "rv32i2p1_m2p0_a2p1_c2p0", "rv32imac");
    assert!(parse(&rv32).is_ok());

    let mut bad_width = rv32;
    bad_width["profiles"][0]["cpu"] = json!("riscv64");
    assert!(parse(&bad_width).is_err());
}
