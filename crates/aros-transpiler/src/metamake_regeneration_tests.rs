use super::*;
use aros_common::{measure_tree_content_cas, mesa_source::MesaSourceSnapshot, sha256_bytes};
use std::{fs, io::Write, path::PathBuf};
use tempfile::TempDir;

const CONFIG: &[u8] =
    b"[probe]\ndefaultmakefilename rules\ngenmakefilescript generator\nignoredir ignored\n";

struct SourceFixture {
    _temporary: TempDir,
    root: PathBuf,
    snapshot: MesaSourceSnapshot,
}

impl SourceFixture {
    fn capture(root: PathBuf, temporary: TempDir) -> Self {
        let expected = measure_tree_content_cas(&root)
            .unwrap()
            .payload_digest_excluding(None);
        let snapshot = MesaSourceSnapshot::capture(&root, &expected, None).unwrap();
        Self {
            _temporary: temporary,
            root,
            snapshot,
        }
    }

    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("source");
        fs::create_dir_all(root.join("same")).unwrap();
        fs::create_dir_all(root.join("direct")).unwrap();
        fs::create_dir_all(root.join("generated")).unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::create_dir_all(root.join("nested/ignored")).unwrap();

        // A .src owner wins over its same-directory direct makefile.
        fs::write(root.join("same/rules"), b"direct shadow\n").unwrap();
        fs::write(root.join("same/rules.src"), b"generated source\n").unwrap();
        fs::write(root.join("direct/rules"), b"direct input\n").unwrap();
        fs::write(root.join("generated/rules.src"), b"another source\n").unwrap();
        fs::write(root.join("rules"), b"root direct input\n").unwrap();
        fs::write(root.join("ignored/rules.src"), b"ignored source\n").unwrap();
        fs::write(
            root.join("nested/ignored/rules.src"),
            b"nested ignored source\n",
        )
        .unwrap();

        Self::capture(root, temporary)
    }
}

fn derive<'a>(
    fixture: &'a SourceFixture,
    configured: &[u8],
) -> Result<MetaMakeRegenerationPlan<'a>, String> {
    MetaMakeRegenerationPlan::derive(
        &fixture.snapshot,
        configured,
        &sha256_bytes(configured),
        "probe",
    )
}

#[test]
fn plan_prefers_src_owner_ignores_configured_directories_and_retains_direct_inputs() {
    let fixture = SourceFixture::new();
    let plan = derive(&fixture, CONFIG).unwrap();

    assert_eq!(plan.project(), "probe");
    assert_eq!(
        plan.owners()
            .iter()
            .map(|owner| (owner.owner.as_str(), owner.input.as_str(), owner.generated))
            .collect::<Vec<_>>(),
        [
            ("direct/rules", "direct/rules", false),
            ("generated/rules", "generated/rules.src", true),
            ("rules", "rules", false),
            ("same/rules", "same/rules.src", true),
        ]
    );
    assert_eq!(
        plan.generated_files(),
        &["generated/rules".to_owned(), "same/rules".to_owned()]
    );
    assert!(plan.owners().iter().all(|owner| {
        fixture
            .snapshot
            .regular_input_digest(&owner.input)
            .is_some()
    }));
    plan.revalidate(&fixture.root, CONFIG).unwrap();
}

#[test]
fn filesystem_owner_created_after_capture_is_not_adopted_and_revalidation_detects_drift() {
    let fixture = SourceFixture::new();
    let plan = derive(&fixture, CONFIG).unwrap();

    fs::create_dir_all(fixture.root.join("late")).unwrap();
    fs::write(
        fixture.root.join("late/rules.src"),
        b"post-capture source\n",
    )
    .unwrap();

    assert!(!plan
        .owners()
        .iter()
        .any(|owner| owner.owner == "late/rules"));
    assert!(!plan
        .generated_files()
        .iter()
        .any(|path| path == "late/rules"));
    assert_eq!(
        plan.revalidate(&fixture.root, CONFIG).unwrap_err(),
        "complete Mesa source changed"
    );
}

#[test]
fn unrelated_source_asset_names_are_not_interpreted_as_make_identifiers() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("source");
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("assets/Open Sans Italic.font"), b"font").unwrap();
    fs::write(root.join("clang++.in"), b"ordinary tool source").unwrap();
    fs::write(root.join("rules.src"), b"selected owner").unwrap();
    let fixture = SourceFixture::capture(root, temporary);
    let plan = derive(&fixture, CONFIG).unwrap();
    assert_eq!(plan.generated_files(), &["rules".to_owned()]);
    assert!(fixture
        .snapshot
        .regular_input_digest("assets/Open Sans Italic.font")
        .is_some());
    assert!(fixture
        .snapshot
        .regular_input_digest("clang++.in")
        .is_some());
    plan.revalidate(&fixture.root, CONFIG).unwrap();

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("source");
    fs::create_dir_all(root.join("ambiguous owner")).unwrap();
    fs::write(root.join("ambiguous owner/rules.src"), b"not admitted").unwrap();
    let fixture = SourceFixture::capture(root, temporary);
    assert_eq!(
        derive(&fixture, CONFIG).err().unwrap(),
        "MetaMake discovery requires a literal basename"
    );
}

