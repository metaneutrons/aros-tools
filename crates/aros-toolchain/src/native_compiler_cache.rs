//! Opt-in local host-compiler launchers for the sterile native producer.
//!
//! Target compilers, assembly, linking and the Rust collector are not wrapped.
//! The shared managed-cache lease remains held by the enclosing lifecycle.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_cache::{CompilerCacheBuildSelection, CompilerCacheSelection};
use aros_common::sha256_file;
use serde_json::{json, Value};

use crate::{preflight::HostPreflight, ContractError};

/// Private host C/C++ wrappers and their immutable receipt binding.
pub struct HostCompilerLaunchers {
    cc: PathBuf,
    cxx: PathBuf,
    binding: Value,
}

impl HostCompilerLaunchers {
    pub(crate) fn prepare(
        selection: &CompilerCacheBuildSelection,
        host: &HostPreflight,
        work: &Path,
        resume: bool,
    ) -> Result<Option<Self>, ContractError> {
        let CompilerCacheBuildSelection::Managed(managed) = selection else {
            return Ok(None);
        };
        let CompilerCacheSelection::Backend {
            backend,
            executable,
        } = managed.selection()
        else {
            return Err(ContractError::environment(
                "managed compiler cache omitted its launcher",
            ));
        };
        validate_startup_tmp(*backend, &work.join("tmp"))?;
        let directory = work.join("compiler-launchers");
        if resume {
            let metadata = fs::symlink_metadata(&directory).map_err(|_| {
                ContractError::environment("retained compiler launchers are missing")
            })?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o777 != 0o700
            {
                return Err(ContractError::environment(
                    "retained compiler launcher directory is unsafe",
                ));
            }
        } else {
            fs::create_dir(&directory).map_err(|_| {
                ContractError::environment("cannot reserve host compiler launchers")
            })?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|_| {
                ContractError::environment("cannot protect host compiler launchers")
            })?;
        }
        let mut launchers = Vec::new();
        for (role, name) in [("cc", "cc"), ("c++", "cxx")] {
            let compiler = host
                .tools
                .iter()
                .find(|tool| tool.name == role)
                .ok_or_else(|| {
                    ContractError::prerequisite("native preflight omitted a host compiler")
                })?;
            let content = format!(
                "#!/bin/sh\nexec {} {} \"$@\"\n",
                shell_path(executable)?,
                shell_path(&compiler.invocation_path)?
            );
            let path = directory.join(name);
            if resume {
                let metadata = fs::symlink_metadata(&path).map_err(|_| {
                    ContractError::environment("retained host compiler launcher is missing")
                })?;
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || metadata.permissions().mode() & 0o777 != 0o700
                    || metadata.len() != content.len() as u64
                    || fs::read(&path).ok().as_deref() != Some(content.as_bytes())
                {
                    return Err(ContractError::environment(
                        "retained host compiler launcher changed",
                    ));
                }
            } else {
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)
                    .map_err(|_| {
                        ContractError::environment("cannot create host compiler launcher")
                    })?;
                file.write_all(content.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|_| {
                        ContractError::environment("cannot persist host compiler launcher")
                    })?;
                file.set_permissions(fs::Permissions::from_mode(0o700))
                    .map_err(|_| {
                        ContractError::environment("cannot protect host compiler launcher")
                    })?;
            }
            launchers.push((path, json!({
                "role": role,
                "compiler": compiler.invocation_path,
                "compiler_sha256": sha256_file(&compiler.path).map_err(|_| ContractError::environment("cannot measure host compiler"))?.digest,
                "launcher_sha256": aros_common::sha256_bytes(content.as_bytes()),
            })));
        }
        let binding = json!({
            "backend": backend,
            "executable": executable,
            "executable_sha256": sha256_file(executable).map_err(|_| ContractError::environment("cannot measure compiler cache executable"))?.digest,
            "root": managed.root().root(),
            "configuration_sha256": sha256_file(managed.root().configuration_path()).map_err(|_| ContractError::environment("cannot measure compiler cache configuration"))?.digest,
            "environment": managed.environment().values(),
            "host_compilers": launchers.iter().map(|(_, binding)| binding).collect::<Vec<_>>(),
        });
        Ok(Some(Self {
            cc: launchers[0].0.clone(),
            cxx: launchers[1].0.clone(),
            binding,
        }))
    }

    pub(crate) fn apply_compilers(&self, command: &mut Command) {
        command.env("CC", &self.cc).env("CXX", &self.cxx);
    }

    pub(crate) const fn binding(&self) -> &Value {
        &self.binding
    }
}

