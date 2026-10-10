//! Synthetic integration coverage for bounded indexed-package read-back.
//! Package payloads are fixtures, never compiler build or release evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aros_common::{sha256_bytes, ArosCompilerIdentity, DiagnosticCode};
#[cfg(unix)]
use aros_toolchain::release_attestation_manifest_v2::{
    verify_attestation_manifest_v2, write_attestation_manifest_v2, AttestationManifestStageV2,
};
#[cfg(unix)]
use aros_toolchain::release_checksums_v2_writer::write_final_checksums_v2;
#[cfg(unix)]
use aros_toolchain::release_index_v2_writer::write_measured_index_v2;
use aros_toolchain::{
    canonical,
    package::{package_with_format, PackageFormat, PackageRequest},
    release_checksums_v2::verify_final_checksums_v2,
    release_index::{
        compare_package_sets, write_package_comparison_report, PackageComparisonReport,
    },
    release_index_v2::NativeReleaseIndexV2,
    release_index_v2_builder::{build_measured_index_v2, MeasuredReleaseIndexRequestV2},
    release_index_v2_readback::{readback_indexed_packages, IndexedPackageReadbackRequestV2},
    release_inputs::{ReleaseInputs, ACTIVE_HOSTS},
    source_lock::SourceLock,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

const GNU_SOURCE_COMMIT: &str = "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1";
const LLVM_SOURCE_COMMIT: &str = "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2";
const PRODUCER_COMMIT: &str = "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3";
const TOOLS_COMMIT: &str = "b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4";
const PRODUCER_TREE: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
const TOOLS_TREE: &str = "f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6";
const RELEASE_ID: &str = "release-2026.10";
const PRIVATE_MARKER: &str = "synthetic-private-readback-marker";

#[derive(Debug, Clone, PartialEq, Eq)]
enum SnapshotEntry {
    File(Vec<u8>),
    Symlink(PathBuf),
    Directory,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MetadataSnapshotEntry {
    File {
        size: u64,
        modified: Option<SystemTime>,
    },
    Symlink(PathBuf),
    Directory,
    Other,
}

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    release_dir: PathBuf,
    inputs: ReleaseInputs,
    index: NativeReleaseIndexV2,
    index_value: Value,
    build_environments: BTreeMap<String, Map<String, Value>>,
}

impl Fixture {
    fn build() -> Self {
        Self::build_with_documents(input_documents(), 9, 48)
    }

    fn build_target_scope() -> Self {
        Self::build_with_documents(target_scope_input_documents(), 12, 60)
    }

    fn build_with_documents(
        (collection, documents): (Value, BTreeMap<String, Vec<u8>>),
        expected_lanes: usize,
        expected_inventory: usize,
    ) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        // The read-back boundary rejects symlink ancestors. On macOS this
        // resolves the common /var -> /private/var and /tmp -> /private/tmp
        // aliases before any release path is passed to it.
        let root = temporary.path().canonicalize().unwrap();
        let release_dir = root.join("release");
        fs::create_dir(&release_dir).unwrap();

        let collection_bytes = serde_json::to_vec(&collection).unwrap();
        let inputs = ReleaseInputs::parse(&collection_bytes, &documents).unwrap();
        fs::write(
            release_dir.join("toolchain-release-inputs-v2.json"),
            &collection_bytes,
        )
        .unwrap();
        for (name, bytes) in &documents {
            fs::write(release_dir.join(name), bytes).unwrap();
        }

        for (name, contents) in [
            (
                "SHA256SUMS",
                b"synthetic inventory support fixture\n".as_slice(),
            ),
            (
                "toolchain-provenance.sigstore.json",
                b"synthetic inventory support fixture\n".as_slice(),
            ),
            (
                "toolchain-manifest-v2.schema.json",
                include_bytes!(
                    "../../aros-common/tests/fixtures/toolchain-manifest-v2.schema.json"
                )
                .as_slice(),
            ),
            (
                "tree-digest-v1.fixture.json",
                include_bytes!("../../aros-common/tests/fixtures/tree-digest-v1.fixture.json")
                    .as_slice(),
            ),
        ] {
            fs::write(release_dir.join(name), contents).unwrap();
        }

        let mut artifacts = Vec::new();
        let mut build_environments = BTreeMap::new();
        for group in inputs.groups() {
            for profile in group.profiles().entries() {
                for host in ACTIVE_HOSTS {
                    let asset = fixture_asset_name(group.source_lock(), profile, host);
                    let candidate_root = root.join("candidates").join(format!(
                        "{}-{host}-{}",
                        group.id(),
                        profile.name()
                    ));
                    fs::create_dir_all(&candidate_root).unwrap();
                    fs::write(
                        candidate_root.join("fixture-input"),
                        b"synthetic package payload; never execute\n",
                    )
                    .unwrap();
                    if group.source_lock().family()
                        == aros_toolchain::source_lock::CompilerFamily::Gnu
                    {
                        write_gnu_tool_layout(&candidate_root, group.source_lock(), profile);
                    }

                    let build_environment = package_build_environment(host, &asset);
                    let package_request = PackageRequest {
                        candidate_root,
                        output_dir: root.join("packages").join(format!(
                            "{}-{host}-{}",
                            group.id(),
                            profile.name()
                        )),
                        release_id: RELEASE_ID.to_owned(),
                        host: (*host).to_owned(),
                        recipe: group.recipe().clone(),
                        source_lock: group.source_lock().clone(),
                        profile: profile.clone(),
                        build_environment,
                        forbidden_prefixes: Vec::new(),
                    };
                    let output =
                        package_with_format(&package_request, PackageFormat::CompilerFamilyV2)
                            .unwrap();
                    for source in [
                        &output.archive,
                        &output.manifest,
                        &output.checksum,
                        &output.sbom,
                    ] {
                        let name = source.file_name().unwrap();
                        fs::copy(source, release_dir.join(name)).unwrap();
                    }

                    let manifest: Value =
                        serde_json::from_slice(&fs::read(&output.manifest).unwrap()).unwrap();
                    artifacts.push(json!({
                        "group_id": group.id(),
                        "asset": asset,
                        "sha256": output.archive_sha256,
                        "size": output.archive_size,
                        "host": host,
                        "target_profile": profile.name(),
                        "target_triple": profile.target_triple(),
                        "source_commit": group.recipe().source().0.as_str(),
                        "compiler": manifest["compiler"].clone(),
                        "tree_sha256": manifest["tree_sha256"].clone(),
                        "enabled": true,
                        "strip_components": 1,
                        "required_paths": ["fixture-input"]
                    }));
                    // This map is assembled independently from the package
                    // bytes and is supplied again to the read-back request.
                    build_environments
                        .insert(asset.clone(), readback_build_environment(host, &asset));
                }
            }
        }
        artifacts.sort_by(|left, right| left["asset"].as_str().cmp(&right["asset"].as_str()));
        assert_eq!(artifacts.len(), expected_lanes);

        let index_value = json!({
            "schema": 2,
            "release_id": RELEASE_ID,
            "base_url": "https://example.invalid/aros/releases/2026.10",
            "inputs_sha256": sha256_bytes(&collection_bytes),
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "artifacts": artifacts
        });
        let index =
            NativeReleaseIndexV2::parse(&serde_json::to_vec(&index_value).unwrap(), &inputs)
                .unwrap();
        fs::write(
            release_dir.join("toolchain-index-v2.json"),
            index.to_json_bytes().unwrap(),
        )
        .unwrap();
        assert_eq!(index.expected_inventory().len(), expected_inventory);
        assert_eq!(
            release_inventory_names(&release_dir).len(),
            expected_inventory
        );
        let checksum_members = fixture_checksum_members(&release_dir);
        assert_eq!(checksum_members.len(), expected_inventory - 1);
        write_fixture_checksums(&release_dir, &checksum_members);

        Self {
            _temporary: temporary,
            root,
            release_dir: release_dir.canonicalize().unwrap(),
            inputs,
            index,
            index_value,
            build_environments,
        }
    }

    fn request(&self, directory: PathBuf) -> IndexedPackageReadbackRequestV2 {
        IndexedPackageReadbackRequestV2 {
            directory,
            inputs: self.inputs.clone(),
            index: self.index.clone(),
            build_environments: self.build_environments.clone(),
            forbidden_prefixes: Vec::new(),
        }
    }

    fn copy_release(&self, name: &str) -> PathBuf {
        let directory = self.root.join(format!("case-{name}"));
        fs::create_dir(&directory).unwrap();
        for entry in fs::read_dir(&self.release_dir).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
        }
        directory.canonicalize().unwrap()
    }

    fn copy_preindex_stage(&self, name: &str) -> PathBuf {
        let directory = self.copy_release(name);
        for final_output in [
            "toolchain-index-v2.json",
            "SHA256SUMS",
            "toolchain-provenance.sigstore.json",
        ] {
            fs::remove_file(directory.join(final_output)).unwrap();
        }
        assert_eq!(
            release_inventory_names(&directory).len(),
            self.index.expected_inventory().len() - 3
        );
        directory
    }

    fn copy_prechecksum_stage(&self, name: &str) -> PathBuf {
        let directory = self.copy_release(name);
        fs::remove_file(directory.join("SHA256SUMS")).unwrap();
        assert_eq!(
            release_inventory_names(&directory).len(),
            self.index.expected_inventory().len() - 1
        );
        assert!(directory
            .join("toolchain-provenance.sigstore.json")
            .is_file());
        directory
    }

    fn measured_index_request(&self, directory: PathBuf) -> MeasuredReleaseIndexRequestV2 {
        let required_paths = self
            .index
            .artifacts()
            .iter()
            .map(|artifact| {
                (
                    artifact.asset().to_owned(),
                    vec!["fixture-input".to_owned()],
                )
            })
            .collect();
        MeasuredReleaseIndexRequestV2 {
            directory,
            inputs: self.inputs.clone(),
            release_id: RELEASE_ID.to_owned(),
            base_url: "https://example.invalid/aros/releases/2026.10".to_owned(),
            build_environments: self.build_environments.clone(),
            required_paths,
            forbidden_prefixes: Vec::new(),
        }
    }
}

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

fn gnu_profile(width: u8) -> Value {
    let (cpu, triple, abi, architecture, isa) = if width == 32 {
        (
            "riscv",
            "riscv-aros",
            "ilp32f",
            "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
            "rv32imafc",
        )
    } else {
        (
            "riscv64",
            "riscv64-aros",
            "lp64d",
            "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            "rva22u64",
        )
    };
    json!({
        "name": format!("rv{width}-aros"),
        "configure_target": format!("fixture-rv{width}"),
        "upstream_output_target": format!("fixture-rv{width}"),
        "target_triple": triple,
        "cpu": cpu,
        "platform": "fixture",
        "float_abi": abi,
        "capabilities": ["c", "libgcc", "standalone-collector"],
        "target": target(abi, architecture, isa)
    })
}

fn gnu_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v3",
        "family": "gnu",
        "version": "16.2.0",
        "sources": [
            {
                "component": "gcc", "version": "16.2.0", "purpose": "toolchain-component",
                "filename": "gcc.tar.xz", "url": "https://example.invalid/gcc.tar.xz",
                "sha256": "1".repeat(64), "size": 1
            },
            {
                "component": "binutils", "version": "2.47", "purpose": "toolchain-component",
                "filename": "binutils.tar.xz", "url": "https://example.invalid/binutils.tar.xz",
                "sha256": "2".repeat(64), "size": 1
            }
        ],
        "host_python_packages": [{
            "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
            "url": "https://example.invalid/mako.tar.gz", "sha256": "3".repeat(64),
            "size": 1, "source_root": "mako", "python_path": "."
        }]
    }))
    .unwrap()
}

fn gnu_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "gnu",
        "upstream_commit": GNU_SOURCE_COMMIT,
        "profiles": [gnu_profile(32), gnu_profile(64)]
    }))
    .unwrap()
}

fn llvm_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "17.0.6",
        "sources": [{
            "component": "llvm", "version": "17.0.6", "purpose": "toolchain-component",
            "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": "4".repeat(64), "size": 1
        }],
        "host_python_packages": [{
            "name": "wheel", "version": "0.43.0", "filename": "wheel.tar.gz",
            "url": "https://example.invalid/wheel.tar.gz", "sha256": "5".repeat(64),
            "size": 1, "source_root": "wheel", "python_path": "."
        }]
    }))
    .unwrap()
}

fn llvm_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "llvm",
        "upstream_commit": LLVM_SOURCE_COMMIT,
        "profiles": [{
            "name": "pc-x86_64", "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64", "platform": "pc", "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    }))
    .unwrap()
}

fn recipe_bytes(source_lock: &[u8], profiles: &[u8], source_commit: &str) -> Vec<u8> {
    let mut recipe = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": source_commit,
        "source_tree": "7".repeat(40),
        "producer_commit": PRODUCER_COMMIT,
        "producer_tree": PRODUCER_TREE,
        "tools_commit": TOOLS_COMMIT,
        "tools_tree": TOOLS_TREE,
        "source_date_epoch": 0,
        "source_lock_sha256": sha256_bytes(source_lock),
        "profiles_sha256": sha256_bytes(profiles),
        "patches": []
    });
    recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
    serde_json::to_vec(&recipe).unwrap()
}

