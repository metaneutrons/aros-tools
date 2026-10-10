//! Explicit V2 recovery verification; observations are not authentication.

use std::path::PathBuf;

use aros_common::Sha256Digest;
use clap::Args;

use super::qualification_bytes::QualificationArgs;

#[derive(Args)]
pub(super) struct RecoveryArgs {
    #[command(flatten)]
    pub qualification: QualificationArgs,
    /// Absolute closed recovery-request-v2 with independent provider/tag/verifier observations
    #[arg(long)]
    pub recovery_request: PathBuf,
    /// Independently retained exact recovery request digest; not authentication
    #[arg(long)]
    pub recovery_request_sha256: Sha256Digest,
}

pub(super) fn run(args: &RecoveryArgs) -> miette::Result<()> {
    #[cfg(unix)]
    return super::release_evidence_readback::run_recovery(args);
    #[cfg(not(unix))]
    {
        let _ = args;
        Err(miette::miette!(
            "family-v2 recovery byte verification requires a supported Unix host"
        ))
    }
}
