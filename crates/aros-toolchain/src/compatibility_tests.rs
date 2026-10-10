//! Regression tests for the adjacent production module.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::time::Duration;

use aros_common::{
    elf::{AROS_ABI_VERSION, OS_ABI_AROS},
    CancellationToken, DiagnosticCode,
};

use super::{
    prepare, prepare_host_tool_closure, run_probe, run_probe_set, verify_materialized_engine,
    verify_standalone_outputs, CompatibilityCommand, CompatibilityEnvironment,
    CompatibilityHostTool, CompatibilityPhase, CompatibilityPreparation,
    CompatibilityPreparationRequest, CompatibilityProbeReport, CompatibilityProbeRequest,
    CompatibilityProbeSetRequest, HostToolClosureRequest, StandaloneOutputRequest,
    StandaloneTargetArtifacts, CXX_COLLECTOR_SYMBOL, C_COLLECTOR_SYMBOL, REQUIRED_HELPERS,
};

fn request(root: &std::path::Path) -> CompatibilityPreparationRequest {
    let source_root = root.join("source");
    let work_root = root.join("work");
    let helpers_root = root.join("helpers");
    for directory in [&source_root, &work_root, &helpers_root] {
        fs::create_dir(directory).unwrap();
    }
    for helper in REQUIRED_HELPERS {
        let path = helpers_root.join(helper);
        fs::write(&path, b"fixture helper\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    CompatibilityPreparationRequest {
        source_root,
        work_root,
        helpers_root,
    }
}

#[test]
fn standalone_outputs_require_aros_elf_identity_and_collector_symbols() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("standalone");
    fs::create_dir(&output_root).unwrap();
    let c = output_root.join("c-x86_64.o");
    let cxx = output_root.join("cxx-x86_64.o");
    fs::write(&c, fixture_elf64(C_COLLECTOR_SYMBOL, OS_ABI_AROS)).unwrap();
    fs::write(&cxx, fixture_elf64(CXX_COLLECTOR_SYMBOL, OS_ABI_AROS)).unwrap();

    let report = verify_standalone_outputs(&StandaloneOutputRequest {
        output_root: output_root.clone(),
        targets: BTreeMap::from([(
            "x86_64-unknown-aros".into(),
            StandaloneTargetArtifacts {
                c: c.clone(),
                cxx: cxx.clone(),
            },
        )]),
    })
    .unwrap();
    let target = &report.targets["x86_64-unknown-aros"];
    assert_eq!(target.c.class, aros_common::elf::Class::Elf64);
    assert_eq!(target.cxx.class, aros_common::elf::Class::Elf64);
    assert_ne!(target.c.sha256, target.cxx.sha256);

    let linked = output_root.join("c-linked.o");
    symlink(&c, &linked).unwrap();
    let linked_error = verify_standalone_outputs(&StandaloneOutputRequest {
        output_root: output_root.clone(),
        targets: BTreeMap::from([(
            "x86_64-unknown-aros".into(),
            StandaloneTargetArtifacts {
                c: linked,
                cxx: cxx.clone(),
            },
        )]),
    })
    .unwrap_err();
    assert_compatibility(&linked_error);

    let duplicate = verify_standalone_outputs(&StandaloneOutputRequest {
        output_root: output_root.clone(),
        targets: BTreeMap::from([(
            "x86_64-unknown-aros".into(),
            StandaloneTargetArtifacts {
                c: c.clone(),
                cxx: c,
            },
        )]),
    })
    .unwrap_err();
    assert_compatibility(&duplicate);

    let non_aros = output_root.join("c-non-aros.o");
    fs::write(&non_aros, fixture_elf64(C_COLLECTOR_SYMBOL, 0)).unwrap();
    let non_aros_error = verify_standalone_outputs(&StandaloneOutputRequest {
        output_root,
        targets: BTreeMap::from([(
            "x86_64-unknown-aros".into(),
            StandaloneTargetArtifacts { c: non_aros, cxx },
        )]),
    })
    .unwrap_err();
    assert_compatibility(&non_aros_error);
}

