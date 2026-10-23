use super::{MesaMakeCoordinates, MesaRecipeConfiguration, MesaRecipeFlagValue, MesaTargetLto};

fn valid_recipe() -> MesaRecipeConfiguration {
    MesaRecipeConfiguration {
        make_coordinates: MesaMakeCoordinates {
            source_root: "/source".into(),
            build_root: "/build".into(),
            ports_root: "/build/Ports".into(),
            generated_root: "/build/gen".into(),
        },
        safety_cflags: MesaRecipeFlagValue::Defined(vec![
            "-fno-omit-frame-pointer".into(),
            "-fno-omit-frame-pointer".into(),
        ]),
        target_lto: MesaTargetLto::Yes,
        lto_cflags: vec!["-flto".into(), "-O2".into(), "-flto".into()],
        cflags_iquote: vec!["-iquote".into()],
        cflags_iquote_end: Vec::new(),
        target_cc: "/opt/llvm/bin/clang".into(),
        target_cxx: "/opt/llvm/bin/clang++".into(),
        target_sysroot: vec!["--sysroot".into(), "/sdk/target root".into()],
    }
}

#[test]
fn coordinates_are_required_closed_and_outputs_are_inside_the_build() {
    let value = serde_json::to_value(valid_recipe()).unwrap();
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("make_coordinates");
    assert!(serde_json::from_value::<MesaRecipeConfiguration>(missing).is_err());
    for key in ["source_root", "build_root", "ports_root", "generated_root"] {
        let mut missing = value.clone();
        missing["make_coordinates"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(serde_json::from_value::<MesaRecipeConfiguration>(missing).is_err());
        for unsafe_path in [
            "relative",
            "/build/../other",
            "/build/$root",
            "/build/unsafe;path",
        ] {
            let mut invalid = value.clone();
            invalid["make_coordinates"][key] = unsafe_path.into();
            let invalid: MesaRecipeConfiguration = serde_json::from_value(invalid).unwrap();
            assert!(invalid.validate().is_err());
        }
    }
    for key in ["ports_root", "generated_root"] {
        for outside in ["/build", "/build-other/child"] {
            let mut invalid = value.clone();
            invalid["make_coordinates"][key] = outside.into();
            let invalid: MesaRecipeConfiguration = serde_json::from_value(invalid).unwrap();
            assert!(invalid.validate().is_err());
        }
    }
    let mut extra = value;
    extra["make_coordinates"]["guessed"] = "/other".into();
    assert!(serde_json::from_value::<MesaRecipeConfiguration>(extra).is_err());
    for generated in ["/build/Ports", "/build/Ports/nested"] {
        let mut invalid = valid_recipe();
        invalid.make_coordinates.generated_root = generated.into();
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn nominal_original_mapping_keeps_ports_generated_and_nested_build_distinct() {
    use std::path::Path;
    let coordinates = valid_recipe().make_coordinates;
    let build = Path::new("/source/build/arm");
    let ports = build.join("Ports");
    let generated = build.join("gen");
    for (original, expected) in [
        (
            "/source/build/arm/Ports/mesa/src/probe.c",
            "/build/Ports/mesa/src/probe.c",
        ),
        ("/source/build/arm/gen/probe.c", "/build/gen/probe.c"),
        ("/source/build/arm/module/probe.c", "/build/module/probe.c"),
        ("/source/module/probe.c", "/source/module/probe.c"),
    ] {
        assert_eq!(
            coordinates
                .nominal_original(Path::new(original), build, &ports, &generated)
                .unwrap(),
            Path::new(expected)
        );
    }
    for original in [
        "/source-other/probe.c",
        "/source/build/arm/../escape.c",
        "/source/build/arm",
        "/source/build/arm/gen",
    ] {
        assert!(coordinates
            .nominal_original(Path::new(original), build, &ports, &generated)
            .is_err());
    }
    for wrong in [
        ports.clone(),
        ports.join("nested"),
        build.join("../outside"),
    ] {
        assert!(coordinates
            .nominal_original(Path::new("/source/module/probe.c"), build, &ports, &wrong)
            .is_err());
    }
}

#[test]
fn valid_recipe_preserves_order_duplicates_and_projects_all_variables() {
    let recipe = valid_recipe();
    recipe.validate().unwrap();

    let variables = recipe.make_variables();
    assert_eq!(
        variables.get("SAFETY_CFLAGS").map(String::as_str),
        Some("-fno-omit-frame-pointer -fno-omit-frame-pointer")
    );
    assert_eq!(variables.get("TARGET_LTO").map(String::as_str), Some("yes"));
    assert_eq!(
        variables.get("LTO_CFLAGS").map(String::as_str),
        Some("-flto -O2 -flto")
    );
    assert_eq!(
        variables.get("CFLAGS_IQUOTE").map(String::as_str),
        Some("-iquote")
    );
    assert_eq!(
        variables.get("CFLAGS_IQUOTE_END").map(String::as_str),
        Some("")
    );
    assert_eq!(
        variables.get("TARGET_CC").map(String::as_str),
        Some("/opt/llvm/bin/clang")
    );
    assert_eq!(
        variables.get("AROS_CXX").map(String::as_str),
        Some("/opt/llvm/bin/clang++")
    );
    assert_eq!(
        variables.get("TARGET_SYSROOT").map(String::as_str),
        Some("--sysroot /sdk/target root")
    );
    assert!(recipe.undefined_variables().is_empty());
}

#[test]
fn undefined_safety_flags_differ_from_defined_empty_flags() {
    let mut undefined = valid_recipe();
    undefined.safety_cflags = MesaRecipeFlagValue::Undefined;
    undefined.validate().unwrap();
    let undefined_variables = undefined.make_variables();
    assert!(!undefined_variables.contains_key("SAFETY_CFLAGS"));
    assert_eq!(undefined.undefined_variables(), vec!["SAFETY_CFLAGS"]);

    let mut defined_empty = valid_recipe();
    defined_empty.safety_cflags = MesaRecipeFlagValue::Defined(Vec::new());
    defined_empty.validate().unwrap();
    let defined_variables = defined_empty.make_variables();
    assert_eq!(
        defined_variables.get("SAFETY_CFLAGS").map(String::as_str),
        Some("")
    );
    assert!(defined_empty.undefined_variables().is_empty());

    assert_eq!(
        serde_json::to_value(&defined_empty).unwrap()["safety_cflags"],
        serde_json::json!({"state": "defined", "value": []})
    );
}

#[test]
fn wire_requires_every_field_and_rejects_duplicate_or_unknown_fields() {
    let value = serde_json::to_value(valid_recipe()).unwrap();

    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("target_cc");
    assert!(serde_json::from_value::<MesaRecipeConfiguration>(missing).is_err());

    let json = serde_json::to_string(&valid_recipe()).unwrap();
    let duplicate = json.replace(
        "\"target_cc\":\"/opt/llvm/bin/clang\"",
        "\"target_cc\":\"/opt/llvm/bin/clang\",\"target_cc\":\"/opt/llvm/bin/clang\"",
    );
    assert_ne!(duplicate, json);
    assert!(serde_json::from_str::<MesaRecipeConfiguration>(&duplicate).is_err());

    let mut unknown = value;
    unknown
        .as_object_mut()
        .unwrap()
        .insert("future_field".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<MesaRecipeConfiguration>(unknown).is_err());
}

#[test]
fn quote_flags_accept_only_the_reviewed_layout() {
    let mut recipe = valid_recipe();
    recipe.cflags_iquote.clear();
    assert!(recipe.validate().is_err());

    let mut recipe = valid_recipe();
    recipe.cflags_iquote = vec!["-iquote".into(), "-I".into()];
    assert!(recipe.validate().is_err());

    let mut recipe = valid_recipe();
    recipe.cflags_iquote_end = vec!["-iquote".into()];
    assert!(recipe.validate().is_err());
}

#[test]
fn flag_atoms_reject_unsafe_syntax_and_enforce_bounds() {
    for invalid in [
        "",
        "-DVALUE=one two",
        "-DVALUE=$HOME",
        "-x;id",
        "`id`",
        "-x\\y",
        "-x\n-y",
    ] {
        let mut recipe = valid_recipe();
        recipe.lto_cflags = vec![invalid.into()];
        assert!(
            recipe.validate().is_err(),
            "accepted unsafe atom {invalid:?}"
        );
    }

    let mut too_many = valid_recipe();
    too_many.lto_cflags = vec!["-O2".into(); 65];
    assert!(too_many.validate().is_err());

    let mut too_long = valid_recipe();
    too_long.lto_cflags = vec![format!("-{}", "x".repeat(256))];
    assert!(too_long.validate().is_err());
}

#[test]
fn drivers_require_normalized_absolute_paths_and_reviewed_basenames() {
    for invalid in [
        "opt/llvm/bin/clang",
        "/opt/llvm/bin/../clang",
        "/opt//llvm/bin/clang",
        "/opt/llvm/bin/clang/",
        "/opt/llvm/bin/gcc",
        "/opt/$SDK/bin/clang",
        "/opt/llvm;touch/bin/clang",
        "/opt/llvm`touch`/bin/clang",
        "/opt/llvm\n/bin/clang",
    ] {
        let mut recipe = valid_recipe();
        recipe.target_cc = invalid.into();
        assert!(
            recipe.validate().is_err(),
            "accepted driver path {invalid:?}"
        );
    }

    let mut path_with_spaces = valid_recipe();
    path_with_spaces.target_cc = "/opt/llvm tools/bin/clang".into();
    assert!(path_with_spaces.validate().is_ok());
}

#[test]
fn sysroot_is_empty_or_an_exact_option_and_normalized_absolute_path_pair() {
    let mut no_sysroot = valid_recipe();
    no_sysroot.target_sysroot.clear();
    assert!(no_sysroot.validate().is_ok());

    for invalid in [
        vec!["--sysroot="],
        vec!["/sdk"],
        vec!["--sysroot", "relative/sdk"],
        vec!["--sysroot", "/sdk/../other"],
        vec!["--sysroot", "/sdk//other"],
        vec!["--sysroot", "/sdk/$TARGET"],
        vec!["--sysroot", "/sdk/;touch"],
        vec!["--sysroot", "/sdk/`touch`"],
        vec!["--sysroot", "/sdk", "extra"],
    ] {
        let mut recipe = valid_recipe();
        recipe.target_sysroot = invalid.iter().map(|value| (*value).to_owned()).collect();
        assert!(recipe.validate().is_err(), "accepted sysroot {invalid:?}");
    }
}

#[test]
fn lto_wire_values_are_lowercase_yes_or_no() {
    assert_eq!(
        serde_json::to_string(&MesaTargetLto::Yes).unwrap(),
        "\"yes\""
    );
    assert_eq!(serde_json::to_string(&MesaTargetLto::No).unwrap(), "\"no\"");
    assert!(serde_json::from_str::<MesaTargetLto>("\"YES\"").is_err());
}

#[test]
fn one_atom_sysroot_preserves_the_actual_equals_spelling() {
    let mut recipe = valid_recipe();
    recipe.target_sysroot = vec!["--sysroot=/sdk/target root".into()];
    recipe.validate().unwrap();
    assert_eq!(
        recipe.make_variables()["TARGET_SYSROOT"],
        "--sysroot=/sdk/target root"
    );
}
