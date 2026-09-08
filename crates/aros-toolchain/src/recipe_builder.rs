//! Native construction of a closed recipe-v2 document.
//!
//! This is the producer's input-binding step, not a build or release
//! operation.  It reads three explicitly selected, recursively clean Git
//! checkouts and committed producer metadata, then writes one new recipe file.
//! It has no source acquisition, compiler, package, credential, tag or
//! publication capability.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::{sha256_bytes, Sha256Digest};
use serde_json::json;

use crate::inspection::{self, Checkout};
use crate::profiles::Profiles;
use crate::recipe::{safe_relative_path, GitObjectId, Recipe};
use crate::source_audit::{self, Budget};
use crate::source_lock::SourceLock;
use crate::{canonical, ContractError};

const RECIPE_BUILD_TIMEOUT: Duration = Duration::from_secs(60);

/// Explicit inputs to one non-overwriting native recipe construction.
#[derive(Debug, Clone)]
pub struct RecipeBuildRequest {
    /// Exact AROS source checkout containing every declared patch.
    pub source_root: PathBuf,
    /// Exact producer checkout containing the selected lock and profiles.
    pub producer_root: PathBuf,
    /// Exact aros-tools checkout selected as the producer executor.
    pub tools_root: PathBuf,
    /// Producer-root-relative source-lock-v2 document.
    pub source_lock: PathBuf,
    /// Producer-root-relative profiles-v1 document.
    pub profiles: PathBuf,
    /// Absent output file for the closed recipe-v2 document.
    pub output: PathBuf,
}

/// Measured immutable identity of one newly written recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeBuildOutput {
    /// Canonical absolute output path.
    pub path: PathBuf,
    /// Verified recipe-v2 self-digest.
    pub sha256: Sha256Digest,
}

/// Construct one closed recipe from recursively audited committed material.
///
/// The selected lock and profiles must be direct committed regular files below
/// the producer root.  Every lock-declared patch must be a committed regular
/// source file.  All three roots, including recursive submodules, are audited
/// against their raw Git material before the recipe is written.  An existing
/// output is never overwritten.
///
/// # Errors
///
/// Returns typed contract, identity or filesystem diagnostics for unsafe paths,
/// dirty inputs, changed Git material, invalid source metadata or output
/// conflicts.  No external program from a selected checkout is executed.
pub fn build(request: &RecipeBuildRequest) -> Result<RecipeBuildOutput, ContractError> {
    let deadline = Instant::now()
        .checked_add(RECIPE_BUILD_TIMEOUT)
        .ok_or_else(|| {
            ContractError::preflight("recipe construction timeout is not representable")
        })?;
    let source_root = inspection::directory(&request.source_root)?;
    let producer_root = inspection::directory(&request.producer_root)?;
    let tools_root = inspection::directory(&request.tools_root)?;
    if [
        source_root.as_path(),
        producer_root.as_path(),
        tools_root.as_path(),
    ]
    .iter()
    .enumerate()
    .any(|(index, root)| {
        [
            source_root.as_path(),
            producer_root.as_path(),
            tools_root.as_path(),
        ][index + 1..]
            .iter()
            .any(|other| root.starts_with(other) || other.starts_with(root))
    }) {
        return Err(ContractError::preflight(
            "recipe source, producer and tools roots must be distinct and non-overlapping",
        ));
    }

    let source_identity = observed_identity(&source_root, deadline)?;
    let producer_identity = observed_identity(&producer_root, deadline)?;
    let tools_identity = observed_identity(&tools_root, deadline)?;
    let source = Checkout::inspect(
        &source_root,
        (&source_identity.commit, &source_identity.tree),
        deadline,
    )?;
    let producer = Checkout::inspect(
        &producer_root,
        (&producer_identity.commit, &producer_identity.tree),
        deadline,
    )?;
    let tools = Checkout::inspect(
        &tools_root,
        (&tools_identity.commit, &tools_identity.tree),
        deadline,
    )?;
    let mut budget = Budget::new(deadline);
    source_audit::verify(&source, &mut budget, 0)?;
    source_audit::verify(&producer, &mut budget, 0)?;
    source_audit::verify(&tools, &mut budget, 0)?;

    let lock_path = producer_relative(&producer_root, &request.source_lock, "source lock")?;
    let profiles_path = producer_relative(&producer_root, &request.profiles, "profiles")?;
    let lock_bytes = producer.required_file(&lock_path)?;
    let profiles_bytes = producer.required_file(&profiles_path)?;
    let lock = SourceLock::parse(&lock_bytes)?;
    let _profiles = Profiles::parse(&profiles_bytes)?;

    let mut patches = Vec::new();
    for path in lock.source_patch_paths() {
        let bytes = source.required_file(path)?;
        patches.push(json!({
            "path": path,
            "sha256": sha256_bytes(&bytes),
        }));
    }
    patches.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let epoch = source_identity
        .epoch
        .max(producer_identity.epoch)
        .max(tools_identity.epoch);
    let mut document = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": source_identity.commit.as_str(),
        "source_tree": source_identity.tree.as_str(),
        "producer_commit": producer_identity.commit.as_str(),
        "producer_tree": producer_identity.tree.as_str(),
        "tools_commit": tools_identity.commit.as_str(),
        "tools_tree": tools_identity.tree.as_str(),
        "source_date_epoch": epoch,
        "source_lock_sha256": sha256_bytes(&lock_bytes),
        "profiles_sha256": sha256_bytes(&profiles_bytes),
        "patches": patches,
    });
    let digest = sha256_bytes(&canonical::bytes(&document)?);
    document
        .as_object_mut()
        .ok_or_else(|| ContractError::state("recipe document is not an object"))?
        .insert("recipe_sha256".into(), json!(digest.to_string()));
    let encoded = serde_json::to_vec_pretty(&document)
        .map_err(|_| ContractError::state("cannot serialize native recipe document"))?;
    let mut encoded = encoded;
    encoded.push(b'\n');
    let recipe = Recipe::parse(&encoded)?;
    if recipe.sha256() != &digest {
        return Err(ContractError::state(
            "native recipe self-digest changed during serialization",
        ));
    }
    let output = output_path(&request.output)?;
    write_new(&output, &encoded)?;
    Ok(RecipeBuildOutput {
        path: output,
        sha256: digest,
    })
}

