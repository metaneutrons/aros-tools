//! Repository-local semantic state used by transactional source synchronization.

use super::{
    capture_git_stdout, direct_submodules, git_at, git_capture, transport_argument, validate_url,
    TransportKind,
};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

pub(super) fn normalize_url(value: &str) -> String {
    let mut normalized = value.trim().trim_end_matches('/').to_owned();
    if normalized
        .get(normalized.len().saturating_sub(4)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".git"))
    {
        normalized.truncate(normalized.len() - 4);
    }
    if let Some((scheme, remainder)) = normalized.split_once("://") {
        let authority_end = remainder.find('/').unwrap_or(remainder.len());
        let (authority, path) = remainder.split_at(authority_end);
        return format!(
            "{}://{}{path}",
            scheme.to_ascii_lowercase(),
            normalize_authority(authority)
        );
    }
    if let Some((authority, path)) = normalized.split_once(':') {
        if authority.contains('@') && !path.is_empty() {
            return format!("{}:{path}", normalize_authority(authority));
        }
    }
    normalized
}

fn normalize_authority(authority: &str) -> String {
    authority.rsplit_once('@').map_or_else(
        || authority.to_ascii_lowercase(),
        |(identity, hostname)| format!("{identity}@{}", hostname.to_ascii_lowercase()),
    )
}

fn redact_url(value: &str) -> String {
    let Some((scheme, remainder)) = value.split_once("://") else {
        return value
            .split_once(':')
            .and_then(|(authority, path)| {
                authority
                    .rsplit_once('@')
                    .map(|(_, hostname)| format!("{hostname}:{path}"))
            })
            .unwrap_or_else(|| value.to_owned());
    };
    let authority_end = remainder.find('/').unwrap_or(remainder.len());
    let (authority, path) = remainder.split_at(authority_end);
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, hostname)| hostname);
    format!("{scheme}://{authority}{path}")
}

pub(super) fn reject_network_config(repo_root: &Path) -> Result<()> {
    let mut command = git_at(repo_root, TransportKind::Local);
    command.args(["config", "--local", "--name-only", "--list"]);
    let names = capture_git_stdout(&mut command, "local Git network configuration")?;
    let unsafe_name = names.lines().find(|name| {
        let name = name.to_ascii_lowercase();
        (name.starts_with("url.")
            && (name.ends_with(".insteadof") || name.ends_with(".pushinsteadof")))
            || (name.starts_with("http.") && name.ends_with(".extraheader"))
            || name == "http.extraheader"
            || name == "core.sshcommand"
            || name == "core.fsmonitor"
            || name == "core.hookspath"
            || name == "core.attributesfile"
            || name == "core.excludesfile"
            || name == "core.autocrlf"
            || name == "core.eol"
            || name == "core.symlinks"
            || name.starts_with("core.sparsecheckout")
            || name.starts_with("filter.")
            || name.starts_with("include.")
            || name.starts_with("includeif.")
            || name == "credential.helper"
            || name.starts_with("credential.")
            || (name.starts_with("remote.")
                && name
                    .rsplit_once('.')
                    .is_some_and(|(_, suffix)| suffix == "vcs"))
    });
    if let Some(name) = unsafe_name {
        bail!(
            "local Git network-affecting configuration or checkout-affecting configuration '{name}' is not used by `aros source sync`; remove it or use a separately reviewed checkout"
        );
    }
    Ok(())
}

pub(super) fn repository_semantics(repo_root: &Path) -> Result<Vec<u8>> {
    reject_network_config(repo_root)?;
    reject_repository_object_overrides(repo_root)?;
    let mut inventories = BTreeMap::new();
    collect_repository_semantics(repo_root, Path::new("."), &mut inventories)?;
    let mut serialized = Vec::new();
    for (path, state) in inventories {
        append_semantic_field(&mut serialized, path.to_string_lossy().as_bytes())?;
        append_semantic_field(&mut serialized, &state)?;
    }
    Ok(serialized)
}

fn collect_repository_semantics(
    repo_root: &Path,
    relative: &Path,
    inventories: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let repository = repo_root.join(relative);
    reject_network_config(&repository)?;
    reject_repository_object_overrides(&repository)?;
    let mut config = git_at(&repository, TransportKind::Local);
    config.args(["config", "--local", "--null", "--list"]);
    let config = capture_git_stdout(&mut config, "local Git semantic state")?;
    let mut state = config.into_bytes();
    for name in [
        "config",
        "config.worktree",
        "info/exclude",
        "objects/info/alternates",
        "shallow",
    ] {
        append_semantic_field(&mut state, name.as_bytes())?;
        let path = git_capture(
            &repository,
            ["rev-parse", "--git-path", name],
            "repository-local Git semantic path",
        )?;
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            repository.join(path)
        };
        match read_bounded_regular_file(&path, 64 * 1024)? {
            Some(contents) => {
                state.push(1);
                append_semantic_field(&mut state, &contents)?;
            }
            None => state.push(0),
        }
    }
    inventories.insert(relative.to_path_buf(), state);
    for entry in direct_submodules(&repository)? {
        let child_relative = relative.join(&entry.path);
        let child = repo_root.join(&child_relative);
        if child.join(".git").exists() {
            collect_repository_semantics(repo_root, &child_relative, inventories)?;
        }
    }
    Ok(())
}

