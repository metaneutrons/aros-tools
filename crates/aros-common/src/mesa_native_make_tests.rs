use super::*;
use crate::make_native_observation::MakeObservationPhase;

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(&character.to_string().repeat(64)).unwrap()
}

fn call_binding(character: char) -> MesaNativeMakeCallBinding {
    MesaNativeMakeCallBinding {
        document_sha256: digest(character),
        binding: MakeObservationBinding {
            phase: MakeObservationPhase::FirstInputPrelude,
            context_sha256: digest('b'),
            invocation_sha256: digest(character),
            stdout_sha256: digest(character),
        },
    }
}

fn binding() -> MesaNativeMakeBinding {
    MesaNativeMakeBinding {
        version: 1,
        source_inputs_sha256: digest('a'),
        compiler_tree_sha256: digest('b'),
        configuration_stdout_sha256: digest('c'),
        prepared_tree_sha256: digest('d'),
        parent_program_sha256: digest('e'),
        parent_invocation_sha256: digest('f'),
        parent_environment_sha256: digest('0'),
        host_metamake_sha256: digest('1'),
        context_sha256: digest('2'),
        control_sha256: digest('3'),
        calls: vec![call_binding('4')],
        owners: vec![MesaNativeOwnerCall {
            owner: "gl/gallium".into(),
            call_index: 0,
        }],
        source_calls: vec![MesaNativeSourceCall {
            generated_owner: "gl/gallium/selected-input.mk".into(),
            call_index: 0,
        }],
    }
}

fn producer(owner: &str, directory: &str) -> MesaGenmoduleProducer {
    MesaGenmoduleProducer {
        owner: owner.into(),
        declaration_directory: directory.into(),
        module: "mesa-module".into(),
        module_type: "library".into(),
        base_configuration: "config/base.conf".into(),
        override_configuration: None,
    }
}

#[test]
fn context_projection_has_fixed_schema_and_excludes_later_selection_fields() {
    let selected = binding();
    let bytes = native_make_context_bytes(&selected).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "schema": "aros-mesa-native-context-v1",
            "source_inputs_sha256": digest('a'),
            "compiler_tree_sha256": digest('b'),
            "configuration_stdout_sha256": digest('c'),
            "parent_program_sha256": digest('e'),
            "parent_invocation_sha256": digest('f'),
            "parent_environment_sha256": digest('0'),
            "host_metamake_sha256": digest('1'),
        })
    );

    let mut changed_later_fields = selected.clone();
    changed_later_fields.prepared_tree_sha256 = digest('5');
    changed_later_fields.context_sha256 = digest('6');
    changed_later_fields.control_sha256 = digest('7');
    changed_later_fields.calls.push(call_binding('8'));
    changed_later_fields.owners.push(MesaNativeOwnerCall {
        owner: "gl/hidd".into(),
        call_index: 1,
    });
    changed_later_fields
        .source_calls
        .push(MesaNativeSourceCall {
            generated_owner: "gl/hidd/selected-input.mk".into(),
            call_index: 1,
        });
    assert_eq!(
        native_make_context_bytes(&selected).unwrap(),
        native_make_context_bytes(&changed_later_fields).unwrap()
    );

    changed_later_fields.version = 2;
    assert!(native_make_context_bytes(&changed_later_fields).is_err());
}

#[test]
fn native_packet_names_are_fixed_width_and_bounded() {
    assert_eq!(native_packet_file(0).unwrap(), "native-make-call-0000.json");
    assert_eq!(
        native_packet_file(MAX_NATIVE_CALLS - 1).unwrap(),
        "native-make-call-0255.json"
    );
    assert!(native_packet_file(MAX_NATIVE_CALLS).is_err());
}

