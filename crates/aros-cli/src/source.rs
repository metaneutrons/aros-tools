//! Transactional source-checkout creation and verified upstream synchronization.

mod gitmodules;
mod repository_security;
mod transpiler_context;

use crate::{build_tools, observability, repo};
use aros_common::{CommitState, DiagnosticCode, DiagnosticStage, PublicationFailureClass};
use gitmodules::direct_submodules;
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use repository_security::{
    reject_network_config, repository_semantics, verify_expected_upstream, verify_remote_urls,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GIT_OPERATION_TIMEOUT: Duration = Duration::from_mins(30);
const TRANSPILER_TIMEOUT: Duration = Duration::from_mins(10);

/// Canonical upstream repository used unless the caller explicitly selects a
/// different, equally reviewed URL.
pub const DEFAULT_UPSTREAM_URL: &str = "https://github.com/aros-development-team/AROS.git";

const SOURCE_INPUT: observability::ErrorBoundary = observability::ErrorBoundary {
    code: DiagnosticCode::CliSourceInput,
    stage: DiagnosticStage::Configuration,
    hint: "use an explicit HTTPS, SSH, or local source and an unambiguous Git ref",
};
const SOURCE_TRANSPORT: observability::ErrorBoundary = observability::ErrorBoundary {
    code: DiagnosticCode::CliSourceTransport,
    stage: DiagnosticStage::NetworkTransfer,
    hint: "verify the reviewed source URL, non-interactive SSH access, and Git transport policy",
};
const SOURCE_LOCK: observability::ErrorBoundary = observability::ErrorBoundary {
    code: DiagnosticCode::CliSourceLock,
    stage: DiagnosticStage::RepositoryDiscovery,
    hint: "wait for the other source operation to finish; the persistent owner record is protected by a kernel lock and must not be deleted",
};
const SOURCE_STATE: observability::ErrorBoundary = observability::ErrorBoundary {
    code: DiagnosticCode::CliSourceState,
    stage: DiagnosticStage::RepositoryDiscovery,
    hint: "keep the attached branch, index, worktree, and recursive submodules clean, then retry",
};
const SOURCE_VALIDATION: observability::ErrorBoundary = observability::ErrorBoundary {
    code: DiagnosticCode::CliSourceValidation,
    stage: DiagnosticStage::GraphValidation,
    hint: "inspect the isolated candidate failure; the source branch has not been advanced",
};
const SOURCE_PUBLICATION: observability::ErrorBoundary = observability::ErrorBoundary {
    code: DiagnosticCode::CliSourcePublication,
    stage: DiagnosticStage::Publication,
    hint: "inspect the reported destination or checkout state; existing user data was not forcibly reset",
};

/// Fully explicit source-checkout creation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    pub destination: PathBuf,
    pub upstream_url: String,
    pub origin_url: Option<String>,
    pub source_ref: Option<String>,
}

/// A source mutation that has crossed its publication boundary. Rendering an
/// outcome must never make the already committed operation look uncommitted.
#[derive(Debug)]
struct CommittedOutcome {
    lines: Vec<String>,
    warnings: Vec<String>,
}

impl CommittedOutcome {
    fn emit(self) -> Result<()> {
        let mut rendered = self.lines.join("\n");
        for warning in self.warnings {
            rendered.push_str("\n  ⚠ ");
            rendered.push_str(&warning);
        }
        rendered.push('\n');
        observability::classify(
            observability::commit_state(
                aros_common::write_stdout(&rendered).into_diagnostic(),
                CommitState::Committed,
                "source operation committed successfully, but its final status could not be written",
            ),
            SOURCE_PUBLICATION,
            "committed source operation reporting failed",
        )
    }
}

fn progress(message: &str) -> Result<()> {
    aros_common::write_stdout(&format!("{message}\n"))
        .into_diagnostic()
        .wrap_err("could not write source-operation progress")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportKind {
    Https,
    Ssh,
    Local,
}

impl TransportKind {
    const fn allows_file(self) -> bool {
        matches!(self, Self::Local)
    }
}

/// Create a new checkout in sibling staging and publish it with an atomic,
/// no-replace rename only after every invariant has passed.
pub fn initialize(options: &InitOptions) -> Result<()> {
    let upstream_transport = observability::classify(
        validate_url(&options.upstream_url, "upstream"),
        SOURCE_INPUT,
        "invalid source initialization input",
    )?;
    let origin_url = options
        .origin_url
        .as_deref()
        .unwrap_or(&options.upstream_url);
    let origin_transport = observability::classify(
        validate_url(origin_url, "fork/origin"),
        SOURCE_INPUT,
        "invalid source initialization input",
    )?;
    let effective_origin = observability::classify(
        transport_argument(origin_url, origin_transport),
        SOURCE_INPUT,
        "local source resolution failed",
    )?;
    let effective_upstream = observability::classify(
        transport_argument(&options.upstream_url, upstream_transport),
        SOURCE_INPUT,
        "local source resolution failed",
    )?;
    if let Some(source_ref) = &options.source_ref {
        observability::classify(
            validate_init_ref(source_ref),
            SOURCE_INPUT,
            "invalid source initialization input",
        )?;
    }
    let destination = observability::classify(
        new_destination(&options.destination),
        SOURCE_PUBLICATION,
        "source destination preflight failed",
    )?;
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("source destination has no parent directory"))?;
    let staging = observability::classify(
        tempfile::Builder::new()
            .prefix(".aros-source-init-")
            .tempdir_in(parent)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "could not create source staging directory below '{}'",
                    parent.display()
                )
            }),
        SOURCE_PUBLICATION,
        "source staging failed",
    )?;
    // The durable publication primitive requires the prepared tree to be a
    // direct sibling of its destination. Git accepts an existing empty
    // directory as the clone target, so the TempDir itself is the checkout.
    let checkout = staging.path().to_path_buf();

    let operation = (|| {
        progress("⬇️  Cloning AROS into isolated sibling staging...")?;
        let mut clone = git_command(origin_transport);
        clone
            .current_dir(parent)
            .env("GIT_CEILING_DIRECTORIES", parent);
        clone.args([
            "clone",
            "--origin",
            "origin",
            "--no-recurse-submodules",
            "--",
        ]);
        clone.arg(&effective_origin).arg(&checkout);
        observability::classify(
            run_git(&mut clone, "Git clone of the selected AROS origin"),
            SOURCE_TRANSPORT,
            "source clone failed",
        )?;

        let mut add_upstream = git_at(&checkout, TransportKind::Local);
        add_upstream.args(["remote", "add", "upstream", &effective_upstream]);
        observability::classify(
            run_git(
                &mut add_upstream,
                "configuration of the reviewed upstream remote",
            ),
            SOURCE_STATE,
            "source remote configuration failed",
        )?;

        let selected = if let Some(source_ref) = &options.source_ref {
            let (run_ref, oid) = observability::classify(
                fetch_to_run_ref(
                    &checkout,
                    &options.upstream_url,
                    source_ref,
                    upstream_transport,
                ),
                SOURCE_TRANSPORT,
                "selected source ref fetch failed",
            )?;
            let checkout_result = (|| {
                let mut checkout_command = git_at(&checkout, TransportKind::Local);
                checkout_command.args(["checkout", "--detach", &oid]);
                run_git(
                    &mut checkout_command,
                    "checkout of the exact selected source commit",
                )?;
                Ok(oid)
            })();
            finish_run_ref(&checkout, &run_ref, checkout_result)?
        } else {
            git_capture(
                &checkout,
                ["rev-parse", "--verify", "HEAD^{commit}"],
                "new checkout commit",
            )?
        };

        let submodule_transport = if options.source_ref.is_some() {
            upstream_transport
        } else {
            origin_transport
        };
        let submodule_origin = if options.source_ref.is_some() {
            effective_upstream.as_str()
        } else {
            effective_origin.as_str()
        };
        observability::classify(
            initialize_submodules_validated(&checkout, submodule_transport, Some(submodule_origin)),
            SOURCE_TRANSPORT,
            "source submodule initialization failed",
        )?;
        if !repo::is_repo_root(&checkout) {
            return observability::classify(
                Err(miette::miette!(
                    "the selected repository does not have the required AROS source layout (configure, Makefile.in, arch/, compiler/, rom/)"
                )),
                SOURCE_VALIDATION,
                "source layout validation failed",
            );
        }
        observability::classify(
            ensure_clean(&checkout),
            SOURCE_VALIDATION,
            "new source checkout validation failed",
        )?;
        verify_remote_urls(&checkout, &effective_origin, &effective_upstream)?;
        let head = git_capture(
            &checkout,
            ["rev-parse", "--verify", "HEAD^{commit}"],
            "new checkout commit",
        )?;
        if head != selected {
            bail!("selected source commit changed during initialization");
        }

        observability::classify(
            ensure_destination_absent(&destination)
                .and_then(|()| publish_source_checkout(&checkout, &destination)),
            SOURCE_PUBLICATION,
            "source checkout publication failed",
        )?;
        Ok(head)
    })();

    let cleanup = staging
        .close()
        .into_diagnostic()
        .wrap_err("could not remove the source initialization staging directory");
    let (head, cleanup_warning) =
        finish_committed_operation(operation, cleanup, "source initialization cleanup")?;
    let mut lines = vec![
        format!(
            "✅ AROS source checkout created at {}",
            destination.display()
        ),
        format!("  • Commit:   {head}"),
        "  • Origin:   configured".to_owned(),
        "  • Upstream: configured and reviewed".to_owned(),
    ];
    if options.source_ref.is_some() {
        lines.push("  • State:    detached at the exact commit resolved for --ref".to_owned());
    }
    CommittedOutcome {
        lines,
        warnings: cleanup_warning.into_iter().collect(),
    }
    .emit()
}