fn document_reference(name: &str, bytes: &[u8]) -> Value {
    json!({"file": name, "sha256": sha256_bytes(bytes)})
}

fn add_group(
    documents: &mut BTreeMap<String, Vec<u8>>,
    id: &str,
    source_commit: &str,
    source_lock: Vec<u8>,
    profiles: Vec<u8>,
) -> Value {
    let recipe = recipe_bytes(&source_lock, &profiles, source_commit);
    let entries = [
        (format!("{id}-recipe.json"), recipe),
        (format!("{id}-source-lock.json"), source_lock),
        (format!("{id}-profiles.json"), profiles),
    ];
    let refs = entries
        .iter()
        .map(|(name, bytes)| document_reference(name, bytes))
        .collect::<Vec<_>>();
    for (name, bytes) in entries {
        documents.insert(name, bytes);
    }
    json!({
        "id": id,
        "recipe": refs[0],
        "source_lock": refs[1],
        "profiles": refs[2]
    })
}

fn input_documents() -> (Value, BTreeMap<String, Vec<u8>>) {
    let mut documents = BTreeMap::new();
    let gnu = add_group(
        &mut documents,
        "gnu-riscv",
        GNU_SOURCE_COMMIT,
        gnu_source_lock(),
        gnu_profiles(),
    );
    let llvm = add_group(
        &mut documents,
        "llvm-pc",
        LLVM_SOURCE_COMMIT,
        llvm_source_lock(),
        llvm_profiles(),
    );
    (
        json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": ACTIVE_HOSTS,
            "groups": [gnu, llvm]
        }),
        documents,
    )
}

fn target_scope_input_documents() -> (Value, BTreeMap<String, Vec<u8>>) {
    let mut gnu_profile = gnu_profile(32);
    gnu_profile["name"] = json!("rv32-esp32p4");
    gnu_profile["configure_target"] = json!("esp32p4");
    gnu_profile["upstream_output_target"] = json!("esp32p4");
    let gnu_profiles = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "gnu",
        "upstream_commit": GNU_SOURCE_COMMIT,
        "profiles": [gnu_profile]
    }))
    .unwrap();

    let mut llvm_document: Value = serde_json::from_slice(&llvm_profiles()).unwrap();
    let pc_x86_64 = llvm_document["profiles"][0].clone();
    let mut arm_raspi = pc_x86_64.clone();
    arm_raspi["name"] = json!("arm-raspi");
    arm_raspi["configure_target"] = json!("arm-raspi");
    arm_raspi["upstream_output_target"] = json!("arm-raspi");
    arm_raspi["target_triple"] = json!("arm-unknown-aros");
    arm_raspi["cpu"] = json!("arm");
    arm_raspi["platform"] = json!("raspi");
    let mut rpi_aarch64 = pc_x86_64.clone();
    rpi_aarch64["name"] = json!("rpi-aarch64");
    rpi_aarch64["configure_target"] = json!("rpi-aarch64");
    rpi_aarch64["upstream_output_target"] = json!("rpi-aarch64");
    rpi_aarch64["target_triple"] = json!("aarch64-unknown-aros");
    rpi_aarch64["cpu"] = json!("aarch64");
    rpi_aarch64["platform"] = json!("raspi");
    llvm_document["profiles"] = json!([pc_x86_64, arm_raspi, rpi_aarch64]);
    let llvm_profiles = serde_json::to_vec(&llvm_document).unwrap();

    let mut documents = BTreeMap::new();
    let gnu = add_group(
        &mut documents,
        "gnu-riscv",
        GNU_SOURCE_COMMIT,
        gnu_source_lock(),
        gnu_profiles,
    );
    let llvm = add_group(
        &mut documents,
        "llvm-pc",
        LLVM_SOURCE_COMMIT,
        llvm_source_lock(),
        llvm_profiles,
    );
    (
        json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": ACTIVE_HOSTS,
            "groups": [gnu, llvm]
        }),
        documents,
    )
}

fn write_gnu_tool_layout(
    root: &Path,
    lock: &SourceLock,
    profile: &aros_toolchain::profiles::Profile,
) {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let bin = root.join("bin");
    let libexec = root.join("libexec");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&libexec).unwrap();
    let roles = json!({
        "c": "bin/fixture-c",
        "cxx": "bin/fixture-cxx",
        "assembler": "bin/fixture-as",
        "linker": "bin/fixture-ld",
        "archive": "bin/fixture-ar",
        "ranlib": "bin/fixture-ranlib",
        "strip": "bin/fixture-strip",
        "collector": "libexec/fixture-collect"
    });
    for relative in [
        "bin/fixture-c",
        "bin/fixture-cxx",
        "bin/fixture-ld",
        "bin/fixture-ar",
        "bin/fixture-ranlib",
        "bin/fixture-strip",
        "libexec/fixture-collect",
    ] {
        let path = root.join(relative);
        fs::write(&path, b"synthetic GNU executable fixture; never execute\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    symlink("fixture-c", bin.join("fixture-as")).unwrap();
    let binutils = lock
        .source_components()
        .find(|source| source.component() == "binutils")
        .unwrap();
    let compiler = ArosCompilerIdentity::Gnu {
        gcc_version: lock.version().to_owned(),
        binutils_version: binutils.version().to_owned(),
        target: profile.target().unwrap().clone(),
    };
    let layout = json!({
        "schema": "aros-toolchain-tools-v1",
        "compiler": compiler,
        "target_triple": profile.target_triple(),
        "tools": roles
    });
    fs::write(
        root.join("toolchain-tools.json"),
        serde_json::to_vec(&layout).unwrap(),
    )
    .unwrap();
}

fn fixture_asset_name(
    lock: &SourceLock,
    profile: &aros_toolchain::profiles::Profile,
    host: &str,
) -> String {
    match lock.family() {
        aros_toolchain::source_lock::CompilerFamily::Gnu => {
            let binutils = lock
                .source_components()
                .find(|source| source.component() == "binutils")
                .unwrap();
            format!(
                "aros-toolchain-v2-gcc{}-binutils{}-{host}-{}.tar.xz",
                lock.version(),
                binutils.version(),
                profile.name()
            )
        }
        aros_toolchain::source_lock::CompilerFamily::Llvm => format!(
            "aros-toolchain-v2-llvm{}-{host}-{}.tar.xz",
            lock.version(),
            profile.name()
        ),
    }
}

fn package_build_environment(host: &str, asset: &str) -> Map<String, Value> {
    json!({
        "fixture": "synthetic-integration-package-input",
        "host": host,
        "asset": asset,
        "toolchain_id": "fixture-toolchain"
    })
    .as_object()
    .unwrap()
    .clone()
}

fn readback_build_environment(host: &str, asset: &str) -> Map<String, Value> {
    // Re-created from index identity inputs, without reading package metadata.
    json!({
        "fixture": "synthetic-integration-package-input",
        "host": host,
        "asset": asset,
        "toolchain_id": "fixture-toolchain"
    })
    .as_object()
    .unwrap()
    .clone()
}

fn release_inventory_names(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FixtureChecksumMember {
    name: String,
    sha256: String,
    size: u64,
}

fn fixture_checksum_members(directory: &Path) -> Vec<FixtureChecksumMember> {
    let mut members = fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            (name != "SHA256SUMS").then_some((name, entry.path()))
        })
        .map(|(name, path)| {
            // Deliberately measure bytes here instead of using the production
            // read-back implementation or a checksum writer.
            let bytes = fs::read(path).unwrap();
            FixtureChecksumMember {
                name,
                sha256: sha256_bytes(&bytes).as_str().to_owned(),
                size: u64::try_from(bytes.len()).unwrap(),
            }
        })
        .collect::<Vec<_>>();
    members.sort_by(|left, right| left.name.cmp(&right.name));
    members
}

fn fixture_checksum_bytes(members: &[FixtureChecksumMember]) -> Vec<u8> {
    let mut lines = members
        .iter()
        .map(|member| format!("{}  {}\n", member.sha256, member.name))
        .collect::<Vec<_>>();
    lines.sort_unstable();
    lines.concat().into_bytes()
}

fn write_fixture_checksums(directory: &Path, members: &[FixtureChecksumMember]) {
    fs::write(
        directory.join("SHA256SUMS"),
        fixture_checksum_bytes(members),
    )
    .unwrap();
}

fn snapshot(directory: &Path) -> BTreeMap<String, SnapshotEntry> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let contents = if metadata.file_type().is_symlink() {
                SnapshotEntry::Symlink(fs::read_link(&path).unwrap())
            } else if metadata.is_file() {
                SnapshotEntry::File(fs::read(&path).unwrap())
            } else if metadata.is_dir() {
                SnapshotEntry::Directory
            } else {
                SnapshotEntry::Other
            };
            (name, contents)
        })
        .collect()
}

fn metadata_snapshot(directory: &Path) -> BTreeMap<String, MetadataSnapshotEntry> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let state = if metadata.file_type().is_symlink() {
                MetadataSnapshotEntry::Symlink(fs::read_link(&path).unwrap())
            } else if metadata.is_file() {
                MetadataSnapshotEntry::File {
                    size: metadata.len(),
                    modified: metadata.modified().ok(),
                }
            } else if metadata.is_dir() {
                MetadataSnapshotEntry::Directory
            } else {
                MetadataSnapshotEntry::Other
            };
            (name, state)
        })
        .collect()
}

fn assert_rejected_without_mutation(
    request: &IndexedPackageReadbackRequestV2,
    private_marker: Option<&str>,
) {
    let before = snapshot(&request.directory);
    let error = readback_indexed_packages(request).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(request.directory.to_str().unwrap()),
        "read-back diagnostic exposed the release path: {message}"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "read-back diagnostic exposed untrusted file contents: {message}"
        );
    }
    assert_eq!(snapshot(&request.directory), before);
}

fn assert_checksums_rejected_without_mutation(
    directory: &Path,
    index: &NativeReleaseIndexV2,
    private_marker: Option<&str>,
) {
    let before = snapshot(directory);
    let Err(error) = verify_final_checksums_v2(directory, index) else {
        panic!("checksum read-back accepted an invalid release state");
    };
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(directory.to_str().unwrap()),
        "checksum diagnostic exposed the release path"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "checksum diagnostic exposed untrusted file contents"
        );
    }
    let after = snapshot(directory);
    assert!(
        before == after,
        "checksum read-back changed release directory ({} members before, {} after)",
        before.len(),
        after.len()
    );
}

fn assert_checksum_metadata_limit_rejected_without_mutation(
    directory: &Path,
    index: &NativeReleaseIndexV2,
) {
    let before = metadata_snapshot(directory);
    let Err(error) = verify_final_checksums_v2(directory, index) else {
        panic!("checksum read-back accepted oversized checksum metadata");
    };
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    assert!(
        !error.to_string().contains(directory.to_str().unwrap()),
        "checksum metadata-limit diagnostic exposed the release path"
    );
    let after = metadata_snapshot(directory);
    assert!(
        before == after,
        "checksum metadata-limit read-back changed release metadata ({} members before, {} after)",
        before.len(),
        after.len()
    );
}

/// Build a valid four-member comparison receipt from two synthetic byte-copy
/// drivers. This exercises report parsing/persistence, not independent builds
/// or compiler behavior.
fn synthetic_comparison_report(
    fixture: &Fixture,
    lane: usize,
    label: &str,
) -> PackageComparisonReport {
    let artifact = &fixture.index.artifacts()[lane];
    let left = fixture.root.join(format!("comparison-{label}-left"));
    let right = fixture.root.join(format!("comparison-{label}-right"));
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    for name in [
        artifact.asset().to_owned(),
        format!("{}.manifest.json", artifact.asset()),
        format!("{}.sha256", artifact.asset()),
        format!("{}.spdx.json", artifact.asset()),
    ] {
        fs::copy(fixture.release_dir.join(&name), left.join(&name)).unwrap();
        fs::copy(fixture.release_dir.join(&name), right.join(&name)).unwrap();
    }

    let comparison = compare_package_sets(&left, &right).unwrap();
    let persisted = write_package_comparison_report(
        &fixture.root.join(format!("comparison-{label}.json")),
        &comparison,
    )
    .unwrap();
    persisted.report
}

fn recalculate_report_package_digest(report: &mut PackageComparisonReport) {
    let members = serde_json::to_value(&report.members).unwrap();
    report.package_set_sha256 = sha256_bytes(&canonical::bytes(&members).unwrap());
}

