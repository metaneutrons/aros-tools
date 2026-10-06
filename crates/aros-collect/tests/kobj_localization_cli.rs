//! Independent process-boundary parity with the source macro's GNU nm/objcopy
//! recipe. This fixture is not a P4 core build and cannot qualify RV3 by itself.
use std::fs;
use std::path::Path;
use std::process::Command;

const fn collector() -> &'static str {
    env!("CARGO_BIN_EXE_aros-collect")
}

#[test]
fn incomplete_localization_invocations_fail_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let object = root.path().join("retained.o");
    fs::write(&object, b"retained sentinel").unwrap();
    for args in [
        vec!["--localize-kobj"],
        vec!["--localize-kobj", object.to_str().unwrap()],
        vec!["--nm", "nm", "--objcopy", "objcopy"],
        vec![
            "--localize-kobj",
            object.to_str().unwrap(),
            "--nm",
            "missing",
            "--objcopy",
            "missing",
            "--ld",
            "ld",
        ],
    ] {
        let output = Command::new(collector())
            .arg("--diagnostic-format=json")
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(diagnostic["diagnostics"][0]["code"], "AC0001");
        assert_eq!(fs::read(&object).unwrap(), b"retained sentinel");
    }
}

#[cfg(unix)]
#[test]
fn failed_localization_preserves_the_original_and_refuses_symlinks() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let object = root.path().join("retained.o");
    let mut bytes = vec![0u8; 64];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&243u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
    aros_common::elf::read(&bytes).unwrap();
    let nm = root.path().join("nm");
    let objcopy = root.path().join("objcopy");
    for (nm_script, objcopy_script) in [
        ("exit 7", "exit 0"),
        ("printf 'malformed\\n'", "exit 0"),
        (
            "printf '00000000 T __INITLIB_LIST__\\n'",
            "printf invalid > \"$1\"",
        ),
        ("printf '00000000 T __INITLIB_LIST__\\n'", "exit 9"),
        (
            "printf '00000000 T __INITLIB_LIST__\\n'",
            "printf '\\003' | dd of=\"$1\" bs=1 seek=7 conv=notrunc 2>/dev/null",
        ),
        (
            "printf '00000000 T __INITLIB_LIST__\\n'",
            "printf '\\003' | dd of=\"$1\" bs=1 seek=8 conv=notrunc 2>/dev/null",
        ),
        (
            "printf '00000000 T __INITLIB_LIST__\\n'",
            "printf '\\076\\000' | dd of=\"$1\" bs=1 seek=18 conv=notrunc 2>/dev/null",
        ),
    ] {
        fs::write(&object, &bytes).unwrap();
        for (path, script) in [(&nm, nm_script), (&objcopy, objcopy_script)] {
            fs::write(path, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = Command::new(collector())
            .args(["--diagnostic-format=json", "--localize-kobj"])
            .arg(&object)
            .arg("--nm")
            .arg(&nm)
            .arg("--objcopy")
            .arg(&objcopy)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(diagnostic["diagnostics"][0]["code"], "AC0801");
        assert_eq!(fs::read(&object).unwrap(), bytes);
    }
    let alias = root.path().join("alias.o");
    symlink(&object, &alias).unwrap();
    let output = Command::new(collector())
        .arg("--localize-kobj")
        .arg(&alias)
        .arg("--nm")
        .arg(&nm)
        .arg("--objcopy")
        .arg(&objcopy)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&object).unwrap(), bytes);
    // No-symbol fixture: objcopy can correctly leave these bytes unchanged.
    // Publication must preserve the original portable mode in either case.
    fs::write(&nm, "#!/bin/sh\nexit 0\n").unwrap();
    fs::write(&objcopy, "#!/bin/sh\nexit 0\n").unwrap();
    for mode in [0o644, 0o755] {
        fs::set_permissions(&object, fs::Permissions::from_mode(mode)).unwrap();
        let output = Command::new(collector())
            .arg("--localize-kobj")
            .arg(&object)
            .arg("--nm")
            .arg(&nm)
            .arg("--objcopy")
            .arg(&objcopy)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&object).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&object).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }
}

