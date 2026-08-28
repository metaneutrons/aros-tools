use super::*;

fn declaration(file: &str, macro_name: &str) -> Declaration {
    Declaration {
        mmake: "test-target".to_owned(),
        macro_name: macro_name.to_owned(),
        file: file.to_owned(),
        arguments: "mmake=test-target".to_owned(),
    }
}

#[test]
fn architecture_scope_matches_cmake_compatible_cpu_directories() {
    let x86 = ArchitectureScope::new("x86_64", "pc");
    assert!(x86.source_dirs.contains("all-native"));
    assert!(x86.source_dirs.contains("i386-all"));
    assert!(x86.source_dirs.contains("i386-native"));
    assert!(x86.source_dirs.contains("i386-pc"));
    assert!(x86.source_dirs.contains("x86_64-pc"));
    assert!(x86.source_dirs.contains("all-pc"));
    assert!(!x86.source_dirs.contains("arm-native"));
    assert!(!x86.source_dirs.contains("all-all"));

    let aarch64 = ArchitectureScope::new("aarch64", "raspi");
    assert!(aarch64.source_dirs.contains("arm-all"));
    assert!(aarch64.source_dirs.contains("arm-native"));
    assert!(aarch64.source_dirs.contains("arm-raspi"));
    assert!(aarch64.source_dirs.contains("aarch64-raspi"));
    assert!(!aarch64.source_dirs.contains("i386-all"));

    let riscv64 = ArchitectureScope::new("riscv64", "opensbi");
    assert!(riscv64.source_dirs.contains("riscv-all"));
    assert!(riscv64.source_dirs.contains("riscv-native"));
    assert!(riscv64.source_dirs.contains("riscv-opensbi"));
    assert!(riscv64.source_dirs.contains("riscv64-opensbi"));
}

#[test]
fn architecture_scope_uses_the_narrower_cmake_package_set() {
    let scope = ArchitectureScope::new("x86_64", "pc");
    assert!(scope.declaration_is_eligible(&declaration(
        "arch/i386-pc/drivers/mmakefile.src",
        "build_module"
    )));
    assert!(!scope.declaration_is_eligible(&declaration(
        "arch/i386-pc/boot/mmakefile.src",
        "make_package"
    )));
    assert!(scope.declaration_is_eligible(&declaration(
        "arch/x86_64-pc/boot/mmakefile.src",
        "make_package"
    )));
    assert!(scope.declaration_is_eligible(&declaration(
        "arch/all-pc/boot/mmakefile.src",
        "link_kickstart"
    )));
}

#[test]
fn architecture_scope_keeps_common_files_and_rejects_unknown_arch_paths() {
    let scope = ArchitectureScope::new("arm", "raspi");
    assert!(scope.declaration_is_eligible(&declaration("rom/exec/mmakefile.src", "build_module")));
    assert!(scope.declaration_is_eligible(&declaration(
        "arch\\arm-native\\kernel\\mmakefile.src",
        "build_module"
    )));
    assert!(!scope.declaration_is_eligible(&declaration(
        "arch/.unmaintained/m68k-pp-native/mmakefile.src",
        "build_module"
    )));
    assert!(
        !scope.declaration_is_eligible(&declaration("arch/all-all/mmakefile.src", "build_module"))
    );
    assert!(!scope.declaration_is_eligible(&declaration("arch/mmakefile.src", "build_module")));
}

#[test]
fn architecture_cli_requires_a_complete_pair_and_validates_profile() {
    let ok = Args::try_parse_from([
        "aros-verify",
        "--generated",
        "generated.cmake",
        "--work",
        "verify",
        "--cpu",
        "x86_64",
        "--platform",
        "pc",
        "--toolchain",
        "llvm",
        "--bootloader",
        "grub2gfx",
        "--profile",
        "architecture",
    ])
    .unwrap();
    validate_profile_arguments(&ok).unwrap();
    assert_eq!(ok.cpu.as_deref(), Some("x86_64"));
    assert_eq!(ok.platform.as_deref(), Some("pc"));
    assert_eq!(ok.profile, Some(Profile::Architecture));

    assert!(Args::try_parse_from([
        "aros-verify",
        "--generated",
        "generated.cmake",
        "--work",
        "verify",
        "--cpu",
        "x86_64",
    ])
    .is_err());
    let incomplete_profile = Args::try_parse_from([
        "aros-verify",
        "--generated",
        "generated.cmake",
        "--work",
        "verify",
        "--cpu",
        "x86_64",
        "--platform",
        "pc",
    ])
    .unwrap();
    assert!(validate_profile_arguments(&incomplete_profile).is_err());
    assert!(Args::try_parse_from([
        "aros-verify",
        "--generated",
        "generated.cmake",
        "--work",
        "verify",
        "--cpu",
        "x86_64",
        "--platform",
        "pc",
        "--profile",
        "core",
    ])
    .is_err());
}

