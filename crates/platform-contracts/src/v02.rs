//! MIDGARD contract specification v0.2: a protocol-neutral capability manifest whose interfaces
//! reference standard interface descriptions (OpenAPI 3.1 for `http`, AsyncAPI 3.0 for `mq`).
//!
//! Documents are supplied by the publisher (inline or as named files in the publish request);
//! nothing is ever fetched from a URL by the server.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ContractError;

pub const MIDGARD_VERSION: &str = "0.2";
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_INTERFACES: usize = 16;
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

fn err(msg: impl Into<String>) -> ContractError {
    ContractError::Manifest(msg.into())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Caller {
    Service,
    User,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Accepted,
    Completed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Idempotency {
    Required,
    Supported,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Requirement {
    Required,
    Optional,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    Supported,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Mq,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Mq => "mq",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpecType {
    Openapi,
    Asyncapi,
}

impl SpecType {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecType::Openapi => "openapi",
            SpecType::Asyncapi => "asyncapi",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub outcome: Outcome,
    #[serde(default = "default_idempotency")]
    pub idempotency: Idempotency,
}

fn default_idempotency() -> Idempotency {
    Idempotency::None
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuthContext {
    #[serde(default = "optional")]
    pub application: Requirement,
    #[serde(default = "optional")]
    pub tenant: Requirement,
}

fn optional() -> Requirement {
    Requirement::Optional
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    pub permission: String,
    pub callers: Vec<Caller>,
    #[serde(default)]
    pub context: Option<AuthContext>,
    #[serde(default)]
    pub delegation: Option<Support>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceAuthorization {
    pub callers: Vec<Caller>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpecificationRef {
    #[serde(rename = "type")]
    pub kind: SpecType,
    /// Name of a document supplied with the publish request (relative path; no URLs).
    #[serde(default)]
    pub document: Option<String>,
    /// The document itself, embedded in the manifest.
    #[serde(default)]
    pub inline: Option<Value>,
    pub operation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDecl {
    pub id: String,
    pub protocol: Protocol,
    #[serde(default)]
    pub authorization: Option<InterfaceAuthorization>,
    pub specification: SpecificationRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManifestV02 {
    pub midgard: String,
    pub service: String,
    pub capability: String,
    pub version: Version,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub execution: Option<Execution>,
    pub authorization: Authorization,
    pub interfaces: Vec<InterfaceDecl>,
}

impl ManifestV02 {
    /// Stable contract name used by grants, bindings and the resolver: `<service>.<capability>`.
    /// `capability` contains no dots, so the split is unambiguous.
    pub fn contract_name(&self) -> String {
        format!("{}.{}", self.service, self.capability)
    }
}

fn valid_segment(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn valid_interface_id(s: &str) -> bool {
    valid_segment(s, 32)
}

/// One validated, normalized interface of a contract.
#[derive(Debug, Clone)]
pub struct InterfaceRecord {
    pub id: String,
    pub protocol: Protocol,
    pub spec_type: SpecType,
    pub operation: String,
    /// Normalized (canonical JSON) bytes of the interface document.
    pub document: String,
    pub document_digest: String,
    /// Callers allowed at this interface: the capability set, possibly narrowed.
    pub callers: Vec<Caller>,
    /// Facts extracted from the document (method/path, media types, channel, action, ...).
    pub summary: Value,
}

#[derive(Debug, Clone)]
pub struct BundleV02 {
    pub manifest: ManifestV02,
    pub interfaces: Vec<InterfaceRecord>,
}

fn normalize_doc(text: &str) -> Result<(Value, String), ContractError> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(err("interface document exceeds 1 MiB"));
    }
    let v: Value = serde_yaml::from_str(text)
        .map_err(|e| err(format!("interface document is not valid YAML/JSON: {e}")))?;
    let s = serde_json::to_string(&v).map_err(|e| err(e.to_string()))?;
    Ok((v, s))
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn media_types(content: Option<&Value>, ctx: &str) -> Result<Vec<String>, ContractError> {
    let Some(c) = content else {
        return Ok(vec![]);
    };
    let obj = c
        .as_object()
        .ok_or_else(|| err(format!("{ctx}: `content` must be an object")))?;
    for (mt, body) in obj {
        if !mt.contains('/') || !body.is_object() {
            return Err(err(format!(
                "{ctx}: invalid media type entry `{mt}` (must be `type/subtype` mapping to an object)"
            )));
        }
    }
    Ok(obj.keys().cloned().collect())
}

fn inspect_openapi(doc: &Value, operation: &str) -> Result<Value, ContractError> {
    let v = doc.get("openapi").and_then(Value::as_str).unwrap_or("");
    if !v.starts_with("3.1") {
        return Err(err("OpenAPI document must declare `openapi: 3.1.x`"));
    }
    if !doc.get("info").is_some_and(Value::is_object) {
        return Err(err("OpenAPI document requires an `info` object"));
    }
    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| err("OpenAPI document requires a `paths` object"))?;
    let mut found = None;
    for (path, item) in paths {
        for m in HTTP_METHODS {
            let Some(op) = item.get(m) else { continue };
            if op.get("operationId").and_then(Value::as_str) == Some(operation) {
                if found.is_some() {
                    return Err(err(format!("operationId `{operation}` is not unique")));
                }
                found = Some((path.clone(), m, op));
            }
        }
    }
    let (path, method, op) = found.ok_or_else(|| {
        err(format!(
            "OpenAPI operationId `{operation}` not found in document"
        ))
    })?;
    let ctx = format!("operation `{operation}`");
    let responses = op
        .get("responses")
        .and_then(Value::as_object)
        .filter(|r| !r.is_empty())
        .ok_or_else(|| err(format!("{ctx}: `responses` must be a non-empty object")))?;
    let request_types = match op.get("requestBody") {
        Some(rb) => media_types(rb.get("content"), &format!("{ctx} requestBody"))?,
        None => vec![],
    };
    let mut resp = Map::new();
    for (status, r) in responses {
        let ok_status = status == "default"
            || (status.len() == 3 && status.chars().all(|c| c.is_ascii_digit()))
            || (status.len() == 3 && status.ends_with("XX"));
        if !ok_status || !r.is_object() {
            return Err(err(format!("{ctx}: invalid response entry `{status}`")));
        }
        resp.insert(
            status.clone(),
            json!(media_types(
                r.get("content"),
                &format!("{ctx} response {status}")
            )?),
        );
    }
    Ok(json!({
        "method": method.to_uppercase(), "path": path,
        "request_media_types": request_types, "responses": resp,
    }))
}

fn inspect_asyncapi(doc: &Value, operation: &str) -> Result<Value, ContractError> {
    let v = doc.get("asyncapi").and_then(Value::as_str).unwrap_or("");
    if !v.starts_with("3.0") {
        return Err(err("AsyncAPI document must declare `asyncapi: 3.0.x`"));
    }
    if !doc.get("info").is_some_and(Value::is_object) {
        return Err(err("AsyncAPI document requires an `info` object"));
    }
    let op = doc
        .get("operations")
        .and_then(Value::as_object)
        .and_then(|o| o.get(operation))
        .ok_or_else(|| {
            err(format!(
                "AsyncAPI operation `{operation}` not found in document"
            ))
        })?;
    let action = op.get("action").and_then(Value::as_str).unwrap_or("");
    if !matches!(action, "send" | "receive") {
        return Err(err(format!(
            "operation `{operation}`: `action` must be `send` or `receive`"
        )));
    }
    if op.get("reply").is_some() {
        return Err(err(format!(
            "operation `{operation}`: request/reply is not supported in v0.2 (use send/receive)"
        )));
    }
    let chan_ref = op
        .get("channel")
        .and_then(|c| c.get("$ref"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            err(format!(
                "operation `{operation}`: `channel.$ref` is required"
            ))
        })?;
    let chan_key = chan_ref.strip_prefix("#/channels/").ok_or_else(|| {
        err("channel `$ref` must point to `#/channels/<name>` in the same document")
    })?;
    let chan = doc
        .get("channels")
        .and_then(|c| c.get(chan_key))
        .ok_or_else(|| {
            err(format!(
                "channel `{chan_key}` referenced by the operation is not defined"
            ))
        })?;
    let messages = chan
        .get("messages")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            err(format!(
                "channel `{chan_key}` must define at least one message"
            ))
        })?;
    let mut msgs = Vec::new();
    for (name, m) in messages {
        // Messages may be inline or `$ref` to #/components/messages/<x>.
        let resolved = match m.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let key = r.strip_prefix("#/components/messages/").ok_or_else(|| {
                    err("message `$ref` must point to `#/components/messages/<name>`")
                })?;
                doc.pointer(&format!("/components/messages/{key}"))
                    .ok_or_else(|| err(format!("message `{key}` is not defined in components")))?
            }
            None => m,
        };
        let payload = resolved.get("payload");
        if payload.is_none() {
            return Err(err(format!(
                "message `{name}` must declare a `payload` schema"
            )));
        }
        msgs.push(json!({
            "name": name,
            "content_type": resolved.get("contentType").cloned()
                .or_else(|| doc.get("defaultContentType").cloned()),
            "has_headers": resolved.get("headers").is_some(),
            "correlation_id": resolved.get("correlationId").is_some(),
        }));
    }
    Ok(json!({
        "action": action, "interaction": action, "channel": chan_key,
        "address": chan.get("address"), "messages": msgs,
    }))
}

impl BundleV02 {
    /// Validate a v0.2 manifest together with the documents supplied with it.
    pub fn build(
        manifest: ManifestV02,
        documents: &BTreeMap<String, String>,
    ) -> Result<Self, ContractError> {
        if manifest.midgard != MIDGARD_VERSION {
            return Err(err(format!("`midgard` must be \"{MIDGARD_VERSION}\"")));
        }
        let svc_ok = !manifest.service.is_empty()
            && manifest.service.len() <= 64
            && manifest.service.split('.').all(|s| valid_segment(s, 32));
        if !svc_ok {
            return Err(err(
                "`service` must be dotted lowercase segments, e.g. `example.echo`",
            ));
        }
        if !valid_segment(&manifest.capability, 32) {
            return Err(err(
                "`capability` must be a single lowercase segment (no dots)",
            ));
        }
        if !manifest.version.pre.is_empty() || !manifest.version.build.is_empty() {
            return Err(err("pre-release/build versions cannot be published"));
        }
        let p = &manifest.authorization.permission;
        if p.is_empty()
            || p.len() > 64
            || !p
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ":._-".contains(c))
        {
            return Err(err(format!("invalid permission `{p}`")));
        }
        let cap_callers: BTreeSet<Caller> =
            manifest.authorization.callers.iter().copied().collect();
        if cap_callers.is_empty() {
            return Err(err("`authorization.callers` must not be empty"));
        }
        if manifest.interfaces.is_empty() || manifest.interfaces.len() > MAX_INTERFACES {
            return Err(err(format!("1-{MAX_INTERFACES} interfaces are required")));
        }

        let mut seen = BTreeSet::new();
        let mut records = Vec::new();
        for i in &manifest.interfaces {
            if !valid_interface_id(&i.id) {
                return Err(err(format!("invalid interface id `{}`", i.id)));
            }
            if !seen.insert(i.id.clone()) {
                return Err(err(format!("duplicate interface id `{}`", i.id)));
            }
            let want = match i.protocol {
                Protocol::Http => SpecType::Openapi,
                Protocol::Mq => SpecType::Asyncapi,
            };
            if i.specification.kind != want {
                return Err(err(format!(
                    "interface `{}`: protocol `{}` requires specification type `{}`",
                    i.id,
                    i.protocol.as_str(),
                    want.as_str()
                )));
            }
            let callers: Vec<Caller> = match &i.authorization {
                None => cap_callers.iter().copied().collect(),
                Some(a) => {
                    let set: BTreeSet<Caller> = a.callers.iter().copied().collect();
                    if set.is_empty() {
                        return Err(err(format!(
                            "interface `{}`: `callers` must not be empty",
                            i.id
                        )));
                    }
                    if !set.is_subset(&cap_callers) {
                        return Err(err(format!(
                            "interface `{}`: interface authorization may narrow but never widen capability-level callers",
                            i.id
                        )));
                    }
                    set.into_iter().collect()
                }
            };
            let text = match (&i.specification.document, &i.specification.inline) {
                (Some(_), Some(_)) => {
                    return Err(err(format!(
                        "interface `{}`: set either `document` or `inline`, not both",
                        i.id
                    )));
                }
                (None, None) => {
                    return Err(err(format!(
                        "interface `{}`: `document` or `inline` is required",
                        i.id
                    )));
                }
                (Some(name), None) => {
                    let bad = name.is_empty()
                        || name.starts_with('/')
                        || name.contains("://")
                        || name.contains('\\')
                        || name.split('/').any(|s| s == "..");
                    if bad {
                        return Err(err(format!(
                            "interface `{}`: document `{name}` must be a relative path supplied with the request (URLs and parent paths are rejected)",
                            i.id
                        )));
                    }
                    let key = name.strip_prefix("./").unwrap_or(name);
                    documents
                        .get(key)
                        .or_else(|| documents.get(name))
                        .cloned()
                        .ok_or_else(|| {
                            err(format!(
                                "interface `{}`: document `{name}` was not supplied in `documents`",
                                i.id
                            ))
                        })?
                }
                (None, Some(v)) => serde_json::to_string(v).map_err(|e| err(e.to_string()))?,
            };
            let (doc, normalized) = normalize_doc(&text)?;
            let summary = match i.protocol {
                Protocol::Http => inspect_openapi(&doc, &i.specification.operation),
                Protocol::Mq => inspect_asyncapi(&doc, &i.specification.operation),
            }
            .map_err(|e| err(format!("interface `{}`: {e}", i.id)))?;

            if i.protocol == Protocol::Http
                && let Some(e) = &manifest.execution
            {
                let codes: Vec<&String> = summary["responses"]
                    .as_object()
                    .map(|r| r.keys().collect())
                    .unwrap_or_default();
                let has_202 = codes.iter().any(|c| c.as_str() == "202");
                let other_2xx = codes
                    .iter()
                    .any(|c| c.starts_with('2') && c.as_str() != "202");
                match e.outcome {
                    Outcome::Accepted if !has_202 || other_2xx => {
                        return Err(err(format!(
                            "interface `{}`: execution.outcome=accepted requires the HTTP operation to answer 202 (and no other 2xx)",
                            i.id
                        )));
                    }
                    Outcome::Completed if has_202 => {
                        return Err(err(format!(
                            "interface `{}`: execution.outcome=completed must not use 202 Accepted",
                            i.id
                        )));
                    }
                    _ => {}
                }
            }
            records.push(InterfaceRecord {
                id: i.id.clone(),
                protocol: i.protocol,
                spec_type: i.specification.kind,
                operation: i.specification.operation.clone(),
                document_digest: sha(normalized.as_bytes()),
                document: normalized,
                callers,
                summary,
            });
        }
        Ok(Self {
            manifest,
            interfaces: records,
        })
    }

    /// Manifest as stored: `document`/`inline` references are replaced by the document digest, so
    /// the stored manifest is self-contained and immutable.
    pub fn normalized_manifest(&self) -> Value {
        let m = &self.manifest;
        let mut ifaces = Vec::new();
        for r in &self.interfaces {
            ifaces.push(json!({
                "id": r.id, "protocol": r.protocol.as_str(),
                "callers": r.callers,
                "specification": {
                    "type": r.spec_type.as_str(), "operation": r.operation,
                    "digest": r.document_digest,
                },
            }));
        }
        ifaces.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let mut callers = m.authorization.callers.clone();
        callers.sort();
        callers.dedup();
        json!({
            "midgard": MIDGARD_VERSION,
            "service": m.service, "capability": m.capability,
            "id": m.contract_name(), "version": m.version.to_string(),
            "protocol": "multi",
            "permissions": [m.authorization.permission],
            "description": m.description,
            "execution": m.execution,
            "authorization": {
                "permission": m.authorization.permission, "callers": callers,
                "context": m.authorization.context, "delegation": m.authorization.delegation,
            },
            "interfaces": ifaces,
        })
    }

    /// Content address over the normalized manifest (which already embeds every document digest).
    pub fn digest(&self) -> String {
        let canonical = serde_json::to_string(&self.normalized_manifest()).expect("serializable");
        let mut h = Sha256::new();
        h.update(b"midgard-contract-v0.2\0");
        h.update((canonical.len() as u64).to_be_bytes());
        h.update(canonical.as_bytes());
        format!("sha256:{}", hex::encode(h.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const OPENAPI: &str = include_str!("../../../contracts/examples/echo/openapi.yaml");
    pub const ASYNCAPI: &str = include_str!("../../../contracts/examples/echo/asyncapi.yaml");
    const CONTRACT: &str = include_str!("../../../contracts/examples/echo/contract.yaml");

    fn docs() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("openapi.yaml".into(), OPENAPI.into()),
            ("asyncapi.yaml".into(), ASYNCAPI.into()),
        ])
    }

    fn manifest() -> ManifestV02 {
        serde_yaml::from_str(CONTRACT).unwrap()
    }

    #[test]
    fn example_contract_is_valid_with_two_interfaces() {
        let b = BundleV02::build(manifest(), &docs()).unwrap();
        assert_eq!(b.manifest.contract_name(), "example.echo.submit");
        assert_eq!(b.interfaces.len(), 2);
        assert_eq!(b.interfaces[0].summary["method"], "POST");
        assert_eq!(b.interfaces[1].summary["action"], "receive");
        assert!(b.digest().starts_with("sha256:"));
    }

    #[test]
    fn example_contract_validates_against_json_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../contracts/schema/midgard-contract-v0.2.schema.json"
        ))
        .unwrap();
        let v = jsonschema::validator_for(&schema).unwrap();
        let inst: Value = serde_yaml::from_str(CONTRACT).unwrap();
        assert!(v.is_valid(&inst), "example must satisfy the schema");
        let mut bad = inst.clone();
        bad["interfaces"][0]["protocol"] = json!("nats");
        assert!(!v.is_valid(&bad), "vendor names are not protocols");
        let mut bad = inst;
        bad["extra"] = json!(1);
        assert!(!v.is_valid(&bad));
    }

    #[test]
    fn digest_is_stable_and_content_sensitive() {
        let a = BundleV02::build(manifest(), &docs()).unwrap().digest();
        let b = BundleV02::build(manifest(), &docs()).unwrap().digest();
        assert_eq!(a, b);
        // Reformatting the document does not change it; changing content does.
        let mut d = docs();
        d.insert(
            "openapi.yaml".into(),
            OPENAPI.replace("Synthetic Echo API", "Other"),
        );
        assert_ne!(a, BundleV02::build(manifest(), &d).unwrap().digest());
    }

    fn rejects(mutate: impl FnOnce(&mut ManifestV02, &mut BTreeMap<String, String>), needle: &str) {
        let (mut m, mut d) = (manifest(), docs());
        mutate(&mut m, &mut d);
        let e = BundleV02::build(m, &d).unwrap_err().to_string();
        assert!(e.contains(needle), "`{e}` should contain `{needle}`");
    }

    #[test]
    fn rejects_malformed_contracts_with_actionable_errors() {
        rejects(|m, _| m.midgard = "0.1".into(), "midgard");
        rejects(|m, _| m.capability = "a.b".into(), "capability");
        rejects(|m, _| m.service = "Bad".into(), "service");
        rejects(
            |m, _| m.interfaces[1].id = "rest".into(),
            "duplicate interface id",
        );
        rejects(
            |m, _| m.interfaces[0].specification.operation = "nope".into(),
            "operationId `nope` not found",
        );
        rejects(
            |m, _| m.interfaces[1].specification.operation = "nope".into(),
            "AsyncAPI operation `nope` not found",
        );
        rejects(
            |m, _| m.interfaces[0].specification.kind = SpecType::Asyncapi,
            "requires specification type `openapi`",
        );
        rejects(
            |m, _| m.interfaces[0].specification.document = Some("http://evil/x.yaml".into()),
            "relative path",
        );
        rejects(
            |m, _| m.interfaces[0].specification.document = Some("../etc/passwd".into()),
            "relative path",
        );
        rejects(
            |m, _| m.interfaces[0].specification.document = Some("missing.yaml".into()),
            "not supplied",
        );
        rejects(
            |m, _| {
                m.interfaces[1].authorization = Some(InterfaceAuthorization {
                    callers: vec![Caller::Service, Caller::User],
                });
                m.authorization.callers = vec![Caller::Service];
            },
            "never widen",
        );
        rejects(
            |_, d| {
                d.insert("openapi.yaml".into(), OPENAPI.replace("3.1.0", "3.0.3"));
            },
            "3.1",
        );
        rejects(
            |_, d| {
                d.insert("asyncapi.yaml".into(), ASYNCAPI.replace("3.0.0", "2.6.0"));
            },
            "3.0",
        );
        rejects(
            |_, d| {
                d.insert("asyncapi.yaml".into(), "{[".into());
            },
            "not valid YAML",
        );
        rejects(
            |m, _| {
                m.execution = Some(Execution {
                    outcome: Outcome::Completed,
                    idempotency: Idempotency::None,
                })
            },
            "must not use 202",
        );
    }

    #[test]
    fn non_json_media_types_are_represented() {
        let mut d = docs();
        d.insert(
            "openapi.yaml".into(),
            OPENAPI.replace("application/json:", "application/xml:"),
        );
        let b = BundleV02::build(manifest(), &d).unwrap();
        assert_eq!(
            b.interfaces[0].summary["request_media_types"],
            json!(["application/xml"])
        );
    }

    #[test]
    fn interface_authorization_narrows() {
        let b = BundleV02::build(manifest(), &docs()).unwrap();
        let q = b.interfaces.iter().find(|i| i.id == "queue").unwrap();
        assert_eq!(q.callers, vec![Caller::Service]);
        let r = b.interfaces.iter().find(|i| i.id == "rest").unwrap();
        assert_eq!(r.callers, vec![Caller::Service, Caller::User]);
    }
}
