use super::{
    NativeMakeInvocation, MAX_ARGUMENT_BYTES, MAX_CWD_BYTES, MAX_DOCUMENT_BYTES, MAX_ENTRIES,
    MAX_ENVIRONMENT_BYTES,
};
use std::path::PathBuf;

fn invocation(
    arguments: impl IntoIterator<Item = String>,
    working_directory: PathBuf,
    environment: impl IntoIterator<Item = (String, String)>,
) -> NativeMakeInvocation {
    NativeMakeInvocation::new(arguments, working_directory, environment)
        .expect("valid native invocation")
}

fn cwd() -> PathBuf {
    PathBuf::from("/tmp/native-invocation-cwd")
}

#[test]
fn round_trip_preserves_repeated_arguments_sensitive_values_and_debug_redacts_them() {
    let arguments = vec![
        "--token=ARG-SECRET-1".to_owned(),
        "--token=ARG-SECRET-1".to_owned(),
        "$HOME/${UNEXPANDED}\nnext\t\u{0001} café 🚀".to_owned(),
    ];
    let environment = vec![
        ("ENV-KEY-SECRET".to_owned(), "ENV-VALUE-SECRET".to_owned()),
        (
            "  vendor:odd name/🛰".to_owned(),
            "literal=$TOKEN\n\t\u{0002}秘密".to_owned(),
        ),
    ];
    let working_directory = cwd();
    let original = invocation(
        arguments.clone(),
        working_directory.clone(),
        environment.clone(),
    );

    let bytes = original.to_bytes().expect("encode invocation");
    let parsed = NativeMakeInvocation::parse(&bytes).expect("parse invocation");
    assert_eq!(parsed.arguments(), arguments.as_slice());
    assert_eq!(parsed.working_directory(), working_directory);
    assert_eq!(
        parsed.environment().get("ENV-KEY-SECRET"),
        Some(&"ENV-VALUE-SECRET".to_owned())
    );
    assert_eq!(
        parsed.environment().get("  vendor:odd name/🛰"),
        Some(&environment[1].1)
    );
    assert_eq!(parsed, original);
    assert_eq!(parsed.to_bytes().expect("re-encode invocation"), bytes);

    let debug = format!("{original:?}");
    assert!(debug.contains("argument_count: 3"));
    assert!(debug.contains("environment_count: 2"));
    for secret in [
        "ARG-SECRET-1",
        "ENV-KEY-SECRET",
        "ENV-VALUE-SECRET",
        "$TOKEN",
        "native-invocation-cwd",
    ] {
        assert!(!debug.contains(secret), "Debug output exposed {secret}");
    }
}

#[test]
fn parser_rejects_unknown_fields_versions_malformed_documents_and_duplicate_names() {
    let invalid_documents: &[&[u8]] = &[
        br#"{"version":1,"arguments":[],"working_directory":"/tmp","environment":[],"extra":true}"#,
        br#"{"version":2,"arguments":[],"working_directory":"/tmp","environment":[]}"#,
        b"{",
        br#"{"version":1,"arguments":[],"working_directory":"/tmp","environment":[["DUP","first"],["DUP","second"]]}"#,
    ];
    for document in invalid_documents {
        assert!(NativeMakeInvocation::parse(document).is_err());
    }
}

#[test]
fn legacy_serialization_remains_version_one_without_an_argument_zero_field() {
    let original = invocation(vec!["arg".to_owned()], cwd(), Vec::new());
    assert_eq!(original.argument_zero(), None);
    assert_eq!(
        original.to_bytes().expect("encode legacy invocation"),
        br#"{"version":1,"arguments":["arg"],"working_directory":"/tmp/native-invocation-cwd","environment":[]}"#
    );
}