#[derive(Debug)]
struct ObservedIdentity {
    commit: GitObjectId,
    tree: GitObjectId,
    epoch: u64,
}

fn observed_identity(root: &Path, deadline: Instant) -> Result<ObservedIdentity, ContractError> {
    let cancellation = aros_common::CancellationToken::default();
    let commit = git_text(root, &["rev-parse", "HEAD"], deadline, &cancellation)?;
    let tree = git_text(
        root,
        &["show", "-s", "--format=%T", "HEAD"],
        deadline,
        &cancellation,
    )?;
    let epoch = git_text(
        root,
        &["show", "-s", "--format=%ct", "HEAD"],
        deadline,
        &cancellation,
    )?;
    Ok(ObservedIdentity {
        commit: GitObjectId::try_from(commit).map_err(ContractError::identity)?,
        tree: GitObjectId::try_from(tree).map_err(ContractError::identity)?,
        epoch: epoch.parse().map_err(|_| {
            ContractError::identity("checkout commit time is not an unsigned integer")
        })?,
    })
}

fn git_text(
    root: &Path,
    arguments: &[&str],
    deadline: Instant,
    cancellation: &aros_common::CancellationToken,
) -> Result<String, ContractError> {
    let bytes = inspection::git(root, arguments, &[], 4096, deadline, cancellation)?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| ContractError::identity("Git identity output is not UTF-8"))?
        .trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err(ContractError::identity(
            "Git identity output is empty or contains whitespace",
        ));
    }
    Ok(value.to_owned())
}

fn producer_relative(root: &Path, path: &Path, label: &str) -> Result<String, ContractError> {
    let path = inspection::absolute(path)?;
    let parent = path.parent().ok_or_else(|| {
        ContractError::preflight(format!("selected {label} has no parent directory"))
    })?;
    let leaf = path
        .file_name()
        .ok_or_else(|| ContractError::preflight(format!("selected {label} has no file name")))?;
    // Canonicalise the ancestor only.  The final path component remains a
    // Git-tree name and is read through `Checkout::required_file`, so a
    // checkout symlink can never redirect this binding outside the producer.
    let resolved = inspection::directory(parent)?.join(leaf);
    let relative = resolved.strip_prefix(root).map_err(|_| {
        ContractError::preflight(format!("selected {label} is outside the producer checkout"))
    })?;
    let relative = relative
        .to_str()
        .ok_or_else(|| ContractError::preflight(format!("selected {label} path is not UTF-8")))?;
    if !safe_relative_path(relative) {
        return Err(ContractError::preflight(format!(
            "selected {label} is not a canonical producer-relative path"
        )));
    }
    Ok(relative.to_owned())
}