fn assert_comparison_binding_rejected_without_mutation(
    report: &PackageComparisonReport,
    artifact: &aros_toolchain::release_index_v2::NativeReleaseArtifactV2,
    readback: &aros_toolchain::release_checksums_v2::FinalChecksumsReadbackV2,
    directory: &Path,
) {
    let before = snapshot(directory);
    let metadata_before = metadata_snapshot(directory);
    let error = report
        .validate_against_checksums_v2(artifact, readback)
        .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerComparison,
        "expected AX0702 comparison diagnostic"
    );
    let message = error.to_string();
    assert!(
        !message.contains(directory.to_string_lossy().as_ref()),
        "comparison diagnostic exposed the release path: {message}"
    );
    assert_eq!(snapshot(directory), before);
    assert_eq!(metadata_snapshot(directory), metadata_before);
}

#[test]
fn comparison_report_v2_binds_all_nine_indexed_lanes_to_complete_checksum_readback() {
    let fixture = Fixture::build();
    let readback = verify_final_checksums_v2(&fixture.release_dir, &fixture.index).unwrap();
    let release_before = snapshot(&fixture.release_dir);
    let release_metadata_before = metadata_snapshot(&fixture.release_dir);
    let artifacts = fixture.index.artifacts();
    assert_eq!(artifacts.len(), 9);

    let reports = artifacts
        .iter()
        .enumerate()
        .map(|(lane, artifact)| {
            let report = synthetic_comparison_report(&fixture, lane, &format!("lane-{lane}"));
            assert_eq!(report.members[0].name, artifact.asset());
            report
                .validate_against_checksums_v2(artifact, &readback)
                .unwrap();
            report
        })
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 9);
    assert_eq!(snapshot(&fixture.release_dir), release_before);
    assert_eq!(
        metadata_snapshot(&fixture.release_dir),
        release_metadata_before
    );

    let report = &reports[0];
    let artifact = &artifacts[0];
    for member_index in 0..4 {
        let mut wrong_hash = report.clone();
        wrong_hash.members[member_index].sha256 =
            sha256_bytes(format!("counterprobe-wrong-hash-{member_index}").as_bytes());
        recalculate_report_package_digest(&mut wrong_hash);
        assert_comparison_binding_rejected_without_mutation(
            &wrong_hash,
            artifact,
            &readback,
            &fixture.release_dir,
        );

        let mut wrong_size = report.clone();
        wrong_size.members[member_index].size += 1;
        recalculate_report_package_digest(&mut wrong_size);
        assert_comparison_binding_rejected_without_mutation(
            &wrong_size,
            artifact,
            &readback,
            &fixture.release_dir,
        );
    }

    let wrong_lane_report = report.clone();
    assert_comparison_binding_rejected_without_mutation(
        &wrong_lane_report,
        &artifacts[1],
        &readback,
        &fixture.release_dir,
    );

    let mut wrong_schema = report.clone();
    wrong_schema.schema += 1;
    assert_comparison_binding_rejected_without_mutation(
        &wrong_schema,
        artifact,
        &readback,
        &fixture.release_dir,
    );
    let mut false_identity = report.clone();
    false_identity.byte_identical = false;
    assert_comparison_binding_rejected_without_mutation(
        &false_identity,
        artifact,
        &readback,
        &fixture.release_dir,
    );
    let mut inconsistent_digest = report.clone();
    inconsistent_digest.package_set_sha256 = sha256_bytes(b"wrong package-set digest");
    assert_comparison_binding_rejected_without_mutation(
        &inconsistent_digest,
        artifact,
        &readback,
        &fixture.release_dir,
    );
}

#[test]
fn comparison_report_v2_rejects_old_sbom_claim_after_fresh_checksum_readback() {
    let fixture = Fixture::build();
    let artifact = &fixture.index.artifacts()[0];
    let report = synthetic_comparison_report(&fixture, 0, "stale-sbom");
    let directory = fixture.copy_release("comparison-stale-sbom");
    let sbom_name = format!("{}.spdx.json", artifact.asset());
    let sbom_path = directory.join(&sbom_name);
    let mut changed_sbom = fs::read(&sbom_path).unwrap();
    changed_sbom.extend_from_slice(b"\nsynthetic post-report mutation\n");
    fs::write(&sbom_path, changed_sbom).unwrap();

    // The release checksum document is freshly measured after the SBOM byte
    // change. This validates the changed complete inventory independently;
    // it does not certify the SBOM's package semantics or compiler provenance.
    let fresh_members = fixture_checksum_members(&directory);
    write_fixture_checksums(&directory, &fresh_members);
    let changed_readback = verify_final_checksums_v2(&directory, &fixture.index).unwrap();
    let changed_member = changed_readback
        .members()
        .iter()
        .find(|member| member.name() == sbom_name)
        .unwrap();
    let old_claim = report
        .members
        .iter()
        .find(|member| member.name == sbom_name)
        .unwrap();
    assert_ne!(changed_member.sha256(), &old_claim.sha256);
    assert_ne!(changed_member.size(), old_claim.size);

    assert_comparison_binding_rejected_without_mutation(
        &report,
        artifact,
        &changed_readback,
        &directory,
    );
}

fn checksum_lines(directory: &Path) -> Vec<String> {
    String::from_utf8(fs::read(directory.join("SHA256SUMS")).unwrap())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn write_checksum_lines(directory: &Path, lines: &[String], trailing_lf: bool) {
    let mut contents = lines.join("\n");
    if trailing_lf {
        contents.push('\n');
    }
    fs::write(directory.join("SHA256SUMS"), contents).unwrap();
}

fn write_validated_index(
    directory: &Path,
    inputs: &ReleaseInputs,
    value: &Value,
) -> NativeReleaseIndexV2 {
    let index = NativeReleaseIndexV2::parse(&serde_json::to_vec(value).unwrap(), inputs).unwrap();
    fs::write(
        directory.join("toolchain-index-v2.json"),
        index.to_json_bytes().unwrap(),
    )
    .unwrap();
    index
}

fn assert_measured_index_rejected_without_mutation(
    request: &MeasuredReleaseIndexRequestV2,
    private_marker: Option<&str>,
) {
    let before = snapshot(&request.directory);
    let metadata_before = metadata_snapshot(&request.directory);
    let directory_modified_before = fs::metadata(&request.directory).unwrap().modified().ok();
    let error = build_measured_index_v2(request).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(request.directory.to_str().unwrap()),
        "measured-index diagnostic exposed the release path: {message}"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "measured-index diagnostic exposed untrusted input: {message}"
        );
    }
    assert_eq!(snapshot(&request.directory), before);
    assert_eq!(metadata_snapshot(&request.directory), metadata_before);
    assert_eq!(
        fs::metadata(&request.directory).unwrap().modified().ok(),
        directory_modified_before
    );
}

#[cfg(unix)]
fn assert_writer_rejected_without_mutation(
    request: &MeasuredReleaseIndexRequestV2,
    private_marker: Option<&str>,
) {
    let before = snapshot(&request.directory);
    let metadata_before = metadata_snapshot(&request.directory);
    let directory_modified_before = fs::metadata(&request.directory).unwrap().modified().ok();
    let error = write_measured_index_v2(request).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(request.directory.to_string_lossy().as_ref()),
        "index-writer diagnostic exposed the release path: {message}"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "index-writer diagnostic exposed untrusted file contents: {message}"
        );
    }
    assert_eq!(snapshot(&request.directory), before);
    assert_eq!(metadata_snapshot(&request.directory), metadata_before);
    assert_eq!(
        fs::metadata(&request.directory).unwrap().modified().ok(),
        directory_modified_before
    );
}

#[cfg(unix)]
fn assert_checksum_writer_rejected_without_mutation(
    request: &IndexedPackageReadbackRequestV2,
    checksum_was_absent: bool,
    private_marker: Option<&str>,
) {
    let before = snapshot(&request.directory);
    let metadata_before = metadata_snapshot(&request.directory);
    let directory_modified_before = fs::metadata(&request.directory).unwrap().modified().ok();
    let error = write_final_checksums_v2(request).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(request.directory.to_string_lossy().as_ref()),
        "checksum-writer diagnostic exposed the release path: {message}"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "checksum-writer diagnostic exposed untrusted input: {message}"
        );
    }
    assert_eq!(snapshot(&request.directory), before);
    assert_eq!(metadata_snapshot(&request.directory), metadata_before);
    assert_eq!(
        fs::metadata(&request.directory).unwrap().modified().ok(),
        directory_modified_before
    );
    if checksum_was_absent {
        assert!(
            fs::symlink_metadata(request.directory.join("SHA256SUMS")).is_err(),
            "checksum writer reserved an output before rejecting the stage"
        );
    }
}

#[cfg(unix)]
fn prepare_attestation_stage(
    fixture: &Fixture,
    name: &str,
) -> (PathBuf, IndexedPackageReadbackRequestV2, PathBuf) {
    let directory = fixture.copy_preindex_stage(name);
    let written =
        write_measured_index_v2(&fixture.measured_index_request(directory.clone())).unwrap();
    assert_eq!(written.index(), &fixture.index);
    let request = fixture.request(directory.clone());
    let manifest = fixture
        .root
        .join(format!("{name}-attestation-subjects.sha256"));
    (directory, request, manifest)
}

#[cfg(unix)]
fn assert_attestation_manifest_rejected_without_mutation(
    request: &IndexedPackageReadbackRequestV2,
    manifest: &Path,
    stage: AttestationManifestStageV2,
    private_marker: Option<&str>,
) {
    let release_before = snapshot(&request.directory);
    let release_metadata_before = metadata_snapshot(&request.directory);
    let release_modified_before = fs::metadata(&request.directory).unwrap().modified().ok();
    let manifest_parent = manifest.parent().unwrap();
    let parent_metadata_before = metadata_snapshot(manifest_parent);
    let parent_modified_before = fs::metadata(manifest_parent).unwrap().modified().ok();
    let manifest_before = regular_file_bytes(manifest);

    let error = verify_attestation_manifest_v2(request, manifest, stage).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(request.directory.to_string_lossy().as_ref()),
        "attestation-manifest diagnostic exposed the release path: {message}"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "attestation-manifest diagnostic exposed untrusted input: {message}"
        );
    }
    assert_eq!(snapshot(&request.directory), release_before);
    assert_eq!(
        metadata_snapshot(&request.directory),
        release_metadata_before
    );
    assert_eq!(
        fs::metadata(&request.directory).unwrap().modified().ok(),
        release_modified_before
    );
    assert_eq!(metadata_snapshot(manifest_parent), parent_metadata_before);
    assert_eq!(
        fs::metadata(manifest_parent).unwrap().modified().ok(),
        parent_modified_before
    );
    assert_eq!(regular_file_bytes(manifest), manifest_before);
}

#[cfg(unix)]
fn regular_file_bytes(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata.is_file().then(|| fs::read(path).ok()).flatten()
}

#[cfg(unix)]
fn assert_attestation_manifest_writer_rejected_without_mutation(
    request: &IndexedPackageReadbackRequestV2,
    output: &Path,
    private_marker: Option<&str>,
) {
    use std::os::unix::fs::MetadataExt as _;

    let release_before = snapshot(&request.directory);
    let release_metadata_before = metadata_snapshot(&request.directory);
    let release_modified_before = fs::metadata(&request.directory).unwrap().modified().ok();
    let output_parent = output.parent().unwrap();
    let parent_before = metadata_snapshot(output_parent);
    let parent_modified_before = fs::metadata(output_parent).unwrap().modified().ok();
    let output_before = fs::symlink_metadata(output).ok();
    let output_bytes_before = regular_file_bytes(output);

    let error = write_attestation_manifest_v2(request, output).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    let message = error.to_string();
    assert!(
        !message.contains(request.directory.to_string_lossy().as_ref()),
        "attestation-manifest writer diagnostic exposed the release path: {message}"
    );
    if let Some(marker) = private_marker {
        assert!(
            !message.contains(marker),
            "attestation-manifest writer diagnostic exposed untrusted input: {message}"
        );
    }
    assert_eq!(snapshot(&request.directory), release_before);
    assert_eq!(
        metadata_snapshot(&request.directory),
        release_metadata_before
    );
    assert_eq!(
        fs::metadata(&request.directory).unwrap().modified().ok(),
        release_modified_before
    );
    assert_eq!(metadata_snapshot(output_parent), parent_before);
    assert_eq!(
        fs::metadata(output_parent).unwrap().modified().ok(),
        parent_modified_before
    );
    let output_after = fs::symlink_metadata(output).ok();
    let fingerprint = |metadata: &std::fs::Metadata| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.mode(),
            metadata.len(),
            metadata.modified().ok(),
        )
    };
    assert_eq!(
        output_after.as_ref().map(fingerprint),
        output_before.as_ref().map(fingerprint)
    );
    assert_eq!(regular_file_bytes(output), output_bytes_before);
}

