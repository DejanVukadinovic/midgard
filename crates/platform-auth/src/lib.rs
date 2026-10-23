//! Token verification and tenant-context derivation shared by MIDGARD services.
//!
//! Security properties enforced here:
//! * signature (RS256/ES256 only), issuer allowlist, audience, `exp`/`nbf`
//! * ID tokens are rejected when presented as access tokens
//! * the tenant/organization context is *derived from verified claims*, never
//!   from caller-supplied values; a requested organization is only a selection.

mod jwks;
pub mod tenancy;
mod verifier;

pub use jwks::JwksCache;
pub use tenancy::{ActiveOrg, ActorType, AuthContext, OrgSelectionError};
pub use verifier::{AuthError, TokenVerifier, VerifiedToken, VerifierConfig};
