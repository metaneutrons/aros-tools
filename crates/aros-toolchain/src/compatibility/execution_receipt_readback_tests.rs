//! Synthetic counterprobes for bounded native compatibility receipt read-back.

use std::collections::BTreeMap;

use aros_common::elf::Class;
use aros_common::{
    sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, ArosToolchainManifestEntry,
    Sha256Digest,
};
use serde_json::{json, Value};

use super::{
    readback_native_compatibility_receipt, CompatibilityReceiptArtifact,
    CompatibilityReceiptDocument, CompatibilityReceiptPackage, CompatibilityReceiptPhase,
    CompatibilityReceiptPortsSource, CompatibilityReceiptTarget, NativeCompatibilityCommandLogs,
    NativeCompatibilityExpectedPackage, NativeCompatibilityExpectedPortSource,
    NativeCompatibilityReceiptReadbackRequest,
};
use crate::compatibility::{
    CompatibilityCommandReport, CompatibilityHelperReport, CompatibilityHostToolReport,
    CompatibilityPhase, CompatibilityProbeReport, StandaloneArtifactIdentity,
    StandaloneOutputReport, StandaloneTargetReport,
};
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;

const UPSTREAM_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PACKAGE_COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const UPSTREAM_TREE: &str = "cccccccccccccccccccccccccccccccccccccccc";

#[test]
fn source_v2_requirement_cannot_accept_an_older_gnu_receipt() {
    let fixture = Fixture::new(true);
    let mut request = fixture.request();
    request.native_sdk_required = true;
    let failure = readback_native_compatibility_receipt(&request).unwrap_err();
    assert!(failure.to_string().contains("schema does not match"));
}

#[test]
fn llvm_cannot_select_the_gnu_sdk_proof_boundary() {
    let fixture = Fixture::new(false);
    let mut request = fixture.request();
    request.native_sdk_required = true;
    let failure = readback_native_compatibility_receipt(&request).unwrap_err();
    assert!(failure
        .to_string()
        .contains("selected GNU consumer-v2 build"));
}

pub struct Fixture {
    pub(crate) manifest: ArosToolchainManifest,
    pub(crate) archive_sha256: Sha256Digest,
    pub(crate) archive_size: u64,
    pub(crate) compiler: ArosCompilerIdentity,
    pub(crate) package_source_commit: GitObjectId,
    pub(crate) host: String,
    pub(crate) profiles: Profiles,
    pub(crate) profile: Profile,
    pub(crate) source_preset: Option<String>,
    pub(crate) cmake_build_required: bool,
    pub(crate) sdk_source_tree: Sha256Digest,
    engine_sha256: Sha256Digest,
    helpers: BTreeMap<String, CompatibilityHelperReport>,
    pub(crate) host_tools: BTreeMap<String, CompatibilityHostToolReport>,
    sdk_environment: Sha256Digest,
    standalone_environment: Sha256Digest,
    pub(crate) upstream_commit: GitObjectId,
    pub(crate) upstream_tree: GitObjectId,
    ports: Vec<NativeCompatibilityExpectedPortSource>,
    pub(crate) standalone: StandaloneOutputReport,
    pub(crate) phase_reports: BTreeMap<CompatibilityPhase, Vec<u8>>,
    pub(crate) command_logs: BTreeMap<CompatibilityPhase, Vec<NativeCompatibilityCommandLogs>>,
    pub(crate) receipt: Vec<u8>,
}

impl Fixture {
    pub(crate) fn new(gnu: bool) -> Self {
        Self::with_cmake_build_required(gnu, gnu)
    }

    fn gnu_configure_only() -> Self {
        Self::with_cmake_build_required(true, false)
    }