#[test]
fn architecture_report_key_is_stable_and_path_safe() {
    let scope = ArchitectureScope::new("x86_64", "pc");
    assert_eq!(scope.key(), "architecture-x86_64-pc");
    assert!(parse_arch_component("../pc").is_err());
    assert!(parse_arch_component("aarch64").is_ok());
}

#[test]
fn legacy_grub_is_inactive_only_for_an_explicit_non_grub_profile() {
    let declaration = Declaration {
        mmake: "grub".to_owned(),
        macro_name: "build_with_configure".to_owned(),
        file: LEGACY_GRUB_FILE.to_owned(),
        arguments: LEGACY_GRUB_ARGUMENTS.to_owned(),
    };
    let grub2 = ArchitectureScope::with_configuration("x86_64", "pc", "llvm", "grub2gfx");
    let grub = ArchitectureScope::with_configuration("x86_64", "pc", "llvm", "grub");
    assert!(is_inactive_profile_declaration(&declaration, Some(&grub2)));
    assert!(!is_inactive_profile_declaration(&declaration, Some(&grub)));

    let mut drifted = declaration;
    drifted.arguments.push_str(" unexpected=yes");
    assert!(!is_inactive_profile_declaration(&drifted, Some(&grub2)));
}

#[test]
fn manual_hiddstubs_contract_is_admitted_and_drift_fails_closed() {
    let dir =
        std::env::temp_dir().join(format!("aros-verify-test-hiddstubs-{}", std::process::id()));
    let compiler = dir.join("compiler/libhiddstubs");
    fs::create_dir_all(&compiler).unwrap();
    let source = "#MM- linklibs : linklibs-hiddstubs\n\
                      #MM- linklibs-hiddstubs: linklibs-hidd-stubs\n\
                      HIDD_LIB := $(AROS_LIB)/libhiddstubs.a\n\
                      HIDD_STUBS_OBJ := $(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))\n\
                      #MM\n\
                      linklibs-hiddstubs: $(HIDD_LIB)\n\
                      $(HIDD_LIB) : $(HIDD_STUBS_OBJ)\n\
                      \t%mklib_q from=$^\n";
    fs::write(compiler.join("mmakefile.src"), source).unwrap();
    let declarations = collect_manual_aggregate_declarations(&dir);
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].mmake, "linklibs-hiddstubs");

    fs::write(
        compiler.join("mmakefile.src"),
        source.replace("%mklib_q from=$^", "%mklib_q from=$^ extra=yes"),
    )
    .unwrap();
    assert!(collect_manual_aggregate_declarations(&dir).is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn profile_declarations_follow_make_conditionals_without_guessing_unknowns() {
    let dir = std::env::temp_dir().join(format!(
        "aros-verify-test-conditional-profile-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mmakefile.src");
    fs::write(
        &file,
        "ifneq ($(AROS_TARGET_CPU32),)\n\
             %build_linklib mmake=cpu32 libname=cpu32 files=cpu32\n\
             endif\n\
             ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             %build_prog mmake=x86 progname=x86 files=x86\n\
             else ifneq (,$(filter arm aarch64,$(AROS_TARGET_CPU)))\n\
             %build_prog mmake=arm-family progname=arm-family files=arm-family\n\
             endif\n\
             SELECTED := $(AROS_TARGET_CPU)\n\
             ifeq ($(findstring arm,$(SELECTED)),arm)\n\
             %build_prog mmake=arm-spelling progname=arm-spelling files=arm-spelling\n\
             endif\n\
             ifeq ($(EXTERNAL_SWITCH),yes)\n\
             %build_prog mmake=unresolved progname=unresolved files=unresolved\n\
             endif\n",
    )
    .unwrap();

    let ids = |scope: &ArchitectureScope| -> BTreeSet<String> {
        collect_declarations_for_profile(&dir, std::slice::from_ref(&file), scope)
            .into_iter()
            .map(|declaration| declaration.mmake)
            .collect()
    };
    assert_eq!(
        ids(&ArchitectureScope::new("x86_64", "pc")),
        BTreeSet::from([
            "cpu32".to_owned(),
            "unresolved".to_owned(),
            "x86".to_owned()
        ])
    );
    assert_eq!(
        ids(&ArchitectureScope::new("arm", "raspi")),
        BTreeSet::from([
            "arm-family".to_owned(),
            "arm-spelling".to_owned(),
            "unresolved".to_owned(),
        ])
    );
    assert_eq!(
        ids(&ArchitectureScope::new("aarch64", "raspi")),
        BTreeSet::from(["arm-family".to_owned(), "unresolved".to_owned()])
    );

    // No profile means the historic global inventory: every textual
    // declaration remains visible, including mutually exclusive branches.
    assert_eq!(collect_declarations(&dir, &[file]).len(), 5);
    fs::remove_dir_all(dir).unwrap();
}

/// AROS keeps its translations in submodules, and 71 of the tree's
/// mmakefiles live there. A checkout without them yields a smaller
/// inventory than the counts below, so say that rather than compare against
/// a different tree.
fn require_translation_submodules(root: &Path) {
    assert!(
        root.join("rom/dos/catalogs/mmakefile.src").exists(),
        "the translation submodules are not checked out"
    );
}

fn eligible_declarations(
    root: &Path,
    files: &[PathBuf],
    scope: &ArchitectureScope,
    conditional: bool,
) -> Vec<Declaration> {
    let declarations = if conditional {
        collect_declarations_for_profile(root, files, scope)
    } else {
        collect_declarations(root, files)
    };
    declarations
        .into_iter()
        .filter(|declaration| scope.declaration_is_eligible(declaration))
        .collect()
}

fn eligible_ids(
    root: &Path,
    files: &[PathBuf],
    scope: &ArchitectureScope,
    conditional: bool,
) -> BTreeSet<String> {
    eligible_declarations(root, files, scope, conditional)
        .into_iter()
        .map(|declaration| declaration.mmake)
        .collect()
}

// The inventory counts and the provisioning split used to be one test, with
// the provisioning fingerprint asserted first. That made the fingerprint a
// gate in front of a gate: while it was stale the eight counts behind it
// were never evaluated, and they had been wrong by 71 or 72 the whole time
// without anything saying so. These counts do not depend on the
// classification, so they are their own test now, and a stale pin costs the
// two tests that really depend on it. OPEN-POINTS 7.
#[test]
fn current_architecture_denominators_are_pinned() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    require_translation_submodules(&root);
    let files = find_mmakefiles(&root);
    let ids = |scope: &ArchitectureScope, conditional: bool| -> BTreeSet<String> {
        eligible_ids(&root, &files, scope, conditional)
    };

    let x86 = ArchitectureScope::new("x86_64", "pc");
    let arm = ArchitectureScope::new("arm", "raspi");
    let aarch64 = ArchitectureScope::new("aarch64", "raspi");
    let global: BTreeSet<String> = collect_declarations(&root, &files)
        .into_iter()
        .map(|declaration| declaration.mmake)
        .collect();

    // The August 2026 upstream sync adds the LLVM runtimes umbrella and
    // new Bluetooth, Raspberry Pi and driver declarations while replacing
    // the split rtl8168/rtl8169 lanes with rtl816x.
    assert_eq!(global.len(), 1211);
    assert_eq!(ids(&x86, true).len(), 1088);
    assert_eq!(ids(&arm, true).len(), 1082);
    assert_eq!(ids(&aarch64, true).len(), 1082);
    assert!(global.contains("test-library-dummytest_auto"));
    assert!(!global.contains("mesa3d-linklib-galliumvm"));

    let arm_removed: BTreeSet<String> = ids(&arm, false)
        .difference(&ids(&arm, true))
        .cloned()
        .collect();
    assert_eq!(
        arm_removed,
        BTreeSet::from([
            "crosstools-compiler-rt32".to_owned(),
            // The release producer of the same runtime, declared beside it
            // and gated the same way.
            "crosstools-compiler-rt32-release".to_owned(),
            "linklibs-amiga32".to_owned(),
            "linklibs-arossupport32".to_owned(),
            "linklibs-autoinit32".to_owned(),
        ])
    );
    let aarch64_removed: BTreeSet<String> = ids(&aarch64, false)
        .difference(&ids(&aarch64, true))
        .cloned()
        .collect();
    assert_eq!(aarch64_removed, arm_removed);
}

// The other half: what the provisioning classification takes out of the
// target obligations. This one does depend on the audited fingerprints, so
// it says so in its own failure rather than taking other assertions down
// with it.
#[test]
fn toolchain_provisioning_splits_the_target_obligations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    require_translation_submodules(&root);
    let files = find_mmakefiles(&root);
    let context = detect_toolchain_provisioning_context(&root);
    assert!(
        context.llvm,
        "the LLVM provisioning boundary is no longer structurally valid; \
             the nine host-tool declarations have been returned to the \
             ordinary target coverage gate"
    );
    assert!(
        context.gcc_libatomic,
        "the GCC libatomic provisioning boundary is no longer structurally valid"
    );
    let target_ids = |scope: &ArchitectureScope| -> BTreeSet<String> {
        eligible_declarations(&root, &files, scope, true)
            .into_iter()
            .filter(|declaration| !is_toolchain_provisioning_declaration(declaration, context))
            .map(|declaration| declaration.mmake)
            .collect()
    };
    let provisioning_ids = |scope: &ArchitectureScope| -> BTreeSet<String> {
        eligible_declarations(&root, &files, scope, true)
            .into_iter()
            .filter(|declaration| is_toolchain_provisioning_declaration(declaration, context))
            .map(|declaration| declaration.mmake)
            .collect()
    };

    let x86 = ArchitectureScope::new("x86_64", "pc");
    let arm = ArchitectureScope::new("arm", "raspi");
    let aarch64 = ArchitectureScope::new("aarch64", "raspi");
    let global_declarations = collect_declarations(&root, &files);
    let global_target: BTreeSet<String> = global_declarations
        .iter()
        .filter(|declaration| !is_toolchain_provisioning_declaration(declaration, context))
        .map(|declaration| declaration.mmake.clone())
        .collect();
    let global_provisioning: BTreeSet<String> = global_declarations
        .iter()
        .filter(|declaration| is_toolchain_provisioning_declaration(declaration, context))
        .map(|declaration| declaration.mmake.clone())
        .collect();

    assert_eq!(global_target.len(), 1201);
    assert_eq!(target_ids(&x86).len(), 1078);
    assert_eq!(target_ids(&arm).len(), 1074);
    assert_eq!(target_ids(&aarch64).len(), 1074);
    let common_provisioning = BTreeSet::from([
        "crosstools-compiler-rt".to_owned(),
        "crosstools-compiler-rt-release".to_owned(),
        "crosstools-libunwind".to_owned(),
        "crosstools-libunwind-release".to_owned(),
        "crosstools-llvm-runtimes".to_owned(),
        "crosstools-llvm-runtimes-release".to_owned(),
        "crosstools-llvm-toolchain".to_owned(),
        "tools-crosstools-gcc-libatomic".to_owned(),
    ]);
    let mut x86_provisioning = common_provisioning.clone();
    x86_provisioning.insert("crosstools-compiler-rt32".to_owned());
    x86_provisioning.insert("crosstools-compiler-rt32-release".to_owned());
    assert_eq!(global_provisioning, x86_provisioning);
    assert_eq!(provisioning_ids(&x86), x86_provisioning);
    assert_eq!(provisioning_ids(&arm), common_provisioning);
    assert_eq!(provisioning_ids(&aarch64), common_provisioning);
    // GCC builds libatomic with the target compiler, but does so below the
    // host-side compiler work tree and installs it into the compiler
    // provisioning lane. It is not a target-tree product.
    assert!(!global_target.contains("tools-crosstools-gcc-libatomic"));
    for inventory in [target_ids(&x86), target_ids(&arm), target_ids(&aarch64)] {
        assert!(inventory.contains("test-library-dummytest_auto"));
        assert!(!inventory.contains("mesa3d-linklib-galliumvm"));
        assert!(!inventory.contains("tools-crosstools-gcc-libatomic"));
    }
}