#[cfg(unix)]
fn assert_attestation_resource_limit_rejected_without_mutation(
    request: &IndexedPackageReadbackRequestV2,
    manifest: &Path,
    output: &Path,
    expected_reason: &str,
) {
    assert!(fs::symlink_metadata(output).is_err());
    let release_before = snapshot(&request.directory);
    let release_metadata_before = metadata_snapshot(&request.directory);
    let release_modified_before = fs::metadata(&request.directory).unwrap().modified().ok();
    let manifest_before = regular_file_bytes(manifest);
    let output_parent = output.parent().unwrap();
    let parent_before = metadata_snapshot(output_parent);
    let parent_modified_before = fs::metadata(output_parent).unwrap().modified().ok();

    let readback_error = verify_attestation_manifest_v2(
        request,
        manifest,
        AttestationManifestStageV2::PreAttestation,
    )
    .unwrap_err();
    let writer_error = write_attestation_manifest_v2(request, output).unwrap_err();
    for (operation, error) in [
        ("manifest readback", readback_error),
        ("manifest writer", writer_error),
    ] {
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerIndex,
            "{operation} returned the wrong diagnostic class"
        );
        assert!(
            error.to_string().contains(expected_reason),
            "{operation} did not report {expected_reason:?}: {error}"
        );
        assert_eq!(snapshot(&request.directory), release_before);
        assert_eq!(
            metadata_snapshot(&request.directory),
            release_metadata_before
        );
        assert_eq!(
            fs::metadata(&request.directory).unwrap().modified().ok(),
            release_modified_before
        );
        assert_eq!(regular_file_bytes(manifest), manifest_before);
        assert_eq!(metadata_snapshot(output_parent), parent_before);
        assert_eq!(
            fs::metadata(output_parent).unwrap().modified().ok(),
            parent_modified_before
        );
        assert!(
            fs::symlink_metadata(output).is_err(),
            "{operation} created its output before enforcing request resource bounds"
        );
    }
}

fn relative_path_from(base: &Path, target: &Path) -> PathBuf {
    let base = base.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..base.len() {
        relative.push("..");
    }
    for component in &target[common..] {
        relative.push(component.as_os_str());
    }
    relative
}

#[test]
fn indexed_package_readback_measures_all_nine_and_rejects_corrupt_or_inexact_release_state() {
    let fixture = Fixture::build();
    assert_eq!(
        fixture
            .inputs
            .hosts()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ACTIVE_HOSTS
    );
    assert_eq!(fixture.index.artifacts().len(), 9);

    let success = readback_indexed_packages(&fixture.request(fixture.release_dir.clone())).unwrap();
    assert_eq!(success.packages().len(), 9);
    assert_eq!(
        success
            .packages()
            .iter()
            .map(aros_toolchain::release_index_v2_readback::IndexedPackageV2::asset)
            .collect::<Vec<_>>(),
        fixture
            .index
            .artifacts()
            .iter()
            .map(aros_toolchain::release_index_v2::NativeReleaseArtifactV2::asset)
            .collect::<Vec<_>>()
    );
    for (measured, indexed) in success.packages().iter().zip(fixture.index.artifacts()) {
        assert_eq!(measured.asset(), indexed.asset());
        assert_eq!(&measured.package().archive_sha256, indexed.sha256());
        assert_eq!(measured.package().archive_size, indexed.size());
        assert_eq!(
            measured.package().manifest.tree_sha256,
            indexed.tree_sha256().as_str()
        );
    }

    let first_asset = fixture.index.artifacts()[0].asset().to_owned();

    for (case, name) in [
        (
            "corrupt-manifest-schema-support",
            "toolchain-manifest-v2.schema.json",
        ),
        ("corrupt-tree-digest-support", "tree-digest-v1.fixture.json"),
    ] {
        let directory = fixture.copy_release(case);
        let path = directory.join(name);
        let mut contents = fs::read(&path).unwrap();
        contents.extend_from_slice(PRIVATE_MARKER.as_bytes());
        fs::write(path, contents).unwrap();
        assert_rejected_without_mutation(&fixture.request(directory), Some(PRIVATE_MARKER));
    }

    for (case, suffix, marker) in [
        ("manifest", ".manifest.json", Some(PRIVATE_MARKER)),
        ("sbom", ".spdx.json", Some(PRIVATE_MARKER)),
        ("checksum", ".sha256", Some(PRIVATE_MARKER)),
    ] {
        let directory = fixture.copy_release(case);
        fs::write(
            directory.join(format!("{first_asset}{suffix}")),
            format!("{PRIVATE_MARKER}\n"),
        )
        .unwrap();
        assert_rejected_without_mutation(&fixture.request(directory), marker);
    }

    let directory = fixture.copy_release("archive");
    let archive_path = directory.join(&first_asset);
    let mut archive = fs::read(&archive_path).unwrap();
    archive[0] ^= 1;
    fs::write(&archive_path, archive).unwrap();
    assert_rejected_without_mutation(&fixture.request(directory), None);

    let directory = fixture.copy_release("extra-inventory");
    fs::write(
        directory.join("unindexed-extra.txt"),
        b"extra fixture member\n",
    )
    .unwrap();
    assert_rejected_without_mutation(&fixture.request(directory), None);

    let directory = fixture.copy_release("missing-inventory");
    fs::remove_file(directory.join("SHA256SUMS")).unwrap();
    assert_rejected_without_mutation(&fixture.request(directory), None);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let directory = fixture.copy_release("nonregular-inventory");
        symlink("SHA256SUMS", directory.join("unindexed-link")).unwrap();
        assert_rejected_without_mutation(&fixture.request(directory), None);
    }

    let directory = fixture.copy_release("changed-collection");
    let collection_path = directory.join("toolchain-release-inputs-v2.json");
    let mut collection = fs::read(&collection_path).unwrap();
    collection.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(&collection_path, collection).unwrap();
    assert_rejected_without_mutation(&fixture.request(directory), Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("changed-bound-document");
    let mut document = fs::read(directory.join("gnu-riscv-source-lock.json")).unwrap();
    document.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(directory.join("gnu-riscv-source-lock.json"), document).unwrap();
    assert_rejected_without_mutation(&fixture.request(directory), Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("changed-disk-index");
    let mut disk_index: Value =
        serde_json::from_slice(&fs::read(directory.join("toolchain-index-v2.json")).unwrap())
            .unwrap();
    disk_index["base_url"] = json!("https://example.invalid/aros/releases/substituted");
    fs::write(
        directory.join("toolchain-index-v2.json"),
        serde_json::to_vec(&disk_index).unwrap(),
    )
    .unwrap();
    assert_rejected_without_mutation(&fixture.request(directory), None);

    let directory = fixture.copy_release("missing-build-environment");
    let mut request = fixture.request(directory);
    request.build_environments.remove(&first_asset);
    assert_rejected_without_mutation(&request, None);

    let directory = fixture.copy_release("extra-build-environment");
    let mut request = fixture.request(directory);
    request.build_environments.insert(
        "unindexed-extra.tar.xz".to_owned(),
        Map::from_iter([("synthetic".to_owned(), json!(true))]),
    );
    assert_rejected_without_mutation(&request, None);

    for (case, field, value) in [
        ("wrong-index-hash", "sha256", json!("9".repeat(64))),
        ("wrong-index-tree", "tree_sha256", json!("8".repeat(64))),
    ] {
        let directory = fixture.copy_release(case);
        let mut index_value = fixture.index_value.clone();
        index_value["artifacts"][0][field] = value;
        let mut request = fixture.request(directory.clone());
        request.index = write_validated_index(&directory, &fixture.inputs, &index_value);
        assert_rejected_without_mutation(&request, None);
    }

    let directory = fixture.copy_release("wrong-index-size");
    let mut index_value = fixture.index_value.clone();
    let size = index_value["artifacts"][0]["size"].as_u64().unwrap();
    index_value["artifacts"][0]["size"] = json!(size + 1);
    let mut request = fixture.request(directory.clone());
    request.index = write_validated_index(&directory, &fixture.inputs, &index_value);
    assert_rejected_without_mutation(&request, None);

    let directory = fixture.copy_release("missing-required-path");
    let mut index_value = fixture.index_value.clone();
    index_value["artifacts"][0]["required_paths"] = json!(["missing/fixture-required-file"]);
    let mut request = fixture.request(directory.clone());
    request.index = write_validated_index(&directory, &fixture.inputs, &index_value);
    assert_rejected_without_mutation(&request, None);
}

#[test]
fn final_checksums_readback_measures_complete_flat_inventory_and_rejects_malformed_or_mismatched_state(
) {
    let fixture = Fixture::build();
    let checksums_path = fixture.release_dir.join("SHA256SUMS");
    let checksum_bytes = fs::read(&checksums_path).unwrap();
    let expected_members = fixture_checksum_members(&fixture.release_dir);
    assert_eq!(expected_members.len(), 47);
    assert!(
        checksum_bytes == fixture_checksum_bytes(&expected_members),
        "fixture SHA256SUMS differs from independent raw-file measurements"
    );
    assert!(expected_members
        .iter()
        .any(|member| member.name == "toolchain-provenance.sigstore.json"));

    let before = snapshot(&fixture.release_dir);
    let result = verify_final_checksums_v2(&fixture.release_dir, &fixture.index).unwrap();
    assert_eq!(result.members().len(), 47);
    let checksums_digest = sha256_bytes(&checksum_bytes);
    assert_eq!(
        result.checksums_sha256().as_str(),
        checksums_digest.as_str()
    );
    for (measured, expected) in result.members().iter().zip(&expected_members) {
        assert_eq!(measured.name(), expected.name);
        assert_eq!(measured.sha256().as_str(), expected.sha256);
        assert_eq!(measured.size(), expected.size);
    }
    assert!(
        before == snapshot(&fixture.release_dir),
        "successful checksum read-back changed the release directory"
    );

    let directory = fixture.copy_release("checksums-missing");
    fs::remove_file(directory.join("SHA256SUMS")).unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-duplicate");
    let mut lines = checksum_lines(&directory);
    let duplicate = lines[0].clone();
    lines.insert(1, duplicate);
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-extra");
    let mut lines = checksum_lines(&directory);
    lines.push(format!("{}  zz-{PRIVATE_MARKER}", "0".repeat(64)));
    lines.sort_unstable();
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-path-traversal");
    let mut lines = checksum_lines(&directory);
    lines.insert(0, format!("{}  ../{PRIVATE_MARKER}", "0".repeat(64)));
    lines.sort_unstable();
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-uppercase");
    let mut lines = checksum_lines(&directory);
    let line_index = lines
        .iter()
        .position(|line| line.as_bytes()[..64].iter().any(u8::is_ascii_lowercase))
        .unwrap();
    let digest = lines[line_index][..64].to_ascii_uppercase();
    lines[line_index].replace_range(..64, &digest);
    lines.sort_unstable();
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-one-space");
    let mut lines = checksum_lines(&directory);
    lines[0].replace_range(64..66, " ");
    lines.sort_unstable();
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-reordered");
    let mut lines = checksum_lines(&directory);
    lines.swap(0, 1);
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-no-newline");
    let mut contents = fs::read(directory.join("SHA256SUMS")).unwrap();
    assert_eq!(contents.pop(), Some(b'\n'));
    fs::write(directory.join("SHA256SUMS"), contents).unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-wrong-hash");
    let mut contents = fs::read(directory.join("SHA256SUMS")).unwrap();
    contents[0] = if contents[0] == b'0' { b'1' } else { b'0' };
    let mut lines = String::from_utf8(contents)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.sort_unstable();
    write_checksum_lines(&directory, &lines, true);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-changed-provenance");
    let provenance = directory.join("toolchain-provenance.sigstore.json");
    let mut contents = fs::read(&provenance).unwrap();
    contents.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(provenance, contents).unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-changed-archive");
    let archive = directory.join(fixture.index.artifacts()[0].asset());
    let mut contents = fs::read(&archive).unwrap();
    contents.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(archive, contents).unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-unexpected-member");
    fs::write(
        directory.join(format!("zz-{PRIVATE_MARKER}")),
        b"synthetic unindexed file\n",
    )
    .unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-nonregular-member");
    fs::create_dir(directory.join(format!("zz-{PRIVATE_MARKER}"))).unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-expected-directory-member");
    let expected_provenance = directory.join("toolchain-provenance.sigstore.json");
    fs::remove_file(&expected_provenance).unwrap();
    fs::create_dir(&expected_provenance).unwrap();
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let directory = fixture.copy_release("checksums-symlink-member");
        symlink("SHA256SUMS", directory.join(format!("zz-{PRIVATE_MARKER}"))).unwrap();
        assert_checksums_rejected_without_mutation(
            &directory,
            &fixture.index,
            Some(PRIVATE_MARKER),
        );

        let directory = fixture.copy_release("checksums-expected-symlink-member");
        let expected_provenance = directory.join("toolchain-provenance.sigstore.json");
        fs::remove_file(&expected_provenance).unwrap();
        symlink("SHA256SUMS", &expected_provenance).unwrap();
        assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);
    }

    for (case, field) in [
        ("checksums-index-hash-only", "sha256"),
        ("checksums-index-size-only", "size"),
    ] {
        let directory = fixture.copy_release(case);
        let mut index_value = fixture.index_value.clone();
        if field == "sha256" {
            index_value["artifacts"][0]["sha256"] = json!("9".repeat(64));
        } else {
            let size = index_value["artifacts"][0]["size"].as_u64().unwrap();
            index_value["artifacts"][0]["size"] = json!(size + 1);
        }
        let index = NativeReleaseIndexV2::parse(
            &serde_json::to_vec(&index_value).unwrap(),
            &fixture.inputs,
        )
        .unwrap();
        fs::write(
            directory.join("toolchain-index-v2.json"),
            index.to_json_bytes().unwrap(),
        )
        .unwrap();
        let remeasured_members = fixture_checksum_members(&directory);
        write_fixture_checksums(&directory, &remeasured_members);
        assert_checksums_rejected_without_mutation(&directory, &index, None);
    }

    let directory = fixture.copy_release("checksums-rehashed-index-mismatch");
    let archive_name = fixture.index.artifacts()[0].asset();
    let archive = directory.join(archive_name);
    let mut contents = fs::read(&archive).unwrap();
    contents.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(archive, contents).unwrap();
    let remeasured_members = fixture_checksum_members(&directory);
    let remeasured_archive = remeasured_members
        .iter()
        .find(|member| member.name == archive_name)
        .unwrap();
    let indexed_archive = &fixture.index.artifacts()[0];
    assert_ne!(remeasured_archive.sha256, indexed_archive.sha256().as_str());
    assert_ne!(remeasured_archive.size, indexed_archive.size());
    write_fixture_checksums(&directory, &remeasured_members);
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, Some(PRIVATE_MARKER));

    let directory = fixture.copy_release("checksums-substituted-canonical-index");
    let mut changed = fixture.index_value.clone();
    changed["base_url"] = json!("https://example.invalid/substituted-release");
    let changed_index =
        NativeReleaseIndexV2::parse(&serde_json::to_vec(&changed).unwrap(), &fixture.inputs)
            .unwrap();
    fs::write(
        directory.join("toolchain-index-v2.json"),
        changed_index.to_json_bytes().unwrap(),
    )
    .unwrap();
    write_fixture_checksums(&directory, &fixture_checksum_members(&directory));
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-noncanonical-index-bytes");
    let mut changed = fixture.index.to_json_bytes().unwrap();
    changed.push(b' ');
    fs::write(directory.join("toolchain-index-v2.json"), changed).unwrap();
    write_fixture_checksums(&directory, &fixture_checksum_members(&directory));
    assert_checksums_rejected_without_mutation(&directory, &fixture.index, None);

    let directory = fixture.copy_release("checksums-metadata-limit");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(directory.join("SHA256SUMS"))
        .unwrap();
    file.set_len(16_u64 * 1024 * 1024 + 1).unwrap();
    drop(file);
    assert_checksum_metadata_limit_rejected_without_mutation(&directory, &fixture.index);

    let directory = fixture.copy_release("checksums-member-metadata-limit");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(directory.join("toolchain-provenance.sigstore.json"))
        .unwrap();
    file.set_len(16_u64 * 1024 * 1024 + 1).unwrap();
    drop(file);
    assert_checksum_metadata_limit_rejected_without_mutation(&directory, &fixture.index);
}