fn fixture_elf64(symbol: &str, os_abi: u8) -> Vec<u8> {
    let mut names = Vec::from([0_u8]);
    names.extend_from_slice(symbol.as_bytes());
    names.push(0);
    let section_offset = 64_usize;
    let section_size = 64_usize;
    let strtab_offset = section_offset + 3 * section_size;
    let symtab_offset = strtab_offset + names.len();
    let mut object = vec![0_u8; symtab_offset + 2 * 24];
    object[..4].copy_from_slice(b"\x7fELF");
    object[4] = 2;
    object[5] = 1;
    object[6] = 1;
    object[7] = os_abi;
    object[8] = AROS_ABI_VERSION;
    write_u32(&mut object, 0x14, 1);
    write_u64(&mut object, 0x28, section_offset as u64);
    write_u16(&mut object, 0x34, 64);
    write_u16(&mut object, 0x3a, section_size as u16);
    write_u16(&mut object, 0x3c, 3);

    let strtab = section_offset + section_size;
    write_u32(&mut object, strtab + 4, 3);
    write_u64(&mut object, strtab + 24, strtab_offset as u64);
    write_u64(&mut object, strtab + 32, names.len() as u64);
    write_u64(&mut object, strtab + 48, 1);

    let symtab = strtab + section_size;
    write_u32(&mut object, symtab + 4, 2);
    write_u64(&mut object, symtab + 24, symtab_offset as u64);
    write_u64(&mut object, symtab + 32, 48);
    write_u32(&mut object, symtab + 40, 1);
    write_u64(&mut object, symtab + 48, 8);
    write_u64(&mut object, symtab + 56, 24);

    object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
    let symbol_entry = symtab_offset + 24;
    write_u32(&mut object, symbol_entry, 1);
    object[symbol_entry + 4] = 0x10;
    write_u16(&mut object, symbol_entry + 6, 1);
    object
}

fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn preparation_materializes_only_the_embedded_engine_and_exact_helpers() {
    let temporary = tempfile::tempdir().unwrap();
    let request = request(temporary.path());
    let prepared = prepare(&request).unwrap();

    assert!(prepared.engine_root.join("CMakeLists.txt").is_file());
    assert!(prepared.engine_root.join("AROS.cmake").is_file());
    assert_eq!(prepared.helpers.len(), REQUIRED_HELPERS.len());
    let helpers_root = request.helpers_root.canonicalize().unwrap();
    assert!(prepared
        .helpers
        .values()
        .all(|helper| helper.path.starts_with(&helpers_root)));
    assert!(!request.source_root.join("cmake").exists());
}

#[test]
fn preparation_rejects_source_engine_reused_destination_and_bad_helper() {
    let temporary = tempfile::tempdir().unwrap();
    let request = request(temporary.path());
    fs::create_dir(request.source_root.join("cmake")).unwrap();
    let source_engine_error = prepare(&request).unwrap_err();
    assert_compatibility(&source_engine_error);
    fs::remove_dir(request.source_root.join("cmake")).unwrap();

    fs::create_dir(request.work_root.join("aros-cmake-engine")).unwrap();
    let reused_engine_error = prepare(&request).unwrap_err();
    assert_compatibility(&reused_engine_error);
    fs::remove_dir(request.work_root.join("aros-cmake-engine")).unwrap();

    let helper = request.helpers_root.join("aros-fetch");
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
    let error = prepare(&request).unwrap_err();
    assert_compatibility(&error);
}

#[test]
fn materialized_engine_rejects_foreign_or_linked_entries() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = temporary.path().join("engine");
    fs::create_dir(&engine).unwrap();
    aros_cmake_engine::materialize(&engine).unwrap();
    let digest = aros_common::Sha256Digest::parse(aros_cmake_engine::digest()).unwrap();
    fs::write(engine.join("foreign.cmake"), b"unexpected\n").unwrap();
    let error = verify_materialized_engine(&engine, &digest).unwrap_err();
    assert_compatibility(&error);
}

#[test]
fn probe_persists_a_canonical_report_bound_to_the_preparation() {
    let temporary = tempfile::tempdir().unwrap();
    let first_request = request(temporary.path());
    let preparation = prepare(&first_request).unwrap();
    let program = script(
        temporary.path(),
        "successful-probe",
        "[ \"$PATH\" = /nonexistent ] && [ -z \"${HOME+x}\" ] || exit 9; printf standard; printf error >&2",
    );
    let probe = probe_request(
        temporary.path(),
        preparation.clone(),
        CompatibilityPhase::CmakeConsumer,
        program,
        Duration::from_secs(5),
    );

    let report = run_probe(&probe, &CancellationToken::default()).unwrap();
    assert_eq!(report.engine_sha256, preparation.engine_sha256);
    assert_eq!(report.source_tree_sha256, preparation.source_tree_sha256);
    assert_eq!(report.helpers.len(), REQUIRED_HELPERS.len());
    let bytes = fs::read(probe.reports_root.join("cmake-consumer.report.json")).unwrap();
    assert!(bytes.ends_with(b"\n"));
    assert_eq!(CompatibilityProbeReport::parse(&bytes).unwrap(), report);
    assert_eq!(
        fs::read(probe.reports_root.join("cmake-consumer.stdout.log")).unwrap(),
        b"standard"
    );
    assert_eq!(
        fs::read(probe.reports_root.join("cmake-consumer.stderr.log")).unwrap(),
        b"error"
    );

    let rerun = run_probe(&probe, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&rerun);
}

