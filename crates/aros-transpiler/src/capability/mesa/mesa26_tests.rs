use super::{archive_sources, compile_contract, glapi_sources, GLAPI_DIR, GLAPI_MMAKE};
use crate::parser::TargetContext;
use std::fs;
use std::path::Path;

fn mesa26_source_root() -> std::path::PathBuf {
    let configured = std::env::var_os("AROS_TEST_MESA26_SOURCE_ROOT")
        .or_else(|| std::env::var_os("AROS_TEST_SOURCE_ROOT"))
        .expect("AROS_TEST_MESA26_SOURCE_ROOT must name the Mesa 26 checkout");
    std::path::PathBuf::from(configured)
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
        mesa_version: Some("26.0.0".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new("workbench/libs/gallium");
    let contract = compile_contract(relative, "workbench-libs-gallium", Some(&profile))
        .unwrap()
        .unwrap();
    assert!(contract
        .includes
        .iter()
        .any(|path| { path == "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include" }));
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
        mesa_version: Some("26.0.0".to_owned()),
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
        .contains(&"${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/egl/main".to_owned()));
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
        mesa_version: Some("26.0.0".to_owned()),
        ..TargetContext::default()
    };
    let relative = Path::new("arch/arm-native/soc/broadcom/2708/hidd/v3d");
    let baseline = archive_sources(&root, relative, "linklibs-gallium_v3d", Some(&profile))
        .unwrap()
        .expect("reviewed Mesa 26 V3D archive");

    let temporary = tempfile::tempdir().unwrap();
    for path in [
        "arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src",
        "arch/arm-native/soc/broadcom/2708/hidd/v3d/v3d-26.0.0.sources",
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
    let marker = "V3D_HIDD_SOURCES := \\\n    v3d_init \\\n";
    assert_eq!(original.matches(marker).count(), 1);
    let updated = original.replacen(marker, &format!("{marker}    v3d_dt \\\n"), 1);
    assert_eq!(
        aros_common::sha256_bytes(updated.as_bytes()).to_string(),
        crate::fingerprints::fingerprint("mesa26-v3d-recipe-device-tree").unwrap()
    );
    fs::write(&recipe, &updated).unwrap();
    let admitted = archive_sources(
        temporary.path(),
        relative,
        "linklibs-gallium_v3d",
        Some(&profile),
    )
    .unwrap()
    .expect("reviewed upstream V3D recipe");
    assert_eq!(admitted.c, baseline.c, "Mesa archive must not change");

    fs::write(&recipe, updated.replace("v3d_dt", "v3d_unreviewed")).unwrap();
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
        mesa_version: Some("26.0.0".to_owned()),
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
        mesa_version: Some("26.0.0".to_owned()),
        ..TargetContext::default()
    };
    for (relative, mmake, c_count, cxx_count) in [
        (
            "workbench/libs/mesa/libcompiler",
            "mesa3d-linklib-compiler",
            272,
            51,
        ),
        ("workbench/libs/mesa/libmesa", "mesa3d-linklib-mesa", 215, 4),
        (
            "workbench/libs/mesa/libgalliumaux",
            "mesa3d-linklib-galliumauxiliary",
            150,
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
        "workbench/libs/mesa/libmesa/mesa-26.0.0.sources",
    ] {
        let output = temporary.path().join(relative);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::copy(original.join(relative), output).unwrap();
    }
    let manifest = temporary
        .path()
        .join("workbench/libs/mesa/libmesa/mesa-26.0.0.sources");
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