// This is deliberately not CompilerCacheEnvironment::apply_to: that public
// product-build helper imports unrelated ambient variables. The native
// producer has already installed its sterile environment and must retain it.
/// Compose only managed cache values into an already sterile environment.
pub fn apply_environment(selection: &CompilerCacheBuildSelection, command: &mut Command) {
    if let CompilerCacheBuildSelection::Managed(managed) = selection {
        command.envs(managed.environment().values());
    }
}

fn shell_path(path: &Path) -> Result<String, ContractError> {
    let value = path
        .to_str()
        .filter(|value| path.is_absolute() && !value.contains(['\n', '\r', '\0']))
        .ok_or_else(|| {
            ContractError::environment("host compiler launcher path is not representable")
        })?;
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn validate_startup_tmp(
    backend: aros_cache::CompilerBackend,
    tmp: &Path,
) -> Result<(), ContractError> {
    // sccache also creates a startup-notification socket beneath TMPDIR,
    // independently of SCCACHE_SERVER_UDS (mozilla/sccache commands.rs).
    // Reserve space for the generated subdirectory and socket filename.
    if backend == aros_cache::CompilerBackend::Sccache
        && tmp.as_os_str().as_encoded_bytes().len() + 32 > 103
    {
        return Err(ContractError::environment("sccache startup socket cannot fit beneath the owned TMPDIR; use a shorter --work-dir or --compiler-cache ccache/off"));
    }
    Ok(())
}

/// Compare physical root locations, never ancestor-symlink spellings.
pub fn require_disjoint_root(cache: &Path, producer: &Path) -> Result<(), ContractError> {
    if cache.starts_with(producer) || producer.starts_with(cache) {
        return Err(ContractError::environment(
            "compiler cache must be separate from every producer input, work and output root",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_paths_are_quoted_not_evaluated() {
        assert_eq!(
            shell_path(Path::new("/tmp/a'b $(false)")).unwrap(),
            "'/tmp/a'\\''b $(false)'"
        );
        assert!(shell_path(Path::new("relative")).is_err());
        assert!(shell_path(Path::new("/tmp/a\nb")).is_err());
    }

    #[test]
    fn sccache_startup_socket_is_preflighted_even_with_a_short_cache_root() {
        let long = PathBuf::from(format!("/{}", "x".repeat(80)));
        assert!(validate_startup_tmp(aros_cache::CompilerBackend::Sccache, &long).is_err());
        assert!(validate_startup_tmp(aros_cache::CompilerBackend::Ccache, &long).is_ok());
        assert!(validate_startup_tmp(
            aros_cache::CompilerBackend::Sccache,
            Path::new("/tmp/producer/tmp")
        )
        .is_ok());
    }

    #[test]
    fn physical_overlap_is_rejected_through_ancestor_symlinks_and_parent_segments() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let cache = source.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let alias = temporary.path().join("alias");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        for spelling in [alias.join("cache"), source.join("../source/cache")] {
            assert!(require_disjoint_root(
                &spelling.canonicalize().unwrap(),
                &source.canonicalize().unwrap()
            )
            .is_err());
        }
        assert!(require_disjoint_root(
            &cache.canonicalize().unwrap(),
            &cache.canonicalize().unwrap().join("work")
        )
        .is_err());
        assert!(require_disjoint_root(
            &cache.canonicalize().unwrap(),
            &temporary.path().join("other")
        )
        .is_ok());
    }
}