#[test]
fn configuration_digest_and_selected_project_are_independent_inputs() {
    let fixture = SourceFixture::new();
    assert_eq!(
        MetaMakeRegenerationPlan::derive(
            &fixture.snapshot,
            CONFIG,
            &sha256_bytes(b"different configured bytes"),
            "probe",
        )
        .err()
        .expect("mismatched configuration digest must be rejected"),
        "MetaMake regeneration configuration bytes changed"
    );

    assert_eq!(
        MetaMakeRegenerationPlan::derive(
            &fixture.snapshot,
            CONFIG,
            &sha256_bytes(CONFIG),
            "missing",
        )
        .err()
        .expect("absent selected project must be rejected"),
        "MetaMake project \"missing\" is absent"
    );
}

#[test]
fn top_and_added_makefiles_require_separate_input_closure() {
    let fixture = SourceFixture::new();
    let with_top = b"[probe]\ndefaultmakefilename rules\ntop elsewhere\n";
    assert_eq!(
        derive(&fixture, with_top)
            .err()
            .expect("unproven TOP must be rejected"),
        "MetaMake top selection requires a separate root identity proof"
    );

    let with_add = b"[probe]\ndefaultmakefilename rules\nadd extra/rules.src\n";
    assert_eq!(
        derive(&fixture, with_add)
            .err()
            .expect("build-tree additions must be rejected"),
        "MetaMake added makefiles require build-tree input closure"
    );
}

#[test]
fn unsafe_configured_paths_and_source_symlinks_are_refused() {
    let fixture = SourceFixture::new();
    let unsafe_add = b"[probe]\ndefaultmakefilename rules\nadd ../outside\n";
    assert_eq!(
        derive(&fixture, unsafe_add)
            .err()
            .expect("unsafe configured path must be rejected"),
        "MetaMake discovery requires a literal basename"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("source");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("linked-directory")).unwrap();
        let linked = SourceFixture::capture(root, temporary);
        assert_eq!(
            derive(&linked, CONFIG)
                .err()
                .expect("external symbolic-link target must be rejected"),
            "unsupported MetaMake link target: linked-directory"
        );
    }
}

#[test]
fn linked_source_directory_retains_logical_owner_and_original_byte_binding() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("source");
    fs::create_dir_all(root.join("actual")).unwrap();
    fs::write(root.join("actual/rules.src"), b"original rule bytes").unwrap();
    symlink("actual", root.join("alias")).unwrap();
    let fixture = SourceFixture::capture(root, temporary);
    let plan = derive(&fixture, CONFIG).unwrap();
    assert_eq!(plan.generated_files(), &["actual/rules", "alias/rules"]);
    assert_eq!(
        plan.original_input("alias/rules.src"),
        Some("actual/rules.src")
    );
    assert_eq!(
        fixture
            .snapshot
            .regular_input_digest(plan.original_input("alias/rules.src").unwrap()),
        Some(&sha256_bytes(b"original rule bytes")),
    );
    plan.revalidate(&fixture.root, CONFIG).unwrap();
    fs::remove_file(fixture.root.join("alias")).unwrap();
    symlink("missing", fixture.root.join("alias")).unwrap();
    assert!(plan.revalidate(&fixture.root, CONFIG).is_err());
}

