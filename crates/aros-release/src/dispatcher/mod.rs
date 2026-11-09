//! Dispatcher-side controller commands of the protected V2 release start.
//!
//! The Python adapters in `metaneutrons/aros-toolchains` (`scripts/toolchain/`)
//! are the behavioural oracle. The closed contracts are kept: fixed fields,
//! byte limits, canonical JSON, digest joins, fail-closed behaviour on every
//! unknown or uncertain provider answer and no retry of a mutation.
//!
//! Layers:
//!
//! * pure codecs ([`json`], [`context`], [`start`], [`claim`], [`binding`]),
//! * a provider boundary ([`provider::Provider`]) that every observation goes
//!   through, with a fixed `gh` implementation and offline fixtures in tests,
//! * the commands ([`select`], [`verify_claim`], [`dispatch`],
//!   [`verify_binding`]).
//!
//! Nothing here installs or triggers a workflow. Each command reports its
//! limits: a successful run is not native qualification or release authority.

pub mod binding;
pub mod claim;
pub mod claim_upload;
pub mod cli;
pub mod context;
pub mod dispatch;
pub mod files;
pub mod json;
pub mod observe;
pub mod provider;
pub mod runtime;
pub mod select;
pub mod start;
pub mod verify_binding;
pub mod verify_claim;
pub mod zip_reader;

#[cfg(test)]
mod claim_upload_tests;
#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod oracle_tests;
#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
pub(crate) mod testing;

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticStage};

use crate::ReleaseFailure;

/// Fixed source repository of the protected release chain.
pub const REPOSITORY: &str = "metaneutrons/aros-toolchains";
/// The only accepted source reference.
pub const MAIN_REF: &str = "refs/heads/main";
/// Bound for provider metadata (Python `MAX_API_BYTES`).
pub const MAX_API_BYTES: usize = 8 * 1024 * 1024;
/// Bound for the runner event file.
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;
/// Bound for a start record, claim, binding or receipt.
pub const MAX_RECORD: usize = 128 * 1024;
/// Bound for a signature bundle.
pub const MAX_BUNDLE: usize = 8 * 1024 * 1024;
/// Page limits for provider listings.
pub const MAX_PAGES: usize = 20;
pub const PAGE_SIZE: usize = 100;

/// Where a closed-contract violation originates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A selected value or record differs from the closed contract.
    Contract,
    /// A local input could not be read safely.
    Input,
    /// A digest, signature or provider observation differs or is unavailable.
    Integrity,
}

/// One fail-closed dispatcher rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub kind: Kind,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Rejection>;

/// Reject with a contract violation.
///
/// # Errors
///
/// Always.
pub fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Rejection {
        kind: Kind::Contract,
        message: message.into(),
    })
}

/// Reject with an integrity failure (digest, signature, provider state).
///
/// # Errors
///
/// Always.
pub fn integrity<T>(message: impl Into<String>) -> Result<T> {
    Err(Rejection {
        kind: Kind::Integrity,
        message: message.into(),
    })
}

/// Reject with an input failure (local file).
///
/// # Errors
///
/// Always.
pub fn input<T>(message: impl Into<String>) -> Result<T> {
    Err(Rejection {
        kind: Kind::Input,
        message: message.into(),
    })
}

/// Contract check helper (Python `START.require`).
///
/// # Errors
///
/// Fails with a contract violation when `condition` is false.
pub fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        fail(message)
    }
}

/// Integrity check helper.
///
/// # Errors
///
/// Fails with an integrity failure when `condition` is false.
pub fn require_integrity(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        integrity(message)
    }
}

impl From<Rejection> for ReleaseFailure {
    fn from(rejection: Rejection) -> Self {
        let (code, stage, hint) = match rejection.kind {
            Kind::Contract => (
                DiagnosticCode::ReleaseContract,
                DiagnosticStage::ReleaseContract,
                "stop the release start; the selected inputs differ from the closed contract",
            ),
            Kind::Input => (
                DiagnosticCode::ReleaseInput,
                DiagnosticStage::ReleaseInput,
                "select caller-owned, bounded regular files and retry the read-only command",
            ),
            Kind::Integrity => (
                DiagnosticCode::ReleaseIntegrity,
                DiagnosticStage::ReleaseIntegrity,
                "stop the release start; never retry a mutation after an uncertain result",
            ),
        };
        Self::new(Diagnostic::error(code, stage, rejection.message).with_hint(hint))
    }
}