#[test]
fn producer_mapping_is_exact_but_source_calls_account_for_all_calls() {
    let mut selected = binding();
    selected.calls.push(call_binding('5'));
    selected.owners = vec![
        MesaNativeOwnerCall {
            owner: "gl/gallium".into(),
            call_index: 0,
        },
        MesaNativeOwnerCall {
            owner: "gl/hidd".into(),
            call_index: 1,
        },
    ];
    selected.source_calls = vec![
        MesaNativeSourceCall {
            generated_owner: "workbench/libs/mesa/shared/selected-input.mk".into(),
            call_index: 0,
        },
        MesaNativeSourceCall {
            generated_owner: "workbench/libs/mesa/source-only/selected-input.mk".into(),
            call_index: 1,
        },
    ];
    let selected_producers = [
        producer("gl/gallium", "workbench/libs/mesa/gallium"),
        producer("gl/hidd", "workbench/libs/mesa/hidd"),
    ];
    validate_binding_shape(&selected, &selected_producers).unwrap();

    let mut reversed = selected.clone();
    reversed.owners.reverse();
    assert!(validate_binding_shape(&reversed, &selected_producers).is_err());

    let mut source_only = selected.clone();
    source_only.owners[1].call_index = 0;
    validate_binding_shape(&source_only, &selected_producers).unwrap();

    let missing_owner = [producer("gl/gallium", "workbench/libs/mesa/gallium")];
    assert!(validate_binding_shape(&selected, &missing_owner).is_err());

    let duplicate_producer = [
        producer("gl/gallium", "workbench/libs/mesa/gallium"),
        producer("gl/gallium", "workbench/libs/mesa/other"),
    ];
    assert!(validate_binding_shape(&selected, &duplicate_producer).is_err());
}

#[test]
fn owner_mapping_rejects_invalid_indexes_and_empty_call_tables() {
    let selected_producer = [producer("gl/gallium", "workbench/libs/mesa/gallium")];
    let mut invalid_index = binding();
    invalid_index.owners[0].call_index = 1;
    assert!(validate_binding_shape(&invalid_index, &selected_producer).is_err());

    let mut empty_calls = binding();
    empty_calls.calls.clear();
    assert!(validate_binding_shape(&empty_calls, &selected_producer).is_err());

    let mut too_many_calls = binding();
    too_many_calls.calls = (0..=MAX_NATIVE_CALLS)
        .map(|slot| call_binding(char::from(b"0123456789abcdef"[slot % 16])))
        .collect();
    too_many_calls.owners = vec![MesaNativeOwnerCall {
        owner: "gl/gallium".into(),
        call_index: 0,
    }];
    assert!(validate_binding_shape(&too_many_calls, &selected_producer).is_err());
}

#[test]
fn source_call_roster_is_required_dense_bounded_and_independent_of_producers() {
    let mut selected = binding();
    selected.owners.clear();
    validate_binding_shape(&selected, &[]).unwrap();

    let mut missing = selected.clone();
    missing.source_calls.clear();
    assert!(validate_binding_shape(&missing, &[]).is_err());

    let mut later_index = selected.clone();
    later_index.source_calls[0].call_index = 1;
    assert!(validate_binding_shape(&later_index, &[]).is_err());

    let mut oversized = selected.clone();
    oversized.source_calls[0].generated_owner = "a".repeat(16 * 1024 + 1);
    assert!(validate_binding_shape(&oversized, &[]).is_err());

    let mut duplicate_owner = selected.clone();
    duplicate_owner.calls.push(call_binding('5'));
    duplicate_owner.source_calls.push(MesaNativeSourceCall {
        generated_owner: duplicate_owner.source_calls[0].generated_owner.clone(),
        call_index: 1,
    });
    validate_binding_shape(&duplicate_owner, &[]).unwrap();
}

#[test]
fn source_call_paths_must_be_relative_and_lexically_normalized() {
    assert!(validate_generated_owner("workbench/libs/mesa/file.mk").is_ok());
    for owner in [
        "",
        "/absolute/file.mk",
        "workbench//file.mk",
        "workbench/./file.mk",
        "workbench/../file.mk",
        "workbench/file.mk/",
        "workbench\\file.mk",
        "workbench/file\0.mk",
    ] {
        assert!(validate_generated_owner(owner).is_err(), "{owner:?}");
    }

    for input in ["file.mk", "sub/file.mk", "/build/source/file.mk"] {
        assert!(is_lexically_normalized_input(input), "{input}");
    }
    for input in [
        "",
        "./file.mk",
        "sub/../file.mk",
        "sub//file.mk",
        "sub/file.mk/",
        "/build//file.mk",
        "/build/../file.mk",
        "sub\\file.mk",
    ] {
        assert!(!is_lexically_normalized_input(input), "{input:?}");
    }
}