#[test]
fn local_snapshot_omissions_must_match_the_configured_native_scan() {
    use aros_common::local_source::LocalSourceIdentity;
    use std::process::Command;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=Source discovery test",
                "-c",
                "user.email=discovery@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-q"]);
    fs::write(root.join("rules.src"), b"source rule\n").unwrap();
    git(&["add", "rules.src"]);
    git(&["commit", "-qm", "Initial source"]);
    fs::create_dir_all(root.join("build/fixture")).unwrap();
    fs::write(
        root.join("build/fixture/rules.src"),
        b"must not be silently omitted\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("build/other")).unwrap();
    fs::write(root.join("build/other/rules.src"), b"sibling source rule\n").unwrap();
    let local = LocalSourceIdentity::capture(root, "fixture").unwrap();
    let source = MesaSourceSnapshot::capture(root, &local.content_sha256, Some(&local)).unwrap();
    assert!(source.excludes_git_metadata());
    assert_eq!(source.excluded_generated_subtree(), Some("build/fixture"));
    for ignored in [
        "",
        "ignoredir .git\n",
        "ignoredir fixture\n",
        "ignoredir .git\nignoredir fixture\n",
        "ignoredir .git\nignoredir build\n",
        "ignoredir .git\nignoredir build/other\n",
    ] {
        let config =
            format!("[probe]\ndefaultmakefilename rules\ngenmakefilescript generator\n{ignored}");
        let error = MetaMakeRegenerationPlan::derive(
            &source,
            config.as_bytes(),
            &sha256_bytes(config.as_bytes()),
            "probe",
        )
        .err()
        .expect("native scan must not include omitted input namespaces");
        assert!(error.contains("omitted Source namespaces"), "{error}");
    }
    let config = b"[probe]\ndefaultmakefilename rules\ngenmakefilescript generator\nignoredir .git\nignoredir build/fixture\n";
    let plan =
        MetaMakeRegenerationPlan::derive(&source, config, &sha256_bytes(config), "probe").unwrap();
    assert_eq!(plan.generated_files(), &["build/other/rules", "rules"]);
    assert!(plan
        .discovered_inputs()
        .contains_key("build/other/rules.src"));
    assert!(!plan
        .discovered_inputs()
        .contains_key("build/fixture/rules.src"));
    plan.revalidate(root, config).unwrap();
}

/// Opt-in inventory audit against a caller-selected, previously configured
/// host source tree and its exact configured project file. Candidate output
/// coordinates are not runtime, owner, or graph-admission evidence.
#[test]
#[ignore = "requires an explicitly selected successful host Source/configuration pair"]
fn actual_configured_source_plan_is_exact_sorted_and_non_clobbering() {
    let source_root = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_REGEN_SOURCE")
            .expect("set AROS_TEST_MESA_REGEN_SOURCE to the prior successful host Source root"),
    );
    let config_path = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_REGEN_CONFIG")
            .expect("set AROS_TEST_MESA_REGEN_CONFIG to its configured project file"),
    );
    let configured = fs::read(&config_path).unwrap();
    let source_digest = measure_tree_content_cas(&source_root)
        .unwrap()
        .payload_digest_excluding(None);
    let source = MesaSourceSnapshot::capture(&source_root, &source_digest, None).unwrap();
    let plan =
        MetaMakeRegenerationPlan::derive(&source, &configured, &sha256_bytes(&configured), "AROS")
            .unwrap();
    plan.revalidate(&source_root, &configured).unwrap();

    assert!(
        !plan.generated_files().is_empty(),
        "no configured .src input was observed"
    );
    assert!(plan
        .generated_files()
        .windows(2)
        .all(|pair| pair[0] < pair[1]));
    for generated in plan.generated_files() {
        let owner = plan
            .owners()
            .iter()
            .find(|owner| owner.owner.as_str() == generated.as_str() && owner.generated)
            .expect("generated coordinate must retain its source owner");
        assert_eq!(owner.input, format!("{generated}.src"));
        assert!(source
            .regular_input_digest(plan.original_input(&owner.input).unwrap())
            .is_some());
    }
    for owner in plan.owners() {
        assert!(
            source
                .regular_input_digest(plan.original_input(&owner.input).unwrap())
                .is_some(),
            "owner input {} lacks an original Source digest",
            owner.input
        );
    }

    if let Some(result_path) = std::env::var_os("AROS_TEST_MESA_REGEN_RESULT") {
        let result_path = PathBuf::from(result_path);
        assert!(result_path.is_absolute(), "result path must be absolute");
        assert!(
            !result_path.starts_with(&source_root),
            "result must be outside the measured Source root"
        );
        assert!(
            !result_path.exists(),
            "refusing to overwrite an existing result"
        );
        assert!(result_path.parent().is_some_and(Path::is_dir));

        let owner_inputs = plan
            .owners()
            .iter()
            .map(|owner| {
                serde_json::json!({
                    "owner": owner.owner,
                    "input": owner.input,
                    "generated": owner.generated,
                    "original_input": plan.original_input(&owner.input).expect("checked above"),
                    "original_sha256": source
                        .regular_input_digest(plan.original_input(&owner.input).expect("checked above"))
                        .expect("checked above")
                        .as_str(),
                })
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "qualification":"local-source-genmf-candidate-output-plan-not-root-launch-or-sdk-proof",
            "source_inputs_sha256":source_digest,
            "project": plan.project(),
            "configuration_sha256": plan.configuration_sha256().as_str(),
            "generated_files": plan.generated_files(),
            "owners": owner_inputs,
        }))
        .unwrap();
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut result = options.open(&result_path).unwrap();
        result.write_all(&bytes).unwrap();
        result.write_all(b"\n").unwrap();
    }
}
