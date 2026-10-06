//! Read-only qualification of an explicitly selected RV32 compiler input.
//! This is not native P4 kernel, BSP, Developer or release qualification.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aros_common::elf::riscv::{ArtifactRole, TargetContract};
use aros_common::native_build_contract::load_bound_native_build_contract;
use aros_common::{sha256_file, toolchain_tree_inventory};
use serde_json::json;

fn checked(command: &mut Command) -> Output {
    let output = command.output().expect("explicit compiler probe executes");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn sterile(executable: &Path, root: &Path) -> Command {
    let mut command = Command::new(executable);
    command.env_clear().current_dir(root).env("LC_ALL", "C");
    command.env("PATH", "/usr/bin:/bin").env("TMPDIR", root);
    command
}

fn text(output: Output) -> String {
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn flags(target: &TargetContract, sysroot: &Path) -> Vec<String> {
    vec![
        format!("--sysroot={}", sysroot.display()),
        format!("-march={}", target.isa()),
        format!("-mabi={}", target.abi()),
        format!("-mcmodel={}", target.code_model()),
        "-mstrict-align".into(),
        "-nostdinc".into(),
        "-ffreestanding".into(),
        "-O2".into(),
        "-Wall".into(),
        "-Wextra".into(),
        "-Werror".into(),
    ]
}

/// Retains only measured local compiler evidence. No provenance is invented
/// to turn a reference prefix into a producer-manifest-qualified candidate.
#[test]
#[ignore = "requires AROS_RV3_LOCAL_RV32_COMPILER, AROS_RV3_TARGET_CONTRACT and AROS_TEST_P4_SOURCE"]
fn relocated_rv32_compiles_and_links_without_a_make_developer_sysroot() {
    let input = PathBuf::from(std::env::var_os("AROS_RV3_LOCAL_RV32_COMPILER").unwrap())
        .canonicalize()
        .unwrap();
    let target_path = PathBuf::from(std::env::var_os("AROS_RV3_TARGET_CONTRACT").unwrap());
    let target_bytes = fs::read(&target_path).unwrap();
    let target = TargetContract::parse(&target_bytes).unwrap();
    assert_eq!(target.abi(), "ilp32f", "this probe is specifically RV32/P4");
    let source = PathBuf::from(std::env::var_os("AROS_TEST_P4_SOURCE").unwrap());
    let profiles =
        aros_common::TargetProfile::load_from_file(&source.join("aros-targets.toml")).unwrap();
    let profile = profiles
        .iter()
        .find(|profile| profile.name == "esp32p4-d1001")
        .unwrap();
    let source_contract = load_bound_native_build_contract(
        &source,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
    )
    .unwrap();
    assert_eq!(source_contract.contract.abi.isa, target.isa());
    assert_eq!(source_contract.contract.abi.abi, target.abi());
    assert_eq!(source_contract.contract.abi.code_model, target.code_model());
    assert_eq!(target.code_model(), "medany");
    let (input_tree, input_inventory) = toolchain_tree_inventory(&input).unwrap();
    let evidence_parent = std::env::var_os("AROS_RV3_COMPILER_EVIDENCE_PARENT")
        .map_or_else(std::env::temp_dir, PathBuf::from);
    let temporary = tempfile::Builder::new()
        .prefix("rv3-rv32-compiler-only.")
        .tempdir_in(evidence_parent)
        .unwrap();
    // macOS /tmp is an alias of /private/tmp. Containment must compare
    // physical roots, not a mixture of alias and canonical path spellings.
    let physical_root = temporary.path().canonicalize().unwrap();
    let root = physical_root.as_path();
    let compiler = root.join("compiler");
    // Copy exactly the compiler tree, never its sibling Make Developer tree.
    checked(Command::new("/bin/cp").arg("-R").arg(&input).arg(&compiler));
    let (copied_tree, copied_inventory) = toolchain_tree_inventory(&compiler).unwrap();
    assert_eq!(copied_tree, input_tree);
    assert_eq!(copied_inventory, input_inventory);
    let sysroot = root.join("empty-native-sdk");
    fs::create_dir(&sysroot).unwrap();
    let gcc = compiler.join("riscv-aros-gcc");
    let cxx = compiler.join("riscv-aros-g++");
    let ld = compiler.join("riscv-aros-ld");
    let readelf = compiler.join("riscv-aros-readelf");
    for driver in [&gcc, &cxx, &ld, &readelf] {
        assert!(
            driver.canonicalize().unwrap().starts_with(&compiler),
            "driver resolves outside the copied prefix: {}",
            driver.display()
        );
    }
    let mut helper_bindings = Vec::new();
    for (driver, helper) in [(&gcc, "cc1"), (&cxx, "cc1plus"), (&gcc, "as")] {
        let helper = PathBuf::from(text(checked(
            sterile(driver, root).arg(format!("-print-prog-name={helper}")),
        )))
        .canonicalize()
        .unwrap();
        assert!(
            helper.starts_with(&compiler),
            "compiler helper escaped the copied prefix"
        );
        helper_bindings
            .push(json!({"path": helper, "sha256": sha256_file(&helper).unwrap().digest}));
    }
    assert_eq!(
        text(checked(sterile(&gcc, root).arg("-dumpmachine"))),
        "riscv-aros"
    );
    let version = text(checked(sterile(&gcc, root).arg("-dumpfullversion")));
    let baked_sysroot = text(checked(sterile(&gcc, root).arg("-print-sysroot")));
    let selected_sysroot = text(checked(
        sterile(&gcc, root)
            .arg(format!("--sysroot={}", sysroot.display()))
            .arg("-print-sysroot"),
    ));
    assert_eq!(Path::new(&selected_sysroot), sysroot);
    let c = root.join("probe.c");
    fs::write(&c, "_Static_assert(sizeof(void *) == 4, \"RV32 pointer\");\nvolatile unsigned long long numerator = 0x123456789abcdef0ULL;\nvolatile unsigned long long denominator = 17;\nunsigned long long divide(void) { return numerator / denominator; }\n").unwrap();
    let cpp = root.join("probe.cpp");
    fs::write(&cpp, "static_assert(sizeof(void *) == 4, \"RV32 pointer\");\ntemplate<class T> T increment(T x) { return x + 1; }\nextern \"C\" unsigned increment_cpp(unsigned x) { return increment(x); }\n").unwrap();
    let asm = root.join("probe.S");
    fs::write(&asm, ".text\n.globl assembly_probe\n.type assembly_probe, @function\nassembly_probe:\n addi a0, a0, 7\n ret\n.size assembly_probe, .-assembly_probe\n").unwrap();
    let mut outputs = Vec::new();
    for (driver, source, name) in [
        (&gcc, &c, "probe-c.o"),
        (&cxx, &cpp, "probe-cxx.o"),
        (&gcc, &asm, "probe-assembly.o"),
    ] {
        let output = root.join(name);
        let mut command = sterile(driver, root);
        command.args(flags(&target, &sysroot));
        if driver == &cxx {
            command.arg("-nostdinc++");
        }
        checked(command.arg("-c").arg(source).arg("-o").arg(&output));
        target
            .verify(&fs::read(&output).unwrap(), ArtifactRole::CompilationUnit)
            .unwrap();
        outputs.push(output);
    }
    let relocations = text(checked(
        sterile(&readelf, root).args(["-W", "-r"]).arg(&outputs[0]),
    ));
    let is_medany = |relocations: &str| {
        ["numerator", "denominator"].iter().all(|symbol| {
            relocations
                .lines()
                .any(|line| line.contains("R_RISCV_PCREL_HI20") && line.contains(symbol))
        }) && relocations.contains("R_RISCV_PCREL_LO12_I")
            && !relocations
                .lines()
                .any(|line| line.contains("R_RISCV_HI20"))
    };
    assert!(
        is_medany(&relocations),
        "medany address references must actually be PC-relative"
    );
    fs::write(root.join("medany-relocations.txt"), &relocations).unwrap();
    let medlow = root.join("wrong-code-model.o");
    checked(
        sterile(&gcc, root)
            .args(flags(&target, &sysroot))
            .arg("-mcmodel=medlow")
            .arg("-c")
            .arg(&c)
            .arg("-o")
            .arg(&medlow),
    );
    let medlow_relocations = text(checked(
        sterile(&readelf, root).args(["-W", "-r"]).arg(&medlow),
    ));
    assert!(!is_medany(&medlow_relocations));
    assert!(medlow_relocations.contains("R_RISCV_HI20"));
    fs::write(root.join("medlow-relocations.txt"), &medlow_relocations).unwrap();
    let libgcc = PathBuf::from(text(checked(
        sterile(&gcc, root)
            .args(flags(&target, &sysroot))
            .arg("-print-libgcc-file-name"),
    )));
    let libgcc = libgcc.canonicalize().unwrap();
    assert!(
        libgcc.starts_with(&compiler),
        "libgcc must relocate inside the selected compiler"
    );
    let linked = root.join("compiler-only-linked.o");
    let map = root.join("compiler-only.map");
    checked(
        sterile(&ld, root)
            .arg("-r")
            .arg(format!("--sysroot={}", sysroot.display()))
            .arg(format!("-Map={}", map.display()))
            .args(&outputs)
            .arg(&libgcc)
            .arg("-o")
            .arg(&linked),
    );
    target
        .verify(&fs::read(&linked).unwrap(), ArtifactRole::CompilationUnit)
        .unwrap();
    let map_text = fs::read_to_string(&map).unwrap();
    let mut loads = 0;
    for line in map_text
        .lines()
        .filter_map(|line| line.strip_prefix("LOAD "))
    {
        assert!(
            Path::new(line).starts_with(root),
            "link escaped the isolated compiler probe: {line}"
        );
        loads += 1;
    }
    assert_eq!(loads, 4);
    assert!(
        map_text.contains("libgcc.a(_udivdi3.o)"),
        "64-bit division must actually draw on the selected runtime"
    );
    let unavailable = root.join("unavailable-sdk.c");
    fs::write(&unavailable, "#include <exec/types.h>\n").unwrap();
    // Do not use -nostdinc for the refusal: that would suppress the old SDK
    // independently of --sysroot and fail to test the native engine's policy.
    let ordinary_flags = flags(&target, &sysroot)
        .into_iter()
        .filter(|argument| argument != "-nostdinc")
        .collect::<Vec<_>>();
    for (driver, source, label) in [(&gcc, &c, "c"), (&cxx, &cpp, "cxx")] {
        let search = sterile(driver, root)
            .args(&ordinary_flags)
            .args(["-E", "-v"])
            .arg(source)
            .output()
            .unwrap();
        assert!(search.status.success());
        let search_text = String::from_utf8(search.stderr.clone()).unwrap();
        let search_paths = search_text
            .split("#include <...> search starts here:")
            .nth(1)
            .and_then(|section| section.split("End of search list.").next())
            .expect("compiler reports effective include paths");
        let mut paths = 0;
        for path in search_paths
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let resolved = Path::new(path).canonicalize().unwrap();
            assert!(
                resolved.starts_with(&compiler) || resolved.starts_with(&sysroot),
                "compiler include search escaped the selected input: {path}"
            );
            paths += 1;
        }
        assert!(
            paths > 0,
            "actual compiler builtin headers must be observed"
        );
        fs::write(
            root.join(format!("{label}-include-search.stderr")),
            &search.stderr,
        )
        .unwrap();
        let denied = sterile(driver, root)
            .args(&ordinary_flags)
            .args(["-x", if label == "cxx" { "c++" } else { "c" }])
            .arg("-c")
            .arg(&unavailable)
            .arg("-o")
            .arg(root.join("must-not-exist.o"))
            .output()
            .unwrap();
        assert!(!denied.status.success());
        assert!(String::from_utf8_lossy(&denied.stderr).contains("exec/types.h"));
        assert!(!root.join("must-not-exist.o").exists());
        fs::write(
            root.join(format!("{label}-unavailable-sdk.stderr")),
            &denied.stderr,
        )
        .unwrap();
    }
    // A successfully produced soft-float object is not the selected ilp32f ABI.
    let wrong = root.join("wrong-float.o");
    checked(
        sterile(&gcc, root)
            .args(flags(&target, &sysroot))
            .arg("-mabi=ilp32")
            .arg("-c")
            .arg(&c)
            .arg("-o")
            .arg(&wrong),
    );
    let error = target
        .verify(&fs::read(&wrong).unwrap(), ArtifactRole::CompilationUnit)
        .unwrap_err();
    assert!(error.to_string().contains("floating-point ABI mismatch"));
    assert!(fs::read_dir(&sysroot).unwrap().next().is_none());
    assert_eq!(
        toolchain_tree_inventory(&input).unwrap(),
        (input_tree.clone(), input_inventory)
    );
    assert_eq!(toolchain_tree_inventory(&compiler).unwrap().0, copied_tree);
    let receipt = json!({
        "schema": "aros-local-compiler-probe-v1",
        "qualification": "compiler-only-not-native-p4-build",
        "input_root": input, "compiler_tree_sha256": input_tree,
        "relocated_root": compiler, "gcc_version": version,
        "gcc_sha256": sha256_file(&gcc).unwrap().digest,
        "gxx_sha256": sha256_file(&cxx).unwrap().digest,
        "linker_sha256": sha256_file(&ld).unwrap().digest,
        "target_contract_sha256": aros_common::sha256_bytes(&target_bytes),
        "native_source_contract_sha256": source_contract.sha256,
        "source_baseline": source_contract.contract.source_baseline,
        "source_abi": source_contract.contract.abi,
        "compiler_helpers": helper_bindings,
        "baked_sysroot": baked_sysroot, "selected_sysroot": sysroot,
        "make_developer_payload_imported": false,
        "libgcc_sha256": sha256_file(&libgcc).unwrap().digest,
        "linked_sha256": sha256_file(&linked).unwrap().digest,
        "link_map_sha256": sha256_file(&map).unwrap().digest,
        "sdk_header_access": "rejected",
        "wrong_float_abi": "rejected",
        "code_model": "medany-pcrel-addressing-with-medlow-counterprobe",
        "limitations": ["no compiler producer manifest or release provenance", "no source-built Developer SDK", "no collector or final AROS application link", "no native kernel/BSP", "no hardware access"]
    });
    fs::write(
        root.join("compiler-only.receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!(
        "Retained compiler-only evidence: {}",
        temporary.keep().display()
    );
}