    pub(crate) fn with_cmake_build_required(gnu: bool, cmake_build_required: bool) -> Self {
        let (profiles, profile, compiler, preset) = if gnu {
            let profiles_bytes = serde_json::to_vec(&json!({
                "schema": "aros-toolchain-profiles-v2",
                "family": "gnu",
                "upstream_commit": UPSTREAM_COMMIT,
                "profiles": [{
                    "name": "rv32-compat",
                    "configure_target": "fixture-rv32",
                    "upstream_output_target": "fixture-rv32",
                    "target_triple": "riscv-aros",
                    "cpu": "riscv",
                    "platform": "fixture",
                    "float_abi": "ilp32f",
                    "capabilities": [
                        "c", "cxx", "libgcc", "libstdcxx", "libsupcxx",
                        "standalone-collector"
                    ],
                    "target": {
                        "schema": "aros-riscv-target-v1",
                        "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                        "abi": "ilp32f",
                        "code_model": "medany",
                        "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
                        "unaligned_access": false,
                        "atomic_abi": 0,
                        "x3_reg_usage": 0
                    }
                }]
            })).unwrap();
            let profiles = Profiles::parse(&profiles_bytes).unwrap();
            let profile = profiles.select("rv32-compat").unwrap().clone();
            let compiler = ArosCompilerIdentity::Gnu {
                gcc_version: "16.2.0".into(),
                binutils_version: "2.47".into(),
                target: profile.target().unwrap().clone(),
            };
            (
                profiles,
                profile,
                compiler,
                Some("source-rv32-preset".to_owned()),
            )
        } else {
            let profiles_bytes = serde_json::to_vec(&json!({
                "schema": "aros-toolchain-profiles-v1",
                "upstream_commit": UPSTREAM_COMMIT,
                "profiles": [{
                    "name": "pc-x86_64",
                    "configure_target": "pc-x86_64",
                    "upstream_output_target": "pc-x86_64",
                    "target_triple": "x86_64-unknown-aros",
                    "cpu": "x86_64",
                    "platform": "pc",
                    "float_abi": "",
                    "capabilities": ["c", "cxx", "standalone-collector"]
                }]
            }))
            .unwrap();
            let profiles = Profiles::parse(&profiles_bytes).unwrap();
            let profile = profiles.select("pc-x86_64").unwrap().clone();
            let compiler = ArosCompilerIdentity::Llvm {
                version: "18.1.0".into(),
            };
            (profiles, profile, compiler, None)
        };
        let package_source_commit = GitObjectId::try_from(PACKAGE_COMMIT.to_owned()).unwrap();
        let upstream_commit = GitObjectId::try_from(UPSTREAM_COMMIT.to_owned()).unwrap();
        let upstream_tree = GitObjectId::try_from(UPSTREAM_TREE.to_owned()).unwrap();
        let host = "linux-x86_64".to_owned();
        let archive_sha256 = digest("1");
        let manifest = ArosToolchainManifest {
            schema: 2,
            release_id: "readback-fixture".into(),
            host: host.clone(),
            target_profile: profile.name().into(),
            target_triple: profile.target_triple().into(),
            tree_sha256: digest("2").to_string(),
            llvm_version: None,
            compiler: Some(compiler.clone()),
            recipe_sha256: digest("3").to_string(),
            source_lock_sha256: digest("4").to_string(),
            profiles_sha256: profile.document_sha256().to_string(),
            source_commit: package_source_commit.as_str().into(),
            producer_commit: "dddddddddddddddddddddddddddddddddddddddd".into(),
            tools_commit: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
            source_date_epoch: 1,
            capabilities: profile.capabilities().to_vec(),
            build_environment: serde_json::Map::new(),
            files: vec![ArosToolchainManifestEntry {
                path: "bin/compiler".into(),
                mode: "0755".into(),
                kind: "file".into(),
                sha256: Some(digest("5").to_string()),
                size: Some(1),
                target: None,
            }],
        };
        manifest.validate().unwrap();
        let sdk_source_tree = digest("6");
        let engine_sha256 = digest("7");
        let helpers = crate::compatibility::REQUIRED_HELPERS
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    CompatibilityHelperReport {
                        sha256: digest("8"),
                        size: 12,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let host_tools = crate::compatibility::native_compatibility_host_tools(&host)
            .unwrap()
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    CompatibilityHostToolReport {
                        sha256: digest("9"),
                        size: 14,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let sdk_environment = digest("a");
        let standalone_environment = digest("b");
        let ports = vec![NativeCompatibilityExpectedPortSource {
            id: "bzip2".into(),
            cache_filename: "bzip2-1.0.8.tar.gz".into(),
            relative_path: "ports/bzip2-1.0.8.tar.gz".into(),
            fetch_marker: ".bzip2-1.0.8-fetched".into(),
            sha256: digest("c"),
            size: 128,
        }];
        let standalone = standalone_report(&profile);

        let mut phase_reports = BTreeMap::new();
        let mut command_logs = BTreeMap::new();
        for phase in crate::compatibility::REQUIRED_PROBE_PHASES {
            let command_count = match phase {
                CompatibilityPhase::CmakeConsumer => 1 + usize::from(cmake_build_required),
                CompatibilityPhase::UpstreamConfigure
                | CompatibilityPhase::UpstreamIncludes
                | CompatibilityPhase::UpstreamLinklibs => 1,
                CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx => {
                    standalone.targets.len()
                }
            };
            let mut logs = Vec::new();
            let mut commands = Vec::new();
            for index in 0..command_count {
                let stdout = format!("stdout-{}-{index}", phase_name(phase)).into_bytes();
                let stderr = format!("stderr-{}-{index}", phase_name(phase)).into_bytes();
                commands.push(CompatibilityCommandReport {
                    program_sha256: digest("d"),
                    command_sha256: digest("e"),
                    stdout_sha256: sha256_bytes(&stdout),
                    stderr_sha256: sha256_bytes(&stderr),
                });
                logs.push(NativeCompatibilityCommandLogs { stdout, stderr });
            }
            let standalone_phase = matches!(
                phase,
                CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx
            );
            let report = CompatibilityProbeReport {
                schema: "aros-toolchain-compatibility-report-v5".into(),
                phase,
                engine_api_version: 3,
                engine_sha256: engine_sha256.clone(),
                source_tree_sha256: sdk_source_tree.clone(),
                helpers: helpers.clone(),
                host_tools: if standalone_phase {
                    BTreeMap::new()
                } else {
                    host_tools.clone()
                },
                environment_sha256: if standalone_phase {
                    standalone_environment.clone()
                } else {
                    sdk_environment.clone()
                },
                commands,
            };
            let encoded = crate::canonical::bytes(&serde_json::to_value(&report).unwrap()).unwrap();
            phase_reports.insert(phase, encoded);
            command_logs.insert(phase, logs);
        }
        let fixture = Self {
            manifest,
            archive_sha256,
            archive_size: 1024,
            compiler,
            package_source_commit,
            host,
            profiles,
            profile,
            source_preset: preset,
            cmake_build_required,
            sdk_source_tree,
            engine_sha256,
            helpers,
            host_tools,
            sdk_environment,
            standalone_environment,
            upstream_commit,
            upstream_tree,
            ports,
            standalone,
            phase_reports,
            command_logs,
            receipt: Vec::new(),
        };
        let mut fixture = fixture;
        fixture.rebuild_receipt();
        fixture
    }

    pub(crate) fn request(&self) -> NativeCompatibilityReceiptReadbackRequest<'_> {
        NativeCompatibilityReceiptReadbackRequest {
            receipt_bytes: &self.receipt,
            phase_report_bytes: &self.phase_reports,
            command_logs: &self.command_logs,
            package: NativeCompatibilityExpectedPackage {
                manifest: &self.manifest,
                archive_sha256: &self.archive_sha256,
                archive_size: self.archive_size,
                compiler: &self.compiler,
                source_commit: &self.package_source_commit,
                host: &self.host,
            },
            profiles: &self.profiles,
            profile: &self.profile,
            gnu_source_preset: self.source_preset.as_deref(),
            cmake_build_required: self.cmake_build_required,
            native_sdk_required: false,
            native_sdk: None,
            sdk_consumer_source_tree_sha256: &self.sdk_source_tree,
            engine_api_version: 3,
            engine_sha256: &self.engine_sha256,
            helpers: &self.helpers,
            host_tools: &self.host_tools,
            sdk_environment_sha256: &self.sdk_environment,
            standalone_environment_sha256: &self.standalone_environment,
            upstream_source_commit: &self.upstream_commit,
            upstream_source_tree: &self.upstream_tree,
            ports_sources: &self.ports,
            standalone: &self.standalone,
        }
    }

    pub(crate) fn rebuild_receipt(&mut self) {
        let compiler = self.manifest.compiler_identity().unwrap();
        let manifest_bytes =
            crate::canonical::bytes(&serde_json::to_value(&self.manifest).unwrap()).unwrap();
        let package = CompatibilityReceiptPackage {
            compiler,
            host: self.manifest.host.clone(),
            target_profile: self.profile.name().into(),
            target_triple: self.profile.target_triple().into(),
            archive_sha256: self.archive_sha256.clone(),
            archive_size: self.archive_size,
            manifest_sha256: sha256_bytes(&manifest_bytes),
            tree_sha256: Sha256Digest::parse(&self.manifest.tree_sha256).unwrap(),
            source_preset: self.source_preset.clone(),
            source_tree_sha256: self.sdk_source_tree.clone(),
        };
        let document = CompatibilityReceiptDocument {
            schema: if matches!(self.compiler, ArosCompilerIdentity::Gnu { .. }) {
                "aros-toolchain-native-compatibility-receipt-v3".into()
            } else {
                "aros-toolchain-native-compatibility-receipt-v4".into()
            },
            operation: "native-compatibility".into(),
            upstream_source_commit: self.upstream_commit.as_str().into(),
            upstream_source_tree: self.upstream_tree.as_str().into(),
            ports_sources: self
                .ports
                .iter()
                .map(|port| CompatibilityReceiptPortsSource {
                    id: port.id.clone(),
                    cache_filename: port.cache_filename.clone(),
                    relative_path: port.relative_path.clone(),
                    fetch_marker: port.fetch_marker.clone(),
                    sha256: port.sha256.clone(),
                    size: port.size,
                })
                .collect(),
            phase_reports: crate::compatibility::REQUIRED_PROBE_PHASES
                .into_iter()
                .map(|phase| CompatibilityReceiptPhase {
                    phase,
                    report_sha256: sha256_bytes(&self.phase_reports[&phase]),
                })
                .collect(),
            standalone_targets: self
                .standalone
                .targets
                .iter()
                .map(|(triple, target)| {
                    (
                        triple.clone(),
                        CompatibilityReceiptTarget {
                            c: receipt_artifact(&target.c),
                            cxx: receipt_artifact(&target.cxx),
                        },
                    )
                })
                .collect(),
            package: Some(package),
            native_sdk: None,
        };
        document.validate().unwrap();
        self.receipt = crate::canonical::bytes(&serde_json::to_value(&document).unwrap()).unwrap();
    }
}

#[test]
fn reads_back_gnu_rv32_v3_receipt_and_exact_logs() {
    let fixture = Fixture::new(true);
    let result = readback_native_compatibility_receipt(&fixture.request()).unwrap();
    assert_eq!(result.receipt_sha256, sha256_bytes(&fixture.receipt));
    assert_eq!(
        result.phases,
        crate::compatibility::REQUIRED_PROBE_PHASES
            .into_iter()
            .collect()
    );
    assert_eq!(fixture.profile.name(), "rv32-compat");
}

#[test]
fn reads_back_gnu_rv32_configure_only_receipt_with_one_cmake_command() {
    let fixture = Fixture::gnu_configure_only();
    let result = readback_native_compatibility_receipt(&fixture.request()).unwrap();
    assert_eq!(result.receipt_sha256, sha256_bytes(&fixture.receipt));
    assert_eq!(
        fixture.command_logs[&CompatibilityPhase::CmakeConsumer].len(),
        1
    );
}

#[test]
fn reads_back_llvm_v4_receipt_with_x64_and_i386_outputs() {
    let fixture = Fixture::new(false);
    let result = readback_native_compatibility_receipt(&fixture.request()).unwrap();
    assert_eq!(result.receipt_sha256, sha256_bytes(&fixture.receipt));
    assert_eq!(fixture.standalone.targets.len(), 2);
    assert!(fixture
        .standalone
        .targets
        .contains_key("x86_64-unknown-aros"));
    assert!(fixture.standalone.targets.contains_key("i386-unknown-aros"));
}

#[test]
fn rejects_consistently_rehashed_sdk_environment_claims_without_independent_match() {
    let mut fixture = Fixture::new(true);
    let changed = digest("f");
    for phase in [
        CompatibilityPhase::CmakeConsumer,
        CompatibilityPhase::UpstreamConfigure,
        CompatibilityPhase::UpstreamIncludes,
        CompatibilityPhase::UpstreamLinklibs,
    ] {
        let mut report: CompatibilityProbeReport =
            serde_json::from_slice(&fixture.phase_reports[&phase]).unwrap();
        report.environment_sha256 = changed.clone();
        fixture.phase_reports.insert(
            phase,
            crate::canonical::bytes(&serde_json::to_value(&report).unwrap()).unwrap(),
        );
    }
    fixture.rebuild_receipt();
    let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
    assert!(error.to_string().contains(
        "SDK phase host-tool or environment identity differs from independent expectation"
    ));
}

#[test]
fn rejects_changed_actual_log_bytes_even_when_the_receipt_is_valid() {
    let mut fixture = Fixture::new(true);
    fixture
        .command_logs
        .get_mut(&CompatibilityPhase::StandaloneC)
        .unwrap()[0]
        .stdout
        .push(b'!');
    let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
    assert!(error
        .to_string()
        .contains("retained command log bytes differ from their report hashes"));
}

#[test]
fn rejects_wrong_expected_package_source_engine_and_upstream_bindings() {
    {
        let mut fixture = Fixture::new(true);
        fixture.package_source_commit =
            GitObjectId::try_from("ffffffffffffffffffffffffffffffffffffffff".to_owned()).unwrap();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("package source commit differs from its manifest"));
    }
    {
        let mut fixture = Fixture::new(false);
        fixture.archive_sha256 = digest("f");
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("package, profile, preset, or SDK source binding"));
    }
    {
        let mut fixture = Fixture::new(true);
        fixture.compiler = ArosCompilerIdentity::Gnu {
            gcc_version: "17.1.0".into(),
            binutils_version: "2.47".into(),
            target: fixture.profile.target().unwrap().clone(),
        };
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("package compiler, host, or package source commit differs"));
    }
    {
        let mut fixture = Fixture::new(true);
        fixture.manifest.profiles_sha256 = digest("f").to_string();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("not bound to the selected profiles"));
    }
    {
        let mut fixture = Fixture::new(true);
        fixture.source_preset = Some("different-source-preset".into());
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("package, profile, preset, or SDK source binding"));
    }
    {
        let mut fixture = Fixture::new(false);
        fixture.sdk_source_tree = digest("f");
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("package, profile, preset, or SDK source binding"));
    }
    {
        let mut fixture = Fixture::new(true);
        fixture.engine_sha256 = digest("f");
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("mixes engine, helper, or SDK source identities"));
    }
    {
        let mut fixture = Fixture::new(true);
        let first_helper = fixture.helpers.keys().next().unwrap().clone();
        fixture.helpers.get_mut(&first_helper).unwrap().sha256 = digest("f");
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("mixes engine, helper, or SDK source identities"));
    }
    {
        let mut fixture = Fixture::new(false);
        fixture.upstream_tree =
            GitObjectId::try_from("ffffffffffffffffffffffffffffffffffffffff".to_owned()).unwrap();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error
            .to_string()
            .contains("upstream commit or tree differs"));
    }
}

