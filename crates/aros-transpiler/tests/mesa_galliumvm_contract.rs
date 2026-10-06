mod common;

use aros_common::read_source;
use std::{
    collections::{BTreeMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Component, Path, PathBuf},
};

const INVENTORY_PATH: &str = "workbench/libs/mesa/libgalliumvm/gallivm-26.0.0.sources";
const RECIPE_PATH: &str = "workbench/libs/mesa/libgalliumvm/mmakefile.src";

const EXPECTED_C_SOURCES: &str = "\
lp_bld_arit.c
lp_bld_arit_overflow.c
lp_bld_assert.c
lp_bld_bitarit.c
lp_bld_const.c
lp_bld_conv.c
lp_bld_coro.c
lp_bld_flow.c
lp_bld_format_aos_array.c
lp_bld_format_aos.c
lp_bld_format_float.c
lp_bld_format_s3tc.c
lp_bld_format.c
lp_bld_format_soa.c
lp_bld_format_srgb.c
lp_bld_format_yuv.c
lp_bld_gather.c
lp_bld_init_common.c
lp_bld_intr.c
lp_bld_ir_common.c
lp_bld_jit_sample.c
lp_bld_jit_types.c
lp_bld_logic.c
lp_bld_nir.c
lp_bld_nir_aos.c
lp_bld_nir_soa.c
lp_bld_pack.c
lp_bld_passmgr.c
lp_bld_printf.c
lp_bld_quad.c
lp_bld_sample_aos.c
lp_bld_sample.c
lp_bld_sample_soa.c
lp_bld_struct.c
lp_bld_swizzle.c
lp_bld_tgsi_action.c
lp_bld_tgsi.c
lp_bld_tgsi_info.c
lp_bld_tgsi_soa.c
lp_bld_type.c
lp_bld_init.c";

const EXPECTED_CXX_SOURCES: &str = "\
lp_bld_debug.cpp
lp_bld_misc.cpp";

#[derive(Debug, PartialEq, Eq)]
struct GallivmInventory {
    c: Vec<String>,
    cxx: Vec<String>,
}

fn expected_inventory() -> GallivmInventory {
    GallivmInventory {
        c: EXPECTED_C_SOURCES
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        cxx: EXPECTED_CXX_SOURCES
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    }
}

fn validate_source_name(name: &str, extension: &str) -> Result<(), String> {
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || name.contains('/')
        || name.contains('\\')
        || path.extension().and_then(|value| value.to_str()) != Some(extension)
    {
        return Err(format!("unsafe Gallivm source name: {name}"));
    }
    Ok(())
}

fn parse_inventory(text: &str) -> Result<GallivmInventory, String> {
    let mut sections = BTreeMap::<String, Vec<String>>::new();
    let mut current: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(active) = current.clone() {
            let continued = trimmed.ends_with('\\');
            let payload = trimmed.strip_suffix('\\').unwrap_or(trimmed).trim();
            for name in payload.split_whitespace() {
                sections.get_mut(&active).unwrap().push(name.to_owned());
            }
            if !continued {
                current = None;
            }
            continue;
        }

        let Some((name, value)) = trimmed.split_once(":=") else {
            continue;
        };
        let name = name.trim();
        if !name.starts_with("MESA26_GALLIVM_") {
            continue;
        }
        if !matches!(
            name,
            "MESA26_GALLIVM_C_SOURCES" | "MESA26_GALLIVM_CXX_SOURCES"
        ) {
            return Err(format!("unexpected Gallivm inventory section {name}"));
        }
        if sections.insert(name.to_owned(), Vec::new()).is_some() {
            return Err(format!("duplicate Gallivm inventory section {name}"));
        }
        current = Some(name.to_owned());
        let trimmed_value = value.trim();
        let continued = trimmed_value.ends_with('\\');
        let payload = trimmed_value
            .strip_suffix('\\')
            .unwrap_or(trimmed_value)
            .trim();
        for source in payload.split_whitespace() {
            sections.get_mut(name).unwrap().push(source.to_owned());
        }
        if !continued {
            current = None;
        }
    }
    if current.is_some() {
        return Err("unterminated Gallivm inventory section".to_owned());
    }

    let c = sections
        .remove("MESA26_GALLIVM_C_SOURCES")
        .ok_or_else(|| "missing MESA26_GALLIVM_C_SOURCES".to_owned())?;
    let cxx = sections
        .remove("MESA26_GALLIVM_CXX_SOURCES")
        .ok_or_else(|| "missing MESA26_GALLIVM_CXX_SOURCES".to_owned())?;

    let mut seen = HashSet::new();
    for source in &c {
        validate_source_name(source, "c")?;
        if !seen.insert(source) {
            return Err(format!("duplicate Gallivm source {source}"));
        }
    }
    for source in &cxx {
        validate_source_name(source, "cpp")?;
        if !seen.insert(source) {
            return Err(format!("duplicate Gallivm source {source}"));
        }
    }

    Ok(GallivmInventory { c, cxx })
}