#[test]
fn measured_index_v2_builder_measures_exact_nine_package_stage_without_mutation() {
    let fixture = Fixture::build();
    let directory = fixture.copy_preindex_stage("measured-index-success");
    for absent_final_output in [
        "toolchain-index-v2.json",
        "SHA256SUMS",
        "toolchain-provenance.sigstore.json",
    ] {
        assert!(!directory.join(absent_final_output).exists());
    }
    let before = snapshot(&directory);
    let metadata_before = metadata_snapshot(&directory);
    let directory_modified_before = fs::metadata(&directory).unwrap().modified().ok();

    let measured =
        build_measured_index_v2(&fixture.measured_index_request(directory.clone())).unwrap();
    assert_eq!(measured.artifacts().len(), 9);
    assert_eq!(
        measured.to_json_bytes().unwrap(),
        fixture.index.to_json_bytes().unwrap(),
        "measured bytes must match the independent package-build fixture index"
    );
    assert_eq!(snapshot(&directory), before);
    assert_eq!(metadata_snapshot(&directory), metadata_before);
    assert_eq!(
        fs::metadata(&directory).unwrap().modified().ok(),
        directory_modified_before
    );
    for absent_final_output in [
        "toolchain-index-v2.json",
        "SHA256SUMS",
        "toolchain-provenance.sigstore.json",
    ] {
        assert!(!directory.join(absent_final_output).exists());
    }

    let first_asset = fixture.index.artifacts()[0].asset().to_owned();
    for (case, missing_member) in [
        (
            "missing-package-archive",
            PathBuf::from(first_asset.as_str()),
        ),
        (
            "missing-package-manifest",
            PathBuf::from(format!("{first_asset}.manifest.json")),
        ),
    ] {
        let directory = fixture.copy_preindex_stage(case);
        fs::remove_file(directory.join(missing_member)).unwrap();
        assert_measured_index_rejected_without_mutation(
            &fixture.measured_index_request(directory),
            None,
        );
    }

    for (case, extra_name) in [
        ("extra-unindexed-file", "unindexed-extra.txt"),
        (
            "extra-package-archive",
            "aros-toolchain-v2-extra-fixture.tar.xz",
        ),
    ] {
        let directory = fixture.copy_preindex_stage(case);
        fs::write(
            directory.join(extra_name),
            b"synthetic extra inventory fixture\n",
        )
        .unwrap();
        assert_measured_index_rejected_without_mutation(
            &fixture.measured_index_request(directory),
            None,
        );
    }
}

#[test]
fn measured_index_v2_builder_rejects_inexact_lane_maps_receipts_and_required_paths() {
    let fixture = Fixture::build();
    let directory = fixture.copy_preindex_stage("measured-index-invalid-maps");
    let request = fixture.measured_index_request(directory);
    let first_asset = fixture.index.artifacts()[0].asset().to_owned();

    let mut missing_environment = request.clone();
    missing_environment.build_environments.remove(&first_asset);
    assert_measured_index_rejected_without_mutation(&missing_environment, None);

    let mut extra_environment = request.clone();
    extra_environment.build_environments.insert(
        "unindexed-extra.tar.xz".to_owned(),
        Map::from_iter([("synthetic".to_owned(), json!(true))]),
    );
    assert_measured_index_rejected_without_mutation(&extra_environment, None);

    let mut wrong_environment_key = request.clone();
    let receipt = wrong_environment_key
        .build_environments
        .remove(&first_asset)
        .unwrap();
    wrong_environment_key
        .build_environments
        .insert("wrong-canonical-archive.tar.xz".to_owned(), receipt);
    assert_measured_index_rejected_without_mutation(&wrong_environment_key, None);

    let mut wrong_receipt = request.clone();
    wrong_receipt
        .build_environments
        .get_mut(&first_asset)
        .unwrap()
        .insert("fixture".to_owned(), json!(PRIVATE_MARKER));
    assert_measured_index_rejected_without_mutation(&wrong_receipt, Some(PRIVATE_MARKER));

    let mut missing_required_paths = request.clone();
    missing_required_paths.required_paths.remove(&first_asset);
    assert_measured_index_rejected_without_mutation(&missing_required_paths, None);

    let mut extra_required_paths = request.clone();
    extra_required_paths.required_paths.insert(
        "unindexed-extra.tar.xz".to_owned(),
        vec!["fixture-input".to_owned()],
    );
    assert_measured_index_rejected_without_mutation(&extra_required_paths, None);

    let mut invalid_required_path = request.clone();
    invalid_required_path
        .required_paths
        .insert(first_asset.clone(), vec![format!("../{PRIVATE_MARKER}")]);
    assert_measured_index_rejected_without_mutation(&invalid_required_path, Some(PRIVATE_MARKER));

    let mut absent_payload_path = request;
    absent_payload_path
        .required_paths
        .insert(first_asset, vec![format!("missing/{PRIVATE_MARKER}")]);
    assert_measured_index_rejected_without_mutation(&absent_payload_path, Some(PRIVATE_MARKER));
}

#[test]
fn measured_index_v2_builder_sanitizes_document_archive_identity_and_symlink_failures() {
    let fixture = Fixture::build();

    for (case, name) in [
        (
            "changed-input-collection",
            "toolchain-release-inputs-v2.json",
        ),
        ("changed-source-document", "gnu-riscv-source-lock.json"),
        (
            "changed-manifest-schema",
            "toolchain-manifest-v2.schema.json",
        ),
        ("changed-tree-fixture", "tree-digest-v1.fixture.json"),
    ] {
        let directory = fixture.copy_preindex_stage(case);
        let path = directory.join(name);
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(PRIVATE_MARKER.as_bytes());
        fs::write(path, bytes).unwrap();
        assert_measured_index_rejected_without_mutation(
            &fixture.measured_index_request(directory),
            Some(PRIVATE_MARKER),
        );
    }

    let first_asset = fixture.index.artifacts()[0].asset().to_owned();
    for suffix in [".manifest.json", ".sha256", ".spdx.json"] {
        let case = format!("corrupt-package-sidecar-{}", suffix.trim_start_matches('.'));
        let directory = fixture.copy_preindex_stage(&case);
        fs::write(
            directory.join(format!("{first_asset}{suffix}")),
            PRIVATE_MARKER.as_bytes(),
        )
        .unwrap();
        assert_measured_index_rejected_without_mutation(
            &fixture.measured_index_request(directory),
            Some(PRIVATE_MARKER),
        );
    }

    let directory = fixture.copy_preindex_stage("corrupt-package-archive");
    let archive_path = directory.join(&first_asset);
    let mut archive = fs::read(&archive_path).unwrap();
    archive.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(&archive_path, archive).unwrap();
    assert_measured_index_rejected_without_mutation(
        &fixture.measured_index_request(directory),
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_preindex_stage("unsafe-release-id");
    let mut unsafe_release_id = fixture.measured_index_request(directory);
    unsafe_release_id.release_id = format!("../{PRIVATE_MARKER}");
    assert_measured_index_rejected_without_mutation(&unsafe_release_id, Some(PRIVATE_MARKER));

    let directory = fixture.copy_preindex_stage("unsafe-base-url");
    let mut unsafe_base_url = fixture.measured_index_request(directory);
    unsafe_base_url.base_url =
        format!("https://{PRIVATE_MARKER}@example.invalid/aros/releases/2026.10");
    assert_measured_index_rejected_without_mutation(&unsafe_base_url, Some(PRIVATE_MARKER));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let directory = fixture.copy_preindex_stage("symlink-package-archive");
        let archive_path = directory.join(&first_asset);
        fs::remove_file(&archive_path).unwrap();
        let other_asset = fixture.index.artifacts()[1].asset();
        symlink(other_asset, &archive_path).unwrap();
        assert_measured_index_rejected_without_mutation(
            &fixture.measured_index_request(directory),
            None,
        );
    }
}

#[test]
fn measured_index_v2_builder_rejects_final_outputs_and_preallocation_resource_excess() {
    let fixture = Fixture::build();
    for (case, final_output) in [
        ("premature-index", "toolchain-index-v2.json"),
        ("premature-checksums", "SHA256SUMS"),
        ("premature-provenance", "toolchain-provenance.sigstore.json"),
    ] {
        let directory = fixture.copy_preindex_stage(case);
        fs::write(directory.join(final_output), b"not a pre-index member\n").unwrap();
        assert_measured_index_rejected_without_mutation(
            &fixture.measured_index_request(directory),
            None,
        );
    }

    let directory = fixture.copy_preindex_stage("oversized-builder-request");
    let mut request = fixture.measured_index_request(directory);
    request.base_url = format!("https://example.invalid/{}", "a".repeat(16 * 1024 * 1024));
    let error = build_measured_index_v2(&request).unwrap_err();
    assert!(error
        .to_string()
        .contains("base URL exceeds the metadata bound"));
    assert_measured_index_rejected_without_mutation(&request, None);

    request.base_url = "https://example.invalid/aros/releases/2026.10".to_owned();
    let asset = fixture.index.artifacts()[0].asset().to_owned();
    let suffix = "p".repeat(750);
    request.required_paths.insert(
        asset,
        (0..23_000)
            .map(|number| format!("{number:06}-{suffix}"))
            .collect(),
    );
    let error = build_measured_index_v2(&request).unwrap_err();
    assert!(error
        .to_string()
        .contains("paths exceed the metadata bound"));
    assert_measured_index_rejected_without_mutation(&request, None);
}