#[test]
fn llvm_provisioning_context_is_structural_not_pinned() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let mmake = read_source(&root.join(LLVM_PROVISIONING_FILE)).unwrap();
    let make_config = read_source(&root.join("config/make.cfg.in")).unwrap();
    let cmake_lists = read_source(&root.join("CMakeLists.txt")).unwrap();

    assert!(llvm_provisioning_context_matches_sources(
        &mmake,
        &make_config,
        &cmake_lists,
    ));
    assert!(llvm_provisioning_context_matches_sources(
        &(mmake + "\nUNRELATED_RELEASE_SETTING := changed\n"),
        &make_config,
        &cmake_lists,
    ));
}

#[test]
fn llvm_provisioning_contract_mutations_fail_closed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let mmake = read_source(&root.join(LLVM_PROVISIONING_FILE)).unwrap();
    let make_config = read_source(&root.join("config/make.cfg.in")).unwrap();
    let cmake_lists = read_source(&root.join("CMakeLists.txt")).unwrap();
    assert!(llvm_provisioning_context_matches_sources(
        &mmake,
        &make_config,
        &cmake_lists,
    ));

    let assert_context_rejected = |mmake: &str, make_config: &str, cmake_lists: &str| {
        assert!(!llvm_provisioning_context_matches_sources(
            mmake,
            make_config,
            cmake_lists,
        ));
    };
    assert_context_rejected(
        &mmake.replace(
            "LLVM_BUILD_BINDIR:=$(CROSSTOOLSDIR)/bin",
            "LLVM_BUILD_BINDIR:=$(HOSTDIR)/bin",
        ),
        &make_config,
        &cmake_lists,
    );
    assert_context_rejected(
        &mmake.replace(
            "AROS_TOOLCHAIN_DEFAULT_SYSROOT ?= $(AROS_DEVELOPER)",
            "AROS_TOOLCHAIN_DEFAULT_SYSROOT := /fixed/non-relocatable/sysroot",
        ),
        &make_config,
        &cmake_lists,
    );
    assert_context_rejected(
        &mmake,
        &make_config.replace("@AROS_CROSSTOOLSDIR@", "${AROS_BUILD_DIR}/toolchain"),
        &cmake_lists,
    );
    assert_context_rejected(
        &mmake,
        &make_config,
        &cmake_lists.replace(
            "set(CMAKE_SYSTEM_NAME Generic)",
            "set(CMAKE_SYSTEM_NAME Darwin)",
        ),
    );

    let declarations = collect_declarations(
        &root,
        std::slice::from_ref(&root.join(LLVM_PROVISIONING_FILE)),
    );
    let context = ToolchainProvisioningContext {
        llvm: true,
        gcc_libatomic: true,
    };
    for (needle, replacement) in [
        ("compiler=host", "compiler=target"),
        (
            "prefix=\"$(CROSSTOOLSDIR)\"",
            "prefix=\"$(AROS_DEVELOPER)\"",
        ),
        ("usecppflags=no", "usecppflags=yes"),
    ] {
        let mut declaration = declarations
            .iter()
            .find(|declaration| declaration.mmake == "crosstools-compiler-rt")
            .unwrap()
            .clone();
        declaration.arguments = declaration.arguments.replace(needle, replacement);
        assert!(!is_toolchain_provisioning_declaration(
            &declaration,
            context
        ));
        // With no generated target, falling out of the provisioning
        // contract makes this an ordinary missing target again.
        let inventory = [&declaration];
        let (provisioning, target_graph) = split_toolchain_provisioning(&inventory, context);
        assert!(provisioning.is_empty());
        assert_eq!(target_graph[0].mmake, "crosstools-compiler-rt");
    }

    let gnu_declarations = collect_declarations(
        &root,
        std::slice::from_ref(&root.join(GCC_PROVISIONING_FILE)),
    );
    let libatomic = gnu_declarations
        .iter()
        .find(|declaration| declaration.mmake == "tools-crosstools-gcc-libatomic")
        .unwrap();
    assert!(is_toolchain_provisioning_declaration(libatomic, context));
    let mut drifted = libatomic.clone();
    drifted.arguments = drifted.arguments.replace("basedir=", "basedir=$(GENDIR)");
    assert!(!is_toolchain_provisioning_declaration(&drifted, context));
}

