use super::*;
use crate::contract::Mode;
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::TempDir;

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn test_contract(root: &Path, make_body: &str) -> Contract {
    let build_root = root.join("build");
    let binary_dir = build_root.join("gen/configure/workbench/devs/AHI/x86_64");
    let stage_build = binary_dir.join("build");
    fs::create_dir_all(&stage_build).unwrap();
    let make = root.join("make");
    write_executable(&make, make_body);
    let install_prefix = build_root.join("SYS");
    let relative = vec![PathBuf::from("C/one"), PathBuf::from("Libs/two")];
    let install_products = relative
        .iter()
        .map(|path| install_prefix.join(path))
        .collect();
    Contract {
        mode: Mode::X86_64,
        source_root: root.join("source"),
        engine_root: root.join("engine"),
        build_root,
        source_dir: root.join("source/workbench/devs/AHI"),
        source_manifest: root.join("source/workbench/devs/AHI/ahi-build.inputs"),
        source_manifest_sha256: "0".repeat(64),
        product_manifest: root.join("engine/manifests/ahi-x86_64.install"),
        product_manifest_sha256: "0".repeat(64),
        stage_source: binary_dir.join("source"),
        stage_linklibs: binary_dir.join("linklibs"),
        binary_dir,
        stage_build,
        install_prefix,
        host_sfdc: root.join("sfdc"),
        host_perl: root.join("perl"),
        host_flexcat: root.join("host-flexcat"),
        flexcat: root.join("flexcat"),
        make,
        cc: root.join("cc"),
        collect: root.join("collect"),
        assembler: root.join("as"),
        ar: root.join("ar"),
        ranlib: root.join("ranlib"),
        objcopy: root.join("objcopy"),
        strip: root.join("strip"),
        lld: root.join("lld"),
        sdk_include: root.join("sdk/include"),
        gen_include: root.join("gen/include"),
        feature_headers: Vec::new(),
        build_triplet: "aarch64-apple-darwin".into(),
        target_triple: "x86_64-unknown-aros".into(),
        elf_class: "02".into(),
        elf_machine_hex: "3e00".into(),
        target_cflags: Vec::new(),
        target_cppflags: Vec::new(),
        target_asflags: Vec::new(),
        target_ldflags: Vec::new(),
        input_relative: Vec::new(),
        input_sha256: Vec::new(),
        product_relative: relative,
        product_kinds: vec![ProductKind::Data, ProductKind::Data],
        install_products,
        dependency_products: Vec::new(),
    }
}

fn install_complete_live(contract: &Contract, one: &[u8], two: &[u8]) {
    for (target, contents) in contract.install_products.iter().zip([one, two]) {
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
        fs::set_permissions(target, fs::Permissions::from_mode(0o644)).unwrap();
    }
}

fn prepared_products(contract: &Contract, suffix: u8) -> Vec<PreparedProduct> {
    contract
        .install_products
        .iter()
        .enumerate()
        .map(|(index, target)| PreparedProduct {
            target: target.clone(),
            contents: vec![b'A' + u8::try_from(index).unwrap(), suffix],
            mode: if index == 0 { 0o755 } else { 0o644 },
        })
        .collect()
}

fn assert_live(contract: &Contract, one: &[u8], two: &[u8]) {
    assert_eq!(fs::read(&contract.install_products[0]).unwrap(), one);
    assert_eq!(fs::read(&contract.install_products[1]).unwrap(), two);
}

fn assert_no_private_stages(contract: &Contract) {
    let stages = fs::read_dir(&contract.binary_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGE_PREFIX)
        })
        .count();
    assert_eq!(stages, 0);
}

#[test]
fn failed_private_install_does_not_change_complete_live_set() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C"
printf partial >"$prefix/C/one"
exit 23
"#,
    );
    install_complete_live(&contract, b"old-one", b"old-two");

    let error = prepare(&contract).unwrap_err();

    assert_eq!(error.diagnostic().code, DiagnosticCode::AhiBuild);
    assert_live(&contract, b"old-one", b"old-two");
    assert_no_private_stages(&contract);
}

#[test]
fn missing_private_product_does_not_change_complete_live_set() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C"
printf candidate >"$prefix/C/one"
"#,
    );
    install_complete_live(&contract, b"old-one", b"old-two");

    let error = prepare(&contract).unwrap_err();

    assert_eq!(
        error.diagnostic().code,
        DiagnosticCode::AhiProductValidation
    );
    assert_live(&contract, b"old-one", b"old-two");
    assert_no_private_stages(&contract);
}

#[test]
fn cleanup_failure_happens_before_live_publication() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C" "$prefix/Libs"
printf one >"$prefix/C/one"
printf two >"$prefix/Libs/two"
/bin/chmod 500 "$(/usr/bin/dirname "$DESTDIR")"
"#,
    );
    install_complete_live(&contract, b"old-one", b"old-two");

    let result = prepare(&contract);
    fs::set_permissions(&contract.binary_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.unwrap_err();

    assert_eq!(error.diagnostic().code, DiagnosticCode::AhiBuild);
    assert!(error.diagnostic().message.contains("before publication"));
    assert_live(&contract, b"old-one", b"old-two");
}

