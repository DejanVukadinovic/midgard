use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("invalid openapi document: {0}")]
    OpenApi(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub version: Version,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub permissions: Vec<String>,
}

fn default_protocol() -> String {
    "http".into()
}

impl Manifest {
    pub fn validate(&self) -> Result<(), ContractError> {
        let ok_id = !self.id.is_empty()
            && self.id.split('.').count() >= 2
            && self.id.split('.').all(|seg| {
                !seg.is_empty()
                    && seg
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    && seg.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            });
        if !ok_id {
            return Err(ContractError::Manifest(
                "id must be dotted lowercase `namespace.name`".into(),
            ));
        }
        if self.protocol != "http" {
            return Err(ContractError::Manifest(
                "only protocol `http` is supported".into(),
            ));
        }
        if !self.version.pre.is_empty() || !self.version.build.is_empty() {
            return Err(ContractError::Manifest(
                "pre-release/build versions cannot be published".into(),
            ));
        }
        for p in &self.permissions {
            if p.is_empty()
                || p.len() > 64
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ":._-".contains(c))
            {
                return Err(ContractError::Manifest(format!("invalid permission `{p}`")));
            }
        }
        Ok(())
    }
}

/// A publishable bundle: manifest + OpenAPI 3.1 text + semantics text.
#[derive(Debug, Clone)]
pub struct ContractBundle {
    pub manifest: Manifest,
    pub openapi: String,
    pub openapi_doc: Value,
    pub semantics: String,
}

impl ContractBundle {
    pub fn parse(
        manifest: Manifest,
        openapi: String,
        semantics: String,
    ) -> Result<Self, ContractError> {
        manifest.validate()?;
        let doc: Value =
            serde_yaml::from_str(&openapi).map_err(|e| ContractError::OpenApi(e.to_string()))?;
        let v = doc
            .get("openapi")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !v.starts_with("3.1") {
            return Err(ContractError::OpenApi(
                "`openapi` must be a 3.1.x version".into(),
            ));
        }
        if !doc.get("paths").is_some_and(Value::is_object) {
            return Err(ContractError::OpenApi("`paths` object is required".into()));
        }
        if !doc.get("info").is_some_and(Value::is_object) {
            return Err(ContractError::OpenApi("`info` object is required".into()));
        }
        Ok(Self {
            manifest,
            openapi,
            openapi_doc: doc,
            semantics,
        })
    }

    /// Content address of the bundle: `sha256:<hex>` over length-prefixed canonical parts, so
    /// that reformatting the manifest does not change the digest but any content change does.
    pub fn digest(&self) -> String {
        let mut perms = self.manifest.permissions.clone();
        perms.sort();
        perms.dedup();
        let canonical_manifest = serde_json::to_string(&serde_json::json!({
            "id": self.manifest.id,
            "version": self.manifest.version.to_string(),
            "protocol": self.manifest.protocol,
            "permissions": perms,
        }))
        .expect("serializable");
        let mut h = Sha256::new();
        for part in [
            canonical_manifest.as_str(),
            self.openapi.as_str(),
            self.semantics.as_str(),
        ] {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part.as_bytes());
        }
        format!("sha256:{}", hex::encode(h.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OAS: &str = "openapi: 3.1.0\ninfo: {title: t, version: '1'}\npaths: {}\n";

    fn m(v: &str) -> Manifest {
        Manifest {
            id: "platform.example".into(),
            version: v.parse().unwrap(),
            protocol: "http".into(),
            permissions: vec!["example:read".into()],
        }
    }

    #[test]
    fn digest_is_stable_and_content_sensitive() {
        let a = ContractBundle::parse(m("1.0.0"), OAS.into(), "s".into()).unwrap();
        let b = ContractBundle::parse(m("1.0.0"), OAS.into(), "s".into()).unwrap();
        let c = ContractBundle::parse(m("1.0.0"), OAS.into(), "s2".into()).unwrap();
        assert_eq!(a.digest(), b.digest());
        assert_ne!(a.digest(), c.digest());
    }

    #[test]
    fn rejects_bad_documents() {
        assert!(
            ContractBundle::parse(
                m("1.0.0"),
                "openapi: 3.0.0\ninfo: {}\npaths: {}".into(),
                "".into()
            )
            .is_err()
        );
        assert!(ContractBundle::parse(m("1.0.0"), "not: [valid".into(), "".into()).is_err());
        let mut bad = m("1.0.0");
        bad.id = "Bad".into();
        assert!(ContractBundle::parse(bad, OAS.into(), "".into()).is_err());
    }
}