#[test]
fn probe_executes_a_revalidated_sealed_host_tool_closure() {
    let temporary = tempfile::tempdir().unwrap();
    let preparation_request = request(temporary.path());
    let preparation = prepare(&preparation_request).unwrap();
    let selected_tool = script(temporary.path(), "selected-tool-source", "printf closure");
    let closure = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: temporary.path().join("host-tool-closure"),
        tools: vec![CompatibilityHostTool {
            name: "selected-tool".into(),
            program: selected_tool,
        }],
    })
    .unwrap();
    let mut probe = probe_request(
        temporary.path(),
        preparation,
        CompatibilityPhase::UpstreamConfigure,
        script(
            temporary.path(),
            "sealed-host-tool-probe",
            "[ \"$PATH\" = \"$1\" ] && [ -z \"${HOME+x}\" ] || exit 9; selected-tool",
        ),
        Duration::from_secs(5),
    );
    probe.commands[0]
        .arguments
        .push(closure.root.to_string_lossy().into_owned());
    probe.environment = CompatibilityEnvironment::SealedHostTools {
        variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
        host_tools: closure.clone(),
    };

    let mut missing_marker = probe.clone();
    missing_marker.reports_root = temporary.path().join("missing-host-tool-marker-reports");
    fs::create_dir(&missing_marker.reports_root).unwrap();
    missing_marker.environment = CompatibilityEnvironment::SealedHostTools {
        variables: BTreeMap::new(),
        host_tools: closure.clone(),
    };
    assert_compatibility(&run_probe(&missing_marker, &CancellationToken::default()).unwrap_err());

    let report = run_probe(&probe, &CancellationToken::default()).unwrap();
    assert_eq!(
        fs::read(probe.reports_root.join("upstream-configure.stdout.log")).unwrap(),
        b"closure"
    );
    assert_eq!(report.host_tools.len(), 1);
    assert_eq!(
        report.host_tools["selected-tool"].sha256,
        closure.tools["selected-tool"].sha256
    );
}

