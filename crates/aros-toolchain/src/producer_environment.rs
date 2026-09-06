//! Explicit reproducibility environment for future native producer children.
//!
//! This module is intentionally small and has no process-launch capability.
//! It prevents later lifecycle code from inheriting ambient compiler flags,
//! locales, Python paths or arbitrary search paths by accident. A caller must
//! supply the already-reviewed tool directories and compose the returned base
//! environment with the private Python and Cargo environments.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ContractError;

/// Canonical roots whose absolute spellings must not leak into outputs.
#[derive(Debug, Clone)]
pub struct ReproducibilityRoots {
    /// Isolated AROS source material root.
    pub source: PathBuf,
    /// Isolated producer material root.
    pub producer: PathBuf,
    /// Isolated tools/collector material root.
    pub tools: PathBuf,
    /// Fresh owned work root.
    pub work: PathBuf,
    /// Verified source cache used by the later collector's private Cargo vendor tree.
    pub source_cache: PathBuf,
}

/// Sanitized base variables and the exact prefix-map flags they preserve.
#[derive(Debug, Clone)]
pub struct ProducerEnvironment {
    path: OsString,
    prefix_maps: String,
    collector_rustflags: String,
    source_date_epoch: u64,
    jobs: u64,
}

impl ProducerEnvironment {
    /// Construct a deterministic base environment without inheriting the host.
    ///
    /// Every selected root and tool directory is canonicalized before use.
    /// Paths containing whitespace, controls or `=` are rejected because the
    /// upstream MetaMake/CMake execution layer consumes compiler flags through
    /// strings; accepting them would make the remapping contract ambiguous.
    ///
    /// # Errors
    ///
    /// Returns AX0401 when a root/tool directory is absent, duplicated or not
    /// representable in the constrained compiler flag contract. It performs no
    /// process execution and does not reserve or mutate any directory.
    pub fn prepare(
        roots: &ReproducibilityRoots,
        tool_directories: &[PathBuf],
        source_date_epoch: u64,
        jobs: u64,
    ) -> Result<Self, ContractError> {
        if jobs == 0 {
            return Err(ContractError::environment(
                "native producer environment requires positive parallelism",
            ));
        }
        let source = canonical_compiler_root(&roots.source, "isolated AROS source root")?;
        let producer = canonical_compiler_root(&roots.producer, "isolated producer root")?;
        let tools = canonical_compiler_root(&roots.tools, "isolated tools root")?;
        let work = canonical_compiler_root(&roots.work, "owned build work root")?;
        let source_cache = canonical_compiler_root(&roots.source_cache, "verified source cache")?;
        if BTreeSet::from([&source, &producer, &tools, &work, &source_cache]).len() != 5 {
            return Err(ContractError::environment(
                "native producer environment requires distinct reproducibility and cache roots",
            ));
        }
        let path = sanitized_path(tool_directories)?;
        let prefix_maps = prefix_maps(&source, &producer, &tools, &work);
        let collector_rustflags = collector_rustflags(&source_cache, &tools, &work);
        Ok(Self {
            path,
            prefix_maps,
            collector_rustflags,
            source_date_epoch,
            jobs,
        })
    }

    /// Exact compiler prefix-map argument sequence, without ambient flags.
    #[must_use]
    pub fn prefix_maps(&self) -> &str {
        &self.prefix_maps
    }

    /// Exact collector remaps/flags, to be used only by the later Cargo phase.
    #[must_use]
    pub fn collector_rustflags(&self) -> &str {
        &self.collector_rustflags
    }

    /// Apply a fully controlled base environment to a future child command.
    ///
    /// This clears inherited variables. Callers can then compose only the
    /// reviewed [`crate::python_environment::PythonEnvironment`] and
    /// [`crate::cargo_vendor::CargoVendorEnvironment`] entries they need.
    /// It deliberately does not set a compiler, make program or target: M3
    /// owns explicit prerequisite selection and lifecycle arguments.
    pub fn apply_to(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("TZ", "UTC")
            .env("SOURCE_DATE_EPOCH", self.source_date_epoch.to_string())
            .env("ZERO_AR_DATE", "1")
            .env("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
            .env("CMAKE_BUILD_PARALLEL_LEVEL", self.jobs.to_string())
            .env("CFLAGS", &self.prefix_maps)
            .env("CXXFLAGS", &self.prefix_maps)
            .env("AROS_TOOLCHAIN_REPRO_FLAGS", &self.prefix_maps)
            .env_remove("PYTHONHOME")
            .env_remove("PYTHONPATH")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_HOME")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_NET_OFFLINE")
            .env_remove("CARGO_INCREMENTAL");
        #[cfg(target_os = "linux")]
        command.env("ARFLAGS", "crD").env("RANLIBFLAGS", "-D");
    }
}

