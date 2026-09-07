//! Native toolchain producer library: validated inputs, ownership and raw snapshots.
//!
//! Recipe parsing is pure; experimental planning inspects explicit committed
//! inputs through bounded read-only operations. A valid recipe is not proof
//! of source cleanliness, cache integrity, executor origin or build readiness.
//! Work ownership and isolated raw material are separate from planning and do not authorize
//! execution. No compiler driver, consumer installation or publication exists.

pub mod canonical;
#[cfg(unix)]
pub mod cargo_vendor;
mod error;
pub mod executor;
#[cfg(unix)]
mod filesystem;
mod inspection;
pub mod metamake_fetch;
pub mod native_declaration;
#[cfg(unix)]
mod native_lifecycle;
pub mod plan;
pub mod preflight;
pub mod producer_environment;
pub mod profiles;
pub mod python_environment;
pub mod recipe;
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