fn reject_repository_object_overrides(repo_root: &Path) -> Result<()> {
    let replace_refs = git_capture(
        repo_root,
        [
            "for-each-ref",
            "--format=%(refname):%(objectname)",
            "refs/replace",
        ],
        "Git replacement-ref inventory",
    )?;
    if !replace_refs.is_empty() {
        bail!(
            "Git replacement refs are incompatible with isolated candidate validation; remove them before source synchronization"
        );
    }
    for (label, name) in [
        ("legacy Git grafts", "info/grafts"),
        ("repository-local attributes", "info/attributes"),
    ] {
        let path = git_capture(
            repo_root,
            ["rev-parse", "--git-path", name],
            "repository-local Git semantic path",
        )?;
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            repo_root.join(path)
        };
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.len() > 0 || !metadata.file_type().is_file() => {
                bail!(
                    "{label} at '{}' would make the validated and materialized Git views differ",
                    path.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).into_diagnostic().wrap_err_with(|| {
                    format!(
                        "could not inspect repository semantic path '{}'",
                        path.display()
                    )
                });
            }
        }
    }
    reject_repository_hooks(repo_root)?;
    Ok(())
}

fn append_semantic_field(destination: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length = u64::try_from(value.len())
        .into_diagnostic()
        .wrap_err("repository semantic field is too large")?;
    destination.extend_from_slice(&length.to_le_bytes());
    destination.extend_from_slice(value);
    Ok(())
}

fn read_bounded_regular_file(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    #[cfg(not(unix))]
    bail!("source synchronization semantic snapshots are not supported on this platform");
    #[cfg(unix)]
    {
        use rustix::fs::{open, Mode, OFlags};

        let descriptor = match open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => {
                bail!(
                    "could not safely open repository semantic file '{}': {error}",
                    path.display()
                )
            }
        };
        let file = File::from(descriptor);
        let metadata = file.metadata().into_diagnostic()?;
        if !metadata.is_file() {
            bail!(
                "repository semantic path '{}' is not a regular file",
                path.display()
            );
        }
        if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
            bail!(
                "repository semantic file '{}' exceeds the {}-byte limit",
                path.display(),
                limit
            );
        }
        let mut contents = Vec::with_capacity(metadata.len() as usize);
        file.take(u64::try_from(limit + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut contents)
            .into_diagnostic()?;
        if contents.len() > limit {
            bail!(
                "repository semantic file '{}' grew beyond the {}-byte limit while being read",
                path.display(),
                limit
            );
        }
        Ok(Some(contents))
    }
}

fn reject_repository_hooks(repo_root: &Path) -> Result<()> {
    let hooks = git_capture(
        repo_root,
        ["rev-parse", "--git-path", "hooks"],
        "repository-local Git hooks path",
    )?;
    let hooks = PathBuf::from(hooks);
    let hooks = if hooks.is_absolute() {
        hooks
    } else {
        repo_root.join(hooks)
    };
    let entries = match fs::read_dir(&hooks) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).into_diagnostic().wrap_err_with(|| {
                format!("could not inspect Git hooks path '{}'", hooks.display())
            });
        }
    };
    for (index, entry) in entries.enumerate() {
        if index >= 1024 {
            bail!("repository-local Git hooks directory exceeds the 1024-entry inspection limit");
        }
        let entry = entry.into_diagnostic()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let metadata = fs::symlink_metadata(entry.path()).into_diagnostic()?;
        if name.ends_with(".sample") && metadata.file_type().is_file() {
            continue;
        }
        bail!(
            "repository-local hook entry '{}' is incompatible with isolated source synchronization",
            entry.path().display()
        );
    }
    Ok(())
}

pub(super) fn verify_expected_upstream(
    repo_root: &Path,
    expected: &str,
    transport: TransportKind,
) -> Result<()> {
    let configured = git_capture(
        repo_root,
        ["remote", "get-url", "upstream"],
        "upstream remote URL",
    )?;
    let effective_expected = transport_argument(expected, transport)?;
    let configured_identity = reviewed_url_identity(&configured, repo_root)?;
    let expected_identity = reviewed_url_identity(&effective_expected, repo_root)?;
    if configured_identity != expected_identity {
        bail!(
            "the `upstream` remote URL does not match the reviewed URL; configured '{}', expected '{}' (effective identity '{}'). Pass --upstream only after reviewing the intended remote",
            redact_url(&configured),
            redact_url(expected),
            redact_url(&expected_identity)
        );
    }
    Ok(())
}

pub(super) fn verify_remote_urls(checkout: &Path, origin: &str, upstream: &str) -> Result<()> {
    let configured_origin = git_capture(
        checkout,
        ["remote", "get-url", "origin"],
        "configured origin URL",
    )?;
    let configured_upstream = git_capture(
        checkout,
        ["remote", "get-url", "upstream"],
        "configured upstream URL",
    )?;
    if reviewed_url_identity(&configured_origin, checkout)?
        != reviewed_url_identity(origin, checkout)?
        || reviewed_url_identity(&configured_upstream, checkout)?
            != reviewed_url_identity(upstream, checkout)?
    {
        bail!("Git did not retain the reviewed origin/upstream remote configuration");
    }
    Ok(())
}

fn reviewed_url_identity(value: &str, local_base: &Path) -> Result<String> {
    match validate_url(value, "Git remote")? {
        TransportKind::Local => {
            let value = value.strip_prefix("file://").unwrap_or(value);
            let path = Path::new(value);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                local_base.join(path)
            };
            path.canonicalize()
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!(
                        "could not resolve configured local Git source '{}' relative to '{}'",
                        value,
                        local_base.display()
                    )
                })?
                .into_os_string()
                .into_string()
                .map_err(|_| miette::miette!("local Git source paths must be valid UTF-8"))
        }
        TransportKind::Https | TransportKind::Ssh => Ok(normalize_url(value)),
    }
}