#[test]
fn selected_argument_zero_round_trips_as_version_two_without_debug_disclosure() {
    let original = invocation(
        vec!["--query".to_owned()],
        cwd(),
        vec![(
            "TOKEN_NAME_SECRET".to_owned(),
            "TOKEN_VALUE_SECRET".to_owned(),
        )],
    )
    .with_argument_zero("selected-argv0-secret".to_owned())
    .expect("select argument zero");

    let bytes = original.to_bytes().expect("encode version two invocation");
    let text = std::str::from_utf8(&bytes).expect("JSON is UTF-8");
    assert!(text.starts_with(r#"{"version":2,"arguments":["--query"]"#));
    assert!(text.ends_with(r#","argument_zero":"selected-argv0-secret"}"#));
    let parsed = NativeMakeInvocation::parse(&bytes).expect("parse version two invocation");
    assert_eq!(parsed.argument_zero(), Some("selected-argv0-secret"));
    assert_eq!(parsed, original);

    let debug = format!("{parsed:?}");
    for secret in [
        "selected-argv0-secret",
        "TOKEN_NAME_SECRET",
        "TOKEN_VALUE_SECRET",
        "--query",
    ] {
        assert!(!debug.contains(secret), "Debug output exposed {secret}");
    }
}

#[test]
fn parser_enforces_versioned_argument_zero_presence_and_json_field_rules() {
    let invalid_documents: &[&[u8]] = &[
        br#"{"version":2,"arguments":[],"working_directory":"/tmp","environment":[]}"#,
        br#"{"version":2,"arguments":[],"working_directory":"/tmp","environment":[],"argument_zero":null}"#,
        br#"{"version":1,"arguments":[],"working_directory":"/tmp","environment":[],"argument_zero":"selected"}"#,
        br#"{"version":3,"arguments":[],"working_directory":"/tmp","environment":[],"argument_zero":"selected"}"#,
        br#"{"version":2,"arguments":[],"working_directory":"/tmp","environment":[],"argument_zero":"first","argument_zero":"second"}"#,
        br#"{"version":2,"arguments":[],"working_directory":"/tmp","environment":[],"argument_zero":"selected","unknown":true}"#,
    ];
    for document in invalid_documents {
        assert!(NativeMakeInvocation::parse(document).is_err());
    }

    let legacy = br#"{"version":1,"arguments":[],"working_directory":"/tmp","environment":[]}"#;
    assert_eq!(
        NativeMakeInvocation::parse(legacy)
            .expect("parse legacy version one")
            .argument_zero(),
        None
    );
    let version_two = br#"{"version":2,"arguments":[],"working_directory":"/tmp","environment":[],"argument_zero":"selected"}"#;
    assert_eq!(
        NativeMakeInvocation::parse(version_two)
            .expect("parse version two")
            .argument_zero(),
        Some("selected")
    );
}

#[test]
fn selected_argument_zero_obeys_nonempty_nul_and_combined_argument_bounds() {
    let base = || invocation(Vec::new(), cwd(), Vec::new());
    assert!(base().with_argument_zero(String::new()).is_err());
    assert!(base().with_argument_zero("left\0right".to_owned()).is_err());

    let argument_zero_at_limit = "z".repeat(MAX_ARGUMENT_BYTES - 1);
    assert!(base().with_argument_zero(argument_zero_at_limit).is_ok());
    let argument_zero_over_limit = "z".repeat(MAX_ARGUMENT_BYTES);
    assert!(base().with_argument_zero(argument_zero_over_limit).is_err());

    let combined_at_limit = invocation(vec!["a".repeat(MAX_ARGUMENT_BYTES - 3)], cwd(), Vec::new());
    assert!(combined_at_limit.with_argument_zero("z".to_owned()).is_ok());
    let combined_over_limit =
        invocation(vec!["a".repeat(MAX_ARGUMENT_BYTES - 2)], cwd(), Vec::new());
    assert!(combined_over_limit
        .with_argument_zero("z".to_owned())
        .is_err());
}

#[test]
fn constructor_enforces_entry_byte_and_working_directory_limits() {
    let too_many_arguments = vec![String::new(); MAX_ENTRIES + 1];
    assert!(NativeMakeInvocation::new(too_many_arguments, cwd(), Vec::new()).is_err());

    let too_many_environment_entries = (0..=MAX_ENTRIES)
        .map(|index| (format!("K{index}"), "v".to_owned()))
        .collect::<Vec<_>>();
    assert!(NativeMakeInvocation::new(Vec::new(), cwd(), too_many_environment_entries).is_err());

    let argument_at_limit = "a".repeat(MAX_ARGUMENT_BYTES - 1);
    assert!(NativeMakeInvocation::new(vec![argument_at_limit], cwd(), Vec::new()).is_ok());
    let argument_over_limit = "a".repeat(MAX_ARGUMENT_BYTES);
    assert!(NativeMakeInvocation::new(vec![argument_over_limit], cwd(), Vec::new()).is_err());

    let environment_value_at_limit = "v".repeat(MAX_ENVIRONMENT_BYTES - 3);
    assert!(NativeMakeInvocation::new(
        Vec::new(),
        cwd(),
        vec![("k".to_owned(), environment_value_at_limit)],
    )
    .is_ok());
    let environment_value_over_limit = "v".repeat(MAX_ENVIRONMENT_BYTES - 2);
    assert!(NativeMakeInvocation::new(
        Vec::new(),
        cwd(),
        vec![("k".to_owned(), environment_value_over_limit)],
    )
    .is_err());

    let cwd_at_limit = PathBuf::from(format!("/{}", "a".repeat(MAX_CWD_BYTES - 1)));
    assert!(NativeMakeInvocation::new(Vec::new(), cwd_at_limit, Vec::new()).is_ok());
    let cwd_over_limit = PathBuf::from(format!("/{}", "a".repeat(MAX_CWD_BYTES)));
    assert!(NativeMakeInvocation::new(Vec::new(), cwd_over_limit, Vec::new()).is_err());

    let oversized = vec![b' '; MAX_DOCUMENT_BYTES + 1];
    assert!(NativeMakeInvocation::parse(&oversized).is_err());

    // JSON escaping expands these valid control characters substantially.
    let escaped_argument = "\u{0001}".repeat(MAX_ARGUMENT_BYTES - 1);
    let escaped_environment_value = "\u{0002}".repeat(MAX_ENVIRONMENT_BYTES - 3);
    let large_but_bounded = invocation(
        vec![escaped_argument],
        cwd(),
        vec![("k".to_owned(), escaped_environment_value)],
    )
    .to_bytes()
    .expect("bounded invocation remains encodable");
    assert!(large_but_bounded.len() <= MAX_DOCUMENT_BYTES);
}

#[test]
fn constructor_rejects_unusable_paths_and_os_delimiters() {
    for working_directory in [
        PathBuf::from("relative/path"),
        PathBuf::from("/tmp/../private"),
        PathBuf::from("/tmp/./private"),
        PathBuf::from("/tmp//private"),
        PathBuf::from("/tmp/private/"),
        PathBuf::from("/tmp/\0embedded-nul"),
    ] {
        assert!(NativeMakeInvocation::new(Vec::new(), working_directory, Vec::new()).is_err());
    }

    assert!(NativeMakeInvocation::new(vec!["left\0right".to_owned()], cwd(), Vec::new(),).is_err());
    for (name, value) in [
        ("NAME=WITH-EQUALS".to_owned(), "value".to_owned()),
        ("NAME\0WITH-NUL".to_owned(), "value".to_owned()),
        ("NAME".to_owned(), "value\0with-nul".to_owned()),
    ] {
        assert!(NativeMakeInvocation::new(Vec::new(), cwd(), vec![(name, value)]).is_err());
    }
}

#[test]
fn empty_environment_name_is_preserved_as_opaque_data() {
    let original = invocation(Vec::new(), cwd(), vec![(String::new(), "literal".into())]);
    let parsed = NativeMakeInvocation::parse(&original.to_bytes().unwrap()).unwrap();
    assert_eq!(parsed.environment().get(""), Some(&"literal".to_owned()));
}

#[cfg(unix)]
#[test]
fn capture_parent_chain_refuses_intermediate_symlinks_and_detects_replacement() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (_temporary, root) = private_capture_parent();
    let real = root.join("real");
    let leaf = real.join("leaf");
    std::fs::create_dir_all(&leaf).unwrap();
    std::fs::set_permissions(&leaf, std::fs::Permissions::from_mode(0o700)).unwrap();
    let alias = root.join("alias");
    symlink(&real, &alias).unwrap();
    assert!(super::capture_current(&alias.join("leaf/capture.json"), &[]).is_err());
    assert!(!leaf.join("capture.json").exists());

    let chain = super::open_capture_parent(&leaf).unwrap();
    std::fs::rename(&real, root.join("held-real")).unwrap();
    symlink(root.join("held-real"), &real).unwrap();
    assert!(super::recheck_capture_parent(&leaf, &chain).is_err());
}

#[cfg(unix)]
#[test]
fn capture_refuses_special_parent_permission_bits() {
    use std::os::unix::fs::PermissionsExt;
    let (_temporary, parent) = private_capture_parent();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o1700)).unwrap();
    assert!(super::capture_current(&parent.join("capture.json"), &[]).is_err());
    assert!(!parent.join("capture.json").exists());
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn serialization_is_deterministic_across_environment_input_order() {
    let first = invocation(
        vec!["make".to_owned()],
        cwd(),
        vec![
            ("Z_LAST".to_owned(), "z".to_owned()),
            ("A_FIRST".to_owned(), "a".to_owned()),
        ],
    );
    let second = invocation(
        vec!["make".to_owned()],
        cwd(),
        vec![
            ("A_FIRST".to_owned(), "a".to_owned()),
            ("Z_LAST".to_owned(), "z".to_owned()),
        ],
    );
    assert_eq!(
        first.to_bytes().expect("encode first"),
        second.to_bytes().expect("encode second")
    );
}

#[test]
fn constructor_uses_only_explicit_data_and_does_not_require_a_real_cwd() {
    let temporary = tempfile::tempdir().expect("create private test directory");
    let nonexistent_cwd = temporary
        .path()
        .canonicalize()
        .expect("canonicalize test directory")
        .join("not-created")
        .join("still-not-created");
    assert!(!nonexistent_cwd.exists());

    let snapshot = invocation(
        vec!["$HOME/${PATH}".to_owned()],
        nonexistent_cwd.clone(),
        vec![("ONLY_EXPLICIT".to_owned(), "$HOME".to_owned())],
    );
    assert_eq!(snapshot.working_directory(), nonexistent_cwd);
    assert_eq!(snapshot.arguments(), &["$HOME/${PATH}".to_owned()]);
    assert_eq!(snapshot.environment().len(), 1);
    assert_eq!(
        snapshot.environment().get("ONLY_EXPLICIT"),
        Some(&"$HOME".to_owned())
    );
    assert!(!snapshot.environment().contains_key("HOME"));
    assert!(!snapshot.environment().contains_key("PATH"));
}

#[cfg(unix)]
fn private_capture_parent() -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("create private test directory");
    let parent = temporary
        .path()
        .canonicalize()
        .expect("canonicalize private test directory");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
        .expect("set exact private directory mode");
    (temporary, parent)
}

#[cfg(unix)]
#[test]
fn capture_current_writes_a_private_file_in_a_private_canonical_directory() {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    let (_temporary, parent) = private_capture_parent();
    assert_eq!(
        std::fs::metadata(&parent)
            .expect("inspect private parent")
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
    let output = parent.join("invocation.json");
    let arguments = [OsString::from("make"), OsString::from("--query")];

    super::capture_current(&output, &arguments).expect("capture current invocation");

    let metadata = std::fs::metadata(&output).expect("inspect capture file");
    assert!(metadata.is_file());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    let parsed =
        NativeMakeInvocation::parse(&std::fs::read(&output).expect("read captured invocation"))
            .expect("parse captured invocation");
    assert_eq!(
        parsed.arguments(),
        &["make".to_owned(), "--query".to_owned()]
    );
    assert_eq!(parsed.working_directory(), std::env::current_dir().unwrap());
}

#[cfg(unix)]
#[test]
fn capture_with_selected_argument_zero_writes_the_explicit_version_two_value() {
    use std::ffi::OsString;

    let (_temporary, parent) = private_capture_parent();
    let output = parent.join("selected-invocation.json");
    let arguments = [OsString::from("make"), OsString::from("--query")];
    let selected = "selected-wrapper-name";

    super::capture_current_with_selected_argument_zero(&output, &arguments, selected)
        .expect("capture with selected argument zero");

    let parsed = NativeMakeInvocation::parse(&std::fs::read(&output).expect("read capture"))
        .expect("parse selected invocation");
    assert_eq!(parsed.argument_zero(), Some(selected));
    assert_eq!(
        parsed.arguments(),
        &["make".to_owned(), "--query".to_owned()]
    );
}

#[cfg(unix)]
#[test]
fn capture_current_rejects_existing_files_symlink_targets_and_public_parents() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let (_temporary, parent) = private_capture_parent();
    let existing = parent.join("existing.json");
    std::fs::write(&existing, b"existing sentinel").expect("create existing output");
    assert!(super::capture_current(&existing, &[]).is_err());
    assert_eq!(
        std::fs::read(&existing).expect("read existing output"),
        b"existing sentinel"
    );

    let target = parent.join("symlink-target.json");
    std::fs::write(&target, b"symlink sentinel").expect("create symlink target");
    let link = parent.join("output-link.json");
    symlink(&target, &link).expect("create output symlink");
    assert!(super::capture_current(&link, &[]).is_err());
    assert_eq!(
        std::fs::read(&target).expect("read symlink target"),
        b"symlink sentinel"
    );

    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755))
        .expect("make parent public");
    let public_output = parent.join("public-parent-output.json");
    let rejected = super::capture_current(&public_output, &[]).is_err();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
        .expect("restore private parent mode");
    assert!(rejected);
    assert!(!public_output.exists());
}