#[test]
fn unexpected_private_product_is_rejected_before_publication() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C" "$prefix/Libs"
printf one >"$prefix/C/one"
printf two >"$prefix/Libs/two"
printf extra >"$prefix/unexpected"
"#,
    );

    let error = prepare(&contract).unwrap_err();

    assert_eq!(
        error.diagnostic().code,
        DiagnosticCode::AhiProductValidation
    );
    assert!(contract.install_products.iter().all(|path| !path.exists()));
    assert_no_private_stages(&contract);
}

#[test]
fn private_install_keeps_logical_prefix_environment() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C" "$prefix/Libs"
printf %s "$AHI_INSTALL_PREFIX" >"$prefix/C/one"
printf %s "$AHI_INSTALL_PREFIX" >"$prefix/Libs/two"
"#,
    );

    let prepared = prepare(&contract).unwrap();

    let logical = contract.install_prefix.as_os_str().as_bytes();
    assert!(prepared
        .products
        .iter()
        .all(|product| product.contents == logical));
    assert_no_private_stages(&contract);
}

#[test]
fn product_embedding_private_prefix_is_rejected() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C" "$prefix/Libs"
printf %s "$prefix" >"$prefix/C/one"
printf safe >"$prefix/Libs/two"
"#,
    );

    let error = prepare(&contract).unwrap_err();

    assert_eq!(
        error.diagnostic().code,
        DiagnosticCode::AhiProductValidation
    );
    assert!(error.diagnostic().message.contains("embeds the private"));
    assert!(contract.install_products.iter().all(|path| !path.exists()));
}

#[test]
fn a_partial_live_set_is_repaired_rather_than_refused() {
    // A deleted product is the ordinary reason a build runs again. Refusing to
    // act on a set that is merely incomplete left the only repair as deleting
    // the rest by hand, so this is the behaviour that matters: the missing
    // product comes back and the surviving one is replaced from the same
    // validated set.
    let temp = TempDir::new().unwrap();
    let contract = test_contract(
        temp.path(),
        r#"#!/bin/sh
set -eu
prefix=
for argument in "$@"; do
  case "$argument" in PREFIX=*) prefix=${argument#PREFIX=} ;; esac
done
/bin/mkdir -p "$prefix/C" "$prefix/Libs"
printf repaired-one >"$prefix/C/one"
printf repaired-two >"$prefix/Libs/two"
"#,
    );
    install_complete_live(&contract, b"old-one", b"old-two");
    fs::remove_file(&contract.install_products[1]).unwrap();

    let prepared = prepare(&contract).unwrap();
    publish(&contract, &prepared).unwrap();

    assert_live(&contract, b"repaired-one", b"repaired-two");
    assert_no_private_stages(&contract);
}

#[test]
fn a_failed_install_leaves_a_partial_live_set_alone() {
    // The concurrency guard still has to hold over an incomplete set: what was
    // present stays byte-for-byte, and what was missing stays missing.
    let temp = TempDir::new().unwrap();
    let contract = test_contract(temp.path(), "#!/bin/sh\nexit 99\n");
    let target = &contract.install_products[0];
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, b"existing").unwrap();

    let error = prepare(&contract).unwrap_err();

    assert_eq!(error.diagnostic().code, DiagnosticCode::AhiBuild);
    assert_eq!(fs::read(target).unwrap(), b"existing");
    assert!(!contract.install_products[1].exists());
}

#[test]
fn privileged_live_mode_is_rejected_without_changes() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(temp.path(), "#!/bin/sh\nexit 99\n");
    install_complete_live(&contract, b"old-one", b"old-two");
    fs::set_permissions(
        &contract.install_products[0],
        fs::Permissions::from_mode(0o4755),
    )
    .unwrap();

    let error = prepare(&contract).unwrap_err();

    assert_eq!(
        error.diagnostic().code,
        DiagnosticCode::AhiProductValidation
    );
    assert_live(&contract, b"old-one", b"old-two");
    assert_eq!(
        fs::metadata(&contract.install_products[0])
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o4755
    );
}

#[test]
fn complete_live_set_is_replaced_together_with_modes() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(temp.path(), "#!/bin/sh\nexit 0\n");
    install_complete_live(&contract, b"old-one", b"old-two");
    let baseline = measure_live_set(&contract).unwrap();
    let products = prepared_products(&contract, b'1');

    publish_products(&contract, &baseline, &products).unwrap();

    assert_live(&contract, b"A1", b"B1");
    assert_eq!(
        fs::metadata(&contract.install_products[0])
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(&contract.install_products[1])
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
}

#[test]
fn parallel_publications_allow_exactly_one_complete_candidate() {
    let temp = TempDir::new().unwrap();
    let contract = test_contract(temp.path(), "#!/bin/sh\nexit 0\n");
    install_complete_live(&contract, b"old-one", b"old-two");
    let baseline = measure_live_set(&contract).unwrap();
    let barrier = Arc::new(Barrier::new(2));

    let spawn_candidate = |suffix| {
        let contract = contract.clone();
        let baseline = baseline.clone();
        let products = prepared_products(&contract, suffix);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            publish_products(&contract, &baseline, &products)
        })
    };
    let first = spawn_candidate(b'1');
    let second = spawn_candidate(b'2');
    let results = [first.join().unwrap(), second.join().unwrap()];

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let one = fs::read(&contract.install_products[0]).unwrap();
    let two = fs::read(&contract.install_products[1]).unwrap();
    assert!(
        (one == b"A1" && two == b"B1") || (one == b"A2" && two == b"B2"),
        "parallel publication produced a mixed set: {one:?} / {two:?}"
    );
}
