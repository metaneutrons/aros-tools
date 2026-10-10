//! Explicit family-v2 qualification claim acquisition; no trust booleans.

use std::path::PathBuf;

use aros_common::Sha256Digest;
use clap::Args;

use super::release_evidence::EvidenceArgs;

#[derive(Args)]
pub(super) struct QualificationArgs {
    #[command(flatten)]
    pub evidence: EvidenceArgs,
    /// Absolute closed family-v2 qualification document; claims are not authentication
    #[arg(long)]
    pub qualification_evidence: PathBuf,
    /// Independently retained raw qualification document digest
    #[arg(long)]
    pub qualification_sha256: Sha256Digest,
    /// Absolute closed verifier policy, independently selected from downloaded claims
    #[arg(long)]
    pub policy: PathBuf,
    /// Independently retained raw policy document digest
    #[arg(long)]
    pub policy_sha256: Sha256Digest,
}

pub(super) fn run(args: &QualificationArgs) -> miette::Result<()> {
    #[cfg(unix)]
    return super::release_evidence_readback::run_qualification(args);
    #[cfg(not(unix))]
    {
        let _ = args;
        Err(miette::miette!(
            "family-v2 qualification byte read-back requires a supported Unix host"
        ))
    }
}
