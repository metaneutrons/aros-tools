use super::{archive_sources, compile_contract, glapi_sources, GLAPI_DIR, GLAPI_MMAKE};
use crate::parser::TargetContext;
use std::fs;
use std::path::Path;

#[test]
fn util_consumer_contract_is_absent_outside_mesa26() {
    let profile = TargetContext {
        mesa_version: Some("20.0.8".to_owned()),
        ..TargetContext::default()
    };
    for target in [None, Some(&profile)] {
        // Neither fetched source nor local util targets are required when
        // this capability is inactive. No compiler-consumer obligation leaks
        // into the Mesa 20 or unselected context.
        let output = super::parse_mesautil(
            Path::new("unused-source-root"),
            Path::new("workbench/libs/mesa/libmesautil"),
            target,
            &[],
            &[],
        )
        .unwrap();
        assert!(output.is_none());
    }
}

#[test]
fn runtime_identity_rejects_changed_source_module_names() {
    let root = mesa26_source_root();
    let profile = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    assert_eq!(
        super::runtime_module_name(
            &root,
            Path::new("workbench/libs/mesa"),
            "mesa3dgl-library",
            "mesa3dgl$(MESAGLBUILD)",
            Some(&profile),
        )
        .unwrap()
        .as_deref(),
        Some("mesa3dgl26-0")
    );
    for raw_name in ["different$(MESAGLBUILD)", "mesa3dgl$(MESAGLVERSION)"] {
        assert!(super::runtime_module_name(
            &root,
            Path::new("workbench/libs/mesa"),
            "mesa3dgl-library",
            raw_name,
            Some(&profile),
        )
        .unwrap_err()
        .contains("unexpected Mesa 26 runtime modname"));
    }
}

fn mesa26_source_root() -> std::path::PathBuf {
    let configured = std::env::var_os("AROS_TEST_MESA26_SOURCE_ROOT")
        .or_else(|| std::env::var_os("AROS_TEST_SOURCE_ROOT"))
        .expect("AROS_TEST_MESA26_SOURCE_ROOT must name the Mesa 26 checkout");
    std::path::PathBuf::from(configured)
}