#[test]
fn rejects_malformed_digest_missing_phase_and_wrong_command_count() {
    {
        let mut fixture = Fixture::new(true);
        let mut value: Value = serde_json::from_slice(&fixture.receipt).unwrap();
        value["package"]["archive_sha256"] = json!("not-a-digest");
        fixture.receipt = crate::canonical::bytes(&value).unwrap();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("closed JSON document"));
    }
    {
        let mut fixture = Fixture::new(true);
        fixture
            .phase_reports
            .remove(&CompatibilityPhase::StandaloneCxx);
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("exactly six phase reports"));
    }
    {
        let mut fixture = Fixture::new(true);
        let phase = CompatibilityPhase::CmakeConsumer;
        let mut report: CompatibilityProbeReport =
            serde_json::from_slice(&fixture.phase_reports[&phase]).unwrap();
        report.commands.pop();
        fixture.phase_reports.insert(
            phase,
            crate::canonical::bytes(&serde_json::to_value(&report).unwrap()).unwrap(),
        );
        fixture.rebuild_receipt();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("unexpected command count"));
    }
}

#[test]
fn rejects_cmake_build_requirement_mismatches_and_llvm_build_requirement() {
    {
        let mut fixture = Fixture::new(true);
        fixture.cmake_build_required = false;
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("unexpected command count"));
    }
    {
        let mut fixture = Fixture::gnu_configure_only();
        fixture.cmake_build_required = true;
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("unexpected command count"));
    }
    {
        let mut fixture = Fixture::new(false);
        fixture.cmake_build_required = true;
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains(
            "LLVM compatibility cannot require the GNU source-native-consumer CMake build command"
        ));
    }
}

