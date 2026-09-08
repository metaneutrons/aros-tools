//! Native toolchain producer library: validated inputs, local lifecycle, packages and evidence.
//!
//! Recipe parsing is pure; planning inspects explicit committed inputs through
//! bounded read-only operations. A valid recipe is not proof of source
//! cleanliness, cache integrity, executor origin, compatibility or release
//! readiness. Local lifecycle, package and evidence modules deliberately have
//! no consumer-installation, credential, tag or publication authority.

pub mod canonical;
#[cfg(unix)]
pub mod cargo_vendor;
#[cfg(unix)]
pub mod compatibility;
#[cfg(unix)]
pub mod compatibility_source;
mod error;
pub mod executor;
#[cfg(unix)]
mod filesystem;
mod inspection;
pub mod metamake_fetch;
pub mod native_declaration;
#[cfg(unix)]
mod native_lifecycle;
pub mod package;
#[cfg(unix)]
pub mod package_extract;
pub mod package_verify;
pub mod plan;
pub mod preflight;
pub mod producer_environment;
pub mod profiles;
pub mod python_environment;
pub mod qualification_evidence;
pub mod recipe;
#[cfg(unix)]
pub mod recipe_builder;
pub mod recovery;
pub mod release_index;
pub mod repackage;
pub mod snapshot;
#[cfg(unix)]
mod source_audit;
pub mod source_cache;
pub mod source_lock;
pub mod source_usage;
pub mod workspace;

pub use error::ContractError;
pub use recipe::Recipe;

#[cfg(test)]
mod contract_tests;
