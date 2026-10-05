//! Explicit host-only IDF production. No retained venv or existing build adopted.
#![cfg(unix)]

use aros_common::{
    measure_tree_content_cas_bounded, sha256_bytes, sha256_file, CancellationToken, Sha256Digest,
    TargetProfile, TreeTraversalLimits,
};
use aros_toolchain::idf_bootloader::{
    bind_idf_bootloader_lock, build_idf_bootloader, IdfBootloaderLock, IdfBootloaderRequest,
};
use aros_toolchain::wheel_environment::{
    bind_wheel_environment_lock, prepare_wheel_environment, LockedWheel, WheelEnvironmentLock,
    WheelEnvironmentRequest, WheelRuntimePin,
};
use aros_verify::esp_image::{verify_esp_image, EspImagePolicy};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

fn fixture() -> serde_json::Value {
    serde_json::json!({
        "schema_version":1, "format":"aros-idf-bootloader-v1", "qualification":"local-byte-lock-only",
        "host":format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH),
        "idf_version":"6.0.1", "compiler_prefix":"riscv32-esp-elf",
        "idf_tree_sha256":"1".repeat(64), "idf_source_receipt_sha256":"9".repeat(64), "compiler_tree_sha256":"2".repeat(64),
        "compiler_source_receipt_sha256":"a".repeat(64),
        "cmake_source_receipt_sha256":"b".repeat(64),
        "ninja_source_receipt_sha256":"c".repeat(64),
        "cmake_tree_sha256":"3".repeat(64), "ninja_sha256":"4".repeat(64),
        "git_sha256":"5".repeat(64), "requirements_sha256":"6".repeat(64),
        "constraints_sha256":"7".repeat(64), "wheel_lock_sha256":"8".repeat(64)
    })
}

#[test]
fn lock_requires_exact_closed_local_byte_identity() {
    let value = fixture();
    let raw = serde_json::to_vec(&value).unwrap();
    bind_idf_bootloader_lock(&raw, &sha256_bytes(&raw)).unwrap();
    assert!(bind_idf_bootloader_lock(&raw, &sha256_bytes(b"changed")).is_err());
    for (key, changed) in [
        ("schema_version", serde_json::json!(2)),
        ("format", serde_json::json!("other")),
        ("qualification", serde_json::json!("release-qualified")),
        ("host", serde_json::json!("other-host")),
        ("compiler_prefix", serde_json::json!("../outside")),
        ("idf_version", serde_json::json!("6.0.1;command")),
        ("idf_tree_sha256", serde_json::json!("unknown")),
        ("idf_source_receipt_sha256", serde_json::json!("unknown")),
        (
            "compiler_source_receipt_sha256",
            serde_json::json!("unknown"),
        ),
        ("cmake_source_receipt_sha256", serde_json::json!("unknown")),
        ("ninja_source_receipt_sha256", serde_json::json!("unknown")),
        ("script", serde_json::json!("run-anything")),
    ] {
        let mut value = fixture();
        value[key] = changed;
        let raw = serde_json::to_vec(&value).unwrap();
        assert!(
            bind_idf_bootloader_lock(&raw, &sha256_bytes(&raw)).is_err(),
            "{key}"
        );
    }
}

fn explicit(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name}")))
        .canonicalize()
        .unwrap()
}
fn tree(root: &Path) -> Sha256Digest {
    measure_tree_content_cas_bounded(
        root,
        TreeTraversalLimits::new(300_000, 4 * 1024 * 1024 * 1024).unwrap(),
    )
    .unwrap()
    .payload_digest_excluding(None)
}
fn digest(path: &Path) -> Sha256Digest {
    sha256_file(path).unwrap().digest
}

fn prepared_vendor(
    root: &Path,
    cache: &Path,
    role: &str,
    archive: &str,
    suffix: &str,
    checksum: &str,
) -> aros_fetch::engine::source_receipt::VerifiedSourceReceipt {
    let destination = root.join(role);
    let request = aros_fetch::contract::FetchRequest::from_cli(&aros_fetch::contract::Cli {
        archive_origins: cache.to_str().unwrap().into(),
        archive: archive.into(),
        suffixes: suffix.into(),
        destination: destination.clone(),
        patch_origins: ".".into(),
        patches: String::new(),
        base: Some(destination),
        location: cache.to_owned(),
        rename_directory: None,
        checksums: format!("{archive}.{suffix}=sha256:{checksum}"),
        force: false,
        offline: true,
        require_checksums: true,
        diagnostic_format: aros_common::DiagnosticFormat::Human,
        log_level: aros_common::LogLevel::Off,
        log_format: aros_common::LogFormat::Jsonl,
        log_file: None,
    })
    .unwrap();
    aros_fetch::engine::source_receipt::verify_prepared_source(&request).unwrap()
}