#[test]
fn rejects_ports_output_identity_and_family_schema_counterprobes() {
    {
        let mut fixture = Fixture::new(false);
        fixture.ports[0].sha256 = digest("f");
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("ports closure differs"));
    }
    {
        let mut fixture = Fixture::new(false);
        fixture
            .standalone
            .targets
            .get_mut("x86_64-unknown-aros")
            .unwrap()
            .c
            .sha256 = digest("f");
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("ELF output claim differs"));
    }
    {
        let mut fixture = Fixture::new(true);
        let mut document: CompatibilityReceiptDocument =
            serde_json::from_slice(&fixture.receipt).unwrap();
        document.schema = "aros-toolchain-native-compatibility-receipt-v4".into();
        fixture.receipt =
            crate::canonical::bytes(&serde_json::to_value(&document).unwrap()).unwrap();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported schema or operation")
                || error.to_string().contains("schema does not match")
                || error.to_string().contains("invalid package binding"),
            "unexpected family/schema swap rejection: {error}"
        );
    }
}

#[test]
fn rejects_duplicate_unknown_and_reordered_receipt_metadata() {
    {
        let fixture = Fixture::new(true);
        let receipt = String::from_utf8(fixture.receipt.clone()).unwrap();
        let duplicate = receipt.replacen(
            "\"operation\":",
            "\"operation\":\"other\",\"operation\":",
            1,
        );
        let mut request = fixture.request();
        request.receipt_bytes = duplicate.as_bytes();
        let error = readback_native_compatibility_receipt(&request).unwrap_err();
        assert!(error.to_string().contains("duplicate keys"));
    }
    {
        let mut fixture = Fixture::new(false);
        let mut value: Value = serde_json::from_slice(&fixture.receipt).unwrap();
        value["unrecognized"] = json!(true);
        fixture.receipt = crate::canonical::bytes(&value).unwrap();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("closed JSON document"));
    }
    {
        let mut fixture = Fixture::new(false);
        let mut document: CompatibilityReceiptDocument =
            serde_json::from_slice(&fixture.receipt).unwrap();
        document.phase_reports.swap(0, 1);
        fixture.receipt =
            crate::canonical::bytes(&serde_json::to_value(&document).unwrap()).unwrap();
        let error = readback_native_compatibility_receipt(&fixture.request()).unwrap_err();
        assert!(error.to_string().contains("required ordered phase set"));
    }
}

