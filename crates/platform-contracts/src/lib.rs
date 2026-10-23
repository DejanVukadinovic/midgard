//! Contract manifests, content digests, version constraints and OpenAPI compatibility checks.

pub mod compat;
mod manifest;
mod version;

pub use manifest::{ContractBundle, ContractError, Manifest};
pub use version::{ConstraintError, parse_constraint};
