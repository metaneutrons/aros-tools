//! Project-scoped, compare-and-publish toolchain release-lock selection.
//!
//! This module deliberately has no global "active" selector.  A selected
//! release is always the complete `aros-toolchains.lock.toml` in one AROS
//! checkout; explicit local prefixes remain outside this command family.

use crate::artifact::require_absolute_state_path;
use crate::observability;
use crate::repo;
use crate::toolchain;
use crate::toolchain_management::{
    acquire_store_lock, management_store, normalized_absolute_utf8, publication_error,
    stable_token, ResultFormat, MANAGEMENT_DIRECTORY,
};
use aros_common::{
    measure_regular_file_bounded, parse_credential_free_https_url, publish_atomic_file,
    sha256_bytes, AdvisoryFileLock, ArosToolchainLock, AtomicFilePolicy, CommitState, FileIdentity,
};
use clap::Args;
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SELECTION_SCHEMA: &str = "aros-toolchain-selection-v1";
const PROJECT_LOCKS_DIRECTORY: &str = "project-locks/v1";
const MAX_RELEASE_LOCK_BYTES: u64 = 4 * 1024 * 1024;

/// Arguments for the project-scoped release-lock selection operation.
#[derive(Args)]
pub struct SelectArgs {
    /// Absolute TOML release lock to validate and select for this checkout
    #[arg(long, value_name = "FILE")]
    release_lock: PathBuf,

    /// Explicit absolute toolchain-store root used only for management locks
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,

    /// Apply only the exact preview identified by this token
    #[arg(long, value_name = "TOKEN")]
    apply: Option<String>,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

#[derive(Debug, Serialize)]
struct SelectionResult {
    schema: &'static str,
    operation: &'static str,
    state: &'static str,
    project: String,
    project_lock: String,
    candidate: String,
    store: String,
    project_lock_guard: String,
    old_lock_sha256: Option<String>,
    old_release_id: Option<String>,
    new_lock_sha256: String,
    new_release_id: String,
    apply_token: Option<String>,
    note: &'static str,
}

#[derive(Debug, Clone)]
struct MeasuredReleaseLock {
    path_text: String,
    identity: FileIdentity,
    bytes: Vec<u8>,
    sha256: String,
    lock: ArosToolchainLock,
}

#[derive(Debug, Clone)]
struct SelectionPlan {
    project: String,
    project_lock: PathBuf,
    candidate: MeasuredReleaseLock,
    previous: Option<MeasuredReleaseLock>,
    store: PathBuf,
    project_lock_guard: PathBuf,
}

/// Preview or atomically select one complete released toolchain lock.
///
/// # Errors
///
/// Returns an error when the candidate is not one coherent release lock, the
/// checkout's target contract is incompatible, the preview is stale, another
/// selection holds the lock, or the existing project lock changes before the
/// no-clobber/CAS publication boundary.
pub fn select(repo_root: &Path, args: SelectArgs) -> Result<()> {
    let store = management_store(args.store)?;
    let candidate_path = require_absolute_state_path("--release-lock", args.release_lock)?;
    let plan = inspect_selection(repo_root, &candidate_path, &store)?;
    let token = apply_token(&plan);
    let result = if let Some(provided) = args.apply {
        if provided != token {
            return Err(miette::miette!(
                "selection apply token does not match the current project or candidate lock; rerun the selection preview"
            ));
        }
        apply_selection(repo_root, &candidate_path, &store, &provided)?
    } else {
        selection_result(
            &plan,
            "preview",
            Some(token),
            "validated a complete released lock; no project selection was changed",
        )
    };
    print_selection_result(&result, args.format);
    Ok(())
}

fn apply_selection(
    repo_root: &Path,
    candidate_path: &Path,
    store: &Path,
    provided_token: &str,
) -> Result<SelectionResult> {
    let store_lock = acquire_store_lock(store)?;
    let project_text = normalized_absolute_utf8(repo_root, "project root")?;
    let guard_path = project_lock_path(store, &project_text);
    let project_lock = AdvisoryFileLock::acquire(&guard_path)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot acquire project selection lock below '{}'",
                guard_path.display()
            )
        })?;
    revalidate_locks(&store_lock, &project_lock)?;

    // Rebuild the plan while both locks are held.  This binds an apply token to
    // the exact candidate bytes and prior project-lock snapshot, rather than
    // to a name that an editor could have replaced after preview.
    let plan = inspect_selection(repo_root, candidate_path, store)?;
    let observed_token = apply_token(&plan);
    if observed_token != provided_token {
        return Err(miette::miette!(
            "selection input changed after preview; no project lock was published"
        ));
    }
    revalidate_locks(&store_lock, &project_lock)?;
    commit_plan(&plan)?;
    revalidate_locks(&store_lock, &project_lock)?;
    Ok(selection_result(
        &plan,
        "committed",
        None,
        "published one complete release lock atomically; no local or external prefix was selected",
    ))
}