#[test]
fn measured_index_v2_builder_bounds_environment_clones_and_prefix_selectors() {
    let fixture = Fixture::build();
    let directory = fixture.copy_preindex_stage("oversized-independent-material");
    let original = fixture.measured_index_request(directory);
    let asset = fixture.index.artifacts()[0].asset();

    let mut oversized = original.clone();
    oversized.build_environments.get_mut(asset).unwrap().insert(
        "oversized".to_owned(),
        Value::String("e".repeat(16 * 1024 * 1024)),
    );
    let error = build_measured_index_v2(&oversized).unwrap_err();
    assert!(error
        .to_string()
        .contains("environments exceed their aggregate metadata bound"));
    assert_measured_index_rejected_without_mutation(&oversized, None);
    drop(oversized);

    let mut deep = original.clone();
    let mut nested = Value::Null;
    for _ in 0..66 {
        nested = Value::Array(vec![nested]);
    }
    deep.build_environments
        .get_mut(asset)
        .unwrap()
        .insert("nested".to_owned(), nested);
    let error = build_measured_index_v2(&deep).unwrap_err();
    assert!(error.to_string().contains("nesting exceeds 64 levels"));
    assert_measured_index_rejected_without_mutation(&deep, None);

    let mut too_many_prefixes = original.clone();
    too_many_prefixes.forbidden_prefixes = vec![PathBuf::from("/synthetic-prefix"); 65];
    let error = build_measured_index_v2(&too_many_prefixes).unwrap_err();
    assert!(error
        .to_string()
        .contains("forbidden-prefix count exceeds its bound"));
    assert_measured_index_rejected_without_mutation(&too_many_prefixes, None);

    let mut huge_prefix = original;
    huge_prefix.forbidden_prefixes = vec![PathBuf::from(format!("/{}", "p".repeat(16 * 1024)))];
    let error = build_measured_index_v2(&huge_prefix).unwrap_err();
    assert!(error
        .to_string()
        .contains("forbidden-prefix bytes exceed their bound"));
    assert_measured_index_rejected_without_mutation(&huge_prefix, None);
}

#[test]
#[cfg(unix)]
fn written_index_v2_persists_exact_canonical_measurement_without_changing_existing_members() {
    let fixture = Fixture::build();
    let directory = fixture.copy_preindex_stage("written-index-success");
    let request = fixture.measured_index_request(directory.clone());
    let before = snapshot(&directory);
    let metadata_before = metadata_snapshot(&directory);
    let expected_bytes = fixture.index.to_json_bytes().unwrap();

    let written = write_measured_index_v2(&request).unwrap();

    let index_path = directory.join("toolchain-index-v2.json");
    assert_eq!(written.index(), &fixture.index);
    assert_eq!(written.path(), index_path);
    assert_eq!(written.sha256(), &sha256_bytes(&expected_bytes));
    assert_eq!(written.size(), expected_bytes.len() as u64);
    assert_eq!(fs::read(&index_path).unwrap(), expected_bytes);
    assert_eq!(
        written.index().to_json_bytes().unwrap(),
        fs::read(&index_path).unwrap(),
        "the persisted bytes must be the canonical bytes of the returned index"
    );

    let after = snapshot(&directory);
    let added = after
        .keys()
        .filter(|name| !before.contains_key(*name))
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(added, ["toolchain-index-v2.json"]);
    for (name, entry) in &before {
        assert_eq!(
            after.get(name),
            Some(entry),
            "existing member changed: {name}"
        );
    }
    let metadata_after = metadata_snapshot(&directory);
    for (name, metadata) in &metadata_before {
        assert_eq!(
            metadata_after.get(name),
            Some(metadata),
            "existing member metadata changed: {name}"
        );
    }
    assert!(!directory.join("SHA256SUMS").exists());
    assert!(!directory
        .join("toolchain-provenance.sigstore.json")
        .exists());
}

#[test]
#[cfg(unix)]
fn written_index_v2_rejects_retry_and_existing_index_symlink_or_directory_without_changes() {
    let fixture = Fixture::build();

    let directory = fixture.copy_preindex_stage("written-index-retry");
    let request = fixture.measured_index_request(directory.clone());
    let expected_bytes = fixture.index.to_json_bytes().unwrap();
    write_measured_index_v2(&request).unwrap();
    assert_writer_rejected_without_mutation(&request, None);
    assert_eq!(
        fs::read(directory.join("toolchain-index-v2.json")).unwrap(),
        expected_bytes
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let directory = fixture.copy_preindex_stage("written-index-symlink");
        let target = fixture.root.join("writer-symlink-target");
        fs::write(&target, b"preserve this symlink target\n").unwrap();
        symlink(&target, directory.join("toolchain-index-v2.json")).unwrap();
        let request = fixture.measured_index_request(directory);
        assert_writer_rejected_without_mutation(&request, None);
        assert_eq!(fs::read(target).unwrap(), b"preserve this symlink target\n");

        let directory = fixture.copy_preindex_stage("written-index-directory");
        let output_directory = directory.join("toolchain-index-v2.json");
        fs::create_dir(&output_directory).unwrap();
        let sentinel = output_directory.join("sentinel");
        fs::write(&sentinel, b"preserve this existing directory member\n").unwrap();
        let request = fixture.measured_index_request(directory);
        assert_writer_rejected_without_mutation(&request, None);
        assert_eq!(
            fs::read(sentinel).unwrap(),
            b"preserve this existing directory member\n"
        );
    }
}

