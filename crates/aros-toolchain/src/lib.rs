//! Native toolchain producer library: inspection and TCP-M1 ownership primitives.
//!
//! Recipe parsing is pure; experimental planning inspects explicit committed
//! inputs through bounded read-only operations. A valid recipe is not proof
//! of source cleanliness, cache integrity, executor origin or build readiness.
//! Explicit work ownership is separate from planning and does not authorize
//! execution. No compiler driver, consumer installation or publication exists.

pub mod canonical;
mod error;
#[cfg(unix)]
mod filesystem;
mod inspection;
pub mod plan;
pub mod recipe;
pub mod workspace;

pub use error::ContractError;
pub use recipe::Recipe;

#[cfg(test)]
mod contract_tests;
