//! Complete portable byte collection, separate from authenticated execution.

use std::path::PathBuf;

use aros_common::Sha256Digest;
use clap::Args;

use super::ResultFormat;

#[derive(Args)]
pub(super) struct EvidenceArgs {
    /// Absolute canonical final release directory; no intermediate inventory
    #[arg(long)]
    pub directory: PathBuf,
    /// Independently selected release ID; must equal the V2 index
    #[arg(long)]
    pub release_id: String,
    /// Independently selected immutable download base URL
    #[arg(long)]
    pub base_url: String,
    /// Independently retained raw release-input collection digest
    #[arg(long)]
    pub inputs_sha256: Sha256Digest,
    /// Independently retained raw V2 index digest
    #[arg(long)]
    pub index_sha256: Sha256Digest,
    /// Absolute closed lane selection, outside all package and evidence roots
    #[arg(long)]
    pub selection: PathBuf,
    /// Independently retained raw selection digest; not signature verification
    #[arg(long)]
    pub selection_sha256: Sha256Digest,
    /// Absolute retained pre-attestation subject list, outside the release
    #[arg(long)]
    pub subject_manifest: PathBuf,
    /// Independently retained raw pre-attestation subject-list digest
    #[arg(long)]
    pub subject_manifest_sha256: Sha256Digest,
    /// Absolute build root forbidden in archive payloads (repeatable)
    #[arg(long = "forbidden-prefix")]
    pub forbidden_prefixes: Vec<PathBuf>,
    /// Complete read-only byte-consistency result; never release admission
    #[arg(long, value_enum, default_value = "human")]
    pub format: ResultFormat,
}

pub(super) fn run(args: &EvidenceArgs) -> miette::Result<()> {
    #[cfg(unix)]
    return super::release_evidence_readback::run(args);
    #[cfg(not(unix))]
    {
        let _ = args;
        Err(miette::miette!(
            "family-v2 evidence read-back requires a supported Unix host"
        ))
    }
}
