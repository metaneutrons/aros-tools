//! Unit tests for the genmodule library surface.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn config_with_options(
        root: &Path,
        module_type: &str,
        options: &str,
        resident_pri: Option<i32>,
    ) -> ConfModule {
        let config = root.join("probe.conf");
        let mut contents = String::from("##begin config\n");
        if !options.is_empty() {
            let _ = writeln!(contents, "options {options}");
        }
        if let Some(priority) = resident_pri {
            let _ = writeln!(contents, "residentpri {priority}");
        }
        if module_type == "handler" {
            contents.push_str("handler_func ProbeHandler\n");
        }
        contents.push_str("##end config\n");
        fs::write(&config, contents).expect("write genmodule config");

        let mut module = parse_conf_variant(&config, root, None)
            .expect("read genmodule config")
            .expect("parse genmodule config");
        module.mod_type = module_type.to_owned();
        module
    }

    fn reference_genmodule() -> Option<std::path::PathBuf> {
        if let Some(path) = std::env::var_os("AROS_HOST_GENMODULE") {
            return Some(path.into());
        }
        std::env::var_os("AROS_TEST_SOURCE_ROOT")
            .map(|root| std::path::PathBuf::from(root).join("build/pc-x86_64/hosttools/genmodule"))
            .filter(|path| path.is_file())
    }

    fn reference_resident_flags(
        genmodule: &Path,
        config: &Path,
        output_root: &Path,
        module_name: &str,
        module_type: &str,
    ) -> std::io::Result<String> {
        fs::create_dir_all(output_root)?;
        let output = Command::new(genmodule)
            .arg("-c")
            .arg(config)
            .arg("-d")
            .arg(output_root)
            .args(["writelibdefs", module_name, module_type])
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "reference genmodule failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let libdefs = fs::read_to_string(output_root.join(format!("{module_name}_libdefs.h")))?;
        libdefs
            .lines()
            .find_map(|line| {
                line.strip_prefix("#define RESIDENTFLAGS")
                    .map(str::trim)
                    .map(str::to_owned)
            })
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "reference libdefs omitted RESIDENTFLAGS",
                )
            })
    }

    #[test]
    fn generated_output_is_written_only_when_its_bytes_change() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aros-genmodule-write-if-changed-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        let path = dir.join("generated.h");

        assert!(write_if_changed(&path, b"first\n").expect("initial write"));
        assert!(!write_if_changed(&path, b"first\n").expect("unchanged write"));
        assert!(write_if_changed(&path, b"second\n").expect("changed write"));
        assert_eq!(fs::read(&path).expect("read generated output"), b"second\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }

        fs::remove_dir_all(dir).expect("remove test directory");
    }

    fn module(mod_type: &str, funcs: usize, cdef: &str) -> ConfModule {
        // LVOs run consecutively from the module type's first, which is what a
        // conf without `.skip` produces. A fixture that left them at 0 would
        // hide the difference between counting functions and taking the
        // highest LVO.
        let first = varargs::first_lvo(mod_type, false);
        ConfModule {
            name: "x".to_owned(),
            mod_type: mod_type.to_owned(),
            cdef: cdef.to_owned(),
            functions: (0..funcs)
                .map(|i| varargs::Function {
                    name: format!("F{i}"),
                    ret_type: "void ".to_owned(),
                    args: Vec::new(),
                    private: false,
                    novararg: false,
                    lvo: first + u32::try_from(i).expect("fixture function count fits u32"),
                    stack_call: true,
                    declared_version: None,
                    aliases: Vec::new(),
                })
                .collect(),
            ..ConfModule::default()
        }
    }

    /// `.skip` reserves vectors without declaring a function, so the vector
    /// count is the highest LVO and not the function count. kernel.resource is
    /// the measured case: 59 functions, 12 reserved LVOs, 71 slots. Sizing the
    /// base from 59 let MakeFunctions write 0x60 bytes below the allocation,
    /// over the ROM MemHeader. OPEN-POINTS 27g.
    #[test]
    fn functions_count_is_the_highest_lvo_not_the_function_count() {
        // A resource starts at LVO 1, so consecutive LVOs make the two equal.
        let dense = module("resource", 59, "");
        assert_eq!(functions_count(&dense), 59);

        // Reserving 12 LVOs across the list leaves 59 functions on 71 slots.
        let mut sparse = module("resource", 59, "");
        sparse.functions[58].lvo = 71;
        assert_eq!(functions_count(&sparse), 71);
        assert_eq!(sparse.functions.len(), 59);

        // A library's own vectors start at 5, so even a dense list is offset.
        let library = module("library", 3, "");
        assert_eq!(functions_count(&library), 7);
    }

    /// An empty function list still reserves the module type's own vectors:
    /// `firstlvo - 1`, as `writeinclibdefs.c:21` has it.
    #[test]
    fn functions_count_without_functions_reserves_the_type_vectors() {
        assert_eq!(functions_count(&module("library", 0, "")), 4);
        assert_eq!(functions_count(&module("device", 0, "")), 6);
        assert_eq!(functions_count(&module("resource", 0, "")), 0);
    }

    #[test]
    fn resident_autoinit_follows_reference_defaults_not_client_autoinit() {
        let directory = tempfile::tempdir().expect("create test directory");
        let cases = [
            ("library", "", None, "RTF_COLDSTART|RTF_AUTOINIT"),
            ("library", "noautoinit", None, "RTF_COLDSTART|RTF_AUTOINIT"),
            ("library", "autoinit", None, "RTF_COLDSTART|RTF_AUTOINIT"),
            ("library", "selfinit", None, "RTF_COLDSTART"),
            ("library", "noinittable", None, "RTF_COLDSTART|RTF_AUTOINIT"),
            ("resource", "", None, "RTF_COLDSTART"),
            (
                "resource",
                "resautoinit",
                None,
                "RTF_COLDSTART|RTF_AUTOINIT",
            ),
            ("handler", "", None, "RTF_COLDSTART"),
            ("handler", "resautoinit", None, "RTF_COLDSTART|RTF_AUTOINIT"),
            ("library", "", Some(105), "RTF_SINGLETASK|RTF_AUTOINIT"),
        ];

        for (index, (module_type, options, resident_pri, expected)) in cases.iter().enumerate() {
            let root = directory.path().join(index.to_string());
            fs::create_dir_all(&root).expect("create case directory");
            let module = config_with_options(&root, module_type, options, *resident_pri);
            assert_eq!(
                resident_flags(&module),
                *expected,
                "{module_type}: {options}"
            );
            assert_eq!(
                module.options.contains(ConfModuleOptions::NO_INIT_TABLE),
                *options == "noinittable",
                "noinittable option tracking"
            );
        }
    }

    #[test]
    fn contradictory_autoinit_options_fail_closed() {
        let directory = tempfile::tempdir().expect("create test directory");
        for (index, options) in ["resautoinit,selfinit", "selfinit,resautoinit"]
            .into_iter()
            .enumerate()
        {
            let root = directory.path().join(index.to_string());
            fs::create_dir_all(&root).expect("create case directory");
            let config = root.join("probe.conf");
            fs::write(
                &config,
                format!("##begin config\noptions {options}\n##end config\n"),
            )
            .expect("write contradictory options");
            let error = parse_conf_variant(&config, &root, None)
                .expect_err("incompatible Resident options must be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        }

        let config = directory.path().join("client-conflict.conf");
        fs::write(
            &config,
            "##begin config\noptions autoinit,noautoinit\n##end config\n",
        )
        .expect("write contradictory client options");
        let error = parse_conf_variant(&config, directory.path(), None)
            .expect_err("incompatible client autoinit options must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn gl_library_fixture_defaults_resident_autoinit() {
        let directory = tempfile::tempdir().expect("create test directory");
        let config = directory.path().join("gl.conf");
        fs::write(
            &config,
            "##begin config\nversion 20.1\nlibbase GLBase\noptions rellinklib\n##end config\n",
        )
        .expect("write portable GL config fixture");
        let mut module = parse_conf_variant(&config, directory.path(), None)
            .expect("read portable GL config")
            .expect("parse portable GL config");
        module.mod_type = "library".to_owned();
        assert_eq!(resident_flags(&module), "RTF_COLDSTART|RTF_AUTOINIT");
    }

    #[test]
    fn real_gl_config_uses_library_resident_autoinit_default() {
        let Some(source_root) = std::env::var_os("AROS_TEST_SOURCE_ROOT") else {
            eprintln!("skipping real GL config check: no source tree configured");
            return;
        };
        let source_root = std::path::PathBuf::from(source_root);
        let config = source_root.join("workbench/libs/gl/gl.conf");
        assert!(
            config.is_file(),
            "real GL config not found: {}",
            config.display()
        );
        let mut module = parse_conf_variant(&config, &source_root, None)
            .expect("read real GL config")
            .expect("parse real GL config");
        module.mod_type = "library".to_owned();
        assert_eq!(resident_flags(&module), "RTF_COLDSTART|RTF_AUTOINIT");
    }

    #[test]
    fn resident_flags_match_reference_genmodule_for_real_gl_and_option_probes() {
        let Some(genmodule) = reference_genmodule() else {
            eprintln!("skipping reference genmodule comparison: no host tool configured");
            return;
        };
        assert!(
            genmodule.is_file(),
            "configured reference genmodule does not exist: {}",
            genmodule.display()
        );
        let output_root = tempfile::tempdir().expect("create reference output root");
        let cases = [
            ("library", "", None),
            ("library", "noautoinit", None),
            ("library", "selfinit", None),
            ("library", "noinittable", None),
            ("resource", "", None),
            ("handler", "", None),
            ("resource", "resautoinit", None),
            ("handler", "resautoinit", None),
            ("library", "", Some(105)),
        ];
        for (index, (module_type, options, resident_pri)) in cases.iter().enumerate() {
            let root = output_root.path().join(format!("fixture-{index}"));
            fs::create_dir_all(&root).expect("create reference case directory");
            let module = config_with_options(&root, module_type, options, *resident_pri);
            let config = root.join("probe.conf");
            let expected = resident_flags(&module);
            let output = reference_resident_flags(
                &genmodule,
                &config,
                &root.join("generated"),
                "probe",
                module_type,
            )
            .expect("reference genmodule should produce libdefs");
            assert_eq!(expected, output, "{module_type}: {options}");
        }

        if let Some(source_root) = std::env::var_os("AROS_TEST_SOURCE_ROOT") {
            let source_root = std::path::PathBuf::from(source_root);
            let config = source_root.join("workbench/libs/gl/gl.conf");
            assert!(
                config.is_file(),
                "real GL config not found: {}",
                config.display()
            );
            let mut module = parse_conf_variant(&config, &source_root, None)
                .expect("read real GL config")
                .expect("parse real GL config");
            module.mod_type = "library".to_owned();
            let expected = resident_flags(&module);
            let output = reference_resident_flags(
                &genmodule,
                &config,
                &output_root.path().join("real-gl"),
                "gl",
                "library",
            )
            .expect("reference genmodule should produce real GL libdefs");
            assert_eq!(expected, output, "real workbench/libs/gl/gl.conf");
            assert_eq!(expected, "RTF_COLDSTART|RTF_AUTOINIT");
        }
    }

    #[test]
    fn libraries_and_resources_always_export() {
        assert!(exports_public_headers(&module("library", 0, "")));
        assert!(exports_public_headers(&module("resource", 0, "")));
    }

    #[test]
    fn a_hidd_without_functions_claims_no_sdk_namespace() {
        // rom/hidds/pci is a hidd with no functionlist; it must not overwrite
        // the SDK headers of workbench/tools/SysExplorer/Modules/PCI.
        assert!(!exports_public_headers(&module("hidd", 0, "")));
        assert!(exports_public_headers(&module("hidd", 1, "")));
    }

    #[test]
    fn a_device_exports_only_with_an_api_or_a_custom_base() {
        assert!(!exports_public_headers(&module("device", 0, "")));
        assert!(exports_public_headers(&module("device", 2, "")));
        assert!(exports_public_headers(&module(
            "device",
            0,
            "#include <x.h>"
        )));
        let mut custom = module("device", 0, "");
        custom.explicit_base_type_extern = Some("struct MyBase".to_owned());
        assert!(exports_public_headers(&custom));
    }

    #[test]
    fn handlers_export_with_functions_or_a_cdef_block() {
        assert!(!exports_public_headers(&module("handler", 0, "")));
        assert!(exports_public_headers(&module(
            "handler",
            0,
            "#include <y.h>"
        )));
    }

    #[test]
    fn an_undescribed_module_needs_an_api_to_export() {
        // No modtype in the build description: fall back to "has functions".
        assert!(!exports_public_headers(&module("", 0, "")));
        assert!(exports_public_headers(&module("", 1, "")));
    }

    #[test]
    fn arch_filter_keeps_paths_outside_arch() {
        let dirs = vec!["x86_64-pc".to_owned(), "all-pc".to_owned()];
        assert!(arch_dir_applies(Path::new("rom/exec"), &dirs));
        assert!(arch_dir_applies(Path::new("workbench/libs/icon"), &dirs));
    }

    #[test]
    fn arch_filter_selects_matching_architecture_dirs() {
        let dirs = vec![
            "x86_64-pc".to_owned(),
            "all-pc".to_owned(),
            "x86_64-all".to_owned(),
            "all-native".to_owned(),
        ];
        assert!(arch_dir_applies(Path::new("arch/all-pc/exec"), &dirs));
        assert!(arch_dir_applies(Path::new("arch/x86_64-all/kernel"), &dirs));
        assert!(arch_dir_applies(Path::new("arch/all-native/acpica"), &dirs));
        assert!(arch_dir_applies(
            Path::new("arch/all-unix/hidd/unixio"),
            &dirs
        ));
        // Foreign architectures are skipped; this is what stops
        // arch/m68k-amiga/devs/audio from clobbering workbench/devs/audio.
        assert!(!arch_dir_applies(
            Path::new("arch/m68k-amiga/devs/audio"),
            &dirs
        ));
        assert!(!arch_dir_applies(Path::new("arch/arm-native/soc"), &dirs));
    }

    #[test]
    fn empty_arch_list_filters_nothing() {
        assert!(arch_dir_applies(
            Path::new("arch/m68k-amiga/devs/audio"),
            &[]
        ));
    }

    #[test]
    fn external_base_type_follows_the_module_type() {
        // acpica declares `libbasetype struct ACPICABase` but is a library, so
        // consumers of <proto/acpica.h> must see `struct Library *`.
        let mut m = module("library", 0, "");
        m.lib_base_type = "struct ACPICABase".to_owned();
        assert_eq!(extern_base_type(&m), "struct Library *");

        assert_eq!(
            extern_base_type(&module("device", 0, "")),
            "struct Device *"
        );
        assert_eq!(extern_base_type(&module("resource", 0, "")), "APTR ");
        assert_eq!(extern_base_type(&module("handler", 0, "")), "APTR ");
        assert_eq!(extern_base_type(&module("hidd", 0, "")), "struct Library *");
        // Unknown module type falls back to the library form.
        assert_eq!(extern_base_type(&module("", 0, "")), "struct Library *");
    }

    #[test]
    fn explicit_libbasetypeextern_wins() {
        let mut m = module("library", 0, "");
        m.explicit_base_type_extern = Some("struct MyOwnBase".to_owned());
        assert_eq!(extern_base_type(&m), "struct MyOwnBase *");
    }

    #[test]
    fn default_basename_capitalises_the_first_letter() {
        // The library base has to be named exactly as the module's own
        // functions name their last parameter, or LIBBASE binds to the
        // untyped global from the proto header instead.
        assert_eq!(default_basename("layers"), "Layers");
        assert_eq!(default_basename("intuition"), "Intuition");
        assert_eq!(default_basename("acpica"), "Acpica");
    }

    #[test]
    fn default_basename_leaves_an_already_capitalised_name_alone() {
        assert_eq!(default_basename("Layers"), "Layers");
    }

    #[test]
    fn default_basename_handles_an_empty_name() {
        assert_eq!(default_basename(""), "");
    }

    #[test]
    fn shared_config_keeps_default_and_explicit_module_identities() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aros-genmodule-invocation-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        let conf = dir.join("icon.conf");
        fs::write(&conf, "##begin config\n##end config\n").expect("write config");
        fs::write(
            dir.join("mmakefile.src"),
            "%build_module mmake=workbench-libs-icon modname=icon modtype=library files=icon.c\n\
             %build_module mmake=wanderer-classes-icon modname=Icon modtype=mui conffile=icon.conf files=icon.c\n",
        )
        .expect("write make fragment");

        let declarations =
            read_module_declarations(&conf, "icon", &dir).expect("read declarations");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].name, "Icon");
        assert_eq!(declarations[0].mod_type, "mui");
        assert_eq!(declarations[1].name, "icon");
        assert_eq!(declarations[1].mod_type, "library");
        let variants = parse_conf_variants(&conf, &dir).expect("parse variants");
        assert_eq!(variants.len(), 2);
        assert_eq!(functions_count(&variants[0]), 5);
        assert_eq!(functions_count(&variants[1]), 4);

        fs::remove_dir_all(dir).expect("remove test directory");
    }

    #[test]
    fn cfunctionlist_and_spaced_section_markers_are_parsed() {
        let root = std::env::temp_dir().join(format!(
            "aros-genmodule-cfunctionlist-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test directory");
        let conf = root.join("probe.conf");
        fs::write(
            &conf,
            "## begin cfunctionlist\nLONG Probe(LONG value) (D0)\n## end cfunctionlist\n",
        )
        .expect("write config");

        let module = parse_conf_variant(&conf, &root, None)
            .expect("read config")
            .expect("parse config");
        assert_eq!(module.functions.len(), 1);
        assert_eq!(functions_count(&module), 5);

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn confoverride_keeps_the_base_function_list() {
        let root = std::env::temp_dir().join(format!(
            "aros-genmodule-confoverride-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let base_dir = root.join("rom/kernel");
        let override_dir = root.join("arch/all-pc/hpet");
        fs::create_dir_all(&base_dir).expect("create base directory");
        fs::create_dir_all(&override_dir).expect("create override directory");
        fs::write(
            base_dir.join("clocksource.conf"),
            "##begin config\nlibbase CSBase\n##end config\n\
             ##begin functionlist\nLONG First() ()\nLONG Second() ()\n##end functionlist\n",
        )
        .expect("write base config");
        let override_conf = override_dir.join("hpet.conf");
        fs::write(
            &override_conf,
            "##begin config\nlibbase HPETBase\n##end config\n",
        )
        .expect("write override config");
        fs::write(
            override_dir.join("mmakefile.src"),
            "%build_module mmake=kernel-pc-hpet modname=hpet modtype=resource \\\n+             conffile=$(SRCDIR)/rom/kernel/clocksource.conf confoverride=hpet.conf\n",
        )
        .expect("write make fragment");

        let modules = parse_conf_variants(&override_conf, &root).expect("parse variants");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "hpet");
        assert_eq!(modules[0].lib_base, "HPETBase");
        assert_eq!(functions_count(&modules[0]), 2);

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn stale_private_libdefs_are_removed_without_touching_current_outputs() {
        let root = std::env::temp_dir().join(format!(
            "aros-genmodule-prune-libdefs-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let module_dir = root.join("rom/test");
        fs::create_dir_all(&module_dir).expect("create module directory");
        let current = module_dir.join("current_libdefs.h");
        let stale = module_dir.join("stale_libdefs.h");
        let unrelated = module_dir.join("generated.h");
        fs::write(&current, "current\n").expect("write current output");
        fs::write(&stale, "stale\n").expect("write stale output");
        fs::write(&unrelated, "unrelated\n").expect("write unrelated output");
        let module = ConfModule {
            name: "current".to_owned(),
            rel_dir: PathBuf::from("rom/test"),
            ..ConfModule::default()
        };

        let mut transaction = FileTransaction::for_output_root(&root).unwrap();
        assert_eq!(
            prune_stale_private_libdefs(&root, &[module], &mut transaction)
                .expect("stage prune outputs"),
            1
        );
        transaction.commit().expect("commit prune outputs");
        assert!(current.exists());
        assert!(!stale.exists());
        assert!(unrelated.exists());

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn public_headers_keep_the_exact_include_name_spelling() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aros-genmodule-include-name-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        let module = ConfModule {
            name: "Icon".to_owned(),
            include_name: "Icon".to_owned(),
            lib_base: "IconBase".to_owned(),
            lib_base_type: "struct Library".to_owned(),
            mod_type: "mui".to_owned(),
            // A MUI module with a cdef block owns public headers in the
            // reference generator too.
            cdef: "typedef int IconPublic;\n".to_owned(),
            ..ConfModule::default()
        };

        let mut transaction = FileTransaction::for_output_root(&dir).unwrap();
        generate_sdk_headers(
            &module,
            &dir,
            Some(&dir.join("gen")),
            true,
            &mut transaction,
        )
        .expect("stage headers");
        transaction.commit().expect("commit headers");
        let proto = dir.join("proto/Icon.h");
        assert!(proto.exists());
        let names: Vec<String> = fs::read_dir(dir.join("proto"))
            .expect("read proto directory")
            .map(|entry| {
                entry
                    .expect("read proto entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(names.iter().any(|name| name == "Icon.h"));
        assert!(!names.iter().any(|name| name == "icon.h"));
        let contents = fs::read_to_string(proto).expect("read proto header");
        assert!(contents.contains("#include <clib/Icon_protos.h>"));
        assert!(contents.contains("#include <defines/Icon.h>"));

        fs::remove_dir_all(dir).expect("remove test directory");
    }

    #[test]
    fn bootstrap_withholds_case_only_public_header_collisions() {
        let mut library = module("library", 0, "");
        library.name = "icon".to_owned();
        library.include_name = "icon".to_owned();
        let mut mui = module("mui", 0, "typedef int IconPublic;");
        mui.name = "Icon".to_owned();
        mui.include_name = "Icon".to_owned();

        let collisions = colliding_public_include_names(&[library, mui]);
        assert_eq!(collisions.len(), 1);
        assert!(collisions.contains("icon"));
    }

    #[test]
    fn gm_uniquename_is_the_basename_not_the_module_name() {
        let root =
            std::env::temp_dir().join(format!("aros-genmodule-basename-{}", std::process::id()));
        let dir = root.join("rom/kernel");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).expect("module dir");
        std::fs::write(
            dir.join("kernel.conf"),
            "##begin config\nlibbase KernelBase\nlibbasetype struct KernelBase\n##end config\n",
        )
        .expect("write conf");
        let module = parse_conf_variant(&dir.join("kernel.conf"), &root, None)
            .expect("read config")
            .expect("parse config");

        // tools/genmodule/config.c:1333 capitalises the module name when the
        // config states no basename, and writeinclibdefs.c:82 names every
        // generated symbol after it. rom/kernel/kernel_init.c:62 declares
        // GM_UNIQUENAME(FuncTable), so that has to be Kernel_FuncTable.
        assert_eq!(module.base_name, "Kernel");
        assert_eq!(module.lib_base, "KernelBase");

        // An explicit basename wins, and does not overwrite an explicit libbase.
        std::fs::write(
            dir.join("kernel.conf"),
            "##begin config\nlibbase KernelBase\nbasename Kern\n##end config\n",
        )
        .expect("write conf");
        let module = parse_conf_variant(&dir.join("kernel.conf"), &root, None)
            .expect("read config")
            .expect("parse config");
        assert_eq!(module.base_name, "Kern");
        assert_eq!(module.lib_base, "KernelBase");

        // Without an explicit libbase, basename derives it.
        std::fs::write(
            dir.join("kernel.conf"),
            "##begin config\nbasename Kern\n##end config\n",
        )
        .expect("write conf");
        let module = parse_conf_variant(&dir.join("kernel.conf"), &root, None)
            .expect("read config")
            .expect("parse config");
        assert_eq!(module.base_name, "Kern");
        assert_eq!(module.lib_base, "KernBase");

        let _ = std::fs::remove_dir_all(&root);
    }
}