#[test]
fn finds_and_scans_both_mmakefile_names() {
    let dir = std::env::temp_dir().join(format!(
        "aros-verify-test-mmakefiles-{}",
        std::process::id()
    ));
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(dir.join("mmakefile"), "%build_prog mmake=plain\n").unwrap();
    fs::write(nested.join("mmakefile.src"), "%build_prog mmake=with-src\n").unwrap();
    fs::write(dir.join("mmakefile.txt"), "%build_prog mmake=ignored\n").unwrap();

    let files = find_mmakefiles(&dir);
    let relative: Vec<String> = files
        .iter()
        .map(|file| {
            file.strip_prefix(&dir)
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(relative, ["mmakefile", "nested/mmakefile.src"]);

    let declarations = collect_declarations(&dir, &files);
    let ids: BTreeSet<&str> = declarations
        .iter()
        .map(|declaration| declaration.mmake.as_str())
        .collect();
    assert_eq!(ids, BTreeSet::from(["plain", "with-src"]));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cache_timestamp_must_be_newer_than_every_input() {
    let base = SystemTime::UNIX_EPOCH;
    let one = base + std::time::Duration::from_secs(1);
    let two = base + std::time::Duration::from_secs(2);
    let three = base + std::time::Duration::from_secs(3);

    assert!(timestamps_are_fresh(three, &[one, two]));
    assert!(!timestamps_are_fresh(two, &[one, two]));
    assert!(!timestamps_are_fresh(two, &[three]));
}

#[test]
fn discovers_recursive_genmf_template_dependencies() {
    let dir = std::env::temp_dir().join(format!(
        "aros-verify-test-genmf-dependencies-{}",
        std::process::id()
    ));
    let config = dir.join("config");
    let tools = dir.join("tools/genmf");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        config.join("make.tmpl"),
        "%include make-cmake.tmpl\n%include \"make-meson.tmpl\"\n",
    )
    .unwrap();
    fs::write(
        config.join("make-cmake.tmpl"),
        "%include make-common.tmpl\n",
    )
    .unwrap();
    fs::write(config.join("make-meson.tmpl"), "").unwrap();
    fs::write(config.join("make-common.tmpl"), "").unwrap();
    fs::write(tools.join("genmf.py"), "").unwrap();

    let relative: Vec<String> = genmf_dependency_files(&dir)
        .iter()
        .map(|path| {
            path.strip_prefix(&dir)
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(
        relative,
        [
            "config/make-cmake.tmpl",
            "config/make-common.tmpl",
            "config/make-meson.tmpl",
            "config/make.tmpl",
            "tools/genmf/genmf.py",
        ]
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn genmf_failure_report_is_sorted_deduplicated_and_cleared() {
    let report = std::env::temp_dir().join(format!(
        "aros-verify-test-genmf-errors-{}.txt",
        std::process::id()
    ));
    write_failure_report(
        &report,
        vec![
            "z-error".to_owned(),
            "a-error".to_owned(),
            "z-error".to_owned(),
        ],
        "test failures",
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&report).unwrap(), "a-error\nz-error\n");

    write_failure_report(&report, Vec::new(), "no failures").unwrap();
    assert!(!report.exists());
}

#[test]
fn records_a_failed_genmf_expansion_instead_of_dropping_it() {
    let dir = std::env::temp_dir().join(format!(
        "aros-verify-test-genmf-failure-{}",
        std::process::id()
    ));
    let tools = dir.join("tools/genmf");
    let config = dir.join("config");
    let cache = dir.join("cache");
    fs::create_dir_all(&tools).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::write(config.join("make.tmpl"), "").unwrap();
    fs::write(
        tools.join("genmf.py"),
        "import sys\nsys.stderr.write('intentional genmf failure\\n')\nsys.exit(9)\n",
    )
    .unwrap();
    let mmakefile = dir.join("mmakefile");
    fs::write(&mmakefile, "").unwrap();

    let result = expand_all(&dir, &cache, &[mmakefile], true);
    assert!(result.expanded.is_empty());
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].file, "mmakefile");
    assert!(result.failures[0]
        .message
        .starts_with("mmakefile: genmf exited with"));
    assert!(result.failures[0]
        .message
        .contains("intentional genmf failure"));
    assert!(!cache.join("mmakefile.mk").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reads_a_declaration_spread_over_several_lines() {
    let dir = std::env::temp_dir().join("aros-verify-test-decl");
    let sub = dir.join("rom/dos");
    fs::create_dir_all(&sub).unwrap();
    let f = sub.join("mmakefile.src");
    fs::write(
        &f,
        "%build_module mmake=kernel-dos \\\n  modname=dos modtype=library \\\n  files=$(FILES)\n",
    )
    .unwrap();
    let decls = collect_declarations(&dir, &[f]);
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].mmake, "kernel-dos");
    assert_eq!(decls[0].macro_name, "build_module");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_trailing_continuation_does_not_swallow_the_next_declaration() {
    let dir = std::env::temp_dir().join("aros-verify-test-trailing-continuation");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("mmakefile.src");
    fs::write(
        &f,
        "%build_prog mmake=first files=first \\\n\n%build_prog mmake=second files=second\n",
    )
    .unwrap();
    let decls = collect_declarations(&dir, &[f]);
    let names: Vec<&str> = decls.iter().map(|d| d.mmake.as_str()).collect();
    assert_eq!(names, vec!["first", "second"]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reads_every_declaration_in_one_file() {
    // The case the transpiler's own regex used to miss: several modules in
    // one mmakefile with a single %common at the end.
    let dir = std::env::temp_dir().join("aros-verify-test-multi");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("mmakefile.src");
    fs::write(
        &f,
        "%build_module  mmake=a modname=A modtype=mui files=a\n\
             %build_module  mmake=b modname=B modtype=mui files=b\n\
             %build_module  mmake=c modname=C modtype=mui files=c\n\
             %common\n",
    )
    .unwrap();
    let decls = collect_declarations(&dir, &[f]);
    let names: Vec<&str> = decls.iter().map(|d| d.mmake.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn counts_a_package_declaration_too() {
    // %make_package and %link_kickstart emit NAME, not MMAKE_ID.
    let generated = "\
aros_make_package(
    NAME kernel-package-base
    OUTPUT \"x\"
)
aros_link_kickstart(
    NAME kernel-pc-x86_64-kernel
    OUTPUT \"y\"
)
";
    let ours = collect_ours(generated);
    assert!(ours.contains_key("kernel-package-base"));
    assert!(ours.contains_key("kernel-pc-x86_64-kernel"));
}

#[test]
fn pairs_each_mmake_id_with_its_target_name() {
    let generated = "\
aros_add_program(
    TARGET SysLog
    MMAKE_ID aros-tcpip-apps-syslog
)
aros_build_module(
    TARGET dos
    MMAKE_ID kernel-dos
)
";
    let ours = collect_ours(generated);
    assert_eq!(ours.get("aros-tcpip-apps-syslog").unwrap(), "SysLog");
    assert_eq!(ours.get("kernel-dos").unwrap(), "dos");
}

#[test]
fn counts_an_icon_declaration_without_a_compiled_target_name() {
    let generated = "\
aros_declare_icon_target(
    MMAKE_ID iconset-Gorilla-wbench-icons
    DIRECTORY \"images/IconSets/Gorilla\"
)
";
    let ours = collect_ours(generated);
    assert!(ours.contains_key("iconset-Gorilla-wbench-icons"));
    assert_eq!(ours["iconset-Gorilla-wbench-icons"], "");
}

#[test]
fn ignores_names_owned_by_catalogs_and_header_transforms() {
    let generated = "\
aros_build_catalogs(
    MMAKE_ID locale-catalogs-dos
    NAME \"dos\"
    SOURCE_DIR \"workbench/catalogs\"
)
aros_transform_header(
    NAME \"workbench-libs-z-geninc\"
    INPUT \"zconf.h.in\"
    OUTPUT \"zconf.h\"
)
";
    let ours = collect_ours(generated);
    assert_eq!(ours.len(), 1);
    assert_eq!(ours["locale-catalogs-dos"], "");
    assert!(!ours.contains_key("\"dos\""));
    assert!(!ours.contains_key("\"workbench-libs-z-geninc\""));
}

#[test]
fn a_declaration_without_a_name_is_skipped() {
    // %build_icons and friends are sometimes invoked without mmake=; they
    // have no identity to compare against.
    let dir = std::env::temp_dir().join("aros-verify-test-noname");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("mmakefile.src");
    fs::write(&f, "%build_icons dir=images\n").unwrap();
    assert!(collect_declarations(&dir, &[f]).is_empty());
    fs::remove_dir_all(&dir).ok();
}
