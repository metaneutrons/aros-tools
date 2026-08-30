//! Closed command contract for native release production and verification.

use std::path::{Component, Path, PathBuf};

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticStage};
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{ReleaseFailure, ReleaseResult};

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Produce and verify deterministic aros-tools release archives"
)]
pub struct Cli {
    #[arg(long, value_enum, default_value_t = aros_common::DiagnosticFormat::Human, env = "AROS_RELEASE_DIAGNOSTIC_FORMAT", global = true)]
    pub diagnostic_format: aros_common::DiagnosticFormat,
    #[arg(long, value_enum, default_value_t = aros_common::LogLevel::Off, env = "AROS_RELEASE_LOG_LEVEL", global = true)]
    pub log_level: aros_common::LogLevel,
    #[arg(long, value_enum, default_value_t = aros_common::LogFormat::Human, env = "AROS_RELEASE_LOG_FORMAT", global = true)]
    pub log_format: aros_common::LogFormat,
    #[arg(long, env = "AROS_RELEASE_LOG_FILE", global = true)]
    pub log_file: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Package one already-built native binary set
    Package(PackageArgs),
    /// Verify an archive and its manifest without trusting filenames
    Verify(VerifyArgs),
    /// Generate package-manager metadata from four verified manifests
    Generate(GenerateArgs),
}

#[derive(Debug, Clone, Args)]
pub struct PackageArgs {
    /// Release version without a leading `v`
    #[arg(long)]
    pub version: String,
    /// Immutable source commit (40 lowercase hexadecimal characters)
    #[arg(long)]
    pub source_commit: String,
    /// Commit timestamp used for every archive member
    #[arg(long, env = "SOURCE_DATE_EPOCH")]
    pub source_date_epoch: u64,
    /// Rust-style native target triple
    #[arg(long)]
    pub target: String,
    /// Directory containing the already-built release binaries
    #[arg(long)]
    pub bin_dir: PathBuf,
    /// aros-tools checkout containing README and licenses
    #[arg(long)]
    pub repository_root: PathBuf,
    /// Destination for archive, manifest and checksum sidecar
    #[arg(long)]
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct VerifyArgs {
    #[arg(long)]
    pub archive: PathBuf,
    #[arg(long)]
    pub manifest: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EcosystemFormat {
    Homebrew,
    Aur,
}

#[derive(Debug, Clone, Args)]
pub struct GenerateArgs {
    #[arg(long, value_enum)]
    pub format: EcosystemFormat,
    /// Canonical release download directory, ending in the immutable tag
    #[arg(long)]
    pub base_url: String,
    /// Four verified native archive manifests
    #[arg(long, required = true, num_args = 4)]
    pub manifests: Vec<PathBuf>,
    #[arg(long)]
    pub output: PathBuf,
}

impl PackageArgs {
    /// Validate all caller-controlled release identity and path fields.
    ///
    /// # Errors
    ///
    /// Returns `AP0101` when the release contract is ambiguous or unsafe.
    pub fn validate(&self) -> ReleaseResult<()> {
        if !valid_version(&self.version) {
            return Err(contract_failure(format!(
                "release version {:?} is not a canonical SemVer value without a leading v",
                self.version
            )));
        }
        if self.source_commit.len() != 40
            || !self
                .source_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(contract_failure(
                "source-commit must contain exactly 40 lowercase hexadecimal characters",
            ));
        }
        if self.source_date_epoch == 0 {
            return Err(contract_failure(
                "source-date-epoch must be greater than zero",
            ));
        }
        if !portable_token(&self.target) || !self.target.contains('-') {
            return Err(contract_failure(format!(
                "target {:?} is not a portable target triple",
                self.target
            )));
        }
        for (name, path) in [
            ("bin-dir", &self.bin_dir),
            ("repository-root", &self.repository_root),
            ("output-dir", &self.output_dir),
        ] {
            if path.as_os_str().is_empty()
                || path.components().any(|part| part == Component::ParentDir)
            {
                return Err(contract_failure(format!(
                    "{name} must be a non-empty path without parent traversal: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

pub(crate) fn valid_version(value: &str) -> bool {
    semver::Version::parse(value).is_ok_and(|version| version.to_string() == value)
}

fn portable_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(crate) fn require_regular(path: &Path, description: &str) -> ReleaseResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ReleaseFailure::new(Diagnostic::error(
            DiagnosticCode::ReleaseInput,
            DiagnosticStage::ReleaseInput,
            format!("cannot inspect {description} {}: {error}", path.display()),
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(ReleaseFailure::new(
            Diagnostic::error(
                DiagnosticCode::ReleaseInput,
                DiagnosticStage::ReleaseInput,
                format!("{description} is not a regular file: {}", path.display()),
            )
            .with_hint("release inputs must not be directories, devices, FIFOs or symbolic links"),
        ));
    }
    Ok(())
}

fn contract_failure(message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleaseContract,
            DiagnosticStage::ReleaseContract,
            message,
        )
        .with_hint("correct the explicit release identity or path; values are never inferred"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> PackageArgs {
        PackageArgs {
            version: "1.2.3-rc.1".into(),
            source_commit: "a".repeat(40),
            source_date_epoch: 1,
            target: "aarch64-apple-darwin".into(),
            bin_dir: "target/release".into(),
            repository_root: ".".into(),
            output_dir: "dist".into(),
        }
    }

    #[test]
    fn accepts_closed_release_identity() {
        valid().validate().unwrap();
    }

    #[test]
    fn rejects_noncanonical_versions_and_commits() {
        for version in [
            "v1.2.3", "1.2", "01.2.3", "1.2.3/", "1.2.3-", "1.2.3+", "1.2.3-01",
        ] {
            let mut args = valid();
            args.version = version.into();
            assert_eq!(
                args.validate().unwrap_err().diagnostic().code,
                DiagnosticCode::ReleaseContract
            );
        }
        let mut args = valid();
        args.source_commit = "A".repeat(40);
        assert!(args.validate().is_err());
    }
}