#[test]
fn probe_batch_binds_each_command_and_retains_failed_command_logs() {
    let temporary = tempfile::tempdir().unwrap();
    let preparation_request = request(temporary.path());
    let preparation = prepare(&preparation_request).unwrap();
    let mut successful = probe_request(
        temporary.path(),
        preparation,
        CompatibilityPhase::StandaloneC,
        script(temporary.path(), "batch-first", "printf first"),
        Duration::from_secs(5),
    );
    successful.commands.push(CompatibilityCommand {
        program: script(temporary.path(), "batch-second", "printf second >&2"),
        arguments: Vec::new(),
    });
    let report = run_probe(&successful, &CancellationToken::default()).unwrap();
    assert_eq!(report.commands.len(), 2);
    assert!(successful
        .reports_root
        .join("standalone-c.1.stdout.log")
        .is_file());
    assert!(successful
        .reports_root
        .join("standalone-c.2.stderr.log")
        .is_file());

    let failing_root = tempfile::tempdir().unwrap();
    let failing_request = request(failing_root.path());
    let mut failing = probe_request(
        failing_root.path(),
        prepare(&failing_request).unwrap(),
        CompatibilityPhase::StandaloneCxx,
        script(failing_root.path(), "batch-success", "printf first"),
        // This fixture tests exit-status/log retention, not scheduling
        // speed. The separate deadline fixture below exercises timeouts.
        Duration::from_secs(10),
    );
    failing.commands.push(CompatibilityCommand {
        program: script(
            failing_root.path(),
            "batch-failure",
            "printf second >&2; exit 7",
        ),
        arguments: Vec::new(),
    });
    let error = run_probe(&failing, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);
    let diagnostics = error.diagnostics();
    let context = diagnostics.diagnostics[0]
        .context
        .as_ref()
        .expect("failed command identity");
    assert_eq!(context.exit_code, Some(7));
    assert_eq!(context.tool.as_deref(), Some("standalone-cxx-2"));
    assert!(failing
        .reports_root
        .join("standalone-cxx.1.stdout.log")
        .is_file());
    assert!(failing
        .reports_root
        .join("standalone-cxx.2.stderr.log")
        .is_file());
    assert!(!failing
        .reports_root
        .join("standalone-cxx.report.json")
        .exists());
    let stale_retry = probe_request(
        failing_root.path(),
        failing.preparation.clone(),
        CompatibilityPhase::StandaloneCxx,
        script(failing_root.path(), "single-retry", "exit 0"),
        Duration::from_secs(1),
    );
    let error = run_probe(&stale_retry, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);

    // Each short command fits individually, but their combined runtime
    // exceeds two seconds. Resetting the budget per command must fail this test.
    for first_delay in ["1", "30"] {
        let deadline_root = tempfile::tempdir().unwrap();
        let deadline_request = request(deadline_root.path());
        let mut deadline = probe_request(
            deadline_root.path(),
            prepare(&deadline_request).unwrap(),
            CompatibilityPhase::StandaloneC,
            script(
                deadline_root.path(),
                "deadline-first",
                &format!("exec /bin/sleep {first_delay}"),
            ),
            Duration::from_secs(2),
        );
        deadline.commands.push(CompatibilityCommand {
            program: script(
                deadline_root.path(),
                "deadline-second",
                "exec /bin/sleep 1.5",
            ),
            arguments: Vec::new(),
        });
        let error = run_probe(&deadline, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        let diagnostics = error.diagnostics();
        let diagnostic = &diagnostics.diagnostics[0];
        if let Some(tool) = diagnostic
            .context
            .as_ref()
            .and_then(|context| context.tool.as_deref())
        {
            let index = match tool {
                "standalone-c-1" => 1,
                "standalone-c-2" => 2,
                _ => panic!("unexpected deadline command: {tool}"),
            };
            assert_eq!(diagnostic.context.as_ref().unwrap().timed_out, Some(true));
            // The whole-phase deadline can expire before command two is
            // launched. Assert retention only for processes that started.
            for started in 1..=index {
                for stream in ["stdout", "stderr"] {
                    assert!(deadline
                        .reports_root
                        .join(format!("standalone-c.{started}.{stream}.log"))
                        .is_file());
                }
            }
            if index == 1 {
                for stream in ["stdout", "stderr"] {
                    assert!(!deadline
                        .reports_root
                        .join(format!("standalone-c.2.{stream}.log"))
                        .exists());
                }
            }
        } else {
            assert!(diagnostic
                .message
                .contains("exhausted its explicit deadline before starting the next command"));
            // Expiry during preparation must not invent command-two logs.
            for stream in ["stdout", "stderr"] {
                assert!(!deadline
                    .reports_root
                    .join(format!("standalone-c.2.{stream}.log"))
                    .exists());
            }
            // Command one may not have started either. If it did, retain
            // both streams instead of requiring logs for an unstarted job.
            assert_eq!(
                deadline
                    .reports_root
                    .join("standalone-c.1.stdout.log")
                    .is_file(),
                deadline
                    .reports_root
                    .join("standalone-c.1.stderr.log")
                    .is_file(),
            );
        }
        assert!(!deadline
            .reports_root
            .join("standalone-c.report.json")
            .exists());
    }

    let changed_root = tempfile::tempdir().unwrap();
    let changed_request = request(changed_root.path());
    let changed_preparation = prepare(&changed_request).unwrap();
    let changed_helper = changed_preparation.helpers["aros-fetch"]
        .path
        .to_string_lossy()
        .into_owned();
    let mut changed = probe_request(
        changed_root.path(),
        changed_preparation,
        CompatibilityPhase::StandaloneC,
        script(
            changed_root.path(),
            "batch-mutator",
            "printf changed > \"$1\"",
        ),
        Duration::from_secs(1),
    );
    changed.commands[0].arguments.push(changed_helper);
    changed.commands.push(CompatibilityCommand {
        program: script(changed_root.path(), "batch-after-mutation", "exit 0"),
        arguments: Vec::new(),
    });
    let error = run_probe(&changed, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);
    assert!(changed
        .reports_root
        .join("standalone-c.1.stdout.log")
        .is_file());
    assert!(!changed
        .reports_root
        .join("standalone-c.2.stdout.log")
        .exists());
    assert!(!changed
        .reports_root
        .join("standalone-c.report.json")
        .exists());

    let moved_root = tempfile::tempdir().unwrap();
    let moved_request = request(moved_root.path());
    let mut moved = probe_request(
        moved_root.path(),
        prepare(&moved_request).unwrap(),
        CompatibilityPhase::StandaloneC,
        script(
            moved_root.path(),
            "move-working-directory",
            "exec /bin/mv \"$1\" \"$1-moved\"",
        ),
        Duration::from_secs(1),
    );
    moved.commands[0]
        .arguments
        .push(moved.current_dir.to_string_lossy().into_owned());
    moved.commands.push(CompatibilityCommand {
        program: script(moved_root.path(), "after-working-directory-move", "exit 0"),
        arguments: Vec::new(),
    });
    let error = run_probe(&moved, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);
    assert!(!moved
        .reports_root
        .join("standalone-c.2.stdout.log")
        .exists());
}

#[test]
fn probe_preserves_diagnostics_for_exit_timeout_and_cancellation() {
    let temporary = tempfile::tempdir().unwrap();
    let request = request(temporary.path());
    let preparation = prepare(&request).unwrap();
    let failing = probe_request(
        temporary.path(),
        preparation.clone(),
        CompatibilityPhase::CmakeConsumer,
        script(
            temporary.path(),
            "failing-probe",
            "printf failure >&2; exit 7",
        ),
        Duration::from_secs(1),
    );
    let error = run_probe(&failing, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);
    assert!(failing
        .reports_root
        .join("cmake-consumer.stderr.log")
        .is_file());
    assert!(!failing
        .reports_root
        .join("cmake-consumer.report.json")
        .exists());

    let timed = probe_request(
        temporary.path(),
        preparation.clone(),
        CompatibilityPhase::UpstreamConfigure,
        script(
            temporary.path(),
            "timed-probe",
            "printf started; exec /bin/sleep 30",
        ),
        Duration::from_secs(5),
    );
    let error = run_probe(&timed, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);
    assert_eq!(
        fs::read(timed.reports_root.join("upstream-configure.stdout.log")).unwrap(),
        b"started"
    );
    assert!(!timed
        .reports_root
        .join("upstream-configure.report.json")
        .exists());

    let cancelled = probe_request(
        temporary.path(),
        preparation,
        CompatibilityPhase::StandaloneC,
        script(temporary.path(), "cancelled-probe", "exit 0"),
        Duration::from_secs(1),
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let error = run_probe(&cancelled, &cancellation).unwrap_err();
    assert_compatibility(&error);
    assert!(!cancelled
        .reports_root
        .join("standalone-c.report.json")
        .exists());
}

#[test]
fn probe_rejects_changed_helper_control_arguments_and_foreign_report_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let first_request = request(temporary.path());
    let preparation = prepare(&first_request).unwrap();
    fs::write(
        preparation.helpers["aros-fetch"].path.clone(),
        b"changed helper\n",
    )
    .unwrap();
    let changed = probe_request(
        temporary.path(),
        preparation,
        CompatibilityPhase::CmakeConsumer,
        script(temporary.path(), "changed-helper-probe", "exit 0"),
        Duration::from_secs(1),
    );
    let error = run_probe(&changed, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);

    let source_changed_root = temporary.path().join("source-changed");
    fs::create_dir(&source_changed_root).unwrap();
    let source_changed_request = request(&source_changed_root);
    let source_changed_preparation = prepare(&source_changed_request).unwrap();
    fs::write(
        source_changed_preparation
            .source_root
            .join("unexpected-source-change"),
        b"changed source\n",
    )
    .unwrap();
    let changed_source = probe_request(
        &source_changed_root,
        source_changed_preparation,
        CompatibilityPhase::CmakeConsumer,
        script(temporary.path(), "changed-source-probe", "exit 0"),
        Duration::from_secs(1),
    );
    let error = run_probe(&changed_source, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);

    let separate = temporary.path().join("separate");
    fs::create_dir(&separate).unwrap();
    let second_request = request(&separate);
    let preparation = prepare(&second_request).unwrap();
    let mut invalid = probe_request(
        &separate,
        preparation,
        CompatibilityPhase::CmakeConsumer,
        script(temporary.path(), "invalid-argument-probe", "exit 0"),
        Duration::from_secs(1),
    );
    invalid.commands[0].arguments.push("line\nbreak".into());
    let error = run_probe(&invalid, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);

    let third = temporary.path().join("third");
    fs::create_dir(&third).unwrap();
    let third_request = request(&third);
    let mut invalid_environment = probe_request(
        &third,
        prepare(&third_request).unwrap(),
        CompatibilityPhase::StandaloneCxx,
        script(temporary.path(), "invalid-environment-probe", "exit 0"),
        Duration::from_secs(1),
    );
    let CompatibilityEnvironment::Poisoned { variables } = &mut invalid_environment.environment
    else {
        panic!("fixture must use a poisoned compatibility environment");
    };
    variables.insert("PATH".into(), "/bin".into());
    let error = run_probe(&invalid_environment, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);

    let malformed =
        br#"{\"schema\":\"aros-toolchain-compatibility-report-v3\",\"unexpected\":true}"#;
    let error = CompatibilityProbeReport::parse(malformed).unwrap_err();
    assert_compatibility(&error);
}

#[test]
fn probe_set_requires_every_phase_once_and_stops_after_a_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let preparation_request = request(temporary.path());
    let preparation = prepare(&preparation_request).unwrap();
    let successful = complete_probe_requests(temporary.path(), &preparation, None);
    let completed = run_probe_set(
        &CompatibilityProbeSetRequest {
            probes: successful.clone(),
        },
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(completed.reports.len(), 6);
    assert!(completed
        .reports
        .contains_key(&CompatibilityPhase::StandaloneCxx));

    let incomplete = CompatibilityProbeSetRequest {
        probes: successful[..5].to_vec(),
    };
    let error = run_probe_set(&incomplete, &CancellationToken::default()).unwrap_err();
    assert_compatibility(&error);

    let mut repeated = successful.clone();
    repeated.push(successful[0].clone());
    let error = run_probe_set(
        &CompatibilityProbeSetRequest { probes: repeated },
        &CancellationToken::default(),
    )
    .unwrap_err();
    assert_compatibility(&error);

    let failed_root = tempfile::tempdir().unwrap();
    let failed_request = request(failed_root.path());
    let failed = complete_probe_requests(
        failed_root.path(),
        &prepare(&failed_request).unwrap(),
        Some(CompatibilityPhase::UpstreamIncludes),
    );
    let error = run_probe_set(
        &CompatibilityProbeSetRequest { probes: failed },
        &CancellationToken::default(),
    )
    .unwrap_err();
    assert_compatibility(&error);
    let reports = failed_root.path().join("reports");
    assert!(reports.join("cmake-consumer.report.json").is_file());
    assert!(reports.join("upstream-configure.report.json").is_file());
    assert!(reports.join("upstream-includes.stderr.log").is_file());
    assert!(!reports.join("upstream-includes.report.json").exists());
    assert!(!reports.join("upstream-linklibs.report.json").exists());
}

fn complete_probe_requests(
    root: &std::path::Path,
    preparation: &CompatibilityPreparation,
    failing: Option<CompatibilityPhase>,
) -> Vec<CompatibilityProbeRequest> {
    [
        CompatibilityPhase::CmakeConsumer,
        CompatibilityPhase::UpstreamConfigure,
        CompatibilityPhase::UpstreamIncludes,
        CompatibilityPhase::UpstreamLinklibs,
        CompatibilityPhase::StandaloneC,
        CompatibilityPhase::StandaloneCxx,
    ]
    .into_iter()
    .map(|phase| {
        let body = if Some(phase) == failing {
            "printf failure >&2; exit 7"
        } else {
            "[ \"$PATH\" = /nonexistent ] || exit 9; printf success"
        };
        probe_request(
            root,
            preparation.clone(),
            phase,
            script(root, &format!("{phase:?}"), body),
            Duration::from_secs(5),
        )
    })
    .collect()
}

fn probe_request(
    root: &std::path::Path,
    preparation: CompatibilityPreparation,
    phase: CompatibilityPhase,
    program: std::path::PathBuf,
    timeout: Duration,
) -> CompatibilityProbeRequest {
    let current_dir = root.join("process");
    let reports_root = root.join("reports");
    fs::create_dir_all(&current_dir).unwrap();
    fs::create_dir_all(&reports_root).unwrap();
    CompatibilityProbeRequest {
        phase,
        commands: vec![CompatibilityCommand {
            program,
            arguments: Vec::new(),
        }],
        environment: CompatibilityEnvironment::Poisoned {
            variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
        },
        current_dir,
        reports_root,
        timeout,
        preparation,
    }
}

fn script(root: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn assert_compatibility(error: &crate::ContractError) {
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerCompatibility
    );
}