fn canonical_compiler_root(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::environment(format!(
            "{label} must be an absolute directory"
        )));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ContractError::environment(format!("{label} cannot be canonicalized")))?;
    if !canonical.is_dir() || !compiler_flag_path(&canonical) {
        return Err(ContractError::environment(format!(
            "{label} is not a usable compiler prefix-map path"
        )));
    }
    Ok(canonical)
}

fn sanitized_path(tool_directories: &[PathBuf]) -> Result<OsString, ContractError> {
    if tool_directories.is_empty() {
        return Err(ContractError::environment(
            "native producer environment requires at least one explicit tool directory",
        ));
    }
    let mut selected = BTreeSet::new();
    let mut ordered = Vec::with_capacity(tool_directories.len());
    for directory in tool_directories {
        let directory = canonical_compiler_root(directory, "selected host tool directory")?;
        if !selected.insert(directory.clone()) {
            return Err(ContractError::environment(
                "native producer environment repeats a selected host tool directory",
            ));
        }
        ordered.push(directory);
    }
    std::env::join_paths(ordered).map_err(|_| {
        ContractError::environment("native producer environment cannot encode its sanitized PATH")
    })
}

fn compiler_flag_path(path: &Path) -> bool {
    path.to_str().is_some_and(|value| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| !byte.is_ascii_whitespace() && !byte.is_ascii_control() && byte != b'=')
    })
}

fn prefix_maps(source: &Path, producer: &Path, tools: &Path, work: &Path) -> String {
    let maps = [
        (source, "/usr/src/aros"),
        (producer, "/usr/src/aros-toolchain-producer"),
        (tools, "/usr/src/aros-tools"),
        (work, "/usr/src/aros-build"),
    ];
    maps.into_iter()
        .flat_map(|(from, to)| {
            let from = from.to_string_lossy();
            [
                format!("-ffile-prefix-map={from}={to}"),
                format!("-fdebug-prefix-map={from}={to}"),
                format!("-fmacro-prefix-map={from}={to}"),
            ]
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn collector_rustflags(source_cache: &Path, tools: &Path, work: &Path) -> String {
    [
        format!(
            "--remap-path-prefix={}=/usr/src/aros-sources",
            source_cache.to_string_lossy()
        ),
        format!(
            "--remap-path-prefix={}=/usr/src/aros-tools",
            tools.to_string_lossy()
        ),
        format!(
            "--remap-path-prefix={}=/usr/src/aros-build",
            work.to_string_lossy()
        ),
        "-Cdebuginfo=0".to_owned(),
        "-Cstrip=symbols".to_owned(),
        "-Ccodegen-units=1".to_owned(),
    ]
    .join(" ")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{ProducerEnvironment, ReproducibilityRoots};

    #[test]
    fn produces_a_closed_environment_with_all_legacy_prefix_maps() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let producer = temporary.path().join("producer");
        let tools = temporary.path().join("tools");
        let work = temporary.path().join("work");
        let source_cache = temporary.path().join("source-cache");
        let tool_bin = temporary.path().join("bin");
        for directory in [&source, &producer, &tools, &work, &source_cache, &tool_bin] {
            fs::create_dir(directory).unwrap();
        }
        let environment = ProducerEnvironment::prepare(
            &ReproducibilityRoots {
                source: source.clone(),
                producer: producer.clone(),
                tools: tools.clone(),
                work: work.clone(),
                source_cache: source_cache.clone(),
            },
            &[tool_bin],
            1_700_000_000,
            4,
        )
        .unwrap();
        let flags = environment.prefix_maps();
        for (root, stable) in [
            (&source, "/usr/src/aros"),
            (&producer, "/usr/src/aros-toolchain-producer"),
            (&tools, "/usr/src/aros-tools"),
            (&work, "/usr/src/aros-build"),
        ] {
            assert!(flags.contains(root.canonicalize().unwrap().to_string_lossy().as_ref()));
            assert!(flags.contains(stable));
        }
        assert!(environment.collector_rustflags().contains(
            source_cache
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        ));
        let mut command = std::process::Command::new("/usr/bin/env");
        environment.apply_to(&mut command);
        let output = String::from_utf8(command.output().unwrap().stdout).unwrap();
        assert!(output.contains("LC_ALL=C"));
        assert!(output.contains("SOURCE_DATE_EPOCH=1700000000"));
        assert!(output.contains("CMAKE_BUILD_PARALLEL_LEVEL=4"));
        assert!(output.contains("AROS_TOOLCHAIN_REPRO_FLAGS="));
    }

    #[test]
    fn rejects_roots_that_would_split_compiler_flags() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("with space");
        let tool_bin = temporary.path().join("bin");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&tool_bin).unwrap();
        let error = ProducerEnvironment::prepare(
            &ReproducibilityRoots {
                source: root.clone(),
                producer: root.clone(),
                tools: root.clone(),
                work: root,
                source_cache: temporary.path().join("source-cache"),
            },
            &[tool_bin],
            0,
            1,
        )
        .unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code.to_string(),
            "AX0401"
        );
    }
}
