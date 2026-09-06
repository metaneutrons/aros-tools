//! Native toolchain producer library: the first, non-executing TCP-M1 slice.
//!
//! Validates recipe syntax and self-digests without filesystem, environment,
//! transport or process access. A valid recipe is an identity claim, not proof
//! of source cleanliness, cache integrity, executor origin or build readiness.
//! No compiler driver, consumer installation or publication is implemented.

pub mod canonical;
mod error;
pub mod recipe;

pub use error::ContractError;
pub use recipe::Recipe;

#[cfg(test)]
mod contract_tests;
