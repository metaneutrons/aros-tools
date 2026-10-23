use super::*;
use std::fmt::Write as _;
use std::fs;
use std::process::Command;

#[test]
fn measured_llvm_state_is_required_and_never_defaults_or_normalizes() {
    let valid = BTreeMap::from([
        ("MESA_MEASURE_OPT_LLVM_STATE".into(), "undefined".into()),
        ("MESA_MEASURE_OPT_LLVM_VALUE".into(), "||".into()),
        (
            "MESA_MEASURE_TARGET_LLVM_VER_STATE".into(),
            "defined".into(),
        ),
        (
            "MESA_MEASURE_TARGET_LLVM_VER_VALUE".into(),
            "|11.0.0|".into(),
        ),
    ]);
    let parsed = from_measurements(&valid).unwrap();
    assert_eq!(parsed.opt_llvm, MesaMakeVersionValue::Undefined);
    assert_eq!(
        parsed.target_llvm_ver,
        MesaMakeVersionValue::Defined("11.0.0".into())
    );
    for key in LLVM_MEASUREMENT_KEYS {
        let mut missing = valid.clone();
        missing.remove(*key);
        assert!(from_measurements(&missing).is_err(), "{key}");
    }
    for (key, value) in [
        ("MESA_MEASURE_OPT_LLVM_VALUE", "|11.0.0|"),
        ("MESA_MEASURE_OPT_LLVM_STATE", "guessed"),
        ("MESA_MEASURE_TARGET_LLVM_VER_VALUE", "11.0.0"),
        ("MESA_MEASURE_TARGET_LLVM_VER_VALUE", "| 11.0.0|"),
        ("MESA_MEASURE_TARGET_LLVM_VER_VALUE", "|11.0.0 |"),
        ("MESA_MEASURE_TARGET_LLVM_VER_VALUE", "|$(shell anything)|"),
        ("MESA_MEASURE_TARGET_LLVM_VER_VALUE", "|011.0.0|"),
    ] {
        let mut invalid = valid.clone();
        invalid.insert(key.into(), value.into());
        assert!(from_measurements(&invalid).is_err(), "{key}={value}");
    }
}

#[test]
fn real_make_capture_distinguishes_undefined_defined_empty_and_version() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("measure.mk");
    for (assignment, expected) in [
        ("", MesaMakeVersionValue::Undefined),
        (
            "OPT_LLVM :=\n",
            MesaMakeVersionValue::Defined(String::new()),
        ),
        (
            "OPT_LLVM := 18.1.8\n",
            MesaMakeVersionValue::Defined("18.1.8".into()),
        ),
    ] {
        let mut source =
            format!("{assignment}TARGET_LLVM_VER := 11.0.0\n{LLVM_MEASUREMENT_ASSIGNMENTS}");
        for name in LLVM_MEASUREMENT_KEYS {
            writeln!(source, "$(info {name}=$(strip $({name})))").unwrap();
        }
        source.push_str(".PHONY: probe\nprobe:;@:\n");
        fs::write(&path, source).unwrap();
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let output = Command::new(make)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .args(["--no-builtin-rules", "--no-builtin-variables", "-f"])
            .arg(&path)
            .arg("probe")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let values = super::super::mesa_preflight_measurement::parse_values(
            &output.stdout,
            LLVM_MEASUREMENT_KEYS,
        )
        .unwrap();
        assert_eq!(from_measurements(&values).unwrap().opt_llvm, expected);
    }
}