fn output_path(path: &Path) -> Result<PathBuf, ContractError> {
    let selected = inspection::absolute(path)?;
    match fs::symlink_metadata(&selected) {
        Ok(_) => {
            return Err(ContractError::state(
                "recipe output already exists and will not be replaced",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ContractError::preflight(
                "recipe output cannot be inspected before creation",
            ));
        }
    }
    let output = inspection::destination(&selected)?;
    let parent = output
        .parent()
        .ok_or_else(|| ContractError::preflight("recipe output has no parent directory"))?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|_| ContractError::preflight("recipe output parent is unavailable"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::preflight(
            "recipe output parent must be a real directory",
        ));
    }
    Ok(output)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ContractError::state("cannot create an absent recipe output"))?;
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|_| ContractError::state("cannot durably write native recipe output"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use aros_common::{run_status, sha256_bytes};

    use super::{build, RecipeBuildRequest};
    use crate::recipe::Recipe;

    fn git(root: &Path, arguments: &[&str]) {
        let mut command = Command::new("git");
        command.current_dir(root).args([
            "-c",
            "user.name=Recipe fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ]);
        command.args(arguments);
        let status = run_status(&mut command).unwrap();
        assert!(status.status.success());
    }

    fn checkout(root: &Path, files: &[(&str, &str)]) {
        fs::create_dir_all(root).unwrap();
        git(root, &["init", "-q"]);
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "test: fixture"]);
    }

    #[test]
    fn rejects_an_existing_output_before_modifying_it() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let producer = temporary.path().join("producer");
        let tools = temporary.path().join("tools");
        checkout(
            &source,
            &[("tools/crosstools/llvm/llvm-aros.diff", "patch")],
        );
        checkout(
            &producer,
            &[
                (
                    "toolchains/lock.sources.json",
                    r#"{"schema":"aros-toolchain-source-lock-v2","family":"llvm","version":"11.0.0","sources":[{"component":"llvm","version":"11.0.0","purpose":"toolchain-component","patch":"tools/crosstools/llvm/llvm-aros.diff","filename":"llvm.tar.xz","url":"https://example.invalid/llvm.tar.xz","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1}],"host_python_packages":[{"name":"mako","version":"1.0.0","filename":"mako.tar.gz","url":"https://example.invalid/mako.tar.gz","sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":1,"source_root":"mako-1.0.0","python_path":"."}]}"#,
                ),
                (
                    "toolchains/profiles.json",
                    r#"{"schema":"aros-toolchain-profiles-v1","upstream_commit":"1111111111111111111111111111111111111111","profiles":[{"name":"pc-x86_64","configure_target":"pc","upstream_output_target":"pc","target_triple":"x86_64-unknown-aros","cpu":"x86_64","platform":"pc","float_abi":"","capabilities":["c"]}]}"#,
                ),
            ],
        );
        checkout(
            &tools,
            &[("contracts/toolchain-producer-v1.toml", "contract")],
        );
        let output = temporary.path().join("recipe.json");
        fs::write(&output, "preserve").unwrap();
        let error = build(&RecipeBuildRequest {
            source_root: source,
            producer_root: producer.clone(),
            tools_root: tools,
            source_lock: producer.join("toolchains/lock.sources.json"),
            profiles: producer.join("toolchains/profiles.json"),
            output: output.clone(),
        })
        .unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
        assert_eq!(fs::read_to_string(output).unwrap(), "preserve");
    }

    #[test]
    fn writes_a_closed_recipe_from_committed_inputs() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let producer = temporary.path().join("producer");
        let tools = temporary.path().join("tools");
        checkout(
            &source,
            &[("tools/crosstools/llvm/llvm-aros.diff", "patch")],
        );
        let source_lock = r#"{"schema":"aros-toolchain-source-lock-v2","family":"llvm","version":"11.0.0","sources":[{"component":"llvm","version":"11.0.0","purpose":"toolchain-component","patch":"tools/crosstools/llvm/llvm-aros.diff","filename":"llvm.tar.xz","url":"https://example.invalid/llvm.tar.xz","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1}],"host_python_packages":[{"name":"mako","version":"1.0.0","filename":"mako.tar.gz","url":"https://example.invalid/mako.tar.gz","sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":1,"source_root":"mako-1.0.0","python_path":"."}]}"#;
        let profiles = r#"{"schema":"aros-toolchain-profiles-v1","upstream_commit":"1111111111111111111111111111111111111111","profiles":[{"name":"pc-x86_64","configure_target":"pc","upstream_output_target":"pc","target_triple":"x86_64-unknown-aros","cpu":"x86_64","platform":"pc","float_abi":"","capabilities":["c"]}]}"#;
        checkout(
            &producer,
            &[
                ("toolchains/lock.sources.json", source_lock),
                ("toolchains/profiles.json", profiles),
            ],
        );
        checkout(
            &tools,
            &[("contracts/toolchain-producer-v1.toml", "contract")],
        );
        let output = temporary.path().join("recipe.json");
        let result = build(&RecipeBuildRequest {
            source_root: source,
            producer_root: producer.clone(),
            tools_root: tools,
            source_lock: producer.join("toolchains/lock.sources.json"),
            profiles: producer.join("toolchains/profiles.json"),
            output: output.clone(),
        })
        .unwrap();
        let recipe_bytes = fs::read(&output).unwrap();
        let recipe = Recipe::parse(&recipe_bytes).unwrap();
        assert_eq!(
            result.path,
            output
                .parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("recipe.json")
        );
        assert_eq!(result.sha256, *recipe.sha256());
        assert_eq!(
            recipe.source_lock_sha256(),
            &sha256_bytes(source_lock.as_bytes())
        );
        assert_eq!(recipe.profiles_sha256(), &sha256_bytes(profiles.as_bytes()));
        assert_eq!(recipe.patches().len(), 1);
        assert_eq!(
            recipe.patches()[0].path(),
            "tools/crosstools/llvm/llvm-aros.diff"
        );
        assert_eq!(recipe.patches()[0].sha256(), &sha256_bytes(b"patch"));
    }
}