/// Synchronize a clean attached branch from one reviewed upstream branch.
/// Candidate validation uses a standalone repository; publication uses a
/// compare-and-swap ref update and never invokes `reset --hard`.
pub fn sync(
    repo_root: &Path,
    expected_upstream_url: &str,
    upstream_branch: &str,
    transpile: bool,
) -> Result<()> {
    let transport = observability::classify(
        validate_url(expected_upstream_url, "expected upstream"),
        SOURCE_INPUT,
        "invalid source synchronization input",
    )?;
    observability::classify(
        validate_branch(upstream_branch, "upstream branch"),
        SOURCE_INPUT,
        "invalid source synchronization input",
    )?;
    observability::classify(
        ensure_git_root(repo_root),
        SOURCE_STATE,
        "source repository preflight failed",
    )?;
    let lock = observability::classify(
        RepoLock::acquire(repo_root),
        SOURCE_LOCK,
        "source repository lock acquisition failed",
    )?;
    let result = sync_locked(
        repo_root,
        expected_upstream_url,
        upstream_branch,
        transpile,
        transport,
        &lock,
    );
    lock.finish(result)
}

fn sync_locked(
    repo_root: &Path,
    expected_upstream_url: &str,
    upstream_branch: &str,
    transpile: bool,
    transport: TransportKind,
    lock: &RepoLock,
) -> Result<()> {
    observability::classify(
        ensure_clean(repo_root)
            .and_then(|()| reject_network_config(repo_root))
            .and_then(|()| verify_expected_upstream(repo_root, expected_upstream_url, transport)),
        SOURCE_STATE,
        "source checkout state preflight failed",
    )?;
    let branch_ref = git_capture(
        repo_root,
        ["symbolic-ref", "--quiet", "HEAD"],
        "current Git branch",
    )
    .map_err(|error| {
        error.wrap_err("upstream synchronization requires an attached local branch")
    })?;
    let branch = branch_ref
        .strip_prefix("refs/heads/")
        .ok_or_else(|| miette::miette!("HEAD is not attached to a local branch"))?
        .to_owned();
    validate_branch(&branch, "current branch")?;
    let snapshot = observability::classify(
        SyncSnapshot::capture(repo_root, branch_ref, branch),
        SOURCE_STATE,
        "source checkout snapshot failed",
    )?;

    let requested_ref = format!("refs/heads/{upstream_branch}");
    progress(&format!(
        "🔄 Fetching reviewed upstream branch {upstream_branch}..."
    ))?;
    let (run_ref, upstream_oid) = observability::classify(
        fetch_to_run_ref(repo_root, expected_upstream_url, &requested_ref, transport),
        SOURCE_TRANSPORT,
        "upstream branch fetch failed",
    )?;

    let operation = (|| {
        let relation = observability::classify(
            classify_relation(repo_root, &snapshot.head, &upstream_oid),
            SOURCE_STATE,
            "upstream relation validation failed",
        )?;
        let final_head = match relation {
            Relation::Behind => upstream_oid.clone(),
            Relation::Equal | Relation::Ahead => snapshot.head.clone(),
        };
        let candidate = observability::classify(
            validate_candidate(
                repo_root,
                &final_head,
                expected_upstream_url,
                transport,
                transpile,
            ),
            SOURCE_VALIDATION,
            "standalone source candidate validation failed",
        )?;
        let graphs = candidate.graphs;
        let publication = if relation == Relation::Behind {
            observability::classify(
                snapshot
                    .verify(repo_root)
                    .and_then(|()| lock.verify_identity())
                    .and_then(|()| {
                        apply_fast_forward(repo_root, &upstream_oid, &snapshot, &candidate.checkout)
                    }),
                SOURCE_PUBLICATION,
                "validated source fast-forward publication failed",
            )
        } else {
            Ok(())
        };
        let cleanup = candidate.close();
        let ((), cleanup_warning) =
            finish_committed_operation(publication, cleanup, "standalone candidate cleanup")?;
        Ok((relation, final_head, graphs, cleanup_warning))
    })();
    let cleanup = cleanup_run_ref(repo_root, &run_ref);
    let ((relation, final_head, graphs, candidate_cleanup_warning), run_ref_cleanup_warning) =
        finish_committed_operation(operation, cleanup, "run-owned ref cleanup")?;
    let change = match relation {
        Relation::Behind => "compare-and-swap fast-forward after validation",
        Relation::Equal => "already at the fetched upstream commit",
        Relation::Ahead => "local branch already contains the fetched upstream commit",
    };
    let graph_status = if transpile {
        format!("{graphs} declared target profile(s) validated in isolation")
    } else {
        "skipped explicitly with --no-transpile".to_owned()
    };
    CommittedOutcome {
        lines: vec![
            "✅ AROS source synchronization complete".to_owned(),
            format!("  • Branch:   {}", snapshot.branch),
            format!("  • Commit:   {final_head}"),
            format!("  • Upstream: refs/heads/{upstream_branch} ({upstream_oid})"),
            format!("  • Change:   {change}"),
            format!("  • Graphs:   {graph_status}"),
        ],
        warnings: candidate_cleanup_warning
            .into_iter()
            .chain(run_ref_cleanup_warning)
            .collect(),
    }
    .emit()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    Equal,
    Ahead,
    Behind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncSnapshot {
    branch_ref: String,
    branch: String,
    head: String,
    index_tree: String,
    submodules: String,
    repository_semantics: Vec<u8>,
}

impl SyncSnapshot {
    fn capture(repo_root: &Path, branch_ref: String, branch: String) -> Result<Self> {
        let head = git_capture(repo_root, ["rev-parse", "HEAD^{commit}"], "local commit")?;
        let index_tree = git_capture(repo_root, ["write-tree"], "Git index snapshot")?;
        let submodules = submodule_state(repo_root)?;
        ensure_submodules_ready(&submodules)?;
        ensure_submodule_worktrees_clean(repo_root)?;
        let repository_semantics = repository_semantics(repo_root)?;
        Ok(Self {
            branch_ref,
            branch,
            head,
            index_tree,
            submodules,
            repository_semantics,
        })
    }

    fn verify(&self, repo_root: &Path) -> Result<()> {
        ensure_clean(repo_root)?;
        let branch_ref = git_capture(
            repo_root,
            ["symbolic-ref", "--quiet", "HEAD"],
            "current Git branch",
        )?;
        let head = git_capture(repo_root, ["rev-parse", "HEAD^{commit}"], "local commit")?;
        let index_tree = git_capture(repo_root, ["write-tree"], "Git index state")?;
        let submodules = submodule_state(repo_root)?;
        ensure_submodule_worktrees_clean(repo_root)?;
        let repository_semantics = repository_semantics(repo_root)?;
        if branch_ref != self.branch_ref
            || head != self.head
            || index_tree != self.index_tree
            || submodules != self.submodules
            || repository_semantics != self.repository_semantics
        {
            bail!("the branch, index, worktree, or submodule state changed after the synchronization snapshot");
        }
        Ok(())
    }
}

fn classify_relation(repo_root: &Path, head: &str, upstream: &str) -> Result<Relation> {
    let head_is_ancestor = git_is_ancestor(repo_root, head, upstream)?;
    let upstream_is_ancestor = git_is_ancestor(repo_root, upstream, head)?;
    match (head == upstream, head_is_ancestor, upstream_is_ancestor) {
        (true, _, _) => Ok(Relation::Equal),
        (false, true, false) => Ok(Relation::Behind),
        (false, false, true) => Ok(Relation::Ahead),
        _ => bail!(
            "the current branch and fetched upstream branch have diverged; no merge was attempted. Integrate upstream through a reviewed branch or pull request"
        ),
    }
}

fn apply_fast_forward(
    repo_root: &Path,
    expected_head: &str,
    snapshot: &SyncSnapshot,
    candidate_checkout: &Path,
) -> Result<()> {
    prefetch_existing_submodule_objects(repo_root, candidate_checkout)?;
    let initialized_submodules = initialized_submodule_paths(repo_root)?;
    let mut cas = git_at(repo_root, TransportKind::Local);
    cas.args([
        "update-ref",
        "--no-deref",
        &snapshot.branch_ref,
        expected_head,
        &snapshot.head,
    ]);
    run_git(
        &mut cas,
        "compare-and-swap update of the validated source branch",
    )?;

    let publication = (|| {
        let mut materialize = git_at(repo_root, TransportKind::Local);
        materialize.args(["read-tree", "-u", "-m", &snapshot.head, expected_head]);
        run_git(
            &mut materialize,
            "non-destructive materialization of the fast-forwarded source tree",
        )?;
        materialize_submodules_from_candidate(repo_root, candidate_checkout)?;
        ensure_clean(repo_root)?;
        let branch_ref = git_capture(
            repo_root,
            ["symbolic-ref", "--quiet", "HEAD"],
            "fast-forwarded Git branch",
        )?;
        let head = git_capture(
            repo_root,
            ["rev-parse", "HEAD^{commit}"],
            "fast-forwarded commit",
        )?;
        let submodules = submodule_state(repo_root)?;
        ensure_submodules_ready(&submodules)?;
        ensure_submodule_worktrees_clean(repo_root)?;
        let expected_tree = git_capture(
            candidate_checkout,
            ["rev-parse", "HEAD^{tree}"],
            "validated candidate tree",
        )?;
        let materialized_tree = git_capture(
            repo_root,
            ["rev-parse", "HEAD^{tree}"],
            "materialized source tree",
        )?;
        if branch_ref != snapshot.branch_ref
            || head != expected_head
            || materialized_tree != expected_tree
        {
            bail!("the completed fast-forward did not retain the expected branch and exact commit");
        }
        Ok(())
    })();
    if let Err(error) = publication {
        let rollback = rollback_fast_forward(
            repo_root,
            expected_head,
            snapshot,
            candidate_checkout,
            &initialized_submodules,
        );
        return match rollback {
            Ok(()) => observability::commit_state(
                Err(error),
                CommitState::RolledBack,
                "source publication failed after branch compare-and-swap; the branch, index, and submodule snapshot was restored without overwriting concurrent working-tree data",
            ),
            Err(rollback_error) => observability::commit_state(
                Err(miette::miette!(
                    "source publication crossed the branch boundary but publication and safe rollback both failed. Publication: {}. Rollback: {}",
                    render_report(&error),
                    render_report(&rollback_error)
                )),
                CommitState::Indeterminate,
                "source publication state could not be proven",
            ),
        };
    }
    Ok(())
}

fn initialized_submodule_paths(repo_root: &Path) -> Result<BTreeSet<PathBuf>> {
    fn collect(repository: &Path, relative: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<()> {
        for entry in direct_submodules(repository)? {
            let child_relative = relative.join(&entry.path);
            let child = repository.join(&entry.path);
            if child.join(".git").exists() {
                paths.insert(child_relative.clone());
                collect(&child, &child_relative, paths)?;
            }
        }
        Ok(())
    }

    let mut paths = BTreeSet::new();
    collect(repo_root, Path::new(""), &mut paths)?;
    Ok(paths)
}

fn prefetch_existing_submodule_objects(target: &Path, candidate: &Path) -> Result<()> {
    for entry in direct_submodules(candidate)? {
        let candidate_child = candidate.join(&entry.path);
        let target_child = target.join(&entry.path);
        if !target_child.join(".git").exists() {
            continue;
        }
        let oid = git_capture(
            &candidate_child,
            ["rev-parse", "--verify", "HEAD^{commit}"],
            "candidate submodule commit",
        )?;
        let run_ref = run_ref_name("submodule-prefetch")?;
        let refspec = format!("{oid}:{run_ref}");
        let mut fetch = git_at_internal(&target_child);
        fetch.args([
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--",
        ]);
        fetch.arg(&candidate_child).arg(&refspec);
        let operation = run_git(&mut fetch, "candidate-backed submodule object prefetch");
        finish_run_ref(&target_child, &run_ref, operation)?;
        prefetch_existing_submodule_objects(&target_child, &candidate_child)?;
    }
    Ok(())
}

fn materialize_submodules_from_candidate(target: &Path, candidate: &Path) -> Result<()> {
    let entries = direct_submodules(candidate)?;
    if entries.is_empty() {
        return Ok(());
    }
    let mut update = git_at_internal(target);
    let previously_initialized = entries
        .iter()
        .map(|entry| {
            (
                entry.path.clone(),
                target.join(&entry.path).join(".git").exists(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for entry in &entries {
        let candidate_child = candidate.join(&entry.path);
        update.arg("-c").arg(format!(
            "submodule.{}.url={}",
            entry.name,
            path_argument(&candidate_child)?
        ));
    }
    update.args(["submodule", "update", "--init", "--no-fetch", "--"]);
    update.args(entries.iter().map(|entry| entry.path.as_os_str()));
    run_git(
        &mut update,
        "candidate-backed, network-free submodule materialization",
    )?;

    for entry in entries {
        let candidate_child = candidate.join(&entry.path);
        let target_child = target.join(&entry.path);
        let expected_oid = git_capture(
            &candidate_child,
            ["rev-parse", "--verify", "HEAD^{commit}"],
            "candidate submodule commit",
        )?;
        let actual_oid = git_capture(
            &target_child,
            ["rev-parse", "--verify", "HEAD^{commit}"],
            "materialized submodule commit",
        )?;
        if actual_oid != expected_oid {
            bail!(
                "submodule '{}' materialized commit {}, expected {}",
                entry.path.display(),
                actual_oid,
                expected_oid
            );
        }
        let reviewed_origin = git_capture(
            &candidate_child,
            ["remote", "get-url", "origin"],
            "candidate submodule origin",
        )?;
        if !previously_initialized[&entry.path] {
            let mut parent_config = git_at(target, TransportKind::Local);
            parent_config.args([
                "config",
                "--local",
                "--replace-all",
                &format!("submodule.{}.url", entry.name),
                &reviewed_origin,
            ]);
            run_git(
                &mut parent_config,
                "publication of the reviewed submodule URL",
            )?;
            let mut child_origin = git_at(&target_child, TransportKind::Local);
            child_origin.args(["remote", "set-url", "origin", &reviewed_origin]);
            run_git(
                &mut child_origin,
                "replacement of the temporary candidate submodule origin",
            )?;
        }
        materialize_submodules_from_candidate(&target_child, &candidate_child)?;
    }
    Ok(())
}

fn rollback_fast_forward(
    repo_root: &Path,
    expected_head: &str,
    snapshot: &SyncSnapshot,
    candidate: &Path,
    initialized_submodules: &BTreeSet<PathBuf>,
) -> Result<()> {
    let branch_ref = git_capture(
        repo_root,
        ["symbolic-ref", "--quiet", "HEAD"],
        "branch before source rollback",
    )?;
    let head = git_capture(
        repo_root,
        ["rev-parse", "HEAD^{commit}"],
        "commit before source rollback",
    )?;
    if branch_ref != snapshot.branch_ref || head != expected_head {
        bail!("source rollback refused because the branch changed concurrently");
    }
    deinitialize_new_submodules(repo_root, candidate, initialized_submodules)?;
    let index_tree = git_capture(repo_root, ["write-tree"], "index before source rollback")?;
    if index_tree != snapshot.index_tree {
        let mut tree = git_at(repo_root, TransportKind::Local);
        tree.args(["read-tree", "-u", "-m", expected_head, &snapshot.head]);
        run_git(&mut tree, "non-forcing source-tree rollback")?;
    }
    let mut rollback_ref = git_at(repo_root, TransportKind::Local);
    rollback_ref.args([
        "update-ref",
        "--no-deref",
        &snapshot.branch_ref,
        &snapshot.head,
        expected_head,
    ]);
    run_git(&mut rollback_ref, "compare-and-swap source-ref rollback")?;
    let mut submodules = git_at_internal(repo_root);
    submodules.args(["submodule", "update", "--init", "--recursive", "--no-fetch"]);
    run_git(&mut submodules, "network-free source-submodule rollback")?;
    let restored_branch = git_capture(
        repo_root,
        ["symbolic-ref", "--quiet", "HEAD"],
        "restored source branch",
    )?;
    let restored_head = git_capture(
        repo_root,
        ["rev-parse", "HEAD^{commit}"],
        "restored source commit",
    )?;
    let restored_index = git_capture(repo_root, ["write-tree"], "restored source index")?;
    let restored_submodules = submodule_state(repo_root)?;
    let restored_semantics = repository_semantics(repo_root)?;
    if restored_branch != snapshot.branch_ref
        || restored_head != snapshot.head
        || restored_index != snapshot.index_tree
        || restored_submodules != snapshot.submodules
        || restored_semantics != snapshot.repository_semantics
    {
        bail!("source rollback did not restore the recorded branch, index, submodule, and repository-semantic state");
    }
    Ok(())
}

fn deinitialize_new_submodules(
    target: &Path,
    candidate: &Path,
    initialized_submodules: &BTreeSet<PathBuf>,
) -> Result<()> {
    fn collect(
        candidate: &Path,
        relative: &Path,
        entries: &mut Vec<(PathBuf, PathBuf)>,
    ) -> Result<()> {
        for entry in direct_submodules(candidate)? {
            let child_relative = relative.join(&entry.path);
            entries.push((relative.to_path_buf(), entry.path.clone()));
            collect(&candidate.join(&entry.path), &child_relative, entries)?;
        }
        Ok(())
    }

    let mut entries = Vec::new();
    collect(candidate, Path::new(""), &mut entries)?;
    entries
        .sort_by_key(|(parent, child)| std::cmp::Reverse(parent.join(child).components().count()));
    for (parent, child) in entries {
        let relative = parent.join(&child);
        if initialized_submodules.contains(&relative)
            || !target.join(&relative).join(".git").exists()
        {
            continue;
        }
        let mut deinit = git_at_internal(&target.join(&parent));
        deinit.args(["submodule", "deinit", "--"]);
        deinit.arg(&child);
        run_git(
            &mut deinit,
            "non-forcing cleanup of a newly materialized submodule",
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct CandidateValidation {
    temporary: tempfile::TempDir,
    checkout: PathBuf,
    graphs: usize,
}

impl CandidateValidation {
    fn close(self) -> Result<()> {
        self.temporary
            .close()
            .into_diagnostic()
            .wrap_err("could not remove the standalone source candidate repository")
    }
}

fn validate_candidate(
    repo_root: &Path,
    commit: &str,
    upstream_url: &str,
    transport: TransportKind,
    transpile: bool,
) -> Result<CandidateValidation> {
    let temporary = tempfile::Builder::new()
        .prefix("aros-sync-candidate-")
        .tempdir()
        .into_diagnostic()
        .wrap_err("could not create standalone sync candidate directory")?;
    let checkout = temporary.path().join("checkout");
    let validation = (|| {
        let mut init = git_command(TransportKind::Local);
        init.arg("init").arg(&checkout);
        run_git(
            &mut init,
            "initialization of the standalone candidate repository",
        )?;

        let candidate_ref = run_ref_name("candidate")?;
        let refspec = format!("{commit}:{candidate_ref}");
        let mut fetch = git_at(&checkout, TransportKind::Local);
        fetch.args(["fetch", "--no-tags", "--no-recurse-submodules", "--"]);
        fetch.arg(repo_root).arg(&refspec);
        run_git(
            &mut fetch,
            "copy of the exact fetched commit into the standalone candidate",
        )?;
        let candidate_oid = git_capture(
            &checkout,
            [
                "rev-parse",
                "--verify",
                &format!("{candidate_ref}^{{commit}}"),
            ],
            "standalone candidate commit",
        )?;
        if candidate_oid != commit {
            bail!("standalone candidate did not resolve to the exact fetched commit");
        }
        let mut checkout_commit = git_at(&checkout, TransportKind::Local);
        checkout_commit.args(["checkout", "--detach", commit]);
        run_git(
            &mut checkout_commit,
            "checkout of the standalone candidate commit",
        )?;
        let effective_upstream = transport_argument(upstream_url, transport)?;
        let mut origin = git_at(&checkout, TransportKind::Local);
        origin.args(["remote", "add", "origin", &effective_upstream]);
        run_git(&mut origin, "standalone candidate upstream configuration")?;
        initialize_submodules_validated(&checkout, transport, None)
            .wrap_err("standalone candidate submodule validation")?;
        if !repo::is_repo_root(&checkout) {
            bail!("the upstream candidate no longer has the required AROS source layout");
        }
        if !transpile {
            return Ok(0);
        }
        validate_target_graphs(repo_root, &checkout, temporary.path())
    })();
    match validation {
        Ok(graphs) => Ok(CandidateValidation {
            temporary,
            checkout,
            graphs,
        }),
        Err(error) => {
            let cleanup = temporary
                .close()
                .into_diagnostic()
                .wrap_err("could not remove the standalone source candidate repository");
            combine_operation_cleanup(Err(error), cleanup, "standalone candidate cleanup")
        }
    }
}

fn validate_target_graphs(repo_root: &Path, checkout: &Path, output_root: &Path) -> Result<usize> {
    let tools = build_tools::ensure(repo_root)?;
    let transpiler = tools.bin_dir.join(executable_name("aros-transpiler"));
    let profiles = repo::load_target_profiles(checkout).wrap_err(
        "candidate graph validation requires checkout-owned aros-targets.toml; use --no-transpile only when intentionally synchronizing pristine upstream",
    )?;
    let outputs = output_root.join("graphs");
    fs::create_dir_all(&outputs)
        .into_diagnostic()
        .wrap_err("could not create isolated target-graph output directory")?;
    for profile in &profiles {
        let profile_dir = outputs.join(&profile.name);
        fs::create_dir_all(&profile_dir).into_diagnostic()?;
        let graph = profile_dir.join("generated_targets.cmake");
        let architecture = profile.arch.to_string();
        let context = transpiler_context::resolve(checkout, profile)?;
        let mut command = Command::new(&transpiler);
        command
            .arg("--source-dir")
            .arg(checkout)
            .arg("--output")
            .arg(&graph)
            .arg("--ports-dir")
            .arg(profile_dir.join("Ports"))
            .args(["--cpu", architecture.as_str()])
            .args(["--platform", &profile.platform])
            .args(["--family", &context.family])
            .args(["--variant", &context.variant])
            .args(["--toolchain", &context.toolchain])
            .args(["--cpu32", &context.cpu32])
            .args(["--use-mmu", &context.use_mmu])
            .args(["--float-abi", &context.float_abi]);
        observability::capture_exit_code_with_timeout(
            &mut command,
            &format!("target-graph validation for profile '{}'", profile.name),
            TRANSPILER_TIMEOUT,
            &[0],
        )?;
        if !graph.is_file() {
            bail!(
                "target-graph validation for profile '{}' returned success without publishing '{}'",
                profile.name,
                graph.display()
            );
        }
    }
    Ok(profiles.len())
}

fn fetch_to_run_ref(
    repo_root: &Path,
    upstream_url: &str,
    requested_ref: &str,
    transport: TransportKind,
) -> Result<(String, String)> {
    let run_ref = run_ref_name("fetch")?;
    let quarantine = tempfile::Builder::new()
        .prefix("aros-source-fetch-")
        .tempdir()
        .into_diagnostic()
        .wrap_err("could not create isolated source-fetch repository")?;
    let bare = quarantine.path().join("objects.git");
    let operation = (|| {
        let mut init = git_command(TransportKind::Local);
        init.args(["init", "--bare", "--initial-branch=aros-fetch"]);
        init.arg(&bare);
        run_git(
            &mut init,
            "initialization of isolated source-fetch repository",
        )?;

        let quarantine_ref = run_ref_name("quarantine")?;
        let external_refspec = format!("+{requested_ref}:{quarantine_ref}");
        let mut fetch = git_at(&bare, transport);
        fetch.args([
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--",
        ]);
        let effective_upstream = transport_argument(upstream_url, transport)?;
        fetch.arg(&effective_upstream).arg(&external_refspec);
        run_git(&mut fetch, "fetch into an isolated source quarantine")?;
        let oid = git_capture(
            &bare,
            [
                "rev-parse",
                "--verify",
                &format!("{quarantine_ref}^{{commit}}"),
            ],
            "exact quarantined source commit",
        )?;

        let import_refspec = format!("+{quarantine_ref}:{run_ref}");
        let mut import = git_at_internal(repo_root);
        import.args([
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--",
        ]);
        import.arg(&bare).arg(&import_refspec);
        run_git(&mut import, "import of the exact quarantined source commit")?;
        let imported = git_capture(
            repo_root,
            ["rev-parse", "--verify", &format!("{run_ref}^{{commit}}")],
            "exact imported source commit",
        )?;
        if imported != oid {
            bail!("imported source commit differs from the quarantined commit");
        }
        Ok(oid)
    })();
    let cleanup = quarantine
        .close()
        .into_diagnostic()
        .wrap_err("could not remove isolated source-fetch repository");
    let operation =
        combine_operation_cleanup(operation, cleanup, "source-fetch quarantine cleanup");
    match operation {
        Ok(oid) => Ok((run_ref, oid)),
        Err(error) => {
            let mut delete = git_at(repo_root, TransportKind::Local);
            delete.args(["update-ref", "-d", &run_ref]);
            combine_operation_cleanup(
                Err(error),
                run_git(&mut delete, "cleanup after failed source fetch"),
                "failed-fetch ref cleanup",
            )
        }
    }
}

fn finish_run_ref<T>(repo_root: &Path, run_ref: &str, operation: Result<T>) -> Result<T> {
    let cleanup = cleanup_run_ref(repo_root, run_ref);
    combine_operation_cleanup(operation, cleanup, "run-owned ref cleanup")
}

fn cleanup_run_ref(repo_root: &Path, run_ref: &str) -> Result<()> {
    let mut delete = git_at(repo_root, TransportKind::Local);
    delete.args(["update-ref", "-d", run_ref]);
    run_git(&mut delete, "cleanup of the run-owned source ref")
}

fn run_ref_name(kind: &str) -> Result<String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .into_diagnostic()
        .wrap_err("system clock is before the Unix epoch")?
        .as_nanos();
    Ok(format!(
        "refs/aros-tools/source-{kind}/{}-{nanos}",
        std::process::id()
    ))
}

fn combine_operation_cleanup<T>(
    operation: Result<T>,
    cleanup: Result<()>,
    cleanup_label: &str,
) -> Result<T> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error).wrap_err(cleanup_label.to_owned()),
        (Err(operation_error), Err(cleanup_error)) => Err(operation_error).wrap_err(format!(
            "{cleanup_label} also failed: {}",
            render_report(&cleanup_error)
        )),
    }
}

/// Finish cleanup after a publication boundary without turning a committed
/// success into an apparent operation failure. Cleanup remains visible as a
/// warning; a primary operation failure retains its typed diagnostic context.
fn finish_committed_operation<T>(
    operation: Result<T>,
    cleanup: Result<()>,
    cleanup_label: &str,
) -> Result<(T, Option<String>)> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok((value, None)),
        (Ok(value), Err(cleanup_error)) => Ok((
            value,
            Some(format!(
                "{cleanup_label} failed after the operation committed: {}",
                render_report(&cleanup_error)
            )),
        )),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Err(operation_error), Err(cleanup_error)) => Err(operation_error).wrap_err(format!(
            "{cleanup_label} also failed: {}",
            render_report(&cleanup_error)
        )),
    }
}

fn render_report(error: &miette::Report) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        source = cause.source();
    }
    messages.join(": ")
}

