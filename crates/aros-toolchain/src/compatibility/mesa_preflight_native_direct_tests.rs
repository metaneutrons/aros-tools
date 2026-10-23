use super::*;
use std::{collections::BTreeMap, path::PathBuf};

fn captured() -> NativeMakeInvocation {
    NativeMakeInvocation::new(
        [
            "--srcdir=/source".into(),
            "--builddir=/build".into(),
            "-q".into(),
            "Fixture.mesa-quick".into(),
        ],
        PathBuf::from("/build"),
        BTreeMap::from([
            ("OPAQUE-DATA".into(), "retain$literal".into()),
            ("EMPTY".into(), String::new()),
            ("_".into(), "native-sentinel".into()),
            ("MAKEFLAGS".into(), "rs --no-print-directory".into()),
        ]),
    )
    .unwrap()
    .with_argument_zero("/build/mmake".into())
    .unwrap()
}

#[test]
fn direct_command_preserves_every_environment_pair_options_and_requested_goal_order() {
    let original = captured();
    let roots = ["mesa-quick".into(), "libinit-libentry".into()];
    let direct = direct_invocation(
        &original,
        Path::new("/build/mmake"),
        Path::new("/build"),
        original.arguments(),
        "Fixture",
        &roots,
    )
    .unwrap();
    assert_eq!(direct.environment(), original.environment());
    assert_eq!(direct.argument_zero(), original.argument_zero());
    assert_eq!(direct.working_directory(), original.working_directory());
    assert_eq!(
        direct.arguments(),
        [
            "--srcdir=/source",
            "--builddir=/build",
            "-q",
            "Fixture.mesa-quick",
            "Fixture.libinit-libentry"
        ]
    );
    // Selecting an ordinary MetaMake goal does not launch the ordinary Root
    // target or invent a -quick alias for it. This test executes no child.
    assert_eq!(original.arguments().last().unwrap(), "Fixture.mesa-quick");
}

#[test]
fn direct_command_rejects_changed_original_coordinates_and_unsafe_or_duplicate_roots() {
    let original = captured();
    let goals = vec!["libinit-libentry".into()];
    for (program, build, project, arguments, roots) in [
        (
            "/foreign/mmake",
            "/build",
            "Fixture",
            original.arguments().to_vec(),
            goals.clone(),
        ),
        (
            "/build/mmake",
            "/foreign",
            "Fixture",
            original.arguments().to_vec(),
            goals.clone(),
        ),
        (
            "/build/mmake",
            "/build",
            "Other",
            vec!["Other.mesa-quick".into()],
            goals.clone(),
        ),
        ("/build/mmake", "/build", "Fixture", vec![], goals.clone()),
        (
            "/build/mmake",
            "/build",
            "Fixture",
            original.arguments().to_vec(),
            vec![],
        ),
        (
            "/build/mmake",
            "/build",
            "Fixture",
            original.arguments().to_vec(),
            vec!["same".into(), "same".into()],
        ),
        (
            "/build/mmake",
            "/build",
            "Other",
            original.arguments().to_vec(),
            goals.clone(),
        ),
        (
            "/build/mmake",
            "/build",
            "Other.Project",
            original.arguments().to_vec(),
            goals,
        ),
    ] {
        assert!(direct_invocation(
            &original,
            Path::new(program),
            Path::new(build),
            &arguments,
            project,
            &roots
        )
        .is_err());
    }
    for invalid in ["-q", "x y", "x;y", "x\ny", "$(execute)", "", "x/y"] {
        assert!(direct_invocation(
            &original,
            Path::new("/build/mmake"),
            Path::new("/build"),
            original.arguments(),
            "Fixture",
            &[invalid.into()]
        )
        .is_err());
    }
    let over = (0..257)
        .map(|index| format!("goal{index}"))
        .collect::<Vec<_>>();
    assert!(direct_invocation(
        &original,
        Path::new("/build/mmake"),
        Path::new("/build"),
        original.arguments(),
        "Fixture",
        &over
    )
    .is_err());
}

#[test]
fn direct_prefix_and_minimum_call_capacity_require_the_exact_selected_data() {
    let invocation = captured();
    check_prefix("env EMPTY= _=native-sentinel", &invocation).unwrap();
    for prefix in [
        "env EMPTY=changed",
        "env MISSING=value",
        "env _=other",
        "EMPTY=",
    ] {
        assert!(check_prefix(prefix, &invocation).is_err());
    }
    check_capacity(1, [1]).unwrap();
    check_capacity(4096, [4096]).unwrap();
    // The real selection has216 owner directories but551 owner-goal pairs.
    // Counting only directories falsely admitted a256-slot collection.
    let selected = (0..216).map(|owner| if owner < 119 { 3 } else { 2 });
    assert!(check_capacity(256, selected.clone()).is_err());
    assert!(check_capacity(550, selected.clone()).is_err());
    check_capacity(551, selected).unwrap();
    assert!(check_capacity(1, []).is_err());
    for (capacity, counts) in [
        (0, vec![1]),
        (4097, vec![1]),
        (4096, vec![4097]),
        (1, vec![0]),
        (4096, vec![usize::MAX, 1]),
        (4096, vec![1, 0]),
    ] {
        assert!(check_capacity(capacity, counts).is_err());
    }
}