fn standalone_report(profile: &Profile) -> StandaloneOutputReport {
    let triples = if profile.name() == "pc-x86_64" {
        vec![
            ("x86_64-unknown-aros", Class::Elf64),
            ("i386-unknown-aros", Class::Elf32),
        ]
    } else {
        vec![(profile.target_triple(), Class::Elf32)]
    };
    StandaloneOutputReport {
        targets: triples
            .into_iter()
            .map(|(triple, class)| {
                (
                    triple.to_owned(),
                    StandaloneTargetReport {
                        c: StandaloneArtifactIdentity {
                            sha256: digest("a"),
                            size: 20,
                            class,
                        },
                        cxx: StandaloneArtifactIdentity {
                            sha256: digest("b"),
                            size: 24,
                            class,
                        },
                    },
                )
            })
            .collect(),
    }
}

fn receipt_artifact(identity: &StandaloneArtifactIdentity) -> CompatibilityReceiptArtifact {
    CompatibilityReceiptArtifact {
        sha256: identity.sha256.clone(),
        size: identity.size,
        class: match identity.class {
            Class::Elf32 => "elf32".into(),
            Class::Elf64 => "elf64".into(),
        },
    }
}

fn digest(character: &str) -> Sha256Digest {
    Sha256Digest::parse(&character.repeat(64)).unwrap()
}

fn phase_name(phase: CompatibilityPhase) -> &'static str {
    match phase {
        CompatibilityPhase::CmakeConsumer => "cmake",
        CompatibilityPhase::UpstreamConfigure => "configure",
        CompatibilityPhase::UpstreamIncludes => "includes",
        CompatibilityPhase::UpstreamLinklibs => "linklibs",
        CompatibilityPhase::StandaloneC => "standalone-c",
        CompatibilityPhase::StandaloneCxx => "standalone-cxx",
    }
}