fn validate_inventory_files(text: &str, source_dir: &Path) -> Result<GallivmInventory, String> {
    let inventory = parse_inventory(text)?;
    if inventory != expected_inventory() {
        return Err("Gallivm list differs from the versioned Mesa 26.0.0 inventory".to_owned());
    }

    let canonical_root = source_dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve Gallivm source root: {error}"))?;
    for source in inventory.c.iter().chain(&inventory.cxx) {
        let path = source_dir.join(source);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect Gallivm source {source}: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err(format!("Gallivm source is not a regular file: {source}"));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve Gallivm source {source}: {error}"))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(format!("Gallivm source escapes its source root: {source}"));
        }
    }
    Ok(inventory)
}

fn render_inventory(c: &[String], cxx: &[String]) -> String {
    fn render_section(name: &str, sources: &[String]) -> String {
        let mut result = format!("{name} := \\\n");
        for (index, source) in sources.iter().enumerate() {
            let suffix = if index + 1 == sources.len() {
                ""
            } else {
                " \\"
            };
            writeln!(result, "    {source}{suffix}").unwrap();
        }
        result
    }

    format!(
        "# Mesa 26.0.0 Gallivm MCJIT inventory\n{}\n{}",
        render_section("MESA26_GALLIVM_C_SOURCES", c),
        render_section("MESA26_GALLIVM_CXX_SOURCES", cxx)
    )
}

fn create_inventory_fixture(root: &Path, text: &str) -> PathBuf {
    let sources = root.join("src/gallium/auxiliary/gallivm");
    fs::create_dir_all(&sources).unwrap();
    let expected = expected_inventory();
    for source in expected.c.iter().chain(&expected.cxx) {
        fs::write(sources.join(source), "/* fixture */\n").unwrap();
    }
    let manifest = root.join(INVENTORY_PATH);
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(&manifest, text).unwrap();
    manifest
}

