//! Common abstractions, architectures, and data structures for AROS-NG tooling.

pub mod arch;
pub mod error;
pub mod target;
pub mod toolchain;

pub use arch::Architecture;
pub use error::{ArosError, Result};
pub use target::TargetProfile;
pub use toolchain::Toolchain;
