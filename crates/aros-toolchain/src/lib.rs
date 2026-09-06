//! Native toolchain producer library: the first, non-executing TCP-M1 slice.
//!
//! Recipe parsing is pure; experimental planning inspects explicit committed
//! inputs through bounded read-only operations. A valid recipe is not proof
//! of source cleanliness, cache integrity, executor origin or build readiness.
//! No compiler driver, consumer installation or publication is implemented.

pub mod canonical;
mod error;
mod inspection;
pub mod plan;
pub mod recipe;

pub use error::ContractError;
pub use recipe::Recipe;

#[cfg(test)]
mod contract_tests;
