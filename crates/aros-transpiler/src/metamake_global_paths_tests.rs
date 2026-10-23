use super::*;

#[test]
fn resolves_paths_from_merged_initial_values_and_sequential_assignments() {
    // The caller supplies already merged values, including project precedence.
    let initial = BTreeMap::from([
        ("TOP".into(), "/source".into()),
        ("ROOT".into(), "/project".into()),
    ]);
    let mut context = ProjectGlobalPathContext::new(&initial).unwrap();

    assert_eq!(
        context.resolve("$(ROOT)/host.vars").unwrap(),
        "/project/host.vars"
    );
    context
        .advance(
            "host.vars",
            b"ROOT := $(TOP)/host\nHOST_FILE := $(ROOT)/target.vars\n",
        )
        .unwrap();
    assert_eq!(
        context.resolve("$(HOST_FILE)").unwrap(),
        "/source/host/target.vars"
    );
}

#[test]
fn unrelated_unsupported_make_value_does_not_block_path_projection() {
    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    context
        .advance(
            "globals",
            b"IGNORED := $(shell uname)\nROOT := /configured\n",
        )
        .unwrap();

    assert_eq!(
        context.resolve("$(ROOT)/child.vars").unwrap(),
        "/configured/child.vars"
    );
}

#[test]
fn unavailable_references_fail_and_later_assignments_restore_bindings() {
    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    context
        .advance(
            "globals",
            b"A := $(strip value)\nBEFORE := $(LATE)\nLATE := after\n",
        )
        .unwrap();

    // A was evaluated before LATE existed, so later definitions do not repair it.
    assert!(context.resolve("$(A)").is_err());
    assert!(context.resolve("$(BEFORE)").is_err());
    assert_eq!(context.resolve("$(LATE)").unwrap(), "after");

    context
        .advance("later", b"A := $(LATE)/restored\n")
        .unwrap();
    assert_eq!(context.resolve("$(A)").unwrap(), "after/restored");

    context.advance("broken", b"B := $(UNKNOWN)\n").unwrap();
    assert!(context.resolve("$(B)/path").is_err());
}

#[test]
fn byte_comments_are_skipped_before_utf8_validation_but_active_lines_are_not() {
    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    context
        .advance("comments", b"#\xff\xfe\nGOOD := value\n")
        .unwrap();
    assert_eq!(context.resolve("$(GOOD)").unwrap(), "value");

    assert!(context.advance("active", b"BAD := \xff\n").is_err());
}

#[test]
fn physical_line_and_substitution_limits_are_preserved() {
    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    assert!(context.advance("unterminated", b"X := value").is_err());
    assert!(context.resolve("literal-path").is_err());

    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    let overlong = format!("X := {}\n", "a".repeat(250));
    assert!(context.advance("overlong", overlong.as_bytes()).is_err());

    let initial = BTreeMap::from([("X".into(), "x".repeat(MAX_VALUE_BYTES))]);
    let context = ProjectGlobalPathContext::new(&initial).unwrap();
    assert!(context.resolve("prefix$(X)").is_err());
    assert_eq!(
        context.resolve(&"x".repeat(MAX_VALUE_BYTES)).unwrap().len(),
        MAX_VALUE_BYTES
    );
    assert!(context.resolve(&"x".repeat(MAX_VALUE_BYTES + 1)).is_err());
}

#[test]
fn file_count_limit_is_enforced() {
    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    for _ in 0..256 {
        context.advance("empty", b"").unwrap();
    }
    assert!(context.advance("too-many", b"").is_err());
}

#[test]
fn oversized_unrelated_seed_is_unavailable_not_a_failure_or_empty_value() {
    let initial = BTreeMap::from([
        ("MAKEFILE_LIST".into(), "x".repeat(MAX_VALUE_BYTES + 1)),
        ("TOP".into(), "/selected".into()),
    ]);
    let context = ProjectGlobalPathContext::new(&initial).unwrap();
    assert_eq!(
        context.resolve("$(TOP)/global.cfg").unwrap(),
        "/selected/global.cfg"
    );
    assert!(context.resolve("$(MAKEFILE_LIST)").is_err());
}

#[test]
fn selected_values_match_exactly_and_accept_known_empty_values() {
    let initial = BTreeMap::from([
        ("EMPTY".into(), String::new()),
        ("VALUE".into(), "selected".into()),
    ]);
    let context = ProjectGlobalPathContext::new(&initial).unwrap();
    let expected = BTreeMap::from([
        ("EMPTY".into(), String::new()),
        ("VALUE".into(), "selected".into()),
    ]);
    let absent = BTreeSet::from(["NOT_CONFIGURED".into()]);

    assert!(context.verify_selected_values(&expected, &absent).is_ok());
}

#[test]
fn selected_value_verification_rejects_mismatches_missing_and_unavailable_values() {
    let initial = BTreeMap::from([
        ("KNOWN".into(), "actual-value".into()),
        ("UNAVAILABLE".into(), "x".repeat(MAX_VALUE_BYTES + 1)),
    ]);
    let context = ProjectGlobalPathContext::new(&initial).unwrap();

    let mismatch = BTreeMap::from([("KNOWN".into(), "private-expected-value".into())]);
    let error = context
        .verify_selected_values(&mismatch, &BTreeSet::new())
        .unwrap_err();
    assert_eq!(error, "selected MetaMake value does not match");
    assert!(!error.contains("KNOWN") && !error.contains("private-expected-value"));

    let missing = BTreeMap::from([("MISSING".into(), String::new())]);
    assert_eq!(
        context
            .verify_selected_values(&missing, &BTreeSet::new())
            .unwrap_err(),
        "selected MetaMake value is missing"
    );

    let unavailable = BTreeMap::from([("UNAVAILABLE".into(), String::new())]);
    assert_eq!(
        context
            .verify_selected_values(&unavailable, &BTreeSet::new())
            .unwrap_err(),
        "selected MetaMake value is unavailable"
    );
}