fn run(command: &mut Command) -> std::process::Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
#[ignore = "requires the explicitly selected verified local RV32 GNU compiler prefix"]
fn real_rv32_collected_kobj_matches_reference_localization_bytes() {
    let prefix = std::env::var_os("AROS_RV32_COMPILER_PREFIX")
        .expect("AROS_RV32_COMPILER_PREFIX must name the compiler directory");
    let prefix = Path::new(&prefix);
    let cc = prefix.join("riscv-aros-gcc");
    let ld = prefix.join("riscv-aros-ld");
    let nm = prefix.join("riscv-aros-nm");
    let objcopy = prefix.join("riscv-aros-objcopy");
    let root = tempfile::tempdir().unwrap();
    // The filename itself matches __aros_lib*: plain GNU nm does not print
    // STT_FILE entries. An indiscriminate ELF-name scan would not be parity.
    let source = root.path().join("__aros_lib_fixture.c");
    fs::write(
        &source,
        br#"
int DOSBase;
int SysBase;
int __aros_lib_fixture = 1;
int __ORDINARY = 2;
extern int __MISSING_LIST__;
int *use_missing(void) { return &__MISSING_LIST__; }
void init(void) {}
void (*entry)(void) __attribute__((section(".aros.set.INITLIB.10"))) = init;
"#,
    )
    .unwrap();
    let input = root.path().join("input.o");
    run(Command::new(&cc)
        .args([
            "-c",
            "-ffreestanding",
            "-nostdinc",
            "-fno-pic",
            "-march=rv32imafc_zicsr_zifencei_zaamo_zalrsc",
            "-mabi=ilp32f",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&input));
    let actual = root.path().join("actual.o");
    let script = root.path().join("sets.ld");
    let report = root.path().join("sets.txt");
    run(Command::new(collector())
        .arg("--ld")
        .arg(&ld)
        .arg("--keep-script")
        .arg(&script)
        .arg("--report")
        .arg(&report)
        .args(["--", "-r", "-m", "riscvelf_aros", "-o"])
        .arg(&actual)
        .arg(&input));
    assert!(fs::read_to_string(&script)
        .unwrap()
        .contains("__INITLIB_LIST__"));
    assert!(!report.exists(), "all fixture sets must be supported");
    let reference = root.path().join("reference.o");
    fs::copy(&actual, &reference).unwrap();
    let symbols = run(Command::new(&nm)
        .env("LC_ALL", "C")
        .arg("--")
        .arg(&reference));
    let text = String::from_utf8(symbols.stdout).unwrap();
    assert!(text
        .lines()
        .any(|line| line.split_whitespace().count() == 2 && line.contains("__MISSING_LIST__")));
    let mut reference_command = Command::new(&objcopy);
    reference_command.arg(&reference);
    for name in [
        "DOSBase",
        "IntuitionBase",
        "LayersBase",
        "GfxBase",
        "OOPBase",
        "UtilityBase",
        "ExpansionBase",
        "KeymapBase",
        "KernelBase",
    ] {
        reference_command.arg("-L").arg(name);
    }
    // Independent literal translation of make.tmpl's awk field-three filter.
    for line in text.lines() {
        if let Some(name) = line.split_whitespace().nth(2) {
            if name
                .strip_prefix("__")
                .is_some_and(|tail| tail.ends_with("_LIST__") || tail.ends_with("_END__"))
                || name.starts_with("__aros_lib")
            {
                reference_command.arg("-L").arg(name);
            }
        }
    }
    run(&mut reference_command);
    run(Command::new(collector())
        .arg("--localize-kobj")
        .arg(&actual)
        .arg("--nm")
        .arg(&nm)
        .arg("--objcopy")
        .arg(&objcopy));
    assert_eq!(fs::read(&actual).unwrap(), fs::read(&reference).unwrap());
    let object = aros_common::elf::read(&fs::read(&actual).unwrap()).unwrap();
    for name in [
        "DOSBase",
        "__INITLIB_LIST__",
        "__INITLIB_END__",
        "__aros_lib_fixture",
    ] {
        let symbol = object
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap();
        assert_eq!(symbol.binding, aros_common::elf::Binding::Local, "{name}");
    }
    for name in ["SysBase", "__ORDINARY", "__MISSING_LIST__"] {
        let symbol = object
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap();
        assert_eq!(symbol.binding, aros_common::elf::Binding::Global, "{name}");
    }
    println!("RV32 KOBJ collected set and byte-identical GNU reference localization passed ({} bytes, SHA256 {})", fs::metadata(&actual).unwrap().len(), aros_common::sha256_bytes(&fs::read(&actual).unwrap()));
}