#[test]
fn source_call_binding_is_a_required_strict_serde_field() {
    let mut value = serde_json::to_value(binding()).unwrap();
    value.as_object_mut().unwrap().remove("source_calls");
    assert!(serde_json::from_value::<MesaNativeMakeBinding>(value).is_err());

    let mut value = serde_json::to_value(binding()).unwrap();
    value["source_calls"][0]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<MesaNativeMakeBinding>(value).is_err());
}

#[test]
fn preparation_chain_has_fixed_count_statuses_and_parent_binding() {
    let selected = binding();
    let configured_program = digest('e');
    let configured_environment = digest('9');
    let stages = [
        MesaNativePreparationStageIdentity {
            program: configured_program.clone(),
            invocation: digest('a'),
            environment: configured_environment.clone(),
            exit_status: 2,
        },
        MesaNativePreparationStageIdentity {
            program: configured_program.clone(),
            invocation: digest('b'),
            environment: configured_environment.clone(),
            exit_status: 0,
        },
        MesaNativePreparationStageIdentity {
            program: digest('e'),
            invocation: digest('f'),
            environment: digest('0'),
            exit_status: 0,
        },
    ];
    validate_native_preparation_stages(
        &selected,
        &configured_program,
        &configured_environment,
        &stages,
    )
    .unwrap();

    assert!(validate_native_preparation_stages(
        &selected,
        &configured_program,
        &configured_environment,
        &stages[..2],
    )
    .is_err());

    let mut changed_status = stages.clone();
    changed_status[0].exit_status = 0;
    assert!(validate_native_preparation_stages(
        &selected,
        &configured_program,
        &configured_environment,
        &changed_status,
    )
    .is_err());

    let mut changed_launch = stages;
    changed_launch[2].environment = digest('8');
    assert!(validate_native_preparation_stages(
        &selected,
        &configured_program,
        &configured_environment,
        &changed_launch,
    )
    .is_err());
}

#[test]
fn control_paths_are_lexically_normalized_without_filesystem_lookup() {
    assert_eq!(
        exact_normalized_absolute(Path::new("/opt/tools/make")),
        Some("/opt/tools/make".into())
    );
    for path in ["relative/make", "/opt/../make", "/opt//make", "/opt/make/"] {
        assert!(
            exact_normalized_absolute(Path::new(path)).is_none(),
            "{path}"
        );
    }
    assert!(validate_declaration_directory("workbench/libs/gl").is_ok());
    for directory in ["", "/absolute", "workbench/../gl", "workbench//gl"] {
        assert!(validate_declaration_directory(directory).is_err());
    }
}

#[test]
fn native_preflight_stage_names_have_one_fixed_order_and_per_owner_tail() {
    let producers = [
        producer("mesa3dgl26-gallium", "workbench/libs/mesa/gallium"),
        producer("mesa3dgl26-hidd", "workbench/libs/mesa/hidd"),
    ];
    assert_eq!(
        native_preflight_stage_names(&producers).unwrap(),
        [
            "configure",
            "feature-inputs",
            "features",
            "configured-values",
            "native-root-query",
            "native-host-metamake-build",
            "native-root-launch",
            "genmodule-inventory",
            "genmodule-build",
            "genmodule-make-mesa3dgl26-gallium",
            "genmodule-make-mesa3dgl26-hidd",
        ]
        .map(str::to_owned)
        .to_vec()
    );
    assert_eq!(
        NATIVE_PREPARATION_STAGES,
        [
            "native-root-query",
            "native-host-metamake-build",
            "native-root-launch",
        ]
    );
}

