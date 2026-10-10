//! Explicit family-v2 packaging execution through the complete byte collector.

use std::path::PathBuf;

use aros_common::Sha256Digest;
use clap::Args;

#[cfg(unix)]
use super::qualification_bytes::QualificationArgs;
#[cfg(unix)]
use super::recovery_family::RecoveryArgs;
#[cfg(unix)]
use super::release_evidence::EvidenceArgs;
use super::RepackageArgs;

/// Conditional V2 inputs; never an optional flatten of required arguments.
#[derive(Args)]
#[group(
    id = "family_repackage",
    multiple = true,
    conflicts_with = "legacy_repackage"
)]
pub(super) struct FamilyRepackageArgs {
    /// V2 only: complete isolated final release, not a single package set
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    release_dir: Option<PathBuf>,
    /// V2 only: independently selected immutable original download URL
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    base_url: Option<String>,
    /// V2 only: independently retained raw release-input collection digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    inputs_sha256: Option<Sha256Digest>,
    /// V2 only: independently retained raw final index digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    index_sha256: Option<Sha256Digest>,
    /// V2 only: absolute closed selection of every original evidence lane
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    selection: Option<PathBuf>,
    /// V2 only: independently retained raw lane-selection digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    selection_sha256: Option<Sha256Digest>,
    /// V2 only: original complete pre-attestation subject list
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    subject_manifest: Option<PathBuf>,
    /// V2 only: independently retained raw subject-list digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    subject_manifest_sha256: Option<Sha256Digest>,
    /// V2 only: closed original qualification claim file
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    qualification_evidence: Option<PathBuf>,
    /// V2 only: independently retained raw qualification digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    qualification_sha256: Option<Sha256Digest>,
    /// V2 only: independently selected qualification policy file
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    policy: Option<PathBuf>,
    /// V2 only: independently retained raw policy digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    policy_sha256: Option<Sha256Digest>,
    /// V2 only: independently retained exact recovery-request digest
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    recovery_request_sha256: Option<Sha256Digest>,
    /// V2 only: exact canonical archive basename from the selected original index
    #[arg(
        long,
        requires = "release_format",
        required_if_eq("release_format", "family-v2")
    )]
    pub asset: Option<String>,
}

pub(super) fn run(args: &RepackageArgs) -> miette::Result<()> {
    #[cfg(not(unix))]
    {
        let _ = args;
        Err(miette::miette!(
            "family-v2 packaging recovery requires a supported Unix host"
        ))
    }
    #[cfg(unix)]
    {
        let family = &args.family;
        let recovery = RecoveryArgs {
            qualification: QualificationArgs {
                evidence: EvidenceArgs {
                    directory: selected(family.release_dir.as_ref(), "--release-dir")?,
                    release_id: args.source_release_id.clone(),
                    base_url: selected(family.base_url.as_ref(), "--base-url")?,
                    inputs_sha256: selected(family.inputs_sha256.as_ref(), "--inputs-sha256")?,
                    index_sha256: selected(family.index_sha256.as_ref(), "--index-sha256")?,
                    selection: selected(family.selection.as_ref(), "--selection")?,
                    selection_sha256: selected(
                        family.selection_sha256.as_ref(),
                        "--selection-sha256",
                    )?,
                    subject_manifest: selected(
                        family.subject_manifest.as_ref(),
                        "--subject-manifest",
                    )?,
                    subject_manifest_sha256: selected(
                        family.subject_manifest_sha256.as_ref(),
                        "--subject-manifest-sha256",
                    )?,
                    forbidden_prefixes: args.forbidden_prefixes.clone(),
                    format: args.format,
                },
                qualification_evidence: selected(
                    family.qualification_evidence.as_ref(),
                    "--qualification-evidence",
                )?,
                qualification_sha256: selected(
                    family.qualification_sha256.as_ref(),
                    "--qualification-sha256",
                )?,
                policy: selected(family.policy.as_ref(), "--policy")?,
                policy_sha256: selected(family.policy_sha256.as_ref(), "--policy-sha256")?,
            },
            recovery_request: args.recovery_request.clone(),
            recovery_request_sha256: selected(
                family.recovery_request_sha256.as_ref(),
                "--recovery-request-sha256",
            )?,
        };
        super::release_evidence_readback::run_repackage(&recovery, args)
    }
}

#[cfg(unix)]
pub(super) fn selected<T: Clone>(value: Option<&T>, name: &str) -> miette::Result<T> {
    value
        .cloned()
        .ok_or_else(|| miette::miette!("family-v2 repackage requires {name}"))
}