fn active_meta_dependencies(source: &str, target: &str) -> Vec<String> {
    let matching: Vec<_> = source
        .lines()
        .filter_map(|line| {
            let body = line.trim_start().strip_prefix("#MM")?;
            if body.starts_with('#') {
                return None;
            }
            let (name, dependencies) = body.trim().split_once(':')?;
            (name.trim() == target).then(|| {
                dependencies
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let [dependencies] = matching.as_slice() else {
        panic!("expected exactly one active #MM rule for {target}, found {matching:#?}");
    };
    dependencies.clone()
}

fn continued_assignment(source: &str, assignment: &str) -> String {
    let mut result = String::new();
    let mut reading = false;
    for line in source.lines() {
        if !reading {
            let trimmed = line.trim_start();
            let Some((name, value)) = trimmed.split_once("+=") else {
                continue;
            };
            if name.trim() != assignment {
                continue;
            }
            reading = true;
            result.push_str(value.trim().trim_end_matches('\\'));
            result.push(' ');
            if !value.trim_end().ends_with('\\') {
                break;
            }
            continue;
        }
        let trimmed = line.trim();
        result.push_str(trimmed.trim_end_matches('\\'));
        result.push(' ');
        if !trimmed.ends_with('\\') {
            break;
        }
    }
    assert!(reading, "missing {assignment} assignment");
    result
}

fn logical_make_rules(source: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let mut current = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(trimmed.trim_end_matches('\\'));
        if !trimmed.ends_with('\\') {
            rules.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        rules.push(current);
    }
    rules
}

#[test]
fn mesa26_gallivm_has_a_closed_versioned_mcjit_archive_contract() {
    let root = common::source_root();
    let inventory_path = root.join(INVENTORY_PATH);
    let inventory_text = read_source(&inventory_path).expect("versioned Gallivm inventory");
    assert_eq!(
        inventory_path.file_name().and_then(|name| name.to_str()),
        Some("gallivm-26.0.0.sources")
    );
    assert!(inventory_text.contains("Mesa 26.0.0 Gallivm MCJIT inventory"));
    let inventory = parse_inventory(&inventory_text).expect("valid Gallivm source inventory");
    assert_eq!(inventory, expected_inventory());
    assert_eq!(inventory.c.len(), 41);
    assert_eq!(inventory.cxx.len(), 2);
    assert!(!inventory
        .cxx
        .iter()
        .any(|source| source == "lp_bld_init_orc.cpp"));

    let recipe = read_source(&root.join(RECIPE_PATH)).expect("Gallivm build recipe");
    assert!(recipe.contains("include $(SRCDIR)/$(CURDIR)/gallivm-26.0.0.sources"));
    assert!(recipe.contains("ifneq ($(OPT_MESAGL),26.0.0)"));
    assert!(recipe.contains("Gallivm requires the reviewed Mesa 26.0.0 source inventory"));

    let gallivm_dependencies = active_meta_dependencies(&recipe, "mesa3d-linklib-galliumvm");
    assert_eq!(
        gallivm_dependencies,
        [
            "workbench-libs-llvm",
            "mesa3d-linklib-compiler-generated",
            "mesa3d-linklib-mesa-generated",
            "mesa3d-linklib-mesautil-generated",
        ]
    );
    let archive_rules: Vec<_> = logical_make_rules(&recipe)
        .into_iter()
        .filter(|line| line.starts_with("%build_linklib mmake=mesa3d-linklib-galliumvm "))
        .collect();
    assert_eq!(archive_rules.len(), 1);
    assert!(archive_rules[0].contains("libname=galliumvm"));
    assert!(archive_rules[0].contains("files=\"$(GALLIVM_SOURCES_C)\""));
    assert!(archive_rules[0].contains("cxxfiles=\"$(GALLIVM_SOURCES_CXX)\""));

    let flags = continued_assignment(&recipe, "USER_CPPFLAGS");
    for flag in [
        "-DDRAW_LLVM_AVAILABLE=1",
        "-DLLVM_AVAILABLE",
        "-DHAVE_LLVM=$(LLVM_VERSION_HEX)",
        "-DMESA_LLVM_VERSION_STRING=\\\"$(OPT_LLVM)\\\"",
        "-DGALLIVM_USE_ORCJIT=0",
        "-DLLVM_IS_SHARED=0",
        "-D__AROS__",
    ] {
        assert!(
            flags.split_whitespace().any(|candidate| candidate == flag),
            "missing MCJIT/target LLVM flag {flag}: {flags}"
        );
    }
    assert!(!flags.contains("-DGALLIVM_USE_ORCJIT=1"));

    let llvmpipe = read_source(&root.join("workbench/libs/mesa/libllvmpipe/mmakefile.src"))
        .expect("llvmpipe recipe");
    let llvmpipe_dependencies = active_meta_dependencies(&llvmpipe, "mesa3d-linklib-llvmpipe");
    assert!(llvmpipe_dependencies.contains(&"workbench-libs-llvm".to_owned()));
    assert!(llvmpipe_dependencies.contains(&"mesa3d-linklib-galliumvm".to_owned()));
    assert!(llvmpipe.lines().any(|line| {
        line.trim_start()
            .starts_with("%build_linklib mmake=mesa3d-linklib-llvmpipe libname=llvmpipe")
    }));

    let compiler = read_source(&root.join("workbench/libs/mesa/libcompiler/mmakefile.src"))
        .expect("Mesa compiler generator recipe");
    assert!(compiler.contains("include $(SRCDIR)/$(CURDIR)/compiler-26.0.0.sources"));
    assert!(compiler.contains(
        "MESA3DGL_NIR_GENERATED_FILES := $(addprefix $(top_builddir)/$(CUR_MESADIR)/,$(NIR_GENERATED_FILES))"
    ));
    assert!(logical_make_rules(&compiler).iter().any(|rule| {
        rule.starts_with("mesa3d-linklib-compiler-generated :")
            && rule.contains("$(MESA3DGL_NIR_GENERATED_FILES)")
    }));
}

#[test]
fn gallivm_inventory_validation_rejects_stale_duplicate_escaped_and_nonregular_inputs() {
    let expected = expected_inventory();
    let canonical_text = render_inventory(&expected.c, &expected.cxx);

    let temp = tempfile::tempdir().unwrap();
    let manifest = create_inventory_fixture(temp.path(), &canonical_text);
    let source_dir = temp.path().join("src/gallium/auxiliary/gallivm");
    let parsed = validate_inventory_files(&fs::read_to_string(&manifest).unwrap(), &source_dir)
        .expect("canonical fixture inventory");
    assert_eq!(parsed, expected);

    let mut stale = expected.c.clone();
    stale[0] = "lp_bld_stale.c".to_owned();
    let stale_text = render_inventory(&stale, &expected.cxx);
    assert!(validate_inventory_files(&stale_text, &source_dir)
        .unwrap_err()
        .contains("versioned Mesa 26.0.0 inventory"));

    let mut duplicate = expected.c.clone();
    duplicate.push(duplicate[0].clone());
    let duplicate_text = render_inventory(&duplicate, &expected.cxx);
    assert!(validate_inventory_files(&duplicate_text, &source_dir)
        .unwrap_err()
        .contains("duplicate Gallivm source"));

    let mut escaped = expected.c.clone();
    escaped[0] = "../escape.c".to_owned();
    let escaped_text = render_inventory(&escaped, &expected.cxx);
    assert!(validate_inventory_files(&escaped_text, &source_dir)
        .unwrap_err()
        .contains("unsafe Gallivm source name"));

    let nonregular = source_dir.join(&expected.c[0]);
    fs::remove_file(&nonregular).unwrap();
    fs::create_dir(&nonregular).unwrap();
    assert!(validate_inventory_files(&canonical_text, &source_dir)
        .unwrap_err()
        .contains("not a regular file"));
}