struct RepoLock {
    path: PathBuf,
    file: File,
    identity: (u64, u64),
    owner_record: Vec<u8>,
}

impl RepoLock {
    fn acquire(repo_root: &Path) -> Result<Self> {
        let common = git_capture(
            repo_root,
            ["rev-parse", "--git-common-dir"],
            "Git common directory",
        )?;
        let common = PathBuf::from(common);
        let common = if common.is_absolute() {
            common
        } else {
            repo_root.join(common)
        };
        let path = common.join("aros-tools-source.lock");
        #[cfg(not(unix))]
        bail!("source synchronization locking is not supported on this platform");
        #[cfg(unix)]
        {
            use rustix::fs::{flock, open, FlockOperation, Mode, OFlags};

            let descriptor = open(
                &path,
                OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|error| {
                miette::miette!(
                    "could not open repository-wide source lock '{}': {error}",
                    path.display()
                )
            })?;
            flock(&descriptor, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
                miette::miette!(
                    "another aros source operation holds '{}': {error}",
                    path.display()
                )
            })?;
            let mut file = File::from(descriptor);
            let identity = lock_file_identity(&file)?;
            let owner_record = format!(
                "pid={}\nstarted_unix_nanos={}\n",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .into_diagnostic()?
                    .as_nanos()
            )
            .into_bytes();
            file.set_len(0).into_diagnostic().wrap_err_with(|| {
                format!("could not reset source lock record '{}'", path.display())
            })?;
            file.seek(SeekFrom::Start(0)).into_diagnostic()?;
            file.write_all(&owner_record).into_diagnostic()?;
            file.sync_all().into_diagnostic()?;
            Ok(Self {
                path,
                file,
                identity,
                owner_record,
            })
        }
    }

    fn verify_identity(&self) -> Result<()> {
        #[cfg(not(unix))]
        bail!("source synchronization locking is not supported on this platform");
        #[cfg(unix)]
        let (identity, contents) = {
            use rustix::fs::{open, Mode, OFlags};

            let descriptor = open(
                &self.path,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| {
                miette::miette!(
                    "could not reopen repository-wide source lock '{}': {error}",
                    self.path.display()
                )
            })?;
            let file = File::from(descriptor);
            let identity = lock_file_identity(&file)?;
            let mut contents = Vec::with_capacity(self.owner_record.len() + 1);
            file.take(u64::try_from(self.owner_record.len() + 1).unwrap_or(u64::MAX))
                .read_to_end(&mut contents)
                .into_diagnostic()?;
            (identity, contents)
        };
        if identity != self.identity || contents != self.owner_record {
            bail!(
                "repository-wide source lock '{}' was replaced or modified before publication",
                self.path.display()
            );
        }
        Ok(())
    }

    fn finish<T>(self, operation: Result<T>) -> Result<T> {
        drop(self.file);
        operation
    }
}