#[test]
#[ignore = "requires explicit measured wheelhouse, Python, IDF sources/tools and P4 source; no hardware"]
fn actual_fresh_idf_bootloader_uses_locked_inputs_and_source_geometry() {
    let wheel_root = explicit("AROS_TEST_VENDOR_WHEEL_ROOT");
    let interpreter = explicit("AROS_TEST_VENDOR_PYTHON");
    let runtime_prefix = explicit("AROS_TEST_VENDOR_PYTHON_PREFIX");
    let runtime_version = std::env::var("AROS_TEST_VENDOR_PYTHON_VERSION").unwrap();
    let parent = explicit("AROS_TEST_IDF_WORK_PARENT");
    let python_parent = explicit("AROS_TEST_VENDOR_WORK_PARENT");
    let inventory: serde_json::Value =
        serde_json::from_slice(&fs::read(wheel_root.join("inventory.json")).unwrap()).unwrap();
    assert_eq!(
        inventory["artifact_status"],
        "provisional-measured-cache-coverage-only"
    );
    let wheels = inventory["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| LockedWheel {
            name: entry["name"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase()
                .replace(['_', '.'], "-"),
            version: entry["version"].as_str().unwrap().into(),
            filename: entry["wheel_filename"].as_str().unwrap().into(),
            sha256: entry["sha256"].as_str().unwrap().into(),
            size_bytes: entry["size_bytes"].as_u64().unwrap(),
        })
        .collect::<Vec<_>>();
    let wheel_lock = WheelEnvironmentLock {
        schema_version: 1,
        format: "aros-python-wheels-v1".into(),
        qualification: "local-byte-lock-only".into(),
        host: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        runtime: WheelRuntimePin {
            version: runtime_version,
            executable_sha256: digest(&interpreter).to_string(),
            prefix_tree_sha256: tree(&runtime_prefix).to_string(),
        },
        bootstrap_filename: wheels
            .iter()
            .find(|wheel| wheel.name == "pip")
            .unwrap()
            .filename
            .clone(),
        wheels,
    };
    let raw = serde_json::to_vec_pretty(&wheel_lock).unwrap();
    let wheel_lock_digest = sha256_bytes(&raw);
    let bound = bind_wheel_environment_lock(&raw, &wheel_lock_digest).unwrap();
    let token = CancellationToken::default();
    let python = prepare_wheel_environment(&WheelEnvironmentRequest {
        lock: &bound,
        interpreter: &interpreter,
        runtime_prefix: &runtime_prefix,
        wheel_cache: &wheel_root.join("wheelhouse"),
        work_parent: &python_parent,
        timeout: Duration::from_secs(300),
        cancellation: &token,
    })
    .unwrap();
    let source = explicit("AROS_TEST_IDF_AROS_SOURCE");
    let idf = explicit("AROS_TEST_IDF_SOURCE");
    let compiler = explicit("AROS_TEST_IDF_COMPILER");
    let cmake_root = explicit("AROS_TEST_IDF_CMAKE_ROOT");
    let cmake = explicit("AROS_TEST_IDF_CMAKE");
    let ninja = explicit("AROS_TEST_IDF_NINJA");
    let git = explicit("AROS_TEST_IDF_GIT");
    let constraints = explicit("AROS_TEST_IDF_CONSTRAINTS");
    let vendor_root = explicit("AROS_TEST_VENDOR_SOURCE_PARENT");
    let vendor_cache = explicit("AROS_TEST_VENDOR_ARCHIVE_CACHE");
    let compiler_source = prepared_vendor(
        &vendor_root,
        &vendor_cache,
        "compiler",
        "riscv32-esp-elf-15.2.0_20251204-aarch64-apple-darwin",
        "tar.xz",
        "0869d1083532c631808543dd802885f02dbe1bb3bd640be0dee827e82ded768d",
    );
    let cmake_source = prepared_vendor(
        &vendor_root,
        &vendor_cache,
        "cmake",
        "cmake-4.0.3-macos-universal",
        "tar.gz",
        "4e85de4daf1c3e82d7dc6b8ba5683972944b466343aeb9c327a742437bb3ce9a",
    );
    let ninja_source = prepared_vendor(
        &vendor_root,
        &vendor_cache,
        "ninja",
        "ninja-mac",
        "zip",
        "89a287444b5b3e98f88a945afa50ce937b8ffd1dcc59c555ad9b1baf855298c9",
    );
    let profiles = TargetProfile::load_from_file(&source.join("aros-targets.toml")).unwrap();
    let profile = profiles.iter().find(|p| p.name == "esp32p4-d1001").unwrap();
    let native_digest =
        Sha256Digest::parse("201bcf7de54ea000da99e38aa4d13225bd695cee00ebb1745465de0c3fc618eb")
            .unwrap();
    let source_request = aros_fetch::contract::FetchRequest::from_cli(&aros_fetch::contract::Cli {
        archive_origins: "https://github.com/espressif/esp-idf/releases/download/v6.0.1".into(),
        archive: "esp-idf-v6.0.1".into(), suffixes: "zip".into(),
        destination: idf.parent().unwrap().to_owned(),
        patch_origins: source.join("arch/riscv-esp32p4/bootloader").to_str().unwrap().into(),
        patches: "esp-idf-6.0.1-standalone-app.diff:esp-idf-v6.0.1:-f,-p1".into(),
        base: Some(idf.parent().unwrap().parent().unwrap().join("patch-cache")),
        location: idf.parent().unwrap().parent().unwrap().join("cache"),
        rename_directory: None,
        checksums: "esp-idf-v6.0.1.zip=sha256:4f294f44ddcf7b6677ae2347808fc708768a4890fed3c6f7227b7d4c076b9019 esp-idf-6.0.1-standalone-app.diff=sha256:df445145767975c64aa8155a818906d7f15de709792fac1746fd0ad1d542d683".into(),
        force: false, offline: true, require_checksums: true,
        diagnostic_format: aros_common::DiagnosticFormat::Human,
        log_level: aros_common::LogLevel::Off, log_format: aros_common::LogFormat::Jsonl,
        log_file: None,
    }).unwrap();
    let prepared_source =
        aros_fetch::engine::source_receipt::verify_prepared_source(&source_request).unwrap();
    let input_index_before = digest(&idf.join(".git/index"));
    let input_tree_before = tree(&idf);
    let lock = IdfBootloaderLock {
        schema_version: 1,
        format: "aros-idf-bootloader-v1".into(),
        qualification: "local-byte-lock-only".into(),
        host: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        idf_version: "6.0.1".into(),
        compiler_prefix: "riscv32-esp-elf".into(),
        idf_tree_sha256: input_tree_before.clone(),
        idf_source_receipt_sha256: prepared_source.receipt_sha256().clone(),
        compiler_source_receipt_sha256: compiler_source.receipt_sha256().clone(),
        cmake_source_receipt_sha256: cmake_source.receipt_sha256().clone(),
        ninja_source_receipt_sha256: ninja_source.receipt_sha256().clone(),
        compiler_tree_sha256: tree(&compiler),
        cmake_tree_sha256: tree(&cmake_root),
        ninja_sha256: digest(&ninja),
        git_sha256: digest(&git),
        requirements_sha256: digest(&idf.join("tools/requirements/requirements.core.txt")),
        constraints_sha256: digest(&constraints),
        wheel_lock_sha256: wheel_lock_digest,
    };
    let raw = serde_json::to_vec_pretty(&lock).unwrap();
    let bound = bind_idf_bootloader_lock(&raw, &sha256_bytes(&raw)).unwrap();
    let mut request = IdfBootloaderRequest {
        lock: &bound,
        python: &python,
        source_root: &source,
        profile,
        native_contract_sha256: &native_digest,
        idf_root: &idf,
        idf_source: &prepared_source,
        compiler_source: &compiler_source,
        cmake_source: &cmake_source,
        ninja_source: &ninja_source,
        compiler_root: &compiler,
        cmake_root: &cmake_root,
        cmake: &cmake,
        ninja: &ninja,
        git: &git,
        constraints: &constraints,
        work_parent: &parent,
        jobs: 12,
        timeout: Duration::from_secs(600),
        cancellation: &token,
    };
    let before = fs::read_dir(&parent).unwrap().count();
    request.jobs = 0;
    assert!(build_idf_bootloader(&request).is_err());
    assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
    request.jobs = 12;
    let mut wrong_source_lock = lock.clone();
    wrong_source_lock.idf_source_receipt_sha256 = sha256_bytes(b"different source receipt");
    let wrong_source_raw = serde_json::to_vec(&wrong_source_lock).unwrap();
    let wrong_source_bound =
        bind_idf_bootloader_lock(&wrong_source_raw, &sha256_bytes(&wrong_source_raw)).unwrap();
    request.lock = &wrong_source_bound;
    let error = build_idf_bootloader(&request).unwrap_err();
    assert!(
        error.to_string().contains("source receipt differs"),
        "{error}"
    );
    assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
    request.lock = &bound;
    for field in [
        "compiler_source_receipt_sha256",
        "cmake_source_receipt_sha256",
        "ninja_source_receipt_sha256",
    ] {
        let mut wrong: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        wrong[field] = serde_json::json!(sha256_bytes(b"wrong vendor receipt"));
        let wrong_raw = serde_json::to_vec(&wrong).unwrap();
        let wrong_bound = bind_idf_bootloader_lock(&wrong_raw, &sha256_bytes(&wrong_raw)).unwrap();
        let wrong_request = IdfBootloaderRequest {
            lock: &wrong_bound,
            ..request
        };
        let error = build_idf_bootloader(&wrong_request).unwrap_err();
        assert!(
            error.to_string().contains("vendor source receipt differs"),
            "{error}"
        );
        assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
    }
    request.lock = &bound;
    let outside = request.compiler_root;
    request.compiler_root = &source;
    let error = build_idf_bootloader(&request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("escapes its archive receipt tree"),
        "{error}"
    );
    assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
    request.compiler_root = outside;
    let mut bad_lock = lock;
    bad_lock.constraints_sha256 = sha256_bytes(b"altered");
    let bad_raw = serde_json::to_vec(&bad_lock).unwrap();
    let bad_bound = bind_idf_bootloader_lock(&bad_raw, &sha256_bytes(&bad_raw)).unwrap();
    request.lock = &bad_bound;
    assert!(build_idf_bootloader(&request).is_err());
    assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
    request.lock = &bound;
    let loaded = aros_common::native_build_contract::load_bound_native_build_contract(
        &source,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
    )
    .unwrap();
    let changed_source = tempfile::tempdir().unwrap();
    let changed_root = changed_source.path().canonicalize().unwrap();
    for relative in loaded
        .contract
        .inputs
        .iter()
        .map(|input| input.path.as_str())
        .chain(std::iter::once(
            profile.native_build_contract.as_deref().unwrap(),
        ))
    {
        let destination = changed_root.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, fs::read(source.join(relative)).unwrap()).unwrap();
    }
    request.source_root = &changed_root;
    for input in &loaded.contract.inputs {
        let path = changed_root.join(&input.path);
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed.push(b'\n');
        fs::write(&path, changed).unwrap();
        let error = build_idf_bootloader(&request).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("native source input binding failed"),
            "{}: {error}",
            input.path
        );
        assert_eq!(fs::read_dir(&parent).unwrap().count(), before);
        fs::write(path, original).unwrap();
    }
    request.source_root = &source;
    let result = build_idf_bootloader(&request).unwrap();
    prepared_source.revalidate().unwrap();
    for source in [&compiler_source, &cmake_source, &ninja_source] {
        source.revalidate().unwrap();
    }
    assert_eq!(result.receipt.vendor_sources.len(), 3);
    for (actual, expected) in
        result
            .receipt
            .vendor_sources
            .iter()
            .zip([&compiler_source, &cmake_source, &ninja_source])
    {
        assert_eq!(actual.receipt_sha256, *expected.receipt_sha256());
        assert_eq!(actual.payload_tree_sha256, *expected.payload_tree_sha256());
    }
    assert_eq!(digest(&idf.join(".git/index")), input_index_before);
    assert_ne!(result.receipt.execution_idf_root, idf);
    assert!(result
        .receipt
        .execution_idf_root
        .starts_with(&result.work_root));
    assert_eq!(result.receipt.execution_idf_tree_before, input_tree_before);
    assert_eq!(
        result.receipt.execution_git_index_before,
        input_index_before
    );
    assert_eq!(
        result.receipt.idf_source_receipt_sha256,
        *prepared_source.receipt_sha256()
    );
    assert_eq!(result.receipt.phases.len(), 3);
    assert!(result
        .receipt
        .phases
        .iter()
        .all(|phase| phase.exit_code == Some(0) && !phase.timed_out && !phase.cancelled));
    assert!(result.receipt.phases.iter().all(|phase| {
        phase
            .environment
            .get("GIT_OPTIONAL_LOCKS")
            .map(String::as_str)
            == Some("0")
    }));
    python.revalidate().unwrap();
    let media = aros_common::native_media::resolve_bound_native_media(
        &source,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
        &native_digest,
    )
    .unwrap();
    let geometry = &media.binding.geometry;
    let slot = media
        .binding
        .layout
        .slots
        .iter()
        .find(|slot| slot.role == "bootloader")
        .unwrap();
    let image = fs::read(&result.receipt.bootloader.path).unwrap();
    let facts = verify_esp_image(
        &image,
        EspImagePolicy {
            chip_id: geometry.chip_id,
            revision_min: geometry.revision_min,
            revision_max: geometry.revision_max,
            flash_bytes: geometry.flash_bytes,
            maximum_image_bytes: slot.range.end - slot.range.start,
        },
    )
    .unwrap();
    assert_eq!(facts.sha256, result.receipt.bootloader.sha256);
    assert!(!result
        .work_root
        .join("build/aros_esp32p4_bootloader.bin")
        .exists());
    println!("retained fresh IDF output: {}", result.work_root.display());
    println!("bootloader {}B SHA256 {}", facts.size_bytes, facts.sha256);
    println!(
        "receipt SHA256 {}",
        digest(&result.work_root.join("bootloader.receipt.json"))
    );
}
