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
mod cargo_vendor_generation;
#[cfg(unix)]
pub mod compatibility;
#[cfg(unix)]
pub mod compatibility_ports;
#[cfg(unix)]
pub mod compatibility_source;
mod error;
pub mod executor;
#[cfg(unix)]
mod filesystem;
#[cfg(unix)]
pub mod idf_bootloader;
mod inspection;
pub mod metamake_fetch;
#[cfg(unix)]
pub mod native_candidate;
#[cfg(unix)]
mod native_compiler_cache;
pub mod native_declaration;
mod native_family;
#[cfg(unix)]
mod native_gnu_collector;
#[cfg(unix)]
mod native_lifecycle;
mod native_make;
#[cfg(unix)]
pub mod native_media_preparation;
pub mod package;
#[cfg(unix)]
pub mod package_extract;
mod package_identity;
mod package_layout;
pub mod package_verify;
pub mod plan;
pub mod preflight;
pub mod producer_environment;
pub mod profiles;
pub mod python_environment;
pub mod qualification_evidence;
pub mod qualification_evidence_v2;
#[cfg(unix)]
pub mod qualification_readback_v2;
#[cfg(unix)]
pub mod qualification_recording_v2;
pub mod recipe;
#[cfg(unix)]
pub mod recipe_builder;
pub mod recovery;
#[cfg(unix)]
pub mod recovery_v2;
#[cfg(unix)]
pub mod release_attestation_manifest_v2;
pub mod release_checksums_v2;
#[cfg(unix)]
pub mod release_checksums_v2_writer;
pub mod release_index;
pub mod release_index_v2;
pub mod release_index_v2_builder;
pub mod release_index_v2_readback;
#[cfg(unix)]
pub mod release_index_v2_writer;
pub mod release_inputs;
pub mod repackage;
pub mod snapshot;
#[cfg(unix)]
mod source_audit;
pub mod source_cache;
#[cfg(unix)]
pub mod source_cache_request;
pub mod source_lock;
pub mod source_usage;
#[cfg(unix)]
pub mod wheel_environment;
pub mod workspace;

pub use error::ContractError;
pub use recipe::Recipe;

#[cfg(test)]
mod contract_tests;