#[cfg(unix)]
fn lock_file_identity(file: &File) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata().into_diagnostic()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
    {
        bail!("repository-wide source lock must be a singly linked, owner-controlled regular file");
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn ensure_git_root(repo_root: &Path) -> Result<()> {
    let actual = git_capture(
        repo_root,
        ["rev-parse", "--show-toplevel"],
        "Git repository root",
    )?;
    let expected = repo_root
        .canonicalize()
        .into_diagnostic()
        .wrap_err("could not canonicalize the discovered AROS checkout")?;
    let actual = PathBuf::from(actual)
        .canonicalize()
        .into_diagnostic()
        .wrap_err("could not canonicalize the Git repository root")?;
    if actual != expected {
        bail!(
            "the discovered AROS root '{}' is not the Git worktree root '{}'",
            expected.display(),
            actual.display()
        );
    }
    Ok(())
}

fn ensure_clean(repo_root: &Path) -> Result<()> {
    let status = git_capture(
        repo_root,
        [
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
        ],
        "Git worktree cleanliness",
    )?;
    if !status.is_empty() {
        let first = status.lines().take(8).collect::<Vec<_>>().join("; ");
        bail!(
            "the AROS worktree is not clean; commit, stash, or remove these changes before synchronization: {first}"
        );
    }
    Ok(())
}

fn submodule_state(repo_root: &Path) -> Result<String> {
    let raw = git_capture(
        repo_root,
        ["submodule", "status", "--recursive"],
        "recursive AROS submodule state",
    )?;
    Ok(raw
        .lines()
        .map(|line| line.rsplit_once(" (").map_or(line, |(state, _)| state))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn ensure_submodules_ready(state: &str) -> Result<()> {
    if let Some(line) = state
        .lines()
        .find(|line| line.as_bytes().first().is_none_or(|marker| *marker != b' '))
    {
        bail!(
            "AROS submodules are not fully initialized and aligned ({line}); run `git submodule update --init --recursive` before synchronization"
        );
    }
    Ok(())
}

fn ensure_submodule_worktrees_clean(repo_root: &Path) -> Result<()> {
    let status = git_capture(
        repo_root,
        [
            "submodule",
            "foreach",
            "--quiet",
            "--recursive",
            "git status --porcelain=v1 --untracked-files=all --ignored=matching",
        ],
        "recursive AROS submodule worktree cleanliness",
    )?;
    if !status.is_empty() {
        let first = status.lines().take(8).collect::<Vec<_>>().join("; ");
        bail!(
            "an AROS submodule worktree is not clean; commit, stash, or remove these changes before synchronization: {first}"
        );
    }
    Ok(())
}

/// Initialize one submodule level at a time. Every child repository's own
/// `.gitmodules` is therefore validated before Git is allowed to contact any
/// grandchild URL.
fn initialize_submodules_validated(
    repo_root: &Path,
    transport: TransportKind,
    root_origin_override: Option<&str>,
) -> Result<()> {
    let entries = direct_submodules(repo_root)?;
    if entries.is_empty() {
        return Ok(());
    }
    let mut command = root_origin_override.map_or_else(
        || git_at(repo_root, transport),
        |origin| git_at_with_origin(repo_root, transport, origin),
    );
    command.args(["submodule", "update", "--init", "--"]);
    command.args(entries.iter().map(|entry| entry.path.as_os_str()));
    run_git(&mut command, "validated AROS submodule initialization")?;
    for entry in entries {
        let child = repo_root.join(entry.path);
        initialize_submodules_validated(&child, transport, None)?;
    }
    Ok(())
}

fn git_is_ancestor(repo_root: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let mut command = git_at(repo_root, TransportKind::Local);
    command.args(["merge-base", "--is-ancestor", ancestor, descendant]);
    match observability::capture_exit_code_with_timeout(
        &mut command,
        "inspection of the upstream branch relation",
        GIT_OPERATION_TIMEOUT,
        &[0, 1],
    )? {
        0 => Ok(true),
        1 => Ok(false),
        code => bail!("Git returned unexpected accepted relation status {code}"),
    }
}

fn git_capture<const N: usize>(
    repo_root: &Path,
    arguments: [&str; N],
    description: &str,
) -> Result<String> {
    let mut command = git_at(repo_root, TransportKind::Local);
    command.args(arguments);
    capture_git_stdout(&mut command, description)
}

fn run_git(command: &mut Command, description: &str) -> Result<()> {
    observability::capture_exit_code_with_timeout(command, description, GIT_OPERATION_TIMEOUT, &[0])
        .map(|_| ())
        .map_err(Into::into)
}

fn capture_git_stdout(command: &mut Command, description: &str) -> Result<String> {
    observability::capture_stdout_with_timeout(command, description, GIT_OPERATION_TIMEOUT)
        .map_err(Into::into)
}

fn git_at(repo_root: &Path, transport: TransportKind) -> Command {
    let mut command = git_command(transport);
    command.arg("-C").arg(repo_root);
    command
}

fn git_at_internal(repo_root: &Path) -> Command {
    let mut command = git_at(repo_root, TransportKind::Local);
    command.args([
        "-c",
        "protocol.https.allow=never",
        "-c",
        "protocol.ssh.allow=never",
        "-c",
        "protocol.file.allow=always",
    ]);
    command
}

fn git_at_with_origin(repo_root: &Path, transport: TransportKind, origin: &str) -> Command {
    let mut command = git_command(transport);
    command
        .arg("-c")
        .arg(format!("remote.origin.url={origin}"))
        .arg("-C")
        .arg(repo_root);
    command
}

fn git_command(transport: TransportKind) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", null_device())
        .env("SSH_ASKPASS", null_device())
        .env(
            "GIT_SSH_COMMAND",
            "ssh -oBatchMode=yes -oClearAllForwardings=yes",
        )
        .env("GIT_SSH_VARIANT", "ssh")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_ALLOW_PROTOCOL")
        .env_remove("GIT_PROTOCOL_FROM_USER")
        .env_remove("GIT_EXEC_PATH")
        .env_remove("GIT_TEMPLATE_DIR")
        .env_remove("GIT_PROXY_COMMAND")
        .env_remove("GIT_CURL_VERBOSE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_SHALLOW_FILE")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.interactive=never",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            if transport.allows_file() {
                "protocol.file.allow=always"
            } else {
                "protocol.file.allow=never"
            },
        ]);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_CONFIG_KEY_")
            || name.to_string_lossy().starts_with("GIT_CONFIG_VALUE_")
            || name.to_string_lossy().starts_with("GIT_TRACE")
        {
            command.env_remove(name);
        }
    }
    command
}

