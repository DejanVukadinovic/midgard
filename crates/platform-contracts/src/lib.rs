//! Contract manifests, content digests, version constraints and OpenAPI compatibility checks.

pub mod compat;
mod manifest;
pub mod v02;
mod version;

pub use manifest::{ContractBundle, ContractError, Manifest};
pub use version::{ConstraintError, parse_constraint};