fn commit_plan(plan: &SelectionPlan) -> Result<()> {
    let policy = plan
        .previous
        .as_ref()
        .map_or(AtomicFilePolicy::NoClobber, |previous| {
            AtomicFilePolicy::ReplaceIf {
                identity: previous.identity,
                sha256: sha256_bytes(&previous.bytes),
            }
        });
    if let Err(error) = publish_atomic_file(&plan.project_lock, &plan.candidate.bytes, policy) {
        return publication_error(
            error,
            "project toolchain-lock publication failed",
            "project toolchain selection may have crossed its atomic publication boundary; inspect the project lock before retrying",
            "project toolchain selection did not cross its atomic publication boundary",
        );
    }
    let published = read_release_lock(&plan.project_lock, "published project lock")?;
    if published.bytes != plan.candidate.bytes || published.lock != plan.candidate.lock {
        return observability::commit_state(
            Err(miette::miette!(
                "published project lock does not match the approved candidate"
            )),
            CommitState::Committed,
            "project toolchain lock was published, but exact readback could not be proven",
        );
    }
    Ok(())
}

fn inspect_selection(
    repo_root: &Path,
    candidate_path: &Path,
    store: &Path,
) -> Result<SelectionPlan> {
    let project = normalized_absolute_utf8(repo_root, "project root")?;
    let project_lock = toolchain::lock_file_path(repo_root);
    let candidate = read_release_lock(candidate_path, "candidate release lock")?;
    validate_candidate_for_project(repo_root, &candidate.lock)?;
    let previous = match measure_regular_file_bounded(&project_lock, MAX_RELEASE_LOCK_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot safely inspect existing project lock '{}'",
                project_lock.display()
            )
        })? {
        Some(_) => Some(read_release_lock(&project_lock, "existing project lock")?),
        None => None,
    };
    if previous
        .as_ref()
        .is_some_and(|previous| previous.sha256 == candidate.sha256)
    {
        return Err(miette::miette!(
            "candidate release lock is already the exact project selection; no change is needed"
        ));
    }
    if previous
        .as_ref()
        .is_some_and(|previous| previous.lock.release_id == candidate.lock.release_id)
    {
        return Err(miette::miette!(
            "candidate release_id '{}' equals the current selection but its immutable lock bytes differ",
            candidate.lock.release_id
        ));
    }
    let project_lock_guard = project_lock_path(store, &project);
    Ok(SelectionPlan {
        project,
        project_lock,
        candidate,
        previous,
        store: store.to_path_buf(),
        project_lock_guard,
    })
}