#[test]
#[cfg(unix)]
fn written_index_v2_rejects_incomplete_or_untrusted_stage_before_creating_output() {
    let fixture = Fixture::build();
    let first_asset = fixture.index.artifacts()[0].asset().to_owned();

    let directory = fixture.copy_preindex_stage("written-index-missing-package");
    fs::remove_file(directory.join(&first_asset)).unwrap();
    assert_writer_rejected_without_mutation(&fixture.measured_index_request(directory), None);

    let directory = fixture.copy_preindex_stage("written-index-extra-package");
    fs::write(
        directory.join("aros-toolchain-v2-extra-fixture.tar.xz"),
        b"unindexed package fixture\n",
    )
    .unwrap();
    assert_writer_rejected_without_mutation(&fixture.measured_index_request(directory), None);

    let directory = fixture.copy_preindex_stage("written-index-tampered-package");
    fs::write(
        directory.join(format!("{first_asset}.manifest.json")),
        PRIVATE_MARKER.as_bytes(),
    )
    .unwrap();
    assert_writer_rejected_without_mutation(
        &fixture.measured_index_request(directory),
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_preindex_stage("written-index-missing-support");
    fs::remove_file(directory.join("tree-digest-v1.fixture.json")).unwrap();
    assert_writer_rejected_without_mutation(&fixture.measured_index_request(directory), None);

    let directory = fixture.copy_preindex_stage("written-index-extra-support");
    fs::write(
        directory.join("unindexed-support.json"),
        b"extra support fixture\n",
    )
    .unwrap();
    assert_writer_rejected_without_mutation(&fixture.measured_index_request(directory), None);

    let directory = fixture.copy_preindex_stage("written-index-tampered-support");
    let support = directory.join("toolchain-manifest-v2.schema.json");
    let mut contents = fs::read(&support).unwrap();
    contents.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(support, contents).unwrap();
    assert_writer_rejected_without_mutation(
        &fixture.measured_index_request(directory),
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_preindex_stage("written-index-missing-lane-map");
    let mut request = fixture.measured_index_request(directory);
    request.build_environments.remove(&first_asset);
    assert_writer_rejected_without_mutation(&request, None);

    let directory = fixture.copy_preindex_stage("written-index-extra-lane-map");
    let mut request = fixture.measured_index_request(directory);
    request.required_paths.insert(
        "unindexed-extra.tar.xz".to_owned(),
        vec!["fixture-input".to_owned()],
    );
    assert_writer_rejected_without_mutation(&request, None);
}

#[test]
#[cfg(unix)]
fn written_index_v2_rejects_symlink_ancestors_and_relative_directories_without_output() {
    let fixture = Fixture::build();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let directory = fixture.copy_preindex_stage("written-index-symlink-ancestor");
        let linked_root = fixture.root.join("written-index-linked-root");
        symlink(&fixture.root, &linked_root).unwrap();
        let mut request = fixture.measured_index_request(directory.clone());
        request.directory = linked_root.join(directory.file_name().unwrap());
        assert_writer_rejected_without_mutation(&request, None);
        assert!(!directory.join("toolchain-index-v2.json").exists());
    }

    let directory = fixture.copy_preindex_stage("written-index-relative-directory");
    let current = std::env::current_dir().unwrap();
    let relative = relative_path_from(&current, &directory);
    assert!(!relative.is_absolute());
    let mut request = fixture.measured_index_request(directory.clone());
    request.directory = relative;
    assert_writer_rejected_without_mutation(&request, None);
    assert!(!directory.join("toolchain-index-v2.json").exists());
}

#[test]
#[cfg(unix)]
fn checksum_writer_v2_persists_exact_independent_inventory_and_only_adds_checksums() {
    let fixture = Fixture::build();
    let directory = fixture.copy_prechecksum_stage("written-checksums-success");
    let request = fixture.request(directory.clone());
    let before = snapshot(&directory);
    let metadata_before = metadata_snapshot(&directory);
    let expected_members = fixture_checksum_members(&directory);
    assert_eq!(expected_members.len(), 47);
    assert!(expected_members
        .iter()
        .any(|member| member.name == "toolchain-provenance.sigstore.json"));
    let expected_bytes = fixture_checksum_bytes(&expected_members);
    assert!(fs::symlink_metadata(directory.join("SHA256SUMS")).is_err());

    let written = write_final_checksums_v2(&request).unwrap();

    let checksum_path = directory.join("SHA256SUMS");
    assert_eq!(written.path(), checksum_path.as_path());
    assert_eq!(written.sha256(), &sha256_bytes(&expected_bytes));
    assert_eq!(written.size(), expected_bytes.len() as u64);
    assert_eq!(fs::read(&checksum_path).unwrap(), expected_bytes);
    assert_eq!(
        written.readback().checksums_sha256(),
        &sha256_bytes(&expected_bytes)
    );
    assert_eq!(written.readback().members().len(), expected_members.len());
    for (measured, expected) in written.readback().members().iter().zip(&expected_members) {
        assert_eq!(measured.name(), expected.name);
        assert_eq!(measured.sha256().as_str(), expected.sha256);
        assert_eq!(measured.size(), expected.size);
    }
    assert_eq!(
        readback_indexed_packages(&request)
            .unwrap()
            .packages()
            .len(),
        9
    );

    let after = snapshot(&directory);
    let added = after
        .keys()
        .filter(|name| !before.contains_key(*name))
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(added, ["SHA256SUMS"]);
    for (name, entry) in &before {
        assert_eq!(
            after.get(name),
            Some(entry),
            "existing member changed: {name}"
        );
    }
    let metadata_after = metadata_snapshot(&directory);
    for (name, metadata) in &metadata_before {
        assert_eq!(
            metadata_after.get(name),
            Some(metadata),
            "existing member metadata changed: {name}"
        );
    }
}

#[test]
#[cfg(unix)]
fn checksum_writer_v2_rejects_retry_and_existing_file_link_directory_or_fifo() {
    use std::os::unix::fs::{symlink, MetadataExt as _};

    let fixture = Fixture::build();

    let directory = fixture.copy_prechecksum_stage("written-checksums-retry");
    let request = fixture.request(directory.clone());
    write_final_checksums_v2(&request).unwrap();
    let written_bytes = fs::read(directory.join("SHA256SUMS")).unwrap();
    assert_checksum_writer_rejected_without_mutation(&request, false, None);
    assert_eq!(
        fs::read(directory.join("SHA256SUMS")).unwrap(),
        written_bytes
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-existing-file");
    fs::write(directory.join("SHA256SUMS"), PRIVATE_MARKER.as_bytes()).unwrap();
    let request = fixture.request(directory.clone());
    assert_checksum_writer_rejected_without_mutation(&request, false, Some(PRIVATE_MARKER));
    assert_eq!(
        fs::read(directory.join("SHA256SUMS")).unwrap(),
        PRIVATE_MARKER.as_bytes()
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-existing-link");
    let target = fixture.root.join("written-checksums-link-target");
    fs::write(&target, b"preserve this existing checksum link target\n").unwrap();
    symlink(&target, directory.join("SHA256SUMS")).unwrap();
    let request = fixture.request(directory.clone());
    assert_checksum_writer_rejected_without_mutation(&request, false, None);
    assert_eq!(
        fs::read_link(directory.join("SHA256SUMS")).unwrap(),
        target.as_path()
    );
    assert_eq!(
        fs::read(target).unwrap(),
        b"preserve this existing checksum link target\n"
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-existing-directory");
    let output_directory = directory.join("SHA256SUMS");
    fs::create_dir(&output_directory).unwrap();
    let sentinel = output_directory.join("sentinel");
    fs::write(&sentinel, b"preserve this directory obstruction\n").unwrap();
    let directory_before = fs::symlink_metadata(&output_directory).unwrap();
    let request = fixture.request(directory);
    assert_checksum_writer_rejected_without_mutation(&request, false, None);
    let directory_after = fs::symlink_metadata(&output_directory).unwrap();
    assert_eq!(directory_after.ino(), directory_before.ino());
    assert_eq!(
        directory_after.modified().ok(),
        directory_before.modified().ok()
    );
    assert_eq!(
        fs::read(sentinel).unwrap(),
        b"preserve this directory obstruction\n"
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-existing-fifo");
    let fifo_path = directory.join("SHA256SUMS");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo_path)
            .status()
            .unwrap()
            .success(),
        "could not create FIFO obstruction"
    );
    let fifo_before = fs::symlink_metadata(&fifo_path).unwrap();
    let request = fixture.request(directory);
    assert_checksum_writer_rejected_without_mutation(&request, false, None);
    let fifo_after = fs::symlink_metadata(&fifo_path).unwrap();
    assert_eq!(fifo_after.ino(), fifo_before.ino());
    assert_eq!(fifo_after.mode(), fifo_before.mode());
    assert_eq!(fifo_after.modified().ok(), fifo_before.modified().ok());
}

#[test]
#[cfg(unix)]
fn checksum_writer_v2_rejects_inexact_or_tampered_stage_before_reserving_output() {
    let fixture = Fixture::build();
    let first_asset = fixture.index.artifacts()[0].asset().to_owned();

    let directory = fixture.copy_prechecksum_stage("written-checksums-missing-archive");
    fs::remove_file(directory.join(&first_asset)).unwrap();
    assert_checksum_writer_rejected_without_mutation(&fixture.request(directory), true, None);

    let directory = fixture.copy_prechecksum_stage("written-checksums-extra-member");
    fs::write(
        directory.join(format!("{PRIVATE_MARKER}.extra")),
        b"unindexed fixture member\n",
    )
    .unwrap();
    assert_checksum_writer_rejected_without_mutation(
        &fixture.request(directory),
        true,
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-missing-provenance");
    fs::remove_file(directory.join("toolchain-provenance.sigstore.json")).unwrap();
    assert_checksum_writer_rejected_without_mutation(&fixture.request(directory), true, None);
}

#[test]
#[cfg(unix)]
fn checksum_writer_v2_rejects_tampered_packages_support_documents_and_index_before_reservation() {
    let fixture = Fixture::build();
    let first_asset = fixture.index.artifacts()[0].asset().to_owned();

    let directory = fixture.copy_prechecksum_stage("written-checksums-tampered-archive");
    let archive = directory.join(&first_asset);
    let mut bytes = fs::read(&archive).unwrap();
    bytes.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(archive, bytes).unwrap();
    assert_checksum_writer_rejected_without_mutation(
        &fixture.request(directory),
        true,
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-tampered-sidecar");
    fs::write(
        directory.join(format!("{first_asset}.manifest.json")),
        PRIVATE_MARKER.as_bytes(),
    )
    .unwrap();
    assert_checksum_writer_rejected_without_mutation(
        &fixture.request(directory),
        true,
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-tampered-static");
    let support = directory.join("toolchain-manifest-v2.schema.json");
    let mut bytes = fs::read(&support).unwrap();
    bytes.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(support, bytes).unwrap();
    assert_checksum_writer_rejected_without_mutation(
        &fixture.request(directory),
        true,
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-tampered-bound-document");
    let document = directory.join("gnu-riscv-source-lock.json");
    let mut bytes = fs::read(&document).unwrap();
    bytes.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(document, bytes).unwrap();
    assert_checksum_writer_rejected_without_mutation(
        &fixture.request(directory),
        true,
        Some(PRIVATE_MARKER),
    );

    let directory = fixture.copy_prechecksum_stage("written-checksums-noncanonical-index");
    let index = directory.join("toolchain-index-v2.json");
    let mut bytes = fs::read(&index).unwrap();
    bytes.push(b' ');
    fs::write(index, bytes).unwrap();
    assert_checksum_writer_rejected_without_mutation(&fixture.request(directory), true, None);
}

#[test]
#[cfg(unix)]
fn checksum_writer_v2_rejects_environment_and_forbidden_prefix_failures_before_reservation() {
    let fixture = Fixture::build();
    let first_asset = fixture.index.artifacts()[0].asset().to_owned();

    let directory = fixture.copy_prechecksum_stage("written-checksums-missing-environment");
    let mut request = fixture.request(directory);
    request.build_environments.remove(&first_asset);
    assert_checksum_writer_rejected_without_mutation(&request, true, None);

    let directory = fixture.copy_prechecksum_stage("written-checksums-mismatched-environment");
    let mut request = fixture.request(directory);
    request
        .build_environments
        .get_mut(&first_asset)
        .unwrap()
        .insert("fixture".to_owned(), json!(PRIVATE_MARKER));
    assert_checksum_writer_rejected_without_mutation(&request, true, Some(PRIVATE_MARKER));

    let directory = fixture.copy_prechecksum_stage("written-checksums-forbidden-prefix");
    let mut request = fixture.request(directory);
    // The synthetic GNU tools receipt contains slash-delimited tool paths, so
    // the absolute root prefix is present in an archived regular-file payload.
    request.forbidden_prefixes = vec![PathBuf::from("/")];
    assert_checksum_writer_rejected_without_mutation(&request, true, None);
}

#[test]
#[cfg(unix)]
fn checksum_writer_v2_rejects_symlink_member_and_ancestor_before_reservation() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::build();

    let directory = fixture.copy_prechecksum_stage("written-checksums-symlink-member");
    let provenance = directory.join("toolchain-provenance.sigstore.json");
    fs::remove_file(&provenance).unwrap();
    symlink("toolchain-index-v2.json", &provenance).unwrap();
    assert_checksum_writer_rejected_without_mutation(&fixture.request(directory), true, None);

    let directory = fixture.copy_prechecksum_stage("written-checksums-symlink-ancestor");
    let linked_root = fixture.root.join("written-checksums-linked-root");
    symlink(&fixture.root, &linked_root).unwrap();
    let mut request = fixture.request(directory.clone());
    request.directory = linked_root.join(directory.file_name().unwrap());
    assert_checksum_writer_rejected_without_mutation(&request, true, None);
    assert!(fs::symlink_metadata(directory.join("SHA256SUMS")).is_err());
}

#[test]
#[cfg(unix)]
fn attestation_manifest_v2_tracks_subjects_across_non_circular_release_stages() {
    let fixture = Fixture::build();
    let (directory, request, manifest_path) =
        prepare_attestation_stage(&fixture, "attestation-manifest-lifecycle");
    assert!(!directory.join("SHA256SUMS").exists());
    assert!(!directory
        .join("toolchain-provenance.sigstore.json")
        .exists());
    assert!(!manifest_path.exists());

    let release_before = snapshot(&directory);
    let release_metadata_before = metadata_snapshot(&directory);
    let mut expected_members = fixture_checksum_members(&directory);
    expected_members.retain(|member| member.name != "toolchain-provenance.sigstore.json");
    assert_eq!(
        expected_members.len(),
        fixture.index.expected_inventory().len() - 2
    );
    let expected_bytes = fixture_checksum_bytes(&expected_members);

    let written = write_attestation_manifest_v2(&request, &manifest_path).unwrap();

    assert_eq!(written.members().len(), expected_members.len());
    for (measured, expected) in written.members().iter().zip(&expected_members) {
        assert_eq!(measured.name(), expected.name);
        assert_eq!(measured.sha256().as_str(), expected.sha256);
        assert_eq!(measured.size(), expected.size);
    }
    assert_eq!(fs::read(&manifest_path).unwrap(), expected_bytes);
    assert_eq!(written.sha256(), &sha256_bytes(&expected_bytes));
    assert_eq!(written.size(), expected_bytes.len() as u64);
    assert!(!directory.join(manifest_path.file_name().unwrap()).exists());
    assert_eq!(snapshot(&directory), release_before);
    assert_eq!(metadata_snapshot(&directory), release_metadata_before);

    let pre_attestation = verify_attestation_manifest_v2(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
    )
    .unwrap();
    assert_eq!(pre_attestation.sha256(), written.sha256());
    assert_eq!(pre_attestation.members(), written.members());
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreChecksums,
        None,
    );
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::Final,
        None,
    );

    fs::copy(
        fixture
            .release_dir
            .join("toolchain-provenance.sigstore.json"),
        directory.join("toolchain-provenance.sigstore.json"),
    )
    .unwrap();
    let pre_checksums = verify_attestation_manifest_v2(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreChecksums,
    )
    .unwrap();
    assert_eq!(pre_checksums.sha256(), written.sha256());
    assert_eq!(pre_checksums.members(), written.members());
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
        None,
    );

    let final_checksums = write_final_checksums_v2(&request).unwrap();
    assert_ne!(written.sha256(), final_checksums.sha256());
    let final_manifest =
        verify_attestation_manifest_v2(&request, &manifest_path, AttestationManifestStageV2::Final)
            .unwrap();
    assert_eq!(final_manifest.sha256(), written.sha256());
    assert_eq!(final_manifest.members(), written.members());
    assert_eq!(
        final_checksums.readback().checksums_sha256(),
        final_checksums.sha256()
    );
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreChecksums,
        None,
    );

    let checksum_path = directory.join("SHA256SUMS");
    let checksum_bytes = fs::read(&checksum_path).unwrap();
    fs::write(&checksum_path, PRIVATE_MARKER.as_bytes()).unwrap();
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::Final,
        Some(PRIVATE_MARKER),
    );
    fs::write(checksum_path, checksum_bytes).unwrap();
}

#[test]
#[cfg(unix)]
fn attestation_manifest_v2_rejects_noncanonical_manifest_and_changed_subjects() {
    let fixture = Fixture::build();
    let (directory, request, manifest_path) =
        prepare_attestation_stage(&fixture, "attestation-manifest-tampering");
    write_attestation_manifest_v2(&request, &manifest_path).unwrap();
    let canonical = fs::read(&manifest_path).unwrap();
    let canonical_lines = String::from_utf8(canonical.clone())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert!(canonical_lines.len() > 2);

    let mut altered = canonical_lines.clone();
    let replacement = if altered[0].as_bytes()[0] == b'0' {
        "1"
    } else {
        "0"
    };
    altered[0].replace_range(..1, replacement);
    let mut reordered = canonical_lines.clone();
    reordered.swap(0, 1);
    let mut duplicate = canonical_lines.clone();
    duplicate.insert(1, duplicate[0].clone());
    let mut missing = canonical_lines.clone();
    missing.pop();
    let mut unknown = canonical_lines;
    unknown.push(format!("{}  {PRIVATE_MARKER}", "0".repeat(64)));
    unknown.sort_unstable();

    for (lines, marker) in [
        (altered, None),
        (reordered, None),
        (duplicate, None),
        (missing, None),
        (unknown, Some(PRIVATE_MARKER)),
    ] {
        let invalid = format!("{}\n", lines.join("\n")).into_bytes();
        fs::write(&manifest_path, invalid).unwrap();
        assert_attestation_manifest_rejected_without_mutation(
            &request,
            &manifest_path,
            AttestationManifestStageV2::PreAttestation,
            marker,
        );
    }
    fs::write(&manifest_path, &canonical).unwrap();

    for name in [
        fixture.index.artifacts()[0].asset().to_owned(),
        format!("{}.manifest.json", fixture.index.artifacts()[0].asset()),
        "toolchain-manifest-v2.schema.json".to_owned(),
        "toolchain-release-inputs-v2.json".to_owned(),
        "toolchain-index-v2.json".to_owned(),
    ] {
        let path = directory.join(name);
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed.extend_from_slice(PRIVATE_MARKER.as_bytes());
        fs::write(&path, changed).unwrap();
        assert_attestation_manifest_rejected_without_mutation(
            &request,
            &manifest_path,
            AttestationManifestStageV2::PreAttestation,
            Some(PRIVATE_MARKER),
        );
        fs::write(path, original).unwrap();
    }

    let asset = fixture.index.artifacts()[0].asset().to_owned();
    let asset_path = directory.join(&asset);
    let asset_bytes = fs::read(&asset_path).unwrap();
    fs::remove_file(&asset_path).unwrap();
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
    fs::write(&asset_path, asset_bytes).unwrap();

    let extra = directory.join(format!("{PRIVATE_MARKER}.extra"));
    fs::write(&extra, b"unindexed file\n").unwrap();
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
        Some(PRIVATE_MARKER),
    );
    fs::remove_file(extra).unwrap();

    let mut missing_environment = request.clone();
    missing_environment
        .build_environments
        .remove(fixture.index.artifacts()[0].asset());
    assert_attestation_manifest_rejected_without_mutation(
        &missing_environment,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
    let mut unknown_environment = request.clone();
    unknown_environment
        .build_environments
        .insert(format!("{PRIVATE_MARKER}.archive"), Map::new());
    assert_attestation_manifest_rejected_without_mutation(
        &unknown_environment,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
    let mut forbidden_prefix = request;
    forbidden_prefix.forbidden_prefixes = vec![PathBuf::from("/")];
    assert_attestation_manifest_rejected_without_mutation(
        &forbidden_prefix,
        &manifest_path,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
}

#[test]
#[cfg(unix)]
fn attestation_manifest_v2_rejects_unsafe_output_and_manifest_paths_without_mutation() {
    use std::os::unix::fs::{symlink, MetadataExt as _};

    let fixture = Fixture::build();
    let (directory, request, _) =
        prepare_attestation_stage(&fixture, "attestation-manifest-output-safety");

    let inside_release = directory.join("subject-manifest.sha256");
    assert_attestation_manifest_writer_rejected_without_mutation(&request, &inside_release, None);
    assert!(!inside_release.exists());

    let existing = fixture.root.join("attestation-existing-file");
    fs::write(&existing, PRIVATE_MARKER.as_bytes()).unwrap();
    assert_attestation_manifest_writer_rejected_without_mutation(
        &request,
        &existing,
        Some(PRIVATE_MARKER),
    );
    assert_eq!(fs::read(&existing).unwrap(), PRIVATE_MARKER.as_bytes());

    let symlink_target = fixture.root.join("attestation-symlink-target");
    fs::write(&symlink_target, b"preserve link target\n").unwrap();
    let symlink_output = fixture.root.join("attestation-existing-symlink");
    symlink(&symlink_target, &symlink_output).unwrap();
    let symlink_before = fs::read_link(&symlink_output).unwrap();
    assert_attestation_manifest_writer_rejected_without_mutation(&request, &symlink_output, None);
    assert_eq!(fs::read_link(&symlink_output).unwrap(), symlink_before);
    assert_eq!(
        fs::read(&symlink_target).unwrap(),
        b"preserve link target\n"
    );

    let hardlink_target = fixture.root.join("attestation-hardlink-target");
    fs::write(&hardlink_target, b"preserve hardlink target\n").unwrap();
    let hardlink_output = fixture.root.join("attestation-existing-hardlink");
    fs::hard_link(&hardlink_target, &hardlink_output).unwrap();
    let hardlink_before = fs::symlink_metadata(&hardlink_output).unwrap();
    assert_attestation_manifest_writer_rejected_without_mutation(&request, &hardlink_output, None);
    let hardlink_after = fs::symlink_metadata(&hardlink_output).unwrap();
    assert_eq!(hardlink_after.ino(), hardlink_before.ino());
    assert_eq!(hardlink_after.nlink(), hardlink_before.nlink());
    assert_eq!(
        fs::read(&hardlink_target).unwrap(),
        b"preserve hardlink target\n"
    );

    let directory_output = fixture.root.join("attestation-existing-directory");
    fs::create_dir(&directory_output).unwrap();
    let sentinel = directory_output.join("sentinel");
    fs::write(&sentinel, b"preserve output directory\n").unwrap();
    let directory_before = fs::symlink_metadata(&directory_output).unwrap();
    assert_attestation_manifest_writer_rejected_without_mutation(&request, &directory_output, None);
    let directory_after = fs::symlink_metadata(&directory_output).unwrap();
    assert_eq!(directory_after.ino(), directory_before.ino());
    assert_eq!(
        directory_after.modified().ok(),
        directory_before.modified().ok()
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserve output directory\n");

    let fifo_output = fixture.root.join("attestation-existing-fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo_output)
        .status()
        .unwrap()
        .success());
    let fifo_before = fs::symlink_metadata(&fifo_output).unwrap();
    assert_attestation_manifest_writer_rejected_without_mutation(&request, &fifo_output, None);
    let fifo_after = fs::symlink_metadata(&fifo_output).unwrap();
    assert_eq!(fifo_after.ino(), fifo_before.ino());
    assert_eq!(fifo_after.mode(), fifo_before.mode());
    assert_eq!(fifo_after.modified().ok(), fifo_before.modified().ok());

    let valid_manifest = fixture.root.join("attestation-valid-for-path-tests.sha256");
    write_attestation_manifest_v2(&request, &valid_manifest).unwrap();
    let linked_parent = fixture.root.join("attestation-symlink-parent");
    symlink(&fixture.root, &linked_parent).unwrap();
    let through_symlink_parent = linked_parent.join("attestation-through-symlink.sha256");
    assert_attestation_manifest_writer_rejected_without_mutation(
        &request,
        &through_symlink_parent,
        None,
    );
    assert!(!fixture
        .root
        .join("attestation-through-symlink.sha256")
        .exists());
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &linked_parent.join(valid_manifest.file_name().unwrap()),
        AttestationManifestStageV2::PreAttestation,
        None,
    );

    let symlink_manifest = fixture.root.join("attestation-manifest-symlink");
    symlink(&valid_manifest, &symlink_manifest).unwrap();
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &symlink_manifest,
        AttestationManifestStageV2::PreAttestation,
        None,
    );

    let hardlinked_manifest = fixture.root.join("attestation-manifest-hardlink");
    fs::hard_link(&valid_manifest, &hardlinked_manifest).unwrap();
    let manifest_links_before = fs::symlink_metadata(&valid_manifest).unwrap().nlink();
    assert_eq!(manifest_links_before, 2);
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &hardlinked_manifest,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
    assert_eq!(
        fs::symlink_metadata(&valid_manifest).unwrap().nlink(),
        manifest_links_before
    );

    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &directory_output,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
    assert_attestation_manifest_rejected_without_mutation(
        &request,
        &fifo_output,
        AttestationManifestStageV2::PreAttestation,
        None,
    );
}

#[test]
#[cfg(unix)]
fn attestation_manifest_v2_enforces_request_resource_bounds_before_readback_or_output() {
    let fixture = Fixture::build();
    let (_, request, manifest) = prepare_attestation_stage(&fixture, "attestation-resource-limits");
    write_attestation_manifest_v2(&request, &manifest).unwrap();
    let asset = fixture.index.artifacts()[0].asset().to_owned();

    let mut oversized_environment = request.clone();
    oversized_environment
        .build_environments
        .get_mut(&asset)
        .unwrap()
        .insert(
            "oversized".to_owned(),
            Value::String("e".repeat(16 * 1024 * 1024)),
        );
    assert_attestation_resource_limit_rejected_without_mutation(
        &oversized_environment,
        &manifest,
        &fixture.root.join("attestation-resource-output-environment"),
        "environments exceed their aggregate metadata bound",
    );
    drop(oversized_environment);

    let mut deep_environment = request.clone();
    let mut nested = Value::Null;
    for _ in 0..66 {
        nested = Value::Array(vec![nested]);
    }
    deep_environment
        .build_environments
        .get_mut(&asset)
        .unwrap()
        .insert("nested".to_owned(), nested);
    assert_attestation_resource_limit_rejected_without_mutation(
        &deep_environment,
        &manifest,
        &fixture.root.join("attestation-resource-output-depth"),
        "environment nesting exceeds 64 levels",
    );

    let mut too_many_prefixes = request.clone();
    too_many_prefixes.forbidden_prefixes = vec![PathBuf::from("/synthetic-prefix"); 65];
    assert_attestation_resource_limit_rejected_without_mutation(
        &too_many_prefixes,
        &manifest,
        &fixture
            .root
            .join("attestation-resource-output-prefix-count"),
        "forbidden-prefix count exceeds its bound",
    );

    let mut oversized_prefix = request.clone();
    oversized_prefix.forbidden_prefixes =
        vec![PathBuf::from(format!("/{}", "p".repeat(16 * 1024)))];
    assert_attestation_resource_limit_rejected_without_mutation(
        &oversized_prefix,
        &manifest,
        &fixture
            .root
            .join("attestation-resource-output-prefix-bytes"),
        "forbidden-prefix bytes exceed their bound",
    );

    let mut excessive_environment_count = request.clone();
    excessive_environment_count
        .build_environments
        .insert(format!("{PRIVATE_MARKER}.archive"), Map::new());
    assert_attestation_resource_limit_rejected_without_mutation(
        &excessive_environment_count,
        &manifest,
        &fixture
            .root
            .join("attestation-resource-output-environment-count"),
        "build-environment map must contain exactly the canonical indexed archives",
    );

    let mut long_wrong_environment_key = request;
    let removed_key = long_wrong_environment_key
        .build_environments
        .keys()
        .next()
        .unwrap()
        .clone();
    let environment = long_wrong_environment_key
        .build_environments
        .remove(&removed_key)
        .unwrap();
    long_wrong_environment_key
        .build_environments
        .insert(format!("z{}", "x".repeat(1024 * 1024)), environment);
    assert_eq!(
        long_wrong_environment_key.build_environments.len(),
        fixture.index.artifacts().len()
    );
    assert_attestation_resource_limit_rejected_without_mutation(
        &long_wrong_environment_key,
        &manifest,
        &fixture.root.join("attestation-resource-output-long-key"),
        "build-environment map must contain exactly the canonical indexed archives",
    );
}

#[test]
#[cfg(unix)]
fn attestation_manifest_v2_target_scope_has_twelve_verified_lanes_and_exact_stage_counts() {
    let fixture = Fixture::build_target_scope();
    assert_eq!(fixture.inputs.groups().len(), 2);
    assert_eq!(fixture.inputs.hosts().len(), 3);
    assert_eq!(fixture.index.artifacts().len(), 12);
    assert_eq!(fixture.index.expected_inventory().len(), 60);
    assert_eq!(release_inventory_names(&fixture.release_dir).len(), 60);

    let actual_group_profiles = fixture
        .inputs
        .groups()
        .iter()
        .flat_map(|group| {
            group
                .profiles()
                .entries()
                .iter()
                .map(|profile| (group.id().to_owned(), profile.name().to_owned()))
        })
        .collect::<BTreeSet<_>>();
    let expected_group_profiles = [
        ("gnu-riscv", "rv32-esp32p4"),
        ("llvm-pc", "pc-x86_64"),
        ("llvm-pc", "arm-raspi"),
        ("llvm-pc", "rpi-aarch64"),
    ]
    .into_iter()
    .map(|(group, profile)| (group.to_owned(), profile.to_owned()))
    .collect::<BTreeSet<_>>();
    assert_eq!(actual_group_profiles, expected_group_profiles);
    assert!(!actual_group_profiles
        .iter()
        .any(|(_, profile)| profile.starts_with("rv64")));

    let actual_lanes = fixture
        .index
        .artifacts()
        .iter()
        .map(|artifact| {
            (
                artifact.group_id().to_owned(),
                artifact.target_profile().to_owned(),
                artifact.host().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    let mut expected_lanes = BTreeSet::new();
    for host in ACTIVE_HOSTS {
        for (group, profile) in [
            ("gnu-riscv", "rv32-esp32p4"),
            ("llvm-pc", "pc-x86_64"),
            ("llvm-pc", "arm-raspi"),
            ("llvm-pc", "rpi-aarch64"),
        ] {
            expected_lanes.insert((group.to_owned(), profile.to_owned(), (*host).to_owned()));
        }
    }
    assert_eq!(actual_lanes, expected_lanes);

    let (directory, request, manifest) =
        prepare_attestation_stage(&fixture, "attestation-target-scope");
    let subjects = write_attestation_manifest_v2(&request, &manifest).unwrap();
    assert_eq!(subjects.members().len(), 58);
    assert!(!subjects.members().iter().any(|member| matches!(
        member.name(),
        "SHA256SUMS" | "toolchain-provenance.sigstore.json"
    )));
    assert_eq!(release_inventory_names(&directory).len(), 58);

    fs::copy(
        fixture
            .release_dir
            .join("toolchain-provenance.sigstore.json"),
        directory.join("toolchain-provenance.sigstore.json"),
    )
    .unwrap();
    assert_eq!(release_inventory_names(&directory).len(), 59);
    let pre_checksums = verify_attestation_manifest_v2(
        &request,
        &manifest,
        AttestationManifestStageV2::PreChecksums,
    )
    .unwrap();
    assert_eq!(pre_checksums.members().len(), 58);
    let final_checksums = write_final_checksums_v2(&request).unwrap();
    assert_eq!(final_checksums.readback().members().len(), 59);
    assert_eq!(release_inventory_names(&directory).len(), 60);
    let final_subjects =
        verify_attestation_manifest_v2(&request, &manifest, AttestationManifestStageV2::Final)
            .unwrap();
    assert_eq!(final_subjects.members().len(), 58);
    assert_eq!(final_subjects.sha256(), subjects.sha256());
}