fn opensbi_riscv64_mesa26_profile() -> TargetContext {
    TargetContext {
        cpu: Some("riscv64".to_owned()),
        platform: Some("opensbi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    }
}

#[test]
fn mesa26_profile_validation_is_scoped_to_exact_capability_targets() {
    let profile = opensbi_riscv64_mesa26_profile();
    let stdc_dir = Path::new("compiler/crt/stdc");

    assert!(
        compile_contract(stdc_dir, "compiler-stdcio", Some(&profile))
            .unwrap()
            .is_none()
    );
    assert!(archive_sources(
        Path::new("unused-source-root"),
        stdc_dir,
        "compiler-stdcio",
        Some(&profile)
    )
    .unwrap()
    .is_none());

    let gallium_dir = Path::new("workbench/libs/gallium");
    assert!(
        compile_contract(gallium_dir, "not-the-gallium-target", Some(&profile))
            .unwrap()
            .is_none()
    );
    assert!(archive_sources(
        Path::new("unused-source-root"),
        gallium_dir,
        "not-the-gallium-target",
        Some(&profile)
    )
    .unwrap()
    .is_none());

    let compile_error = compile_contract(gallium_dir, "workbench-libs-gallium", Some(&profile))
        .err()
        .expect("recognized Mesa compile contract rejects unsupported RISC-V");
    assert!(compile_error.contains(
        "Mesa 26.2.5 capability does not support target profile cpu=riscv64 platform=opensbi"
    ));

    let archive_error = archive_sources(
        Path::new("unused-source-root"),
        Path::new("workbench/libs/mesa/libcompiler"),
        "mesa3d-linklib-compiler",
        Some(&profile),
    )
    .expect_err("recognized Mesa archive capability rejects unsupported RISC-V");
    assert!(archive_error.contains(
        "Mesa 26.2.5 capability does not support target profile cpu=riscv64 platform=opensbi"
    ));
}

#[test]
fn public_gallium_library_uses_the_selected_mesa26_pipe_headers() {
    let profile = TargetContext {
        cpu: Some("arm".to_owned()),
        platform: Some("raspi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some("hard".to_owned()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new("workbench/libs/gallium");
    let contract = compile_contract(relative, "workbench-libs-gallium", Some(&profile))
        .unwrap()
        .unwrap();
    assert!(contract
        .includes
        .iter()
        .any(|path| { path == "${AROS_PORTS_DIR}/mesa/mesa-26.2.5/src/gallium/include" }));
    assert!(!contract
        .includes
        .iter()
        .any(|path| path.contains("mesa-20.0.8")));

    let older = TargetContext {
        mesa_version: Some("20.0.8".to_owned()),
        ..profile
    };
    assert!(
        compile_contract(relative, "workbench-libs-gallium", Some(&older))
            .unwrap()
            .is_none()
    );
}

#[test]
fn egl_archive_is_versioned_and_rejects_recipe_drift() {
    let root = mesa26_source_root();
    let profile = TargetContext {
        cpu: Some("aarch64".to_owned()),
        platform: Some("raspi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new("workbench/libs/egl");
    let sources = super::archive_sources(&root, relative, "workbench-libs-egl", Some(&profile))
        .unwrap()
        .unwrap();
    assert_eq!(sources.c.len(), 16);
    assert!(sources
        .c
        .contains(&format!("{}/src/egl/main/eglsurface", super::SOURCE_ROOT)));
    assert!(sources
        .c
        .contains(&"${AROS_SOURCE_DIR}/workbench/libs/egl/egl_arosmesa".to_owned()));
    let compile = compile_contract(relative, "workbench-libs-egl", Some(&profile))
        .unwrap()
        .unwrap();
    assert!(compile
        .includes
        .contains(&"${AROS_PORTS_DIR}/mesa/mesa-26.2.5/src/egl/main".to_owned()));
    assert!(compile.defines.contains(&"HAVE_AROS_BACKEND".to_owned()));

    let temporary = tempfile::tempdir().unwrap();
    for path in [
        "workbench/libs/egl/mmakefile.src",
        "workbench/libs/mesa/mesa.cfg",
    ] {
        let destination = temporary.path().join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(root.join(path), &destination).unwrap();
    }
    let recipe = temporary.path().join("workbench/libs/egl/mmakefile.src");
    let changed = fs::read_to_string(&recipe).unwrap().replace(
        "$(EGL_PATH)/main/eglsurface",
        "$(EGL_PATH)/main/removed_surface",
    );
    assert!(changed.contains("removed_surface"));
    fs::write(&recipe, changed).unwrap();
    assert!(super::archive_sources(
        temporary.path(),
        relative,
        "workbench-libs-egl",
        Some(&profile)
    )
    .is_err());
}

#[test]
fn v3d_archive_admits_only_the_reviewed_device_tree_recipe() {
    let root = mesa26_source_root();
    let profile = TargetContext {
        cpu: Some("aarch64".to_owned()),
        platform: Some("raspi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new("arch/arm-native/soc/broadcom/2708/hidd/v3d");
    let baseline = archive_sources(&root, relative, "linklibs-gallium_v3d", Some(&profile))
        .unwrap()
        .expect("reviewed Mesa 26 V3D archive");
    let compile = compile_contract(relative, "linklibs-gallium_v3d", Some(&profile))
        .unwrap()
        .expect("Mesa 26 V3D link-library compile contract");
    assert!(compile.defines.contains(&"AROS_MESA26_V3D=1".to_owned()));

    let x86_64 = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    assert!(
        compile_contract(relative, "linklibs-gallium_v3d", Some(&x86_64))
            .unwrap()
            .is_none()
    );

    let temporary = tempfile::tempdir().unwrap();
    for path in [
        "arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src",
        "arch/arm-native/soc/broadcom/2708/hidd/v3d/v3d-26.2.5.sources",
        "workbench/libs/mesa/mesa.cfg",
    ] {
        let destination = temporary.path().join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(root.join(path), destination).unwrap();
    }
    let recipe = temporary
        .path()
        .join("arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src");
    let original = fs::read_to_string(&recipe).unwrap();
    let original_digest = aros_common::sha256_bytes(original.as_bytes()).to_string();
    let old_pin = crate::fingerprints::fingerprint("mesa26-v3d-recipe").unwrap();
    let device_tree_pin =
        crate::fingerprints::fingerprint("mesa26-v3d-recipe-device-tree").unwrap();
    let marker = "V3D_HIDD_SOURCES := \\\n    v3d_init \\\n";
    assert_eq!(original.matches(marker).count(), 1);
    let device_tree_line = "    v3d_dt \\\n";
    let (alternate, alternate_pin) = if original_digest == old_pin {
        (
            original.replacen(marker, &format!("{marker}{device_tree_line}"), 1),
            device_tree_pin,
        )
    } else {
        assert_eq!(original_digest, device_tree_pin);
        assert_eq!(original.matches(device_tree_line).count(), 1);
        (original.replacen(device_tree_line, "", 1), old_pin)
    };
    assert_eq!(
        aros_common::sha256_bytes(alternate.as_bytes()).to_string(),
        alternate_pin
    );
    fs::write(&recipe, &alternate).unwrap();
    let admitted = archive_sources(
        temporary.path(),
        relative,
        "linklibs-gallium_v3d",
        Some(&profile),
    )
    .unwrap()
    .expect("reviewed upstream V3D recipe");
    assert_eq!(admitted.c, baseline.c, "Mesa archive must not change");

    fs::write(
        &recipe,
        format!("{alternate}\n# unreviewed V3D recipe drift\n"),
    )
    .unwrap();
    let error = archive_sources(
        temporary.path(),
        relative,
        "linklibs-gallium_v3d",
        Some(&profile),
    )
    .unwrap_err();
    assert!(
        error.contains("unsupported upstream recipe drift"),
        "{error}"
    );
}

#[test]
fn changed_glapi_recipe_is_rejected_before_source_override() {
    let original = mesa26_source_root();
    let temporary = tempfile::tempdir().unwrap();
    for relative in [
        "workbench/libs/mesa/libglapi/mmakefile.src",
        "workbench/libs/mesa/mesa.cfg",
    ] {
        let output = temporary.path().join(relative);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::copy(original.join(relative), &output).unwrap();
    }
    let profile = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new(GLAPI_DIR);
    assert!(
        glapi_sources(temporary.path(), relative, GLAPI_MMAKE, Some(&profile))
            .unwrap()
            .is_some()
    );
    let recipe = temporary.path().join(GLAPI_DIR).join("mmakefile.src");
    let contents = fs::read_to_string(&recipe).unwrap();
    fs::write(
        &recipe,
        contents.replace("shared-glapi/core", "shared-glapi/other"),
    )
    .unwrap();
    let error = glapi_sources(temporary.path(), relative, GLAPI_MMAKE, Some(&profile)).unwrap_err();
    assert!(
        error.contains("unsupported upstream recipe drift"),
        "{error}"
    );
}

#[test]
fn mesa26_archives_use_only_reviewed_inventories() {
    let original = mesa26_source_root();
    let profile = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    for (relative, mmake, c_count, cxx_count) in [
        (
            "workbench/libs/mesa/libcompiler",
            "mesa3d-linklib-compiler",
            276,
            51,
        ),
        ("workbench/libs/mesa/libmesa", "mesa3d-linklib-mesa", 217, 4),
        (
            "workbench/libs/mesa/libgalliumaux",
            "mesa3d-linklib-galliumauxiliary",
            145,
            0,
        ),
        (
            "workbench/libs/mesa/libmesautil",
            "mesa3d-linklib-mesautil",
            93,
            4,
        ),
    ] {
        let sources = archive_sources(&original, Path::new(relative), mmake, Some(&profile))
            .unwrap()
            .expect("closed Mesa 26 archive");
        assert_eq!(sources.c.len(), c_count, "{mmake}");
        assert_eq!(sources.cxx.len(), cxx_count, "{mmake}");
        assert!(sources.declared);
        assert!(sources.c.iter().all(|source| !source.contains("..")));
    }

    let temporary = tempfile::tempdir().unwrap();
    for relative in [
        "workbench/libs/mesa/mesa.cfg",
        "workbench/libs/mesa/libmesa/mmakefile.src",
        "workbench/libs/mesa/libmesa/mesa-26.2.5.sources",
    ] {
        let output = temporary.path().join(relative);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::copy(original.join(relative), output).unwrap();
    }
    let manifest = temporary
        .path()
        .join("workbench/libs/mesa/libmesa/mesa-26.2.5.sources");
    let mut contents = fs::read_to_string(&manifest).unwrap();
    contents.push_str("\n# unreviewed inventory drift\n");
    fs::write(&manifest, contents).unwrap();
    let error = archive_sources(
        temporary.path(),
        Path::new("workbench/libs/mesa/libmesa"),
        "mesa3d-linklib-mesa",
        Some(&profile),
    )
    .unwrap_err();
    assert!(
        error.contains("unsupported upstream recipe drift"),
        "{error}"
    );
}

#[test]
fn galliumaux_admits_only_the_reviewed_llvmpipe_only_recipe() {
    const AGGREGATE: &str = "\
# mmake reads #MM out of the .src text, so the ifeq above cannot hide these.
# Route through $(AROS_TOOLCHAIN) instead: on gnu the -gnu spelling is never
# declared, and an undeclared metatarget is simply empty.
#MM- mesa3dgl-linklibs : mesa3d-linklib-galliumdrawllvm-$(AROS_TOOLCHAIN)
#MM- mesa3d-linklib-galliumdrawllvm-llvm : mesa3d-linklib-galliumdrawllvm
";
    const LLVMPIPE_ONLY: &str = "\
# Only llvmpipe links this, and hidd-llvmpipe names it directly. Keep it out of
# mesa3dgl-linklibs, so hardware gallium drivers do not wait for the target LLVM.
";
    let root = mesa26_source_root();
    let profile = TargetContext {
        cpu: Some("aarch64".to_owned()),
        platform: Some("raspi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new("workbench/libs/mesa/libgalliumaux");
    let targets = [
        "mesa3d-linklib-galliumauxiliary",
        "mesa3d-linklib-galliumdrawllvm",
        "mesa3d-linklib-galliumtess",
    ];
    let sources = |root: &Path, mmake: &str| {
        archive_sources(root, relative, mmake, Some(&profile))
            .unwrap()
            .expect("reviewed Mesa 26 galliumaux recipe")
    };
    let baseline = targets.map(|mmake| sources(&root, mmake));

    let temporary = tempfile::tempdir().unwrap();
    for path in [
        "workbench/libs/mesa/libgalliumaux/mmakefile.src",
        "workbench/libs/mesa/libgalliumaux/galliumaux-26.2.5.sources",
        "workbench/libs/mesa/mesa.cfg",
    ] {
        let destination = temporary.path().join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(root.join(path), destination).unwrap();
    }
    let recipe = temporary
        .path()
        .join("workbench/libs/mesa/libgalliumaux/mmakefile.src");
    // The upstream-synchronised recipe is a third reviewed state; start from
    // the reference state it was derived from.
    let current = fs::read_to_string(&recipe).unwrap();
    if aros_common::sha256_bytes(current.as_bytes()).to_string()
        == crate::fingerprints::fingerprint(GALLIUMAUX_RECIPE_REVISION.synchronised).unwrap()
    {
        write_other_reviewed_state(&root, &recipe, &GALLIUMAUX_RECIPE_REVISION);
    }
    let original = fs::read_to_string(&recipe).unwrap();
    let original_digest = aros_common::sha256_bytes(original.as_bytes()).to_string();
    let old_pin = crate::fingerprints::fingerprint("mesa26-galliumaux-recipe").unwrap();
    let llvmpipe_only_pin =
        crate::fingerprints::fingerprint("mesa26-galliumaux-recipe-llvmpipe-only").unwrap();
    let (alternate, alternate_pin) = if original_digest == old_pin {
        assert_eq!(original.matches(AGGREGATE).count(), 1);
        (
            original.replacen(AGGREGATE, LLVMPIPE_ONLY, 1),
            llvmpipe_only_pin,
        )
    } else {
        assert_eq!(original_digest, llvmpipe_only_pin);
        assert_eq!(original.matches(LLVMPIPE_ONLY).count(), 1);
        (original.replacen(LLVMPIPE_ONLY, AGGREGATE, 1), old_pin)
    };
    assert_eq!(
        aros_common::sha256_bytes(alternate.as_bytes()).to_string(),
        alternate_pin
    );
    fs::write(&recipe, &alternate).unwrap();
    for (mmake, expected) in targets.iter().zip(&baseline) {
        let admitted = sources(temporary.path(), mmake);
        assert_eq!(admitted.c, expected.c, "{mmake} sources must not change");
        assert_eq!(
            admitted.cxx, expected.cxx,
            "{mmake} sources must not change"
        );
    }

    fs::write(
        &recipe,
        format!("{alternate}\n# unreviewed galliumaux recipe drift\n"),
    )
    .unwrap();
    for mmake in targets {
        let error = archive_sources(temporary.path(), relative, mmake, Some(&profile)).unwrap_err();
        assert!(
            error.contains("unsupported upstream recipe drift"),
            "{mmake}: {error}"
        );
    }
}

/// One reviewed rewrite of a recipe or inventory: `(before, after)` text pairs
/// taking the qualified AROS-NX reference to the state after the upstream
/// synchronisation, with the fingerprint names of both states.
struct ReviewedRevision {
    path: &'static str,
    edits: &'static [(&'static str, &'static str)],
    reference: &'static str,
    synchronised: &'static str,
}

const MESA_CONFIG_REVISION: ReviewedRevision = ReviewedRevision {
    path: "workbench/libs/mesa/mesa.cfg",
    edits: &[(
        "PATCHSPEC := mesa-26.2.5-aros.diff:mesa-26.2.5:-p1\nelse ifeq ($(OPT_MESAGL),20.0.8)\nPATCHSPEC := mesa-20.0.8-aros-nx.diff:mesa-20.0.8:-p1\nelse\n",
        "PATCHSPEC := mesa-26.2.5-aros.diff:mesa-26.2.5:-p1\nelse\n",
    )],
    reference: "mesa26-config",
    synchronised: "mesa26-config-without-mesa20-nx-patch",
};

const GALLIUMAUX_RECIPE_REVISION: ReviewedRevision = ReviewedRevision {
    path: "workbench/libs/mesa/libgalliumaux/mmakefile.src",
    edits: &[
        (
            "ifeq ($(AROS_TOOLCHAIN),llvm)\nGALLIUM_AUXILIARY_GENERATED_SOURCES += \\\n            gallivm/lp_bld_nir_no_integer_algebraic.c\nendif\n",
            "# gallivm's no-integer NIR pass includes lp_bld_nir.h, which needs the LLVM\n# headers: generated for the LLVM toolchain only.\nifeq ($(AROS_TOOLCHAIN),llvm)\nGALLIUM_AUXILIARY_GENERATED_SOURCES += $(MESA26_GALLIUMAUX_GENERATED_LLVM_C_SOURCES)\nendif\n",
        ),
        (
            "# gallivm's pass for LLVM without native integers; it needs LLVM headers and\n# is therefore added only to the LLVM toolchain's generated objects above.\n",
            "# and gallivm's for LLVM without native integers (LLVM toolchain only, see\n# GALLIUM_AUXILIARY_GENERATED_SOURCES); it joins the generated objects here\n# rather than libgalliumvm\n",
        ),
    ],
    reference: "mesa26-galliumaux-recipe",
    synchronised: "mesa26-galliumaux-recipe-llvm-generated-sources",
};

const GALLIUMAUX_MANIFEST_REVISION: ReviewedRevision = ReviewedRevision {
    path: "workbench/libs/mesa/libgalliumaux/galliumaux-26.2.5.sources",
    edits: &[(
        "\tdraw/draw_nir_lower_opcodes.c\n\nMESA26_GALLIUMAUX_GENERATED_HEADERS",
        "\tdraw/draw_nir_lower_opcodes.c\n\n# Generated for the LLVM toolchain only: gallivm's no-integer NIR pass includes\n# lp_bld_nir.h, which needs the LLVM headers. It joins the generated objects of\n# this archive rather than libgalliumvm (Mesa 26.2 recipe in upstream AROS).\nMESA26_GALLIUMAUX_GENERATED_LLVM_C_SOURCES := \\\n\tgallivm/lp_bld_nir_no_integer_algebraic.c\n\nMESA26_GALLIUMAUX_GENERATED_HEADERS",
    )],
    reference: "mesa26-galliumaux-manifest",
    synchronised: "mesa26-galliumaux-manifest-llvm-generated-sources",
};

const LLVMPIPE_HIDD_RECIPE_REVISION: ReviewedRevision = ReviewedRevision {
    path: "workbench/hidds/llvmpipe/mmakefile.src",
    edits: &[(
        "# Link target-side LLVM archives by path. A -l lookup can pick host archives\n# from the crosstools directory before the AROS developer directory.\n",
        "# The target-side LLVM archives, by path: as -l names they would be looked\n# up through every -L in order, and TARGET_CXX_LDFLAGS puts the crosstools\n# lib directory first, which holds the host's own libLLVM*.a when the AROS\n# toolchain was built with LLVM.\n",
    )],
    reference: "mesa26-llvmpipe-hidd-recipe",
    synchronised: "mesa26-llvmpipe-hidd-recipe-llvm-lib-comment",
};

/// Writes the other reviewed state of `revision` into `destination` and checks
/// that its digest is exactly the named fingerprint. The checkout may be in
/// either state; the result is the opposite one.
fn write_other_reviewed_state(root: &Path, destination: &Path, revision: &ReviewedRevision) {
    let original = fs::read_to_string(root.join(revision.path)).unwrap();
    let digest = aros_common::sha256_bytes(original.as_bytes()).to_string();
    let reference = crate::fingerprints::fingerprint(revision.reference).unwrap();
    let synchronised = crate::fingerprints::fingerprint(revision.synchronised).unwrap();
    let (mut text, expected) = if digest == reference {
        (original, synchronised)
    } else {
        assert_eq!(
            digest, synchronised,
            "{} is in no reviewed state",
            revision.path
        );
        (original, reference)
    };
    for (before, after) in revision.edits {
        let (from, to) = if expected == synchronised {
            (before, after)
        } else {
            (after, before)
        };
        assert_eq!(text.matches(from).count(), 1, "{}: {from:?}", revision.path);
        text = text.replacen(from, to, 1);
    }
    assert_eq!(
        aros_common::sha256_bytes(text.as_bytes()).to_string(),
        expected,
        "{}",
        revision.path
    );
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(destination, text).unwrap();
}

fn x86_64_mesa26_profile() -> TargetContext {
    TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.2.5".to_owned()),
        ..TargetContext::default()
    }
}

#[test]
fn upstream_synchronised_gallium_recipes_are_admitted_with_unchanged_sources() {
    let root = mesa26_source_root();
    let profile = x86_64_mesa26_profile();
    let relative = Path::new("workbench/libs/mesa/libgalliumaux");
    let targets = [
        "mesa3d-linklib-galliumauxiliary",
        "mesa3d-linklib-galliumdrawllvm",
        "mesa3d-linklib-galliumtess",
    ];
    let baseline: Vec<_> = targets
        .iter()
        .map(|target| {
            archive_sources(&root, relative, target, Some(&profile))
                .unwrap()
                .expect("reviewed Mesa 26 archive")
        })
        .collect();

    let temporary = tempfile::tempdir().unwrap();
    for revision in [
        &MESA_CONFIG_REVISION,
        &GALLIUMAUX_RECIPE_REVISION,
        &GALLIUMAUX_MANIFEST_REVISION,
    ] {
        write_other_reviewed_state(&root, &temporary.path().join(revision.path), revision);
    }
    for (target, expected) in targets.iter().zip(&baseline) {
        let admitted = archive_sources(temporary.path(), relative, target, Some(&profile))
            .unwrap_or_else(|error| panic!("{target}: {error}"))
            .expect("reviewed synchronised archive");
        assert_eq!(admitted.c, expected.c, "{target}");
        assert_eq!(admitted.cxx, expected.cxx, "{target}");
    }

    let recipe = temporary.path().join(GALLIUMAUX_RECIPE_REVISION.path);
    let text = fs::read_to_string(&recipe).unwrap();
    fs::write(&recipe, format!("{text}\n# unreviewed drift\n")).unwrap();
    let error = archive_sources(
        temporary.path(),
        relative,
        "mesa3d-linklib-galliumdrawllvm",
        Some(&profile),
    )
    .unwrap_err();
    assert!(
        error.contains("unsupported upstream recipe drift"),
        "{error}"
    );
}

#[test]
fn upstream_synchronised_mesa_config_and_llvmpipe_hidd_are_admitted() {
    let root = mesa26_source_root();
    let profile = x86_64_mesa26_profile();
    let hidd = Path::new("workbench/hidds/llvmpipe");
    let baseline = super::module_sources(&root, hidd, "hidd-llvmpipe", Some(&profile))
        .unwrap()
        .expect("reviewed llvmpipe HIDD");
    let identity = |root: &Path| {
        super::runtime_module_name(
            root,
            Path::new("workbench/libs/mesa"),
            "mesa3dgl-library",
            "mesa3dgl$(MESAGLBUILD)",
            Some(&profile),
        )
    };
    let temporary = tempfile::tempdir().unwrap();
    for revision in [&MESA_CONFIG_REVISION, &LLVMPIPE_HIDD_RECIPE_REVISION] {
        write_other_reviewed_state(&root, &temporary.path().join(revision.path), revision);
    }
    let admitted = super::module_sources(temporary.path(), hidd, "hidd-llvmpipe", Some(&profile))
        .unwrap()
        .expect("reviewed synchronised llvmpipe HIDD");
    assert_eq!(admitted.c, baseline.c);
    assert_eq!(
        identity(temporary.path()).unwrap(),
        identity(&root).unwrap()
    );

    let config = temporary.path().join(MESA_CONFIG_REVISION.path);
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{text}\n# unreviewed drift\n")).unwrap();
    assert!(identity(temporary.path())
        .unwrap_err()
        .contains("unsupported upstream recipe drift"));
}