fn null_device() -> &'static OsStr {
    if cfg!(windows) {
        OsStr::new("NUL")
    } else {
        OsStr::new("/dev/null")
    }
}

fn new_destination(requested: &Path) -> Result<PathBuf> {
    ensure_destination_absent(requested)?;
    let name = requested.file_name().ok_or_else(|| {
        miette::miette!(
            "source destination '{}' has no final path component",
            requested.display()
        )
    })?;
    if name == "." || name == ".." {
        bail!("source destination must name a new child directory");
    }
    let parent = requested.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|error| {
        miette::miette!(
            "could not resolve source destination parent '{}': {error}. Create the parent directory first",
            parent.display()
        )
    })?;
    if !parent.is_dir() {
        bail!(
            "source destination parent '{}' is not a directory",
            parent.display()
        );
    }
    let destination = parent.join(name);
    ensure_destination_absent(&destination)?;
    Ok(destination)
}

fn ensure_destination_absent(destination: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => bail!(
            "source destination '{}' already exists; choose a new path",
            destination.display()
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .into_diagnostic()
            .wrap_err_with(|| format!("could not inspect '{}'", destination.display())),
    }
}

fn publish_source_checkout(source: &Path, destination: &Path) -> Result<()> {
    match aros_common::publish_prepared_source_tree_noclobber(source, destination) {
        Ok(_) => Ok(()),
        Err(error) => {
            let class = aros_common::publication_failure_class(&error);
            let report = Err(error).into_diagnostic().wrap_err_with(|| {
                format!(
                    "could not durably publish source checkout at '{}' without replacing existing data ({class:?})",
                    destination.display()
                )
            });
            if matches!(
                class,
                PublicationFailureClass::CommitStateUncertain
                    | PublicationFailureClass::RecoveryIncomplete
            ) {
                observability::commit_state(
                    report,
                    CommitState::Indeterminate,
                    "source checkout publication crossed or may have crossed its rename boundary, but the durable commit state could not be proven",
                )
            } else {
                observability::commit_state(
                    report,
                    CommitState::RolledBack,
                    "source checkout publication did not cross its atomic rename boundary",
                )
            }
        }
    }
}