fn read_release_lock(path: &Path, label: &str) -> Result<MeasuredReleaseLock> {
    if path.extension().is_none_or(|extension| extension != "toml") {
        return Err(miette::miette!(
            "{label} '{}' must be a TOML file",
            path.display()
        ));
    }
    let path_text = normalized_absolute_utf8(path, label)?;
    let Some((identity, bytes)) = measure_regular_file_bounded(path, MAX_RELEASE_LOCK_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely read {label} '{}'", path.display()))?
    else {
        return Err(miette::miette!(
            "{label} '{}' does not exist",
            path.display()
        ));
    };
    let contents = std::str::from_utf8(&bytes)
        .into_diagnostic()
        .wrap_err_with(|| format!("{label} '{}' is not UTF-8", path.display()))?;
    let lock: ArosToolchainLock = toml::from_str(contents)
        .into_diagnostic()
        .wrap_err_with(|| format!("{label} '{}' is not a valid TOML lock", path.display()))?;
    lock.validate()
        .map_err(|message| miette::miette!("{message}"))
        .wrap_err_with(|| format!("{label} '{}' violates the v1 lock contract", path.display()))?;
    Ok(MeasuredReleaseLock {
        path_text,
        identity,
        sha256: sha256_bytes(&bytes).to_string(),
        bytes,
        lock,
    })
}

fn validate_candidate_for_project(repo_root: &Path, lock: &ArosToolchainLock) -> Result<()> {
    let base_url = lock
        .base_url
        .as_deref()
        .ok_or_else(|| miette::miette!("candidate release lock needs a release base_url"))?;
    let parsed_base = parse_credential_free_https_url(base_url)
        .map_err(|message| miette::miette!("{message}"))?;
    if parsed_base
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        != Some(lock.release_id.as_str())
    {
        return Err(miette::miette!(
            "candidate release base_url must end in its release_id '{}'",
            lock.release_id
        ));
    }
    if lock.artifacts.is_empty() {
        return Err(miette::miette!(
            "candidate release lock has no host/profile artifacts"
        ));
    }
    let profiles = repo::load_target_profiles(repo_root)?;
    let expected_profiles = profiles
        .iter()
        .map(|profile| {
            (
                profile.name.as_str(),
                toolchain::target_triple_for_profile(profile),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut by_profile = BTreeMap::<&str, BTreeSet<&str>>::new();
    for artifact in &lock.artifacts {
        let Some(expected_triple) = expected_profiles.get(artifact.target_profile.as_str()) else {
            return Err(miette::miette!(
                "candidate release lock contains unsupported target profile '{}'",
                artifact.target_profile
            ));
        };
        if artifact.target_triple != *expected_triple {
            return Err(miette::miette!(
                "candidate artifact {}/{} declares target triple '{}', expected '{}'",
                artifact.host,
                artifact.target_profile,
                artifact.target_triple,
                expected_triple
            ));
        }
        if artifact.enabled {
            let resolved = lock
                .asset_url(artifact)
                .map_err(|message| miette::miette!("{message}"))?;
            let required_prefix = format!("{}/", base_url.trim_end_matches('/'));
            if !resolved.starts_with(&required_prefix) {
                return Err(miette::miette!(
                    "candidate artifact {}/{} is not below the declared release base_url",
                    artifact.host,
                    artifact.target_profile
                ));
            }
        }
        by_profile
            .entry(artifact.target_profile.as_str())
            .or_default()
            .insert(artifact.host.as_str());
    }
    let mut expected_hosts = None;
    for profile in &profiles {
        let Some(hosts) = by_profile.get(profile.name.as_str()) else {
            return Err(miette::miette!(
                "candidate release lock is missing target profile '{}'",
                profile.name
            ));
        };
        if hosts.is_empty() {
            return Err(miette::miette!(
                "candidate release lock has no host coverage for target profile '{}'",
                profile.name
            ));
        }
        if let Some(expected) = &expected_hosts {
            if expected != hosts {
                return Err(miette::miette!(
                    "candidate release lock has an incoherent host matrix for target profile '{}'",
                    profile.name
                ));
            }
        } else {
            expected_hosts = Some(hosts.clone());
        }
    }
    Ok(())
}

fn project_lock_path(store: &Path, project: &str) -> PathBuf {
    let project_id = stable_token("aros-toolchain-project-lock-v1", &[project]);
    store
        .join(MANAGEMENT_DIRECTORY)
        .join(PROJECT_LOCKS_DIRECTORY)
        .join(format!("{project_id}.lock"))
}

fn apply_token(plan: &SelectionPlan) -> String {
    let previous_sha256 = plan.previous.as_ref().map_or("absent", |lock| &lock.sha256);
    let previous_release = plan
        .previous
        .as_ref()
        .map_or("absent", |lock| lock.lock.release_id.as_str());
    stable_token(
        "aros-toolchain-selection-apply-v1",
        &[
            &plan.project,
            &plan.project_lock.display().to_string(),
            &plan.candidate.path_text,
            &plan.candidate.sha256,
            plan.candidate.lock.release_id.as_str(),
            previous_sha256,
            previous_release,
        ],
    )
}

fn revalidate_locks(store_lock: &AdvisoryFileLock, project_lock: &AdvisoryFileLock) -> Result<()> {
    store_lock
        .revalidate()
        .into_diagnostic()
        .wrap_err("toolchain store lock could not be revalidated")?;
    project_lock
        .revalidate()
        .into_diagnostic()
        .wrap_err("project selection lock could not be revalidated")?;
    Ok(())
}

fn selection_result(
    plan: &SelectionPlan,
    state: &'static str,
    apply_token: Option<String>,
    note: &'static str,
) -> SelectionResult {
    SelectionResult {
        schema: SELECTION_SCHEMA,
        operation: "select",
        state,
        project: plan.project.clone(),
        project_lock: plan.project_lock.display().to_string(),
        candidate: plan.candidate.path_text.clone(),
        store: plan.store.display().to_string(),
        project_lock_guard: plan.project_lock_guard.display().to_string(),
        old_lock_sha256: plan.previous.as_ref().map(|lock| lock.sha256.clone()),
        old_release_id: plan
            .previous
            .as_ref()
            .map(|lock| lock.lock.release_id.clone()),
        new_lock_sha256: plan.candidate.sha256.clone(),
        new_release_id: plan.candidate.lock.release_id.clone(),
        apply_token,
        note,
    }
}

fn print_selection_result(result: &SelectionResult, format: ResultFormat) {
    match format {
        ResultFormat::Human => {
            aros_common::outputln!(
                "Toolchain selection {}: {} -> {}",
                result.state,
                result
                    .old_release_id
                    .as_deref()
                    .unwrap_or("no project lock"),
                result.new_release_id
            );
            aros_common::outputln!("  Project:     {}", result.project);
            aros_common::outputln!("  Lock:        {}", result.project_lock);
            aros_common::outputln!("  Candidate:   {}", result.candidate);
            aros_common::outputln!("  {}", result.note);
            if let Some(token) = &result.apply_token {
                aros_common::outputln!("  Apply token: {token}");
                aros_common::outputln!(
                    "  No project selection changed. Re-run with --apply {token} to commit this exact preview."
                );
            }
        }
        ResultFormat::Json => {
            let document = serde_json::to_string_pretty(result)
                .expect("selection result serialization is infallible");
            aros_common::outputln!("{document}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_selection, apply_token, inspect_selection, project_lock_path, read_release_lock,
    };
    use aros_common::{ArosToolchainArtifact, ArosToolchainLock};
    use std::fs;
    use std::path::Path;

    fn lock(release_id: &str) -> ArosToolchainLock {
        ArosToolchainLock {
            schema: 1,
            release_id: release_id.into(),
            base_url: Some(format!("https://example.invalid/releases/{release_id}")),
            artifacts: vec![ArosToolchainArtifact {
                host: "linux-x86_64".into(),
                target_profile: "pc-x86_64".into(),
                target_triple: "x86_64-unknown-aros".into(),
                asset: format!("aros-toolchain-{release_id}.tar.xz"),
                sha256: "a".repeat(64),
                tree_sha256: "b".repeat(64),
                llvm_version: Some("11.0.0".into()),
                size: Some(1),
                enabled: true,
                disabled_reason: None,
                strip_components: 1,
                required_paths: vec!["bin/clang".into()],
            }],
        }
    }

    fn write_lock(path: &Path, lock: &ArosToolchainLock) {
        fs::write(path, toml::to_string(lock).unwrap()).unwrap();
    }

    fn checkout(root: &Path) {
        for directory in ["arch", "compiler", "rom"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("configure"), "").unwrap();
        fs::write(root.join("Makefile.in"), "").unwrap();
        fs::write(
            root.join("aros-targets.toml"),
            "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
        )
        .unwrap();
    }

    fn checkout_with_two_profiles(root: &Path) {
        checkout(root);
        fs::write(
            root.join("aros-targets.toml"),
            "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n\
             [[targets]]\nname='arm-raspi'\narch='arm'\nplatform='raspi'\nbsp='raspi'\n",
        )
        .unwrap();
    }

    #[test]
    fn plan_requires_a_new_coherent_release_and_binds_the_prior_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let candidate = temporary.path().join("candidate.toml");
        let store = temporary.path().join("store");
        checkout(&project);
        write_lock(
            &project.join("aros-toolchains.lock.toml"),
            &lock("old-release"),
        );
        write_lock(&candidate, &lock("new-release"));

        let plan = inspect_selection(&project, &candidate, &store).unwrap();
        assert_eq!(
            plan.previous.as_ref().unwrap().lock.release_id,
            "old-release"
        );
        assert_eq!(plan.candidate.lock.release_id, "new-release");
        assert_ne!(apply_token(&plan), "");
        assert!(
            inspect_selection(&project, &project.join("aros-toolchains.lock.toml"), &store)
                .is_err()
        );
    }

    #[test]
    fn commit_uses_compare_and_swap_and_preserves_a_concurrent_project_edit() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let candidate = temporary.path().join("candidate.toml");
        let store = temporary.path().join("store");
        checkout(&project);
        let destination = project.join("aros-toolchains.lock.toml");
        write_lock(&destination, &lock("old-release"));
        write_lock(&candidate, &lock("new-release"));
        let plan = inspect_selection(&project, &candidate, &store).unwrap();
        fs::write(&destination, "# edited concurrently\n").unwrap();

        let result = super::commit_plan(&plan);
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "# edited concurrently\n"
        );
    }

    #[test]
    fn initial_selection_uses_no_clobber_and_project_guard_is_deterministic() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let candidate = temporary.path().join("candidate.toml");
        let store = temporary.path().join("store");
        checkout(&project);
        write_lock(&candidate, &lock("new-release"));
        let plan = inspect_selection(&project, &candidate, &store).unwrap();
        let token = apply_token(&plan);
        let result = apply_selection(&project, &candidate, &store, &token).unwrap();

        assert_eq!(result.state, "committed");
        assert_eq!(
            read_release_lock(&project.join("aros-toolchains.lock.toml"), "published")
                .unwrap()
                .lock
                .release_id,
            "new-release"
        );
        assert!(project_lock_path(&store, &result.project).is_file());
    }

    #[test]
    fn selection_rejects_a_wrong_target_triple_or_incoherent_host_matrix() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let candidate = temporary.path().join("candidate.toml");
        let store = temporary.path().join("store");
        checkout(&project);
        let mut wrong_triple = lock("new-release");
        wrong_triple.artifacts[0].target_triple = "wrong-unknown-aros".into();
        write_lock(&candidate, &wrong_triple);
        assert!(inspect_selection(&project, &candidate, &store).is_err());

        checkout_with_two_profiles(&project);
        let mut incoherent = lock("new-release");
        incoherent.artifacts.push(ArosToolchainArtifact {
            host: "macos-aarch64".into(),
            target_profile: "arm-raspi".into(),
            target_triple: "arm-unknown-aros".into(),
            asset: "aros-toolchain-new-release-arm.tar.xz".into(),
            sha256: "c".repeat(64),
            tree_sha256: "d".repeat(64),
            llvm_version: Some("11.0.0".into()),
            size: Some(1),
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: vec!["bin/clang".into()],
        });
        write_lock(&candidate, &incoherent);
        assert!(inspect_selection(&project, &candidate, &store).is_err());
    }
}