#[test]
fn selected_absence_requires_a_name_missing_from_both_context_sets() {
    let initial = BTreeMap::from([
        ("KNOWN_EMPTY".into(), String::new()),
        ("UNAVAILABLE".into(), "x".repeat(MAX_VALUE_BYTES + 1)),
    ]);
    let context = ProjectGlobalPathContext::new(&initial).unwrap();

    for name in ["KNOWN_EMPTY", "UNAVAILABLE"] {
        let absent = BTreeSet::from([name.into()]);
        assert_eq!(
            context
                .verify_selected_values(&BTreeMap::new(), &absent)
                .unwrap_err(),
            "selected MetaMake absence is not established"
        );
    }
    let truly_missing = BTreeSet::from(["MISSING".into()]);
    assert!(context
        .verify_selected_values(&BTreeMap::new(), &truly_missing)
        .is_ok());
}

#[test]
fn selected_value_verification_rejects_conflicts_and_invalid_inputs() {
    let context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();

    let expected = BTreeMap::from([("VALUE".into(), String::new())]);
    let conflicting_absent = BTreeSet::from(["VALUE".into()]);
    assert_eq!(
        context
            .verify_selected_values(&expected, &conflicting_absent)
            .unwrap_err(),
        "conflicting selected MetaMake presence rules"
    );

    for invalid in ["", "has space", "bad/name"] {
        let invalid_name = BTreeMap::from([(invalid.into(), String::new())]);
        assert_eq!(
            context
                .verify_selected_values(&invalid_name, &BTreeSet::new())
                .unwrap_err(),
            "invalid selected MetaMake value"
        );
        let invalid_absent = BTreeSet::from([invalid.into()]);
        assert_eq!(
            context
                .verify_selected_values(&BTreeMap::new(), &invalid_absent)
                .unwrap_err(),
            "invalid selected MetaMake absence"
        );
    }

    for invalid_value in ["line\nbreak", "carriage\rreturn", "nul\0byte"] {
        let invalid = BTreeMap::from([("VALUE".into(), invalid_value.into())]);
        assert_eq!(
            context
                .verify_selected_values(&invalid, &BTreeSet::new())
                .unwrap_err(),
            "invalid selected MetaMake value"
        );
    }
    let oversized = BTreeMap::from([("VALUE".into(), "x".repeat(MAX_VALUE_BYTES + 1))]);
    assert_eq!(
        context
            .verify_selected_values(&oversized, &BTreeSet::new())
            .unwrap_err(),
        "invalid selected MetaMake value"
    );
}

#[test]
fn selected_value_verification_enforces_entry_and_aggregate_limits() {
    let context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    let too_many = (0..=MAX_BINDINGS)
        .map(|index| (format!("V{index}"), String::new()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        context
            .verify_selected_values(&too_many, &BTreeSet::new())
            .unwrap_err(),
        "selected MetaMake values exceed entry limit"
    );

    let oversized_aggregate = (0..4100)
        .map(|index| (format!("V{index}"), "x".repeat(MAX_VALUE_BYTES)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        context
            .verify_selected_values(&oversized_aggregate, &BTreeSet::new())
            .unwrap_err(),
        "selected MetaMake values exceed resource limit"
    );
}

#[test]
fn selected_values_follow_assignment_order_and_later_restoration() {
    let initial = BTreeMap::from([("TOP".into(), "/source".into())]);
    let mut context = ProjectGlobalPathContext::new(&initial).unwrap();
    context
        .advance("first", b"EARLY := $(LATE)\nROOT := $(TOP)/first\n")
        .unwrap();

    let early_empty = BTreeMap::from([("EARLY".into(), String::new())]);
    assert_eq!(
        context
            .verify_selected_values(&early_empty, &BTreeSet::new())
            .unwrap_err(),
        "selected MetaMake value is unavailable"
    );

    context
        .advance("second", b"LATE := second\nCOPY := $(LATE)\n")
        .unwrap();
    let ordered = BTreeMap::from([
        ("LATE".into(), "second".into()),
        ("COPY".into(), "second".into()),
        ("ROOT".into(), "/source/first".into()),
    ]);
    assert!(context
        .verify_selected_values(&ordered, &BTreeSet::new())
        .is_ok());
    assert_eq!(
        context
            .verify_selected_values(&early_empty, &BTreeSet::new())
            .unwrap_err(),
        "selected MetaMake value is unavailable"
    );

    context
        .advance("third", b"EARLY := $(LATE)/restored\n")
        .unwrap();
    let restored = BTreeMap::from([
        ("EARLY".into(), "second/restored".into()),
        ("LATE".into(), "second".into()),
        ("COPY".into(), "second".into()),
        ("ROOT".into(), "/source/first".into()),
    ]);
    assert!(context
        .verify_selected_values(&restored, &BTreeSet::new())
        .is_ok());
}

#[test]
fn selected_value_verification_rejects_a_poisoned_context() {
    let mut context = ProjectGlobalPathContext::new(&BTreeMap::new()).unwrap();
    assert!(context
        .advance("private/provenance", b"BROKEN := value")
        .is_err());

    let error = context
        .verify_selected_values(&BTreeMap::new(), &BTreeSet::new())
        .unwrap_err();
    assert_eq!(error, "MetaMake path context is failed");
    assert!(!error.contains("private/provenance"));
}