fn validate_url(value: &str, label: &str) -> Result<TransportKind> {
    if value.trim() != value
        || value.is_empty()
        || value.chars().any(char::is_control)
        || value.starts_with('-')
        || value.contains('\\')
    {
        bail!("{label} URL is empty or contains unsafe control/option syntax");
    }
    if let Some(remainder) = value.strip_prefix("https://") {
        let authority = remainder.split('/').next().unwrap_or(remainder);
        if authority.contains('@') || !valid_network_authority(authority, false) {
            bail!("{label} HTTPS URL must have a host and must not embed credentials");
        }
        return Ok(TransportKind::Https);
    }
    if let Some(remainder) = value.strip_prefix("ssh://") {
        let authority = remainder.split('/').next().unwrap_or(remainder);
        let user = authority.rsplit_once('@').map(|(user, _)| user);
        if !valid_network_authority(authority, true)
            || user.is_some_and(|identity| !valid_ssh_identity(identity))
        {
            bail!("{label} SSH URL must have a host and must not embed a password");
        }
        return Ok(TransportKind::Ssh);
    }
    if let Some(path) = value.strip_prefix("file://") {
        if path.is_empty() || path.contains('@') || !Path::new(path).is_absolute() {
            bail!("{label} file URL must contain an absolute local path");
        }
        return Ok(TransportKind::Local);
    }
    if value.contains("://") || value.contains("::") {
        bail!("{label} uses an unsupported Git transport; only HTTPS, SSH, and local paths are accepted");
    }
    if let Some((authority, path)) = value.split_once(':') {
        let (identity, hostname) = authority
            .rsplit_once('@')
            .map_or((None, authority), |(identity, host)| (Some(identity), host));
        if !path.is_empty() && valid_hostname(hostname) && identity.is_none_or(valid_ssh_identity) {
            return Ok(TransportKind::Ssh);
        }
        bail!("{label} has invalid SCP-style SSH syntax");
    }
    if Path::new(value).is_absolute() || value.starts_with("./") || value.starts_with("../") {
        return Ok(TransportKind::Local);
    }
    bail!("{label} must be an explicit HTTPS/SSH URL or an absolute/explicit relative local path")
}