#[test]
fn native_preflight_stage_names_reject_empty_unsafe_or_unsorted_owners() {
    assert!(native_preflight_stage_names(&[]).is_err());
    assert!(native_preflight_stage_names(&[producer("../escape", "workbench/libs/gl")]).is_err());
    assert!(native_preflight_stage_names(&[
        producer("mesa3dgl26-hidd", "workbench/libs/mesa/hidd"),
        producer("mesa3dgl26-gallium", "workbench/libs/mesa/gallium"),
    ])
    .is_err());
    assert!(native_preflight_stage_names(&[
        producer("mesa3dgl26-gallium", "workbench/libs/mesa/gallium"),
        producer("mesa3dgl26-gallium", "workbench/libs/mesa/other"),
    ])
    .is_err());
}

#[cfg(unix)]
pub mod actual_native_make_tests {
    use super::*;
    use crate::{
        make_native_invocation::NativeMakeInvocation,
        make_native_observation::RetainedMakeObservation,
        make_native_sampler::{observe_first_input_make, InitialMakeObservationRequest},
        mesa_configuration::MesaConfigurationIdentity,
        process::CancellationToken,
        ArosCompilerIdentity,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::PathBuf,
        process::Command,
        time::Duration,
    };
    use tempfile::TempDir;

    pub struct NativeMakeFixture {
        temporary: Option<TempDir>,
        pub(crate) binding: MesaNativeMakeBinding,
        pub(crate) identity: MesaConfigurationIdentity,
        pub(crate) configured_stdout: Sha256Digest,
        pub(crate) coordinates: MesaMakeCoordinates,
        pub(crate) producers: Vec<MesaGenmoduleProducer>,
        pub(crate) files: BTreeMap<String, Vec<u8>>,
        invocation_bytes: Vec<u8>,
        initial_stdout: Vec<u8>,
        environment_names: BTreeSet<String>,
        other_cwd: PathBuf,
    }

    impl NativeMakeFixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let build_root = root.join("build");
            let declaration_directory = "workbench/libs/mesa/shared";
            let working_directory = build_root.join(declaration_directory);
            let source_root = root.join("source");
            let ports_root = build_root.join("Ports");
            let generated_root = build_root.join("gen");
            fs::create_dir_all(&working_directory).unwrap();
            fs::create_dir_all(&source_root).unwrap();
            fs::create_dir_all(&ports_root).unwrap();
            fs::create_dir_all(&generated_root).unwrap();
            fs::write(working_directory.join("selected-input.mk"), b"\n").unwrap();

            let producers = vec![
                producer("mesa3dgl26-gallium", declaration_directory),
                producer("mesa3dgl26-hidd", declaration_directory),
            ];
            let identity = MesaConfigurationIdentity {
                target_profile: "native-test".into(),
                target_triple: "aarch64-unknown-aros".into(),
                compiler: ArosCompilerIdentity::Llvm {
                    version: "18.1.0".into(),
                },
                compiler_tree_sha256: digest('b'),
                source_inputs_sha256: digest('a'),
                configure_invocation_sha256: digest('c'),
            };
            let coordinates = MesaMakeCoordinates {
                source_root: source_root.to_str().unwrap().to_owned(),
                build_root: build_root.to_str().unwrap().to_owned(),
                ports_root: ports_root.to_str().unwrap().to_owned(),
                generated_root: generated_root.to_str().unwrap().to_owned(),
            };
            let mut fixture =
                Self::from_selection(&identity, &digest('c'), &coordinates, &producers);
            fixture.temporary = Some(temporary);
            fixture.other_cwd = root;
            fixture
        }

        /// Build a live first-input packet for a caller-owned selection.
        ///
        /// All producers must share a declaration directory because this
        /// fixture intentionally maps both owners to one retained native call.
        pub(crate) fn from_selection(
            identity: &MesaConfigurationIdentity,
            configured_stdout: &Sha256Digest,
            coordinates: &MesaMakeCoordinates,
            producers: &[MesaGenmoduleProducer],
        ) -> Self {
            assert!(!producers.is_empty(), "fixture needs selected producers");
            assert!(
                coordinates.validate().is_ok(),
                "fixture requires normalized Mesa coordinates"
            );
            assert!(
                native_preflight_stage_names(producers).is_ok(),
                "fixture requires safe, sorted selected producer owners"
            );
            assert!(
                producers.iter().all(|producer| {
                    producer.declaration_directory == producers[0].declaration_directory
                }),
                "one shared native call requires one producer declaration directory"
            );
            for producer in producers {
                assert!(
                    validate_declaration_directory(&producer.declaration_directory).is_ok(),
                    "fixture requires a normalized producer declaration directory"
                );
            }
            let working_directory =
                PathBuf::from(&coordinates.build_root).join(&producers[0].declaration_directory);
            fs::create_dir_all(&working_directory).unwrap();
            let working_directory = working_directory.canonicalize().unwrap();
            let program = gnu_make();
            let program_sha256 = sha256_bytes(&fs::read(&program).unwrap());
            let mut binding = MesaNativeMakeBinding {
                version: 1,
                source_inputs_sha256: identity.source_inputs_sha256.clone(),
                compiler_tree_sha256: identity.compiler_tree_sha256.clone(),
                configuration_stdout_sha256: configured_stdout.clone(),
                prepared_tree_sha256: digest('d'),
                parent_program_sha256: program_sha256.clone(),
                parent_invocation_sha256: digest('e'),
                parent_environment_sha256: digest('f'),
                host_metamake_sha256: digest('0'),
                context_sha256: digest('1'),
                control_sha256: digest('2'),
                calls: Vec::new(),
                owners: Vec::new(),
                source_calls: Vec::new(),
            };
            let context_bytes = native_make_context_bytes(&binding).unwrap();
            binding.context_sha256 = sha256_bytes(&context_bytes);

            let invocation = NativeMakeInvocation::new(
                [
                    "-f".to_owned(),
                    "selected-input.mk".to_owned(),
                    "probe-target".to_owned(),
                ],
                working_directory,
                std::iter::empty::<(String, String)>(),
            )
            .unwrap()
            .with_argument_zero("make".to_owned())
            .unwrap();
            let request = InitialMakeObservationRequest {
                program: &program,
                executable_sha256: &program_sha256,
                invocation: &invocation,
                context_sha256: &binding.context_sha256,
                timeout: Duration::from_secs(20),
            };
            let observation =
                observe_first_input_make(&request, &CancellationToken::default(), |_, _| Ok(()))
                    .unwrap_or_else(|error| panic!("GNU Make first-input sample failed: {error}"));
            let invocation_bytes = observation.invocation().to_bytes().unwrap();
            let initial_stdout = observation.initial_stdout().to_vec();
            let environment_names = observation.environment_names().clone();
            let packet_bytes = observation.to_bytes().unwrap();
            binding.calls = vec![MesaNativeMakeCallBinding {
                document_sha256: sha256_bytes(&packet_bytes),
                binding: observation.binding().clone(),
            }];
            binding.owners = producers
                .iter()
                .map(|producer| MesaNativeOwnerCall {
                    owner: producer.owner.clone(),
                    call_index: 0,
                })
                .collect();
            binding.source_calls = vec![MesaNativeSourceCall {
                generated_owner: format!(
                    "{}/selected-input.mk",
                    producers[0].declaration_directory
                ),
                call_index: 0,
            }];

            let control_bytes = serde_json::to_vec(&serde_json::json!({
                "version": 4,
                "phase": MakeObservationPhase::FirstInputPrelude,
                "maximum_calls": 1,
                "output_root": PathBuf::from(&coordinates.build_root)
                    .parent()
                    .expect("absolute test build root has a parent")
                    .join("native-make-calls"),
                "program": program,
                "executable_sha256": program_sha256,
                "context_sha256": binding.context_sha256,
                "argument_zero": "make",
            }))
            .unwrap();
            binding.control_sha256 = sha256_bytes(&control_bytes);

            let files = BTreeMap::from([
                (NATIVE_CONTEXT_FILE.to_owned(), context_bytes),
                (NATIVE_CONTROL_FILE.to_owned(), control_bytes),
                (native_packet_file(0).unwrap(), packet_bytes),
            ]);

            Self {
                temporary: None,
                binding,
                identity: identity.clone(),
                configured_stdout: configured_stdout.clone(),
                coordinates: coordinates.clone(),
                producers: producers.to_vec(),
                files,
                invocation_bytes,
                initial_stdout,
                environment_names,
                other_cwd: PathBuf::from(&coordinates.build_root)
                    .parent()
                    .expect("absolute test build root has a parent")
                    .to_path_buf(),
            }
        }

        fn add_actual_source_call(&mut self, generated_owner: &str, goal: &str) {
            let owner_path = Path::new(generated_owner);
            let parent = owner_path.parent().expect("generated owner has a parent");
            let input = owner_path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("generated owner filename is UTF-8");
            let working_directory = PathBuf::from(&self.coordinates.build_root).join(parent);
            fs::create_dir_all(&working_directory).unwrap();
            fs::write(working_directory.join(input), b"\n").unwrap();
            let working_directory = working_directory.canonicalize().unwrap();
            let program = gnu_make();
            let program_sha256 = sha256_bytes(&fs::read(&program).unwrap());
            let invocation = NativeMakeInvocation::new(
                ["-f".to_owned(), input.to_owned(), goal.to_owned()],
                working_directory,
                std::iter::empty::<(String, String)>(),
            )
            .unwrap()
            .with_argument_zero("make".to_owned())
            .unwrap();
            let request = InitialMakeObservationRequest {
                program: &program,
                executable_sha256: &program_sha256,
                invocation: &invocation,
                context_sha256: &self.binding.context_sha256,
                timeout: Duration::from_secs(20),
            };
            let observation =
                observe_first_input_make(&request, &CancellationToken::default(), |_, _| Ok(()))
                    .unwrap_or_else(|error| panic!("GNU Make source-only sample failed: {error}"));
            let packet_bytes = observation.to_bytes().unwrap();
            let slot = self.binding.calls.len();
            self.binding.calls.push(MesaNativeMakeCallBinding {
                document_sha256: sha256_bytes(&packet_bytes),
                binding: observation.binding().clone(),
            });
            self.binding.source_calls.push(MesaNativeSourceCall {
                generated_owner: generated_owner.to_owned(),
                call_index: slot,
            });
            self.files
                .insert(native_packet_file(slot).unwrap(), packet_bytes);

            let mut control: serde_json::Value =
                serde_json::from_slice(self.files.get(NATIVE_CONTROL_FILE).unwrap()).unwrap();
            control["maximum_calls"] = serde_json::json!(self.binding.calls.len());
            let control_bytes = serde_json::to_vec(&control).unwrap();
            self.binding.control_sha256 = sha256_bytes(&control_bytes);
            self.files
                .insert(NATIVE_CONTROL_FILE.to_owned(), control_bytes);
        }

        fn load(
            &self,
            binding: &MesaNativeMakeBinding,
            identity: &MesaConfigurationIdentity,
            files: &BTreeMap<String, Vec<u8>>,
        ) -> crate::Result<LoadedMesaNativeMake> {
            load_native_make(
                binding,
                identity,
                &self.configured_stdout,
                &self.coordinates,
                &self.producers,
                |path, _maximum_bytes| {
                    files
                        .get(path)
                        .cloned()
                        .ok_or_else(|| ArosError::Configuration {
                            file: "sensitive-selected-path".into(),
                            message: "raw argv and environment must not escape".into(),
                        })
                },
            )
        }

        fn packet_variant(
            &self,
            invocation: &NativeMakeInvocation,
            phase: MakeObservationPhase,
        ) -> (MesaNativeMakeCallBinding, Vec<u8>) {
            self.packet_variant_from(
                invocation,
                phase,
                &self.initial_stdout,
                &self.environment_names,
            )
        }

        fn packet_variant_from(
            &self,
            invocation: &NativeMakeInvocation,
            phase: MakeObservationPhase,
            initial_stdout: &[u8],
            environment_names: &BTreeSet<String>,
        ) -> (MesaNativeMakeCallBinding, Vec<u8>) {
            let invocation_bytes = invocation.to_bytes().unwrap();
            let expected = MakeObservationBinding {
                phase,
                context_sha256: self.binding.context_sha256.clone(),
                invocation_sha256: sha256_bytes(&invocation_bytes),
                stdout_sha256: sha256_bytes(initial_stdout),
            };
            let observation = RetainedMakeObservation::from_captured_bytes(
                &invocation_bytes,
                initial_stdout,
                environment_names,
                &expected,
            )
            .unwrap();
            let bytes = observation.to_bytes().unwrap();
            (
                MesaNativeMakeCallBinding {
                    document_sha256: sha256_bytes(&bytes),
                    binding: observation.binding().clone(),
                },
                bytes,
            )
        }
    }

    fn gnu_make() -> PathBuf {
        for name in ["gmake", "make"] {
            let Ok(path) = which::which(name) else {
                continue;
            };
            let path = path.canonicalize().unwrap();
            let version = Command::new(&path).arg("--version").output().unwrap();
            if version.status.success()
                && String::from_utf8_lossy(&version.stdout)
                    .lines()
                    .next()
                    .is_some_and(|line| line.starts_with("GNU Make "))
            {
                return path;
            }
        }
        panic!("tests require GNU Make, found through which gmake or which make");
    }

    #[test]
    fn actual_first_input_packet_loads_and_selected_mutations_are_rejected() {
        let mut fixture = NativeMakeFixture::new();
        fixture.add_actual_source_call(
            "workbench/libs/mesa/source-only/selected-input.mk",
            "source-only-probe",
        );
        let loaded = fixture
            .load(&fixture.binding, &fixture.identity, &fixture.files)
            .unwrap();
        assert_eq!(loaded.binding(), &fixture.binding);
        assert_eq!(loaded.coordinates(), &fixture.coordinates);
        let gallium = loaded
            .observation_for_owner("mesa3dgl26-gallium")
            .unwrap()
            .unwrap();
        let hidd = loaded
            .observation_for_owner("mesa3dgl26-hidd")
            .unwrap()
            .unwrap();
        assert_eq!(gallium.to_bytes().unwrap(), hidd.to_bytes().unwrap());
        assert!(loaded
            .observation_for_owner("missing-owner")
            .unwrap()
            .is_none());
        assert!(loaded.observation(1).unwrap().is_some());
        assert!(loaded.observation(2).unwrap().is_none());

        let mut changed_files = fixture.files.clone();
        changed_files
            .get_mut(NATIVE_CONTEXT_FILE)
            .unwrap()
            .push(b' ');
        assert!(fixture
            .load(&fixture.binding, &fixture.identity, &changed_files)
            .is_err());

        let mut changed_files = fixture.files.clone();
        changed_files
            .get_mut(NATIVE_CONTROL_FILE)
            .unwrap()
            .push(b' ');
        assert!(fixture
            .load(&fixture.binding, &fixture.identity, &changed_files)
            .is_err());

        let mut changed_files = fixture.files.clone();
        changed_files
            .get_mut(&native_packet_file(0).unwrap())
            .unwrap()
            .push(b' ');
        assert!(fixture
            .load(&fixture.binding, &fixture.identity, &changed_files)
            .is_err());

        let mut changed_source = fixture.binding.clone();
        changed_source.source_inputs_sha256 = digest('6');
        assert!(fixture
            .load(&changed_source, &fixture.identity, &fixture.files)
            .is_err());

        let mut changed_compiler = fixture.identity.clone();
        changed_compiler.compiler_tree_sha256 = digest('7');
        assert!(fixture
            .load(&fixture.binding, &changed_compiler, &fixture.files)
            .is_err());

        let mut changed_argv0 = fixture.binding.clone();
        let mut changed_files = fixture.files.clone();
        let mut control: serde_json::Value =
            serde_json::from_slice(changed_files.get(NATIVE_CONTROL_FILE).unwrap()).unwrap();
        control["argument_zero"] = "different-make".into();
        let changed_control = serde_json::to_vec(&control).unwrap();
        changed_argv0.control_sha256 = sha256_bytes(&changed_control);
        changed_files.insert(NATIVE_CONTROL_FILE.to_owned(), changed_control);
        assert!(fixture
            .load(&changed_argv0, &fixture.identity, &changed_files)
            .is_err());

        let original_invocation = NativeMakeInvocation::parse(&fixture.invocation_bytes).unwrap();
        let changed_directory_invocation = NativeMakeInvocation::new(
            original_invocation.arguments().to_vec(),
            fixture.other_cwd.clone(),
            original_invocation
                .environment()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .unwrap()
        .with_argument_zero(original_invocation.argument_zero().unwrap().to_owned())
        .unwrap();
        let (changed_call, changed_packet) = fixture.packet_variant(
            &changed_directory_invocation,
            MakeObservationPhase::FirstInputPrelude,
        );
        let mut changed_cwd = fixture.binding.clone();
        changed_cwd.calls[0] = changed_call;
        let mut changed_files = fixture.files.clone();
        changed_files.insert(native_packet_file(0).unwrap(), changed_packet);
        assert!(fixture
            .load(&changed_cwd, &fixture.identity, &changed_files)
            .is_err());

        let original_invocation = NativeMakeInvocation::parse(&fixture.invocation_bytes).unwrap();
        let (changed_call, changed_packet) =
            fixture.packet_variant(&original_invocation, MakeObservationPhase::PreFirstFile);
        let mut changed_phase = fixture.binding.clone();
        changed_phase.calls[0] = changed_call;
        let mut changed_files = fixture.files.clone();
        changed_files.insert(native_packet_file(0).unwrap(), changed_packet);
        assert!(fixture
            .load(&changed_phase, &fixture.identity, &changed_files)
            .is_err());

        let mut wrong_source_file = fixture.binding.clone();
        wrong_source_file.source_calls[1].generated_owner =
            "workbench/libs/mesa/source-only/other.mk".into();
        assert!(fixture
            .load(&wrong_source_file, &fixture.identity, &fixture.files)
            .is_err());

        let source_only_observation = RetainedMakeObservation::load(
            fixture.files.get(&native_packet_file(1).unwrap()).unwrap(),
            &fixture.binding.calls[1].document_sha256,
            &fixture.binding.calls[1].binding,
        )
        .unwrap();
        let original_invocation = source_only_observation.invocation();
        let changed_directory_invocation = NativeMakeInvocation::new(
            original_invocation.arguments().to_vec(),
            fixture.other_cwd.clone(),
            original_invocation
                .environment()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .unwrap()
        .with_argument_zero(original_invocation.argument_zero().unwrap().to_owned())
        .unwrap();
        let (changed_call, changed_packet) = fixture.packet_variant_from(
            &changed_directory_invocation,
            MakeObservationPhase::FirstInputPrelude,
            source_only_observation.initial_stdout(),
            source_only_observation.environment_names(),
        );
        let mut wrong_cwd = fixture.binding.clone();
        wrong_cwd.calls[1] = changed_call;
        let mut changed_files = fixture.files.clone();
        changed_files.insert(native_packet_file(1).unwrap(), changed_packet);
        assert!(fixture
            .load(&wrong_cwd, &fixture.identity, &changed_files)
            .is_err());

        let mut missing_source_roster = fixture.binding.clone();
        missing_source_roster.source_calls.pop();
        assert!(fixture
            .load(&missing_source_roster, &fixture.identity, &fixture.files)
            .is_err());

        let mut later_source_index = fixture.binding.clone();
        later_source_index.source_calls[1].call_index = 2;
        assert!(fixture
            .load(&later_source_index, &fixture.identity, &fixture.files)
            .is_err());

        let error = load_native_make(
            &fixture.binding,
            &fixture.identity,
            &fixture.configured_stdout,
            &fixture.coordinates,
            &fixture.producers,
            |_, _| {
                Err(ArosError::Configuration {
                    file: "private/raw-argv".into(),
                    message: "private environment secret".into(),
                })
            },
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("private/raw-argv"));
        assert!(!error.contains("private environment secret"));
        assert!(error.contains("cannot read selected native Make data"));
    }
}