fn transport_argument(value: &str, transport: TransportKind) -> Result<String> {
    if transport != TransportKind::Local {
        return Ok(value.to_owned());
    }
    let path = value.strip_prefix("file://").unwrap_or(value);
    Path::new(path)
        .canonicalize()
        .into_diagnostic()
        .wrap_err_with(|| format!("could not resolve local Git source path '{value}'"))?
        .into_os_string()
        .into_string()
        .map_err(|_| miette::miette!("local Git source paths must be valid UTF-8"))
}

fn valid_network_authority(authority: &str, allow_identity: bool) -> bool {
    let (identity, host_port) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(identity, host)| (Some(identity), host));
    if identity.is_some() && !allow_identity {
        return false;
    }
    if identity.is_some_and(|identity| !valid_ssh_identity(identity)) {
        return false;
    }
    if let Some(ipv6) = host_port.strip_prefix('[') {
        let Some((address, suffix)) = ipv6.split_once(']') else {
            return false;
        };
        return !address.is_empty()
            && address
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b':')
            && (suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port));
    }
    let (hostname, port) = host_port
        .rsplit_once(':')
        .map_or((host_port, None), |(host, port)| (host, Some(port)));
    valid_hostname(hostname) && port.is_none_or(valid_port)
}

fn valid_hostname(hostname: &str) -> bool {
    !hostname.is_empty()
        && !hostname.starts_with(['-', '.'])
        && !hostname.ends_with(['-', '.'])
        && hostname
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn valid_ssh_identity(identity: &str) -> bool {
    !identity.is_empty()
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_init_ref(value: &str) -> Result<()> {
    if is_exact_oid(value) {
        return Ok(());
    }
    let Some((namespace, name)) = value
        .strip_prefix("refs/heads/")
        .map(|name| ("branch", name))
        .or_else(|| value.strip_prefix("refs/tags/").map(|name| ("tag", name)))
    else {
        bail!(
            "--ref must be a full refs/heads/NAME or refs/tags/NAME ref, or an exact 40/64-digit commit OID"
        );
    };
    validate_branch(name, namespace)
}

fn is_exact_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_branch(value: &str, label: &str) -> Result<()> {
    let safe_components = value.split('/').all(|component| {
        !component.is_empty()
            && !component.starts_with('.')
            && !component.eq_ignore_ascii_case("HEAD")
            && !component.to_ascii_lowercase().ends_with(".lock")
    });
    let portable = !value.is_empty()
        && !value.starts_with(['-', '/', '.'])
        && !value.ends_with(['/', '.'])
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && safe_components
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'));
    if !portable {
        bail!(
            "{label} '{value}' is not a safe Git branch token (letters, digits, '/', '-', '_' and '.' only)"
        );
    }
    Ok(())
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn path_argument(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        miette::miette!(
            "Git source paths must be valid UTF-8 for portable diagnostics: '{}'",
            path.display()
        )
    })
}

#[cfg(test)]
#[path = "source/source_tests.rs"]
mod tests;
