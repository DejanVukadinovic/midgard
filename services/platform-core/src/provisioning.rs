//! Identity-provider provisioning. `Link` attaches pre-existing IdP projects/clients;
//! `Zitadel` creates them through the ZITADEL management API (idempotent search-then-create).
//!
//! NOTE: the ZITADEL endpoints below follow the documented management API v1 but have not yet
//! been exercised against a live pinned release (see docs/adr/ADR-002 and the Phase-0 spike).
//! Unit tests cover the control flow against a fake server.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClientSpec {
    pub kind: String,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    /// Required in `link` mode: the existing IdP client id.
    #[serde(default)]
    pub zitadel_client_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    pub slug: String,
    pub display_name: String,
    pub existing_project_id: Option<String>,
    pub clients: Vec<ClientSpec>,
}

#[derive(Debug, Clone)]
pub struct Provisioned {
    pub project_id: String,
    /// (kind, client_id) in request order.
    pub clients: Vec<(String, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("identity provider error: {0}")]
    Idp(String),
}

pub enum Provisioner {
    Link,
    Zitadel(ZitadelClient),
}

impl Provisioner {
    /// Cheap validation performed before any state is created.
    pub fn validate(&self, req: &ProvisionRequest) -> Result<(), ProvisionError> {
        if let Provisioner::Link = self {
            if req.existing_project_id.is_none() {
                return Err(ProvisionError::Invalid(
                    "zitadel_project_id is required in link mode".into(),
                ));
            }
            if req.clients.iter().any(|c| c.zitadel_client_id.is_none()) {
                return Err(ProvisionError::Invalid(
                    "zitadel_client_id is required for every client in link mode".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn provision(&self, req: &ProvisionRequest) -> Result<Provisioned, ProvisionError> {
        match self {
            Provisioner::Link => Ok(Provisioned {
                project_id: req
                    .existing_project_id
                    .clone()
                    .ok_or_else(|| ProvisionError::Invalid("zitadel_project_id missing".into()))?,
                clients: req
                    .clients
                    .iter()
                    .map(|c| {
                        Ok((
                            c.kind.clone(),
                            c.zitadel_client_id.clone().ok_or_else(|| {
                                ProvisionError::Invalid("zitadel_client_id missing".into())
                            })?,
                        ))
                    })
                    .collect::<Result<_, ProvisionError>>()?,
            }),
            Provisioner::Zitadel(z) => z.provision(req).await,
        }
    }
}

pub struct ZitadelClient {
    http: reqwest::Client,
    base: String,
    token: String,
    org_id: Option<String>,
}

impl ZitadelClient {
    pub fn new(base: &str, token: String, org_id: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("client"),
            base: base.trim_end_matches('/').to_string(),
            token,
            org_id,
        }
    }

    async fn call(&self, path: &str, body: Value) -> Result<Value, ProvisionError> {
        let mut rb = self
            .http
            .post(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .json(&body);
        if let Some(o) = &self.org_id {
            rb = rb.header("x-zitadel-orgid", o);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| ProvisionError::Idp(format!("request failed: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // Do not echo bodies into errors that may reach logs/clients beyond the message.
            return Err(ProvisionError::Idp(format!("{path} returned {status}")));
        }
        Ok(v)
    }

    async fn ensure_project(&self, name: &str) -> Result<String, ProvisionError> {
        let found = self
            .call("/management/v1/projects/_search", json!({"queries":[{"nameQuery":{"name":name,"method":"TEXT_QUERY_METHOD_EQUALS"}}]}))
            .await?;
        if let Some(id) = found.pointer("/result/0/id").and_then(Value::as_str) {
            return Ok(id.to_string());
        }
        let created = self
            .call("/management/v1/projects", json!({"name":name,"projectRoleAssertion":true,"projectRoleCheck":true,"hasProjectCheck":true}))
            .await?;
        created
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ProvisionError::Idp("project id missing in response".into()))
    }

    async fn ensure_client(
        &self,
        project: &str,
        app_name: &str,
        c: &ClientSpec,
    ) -> Result<String, ProvisionError> {
        let found = self
            .call(
                &format!("/management/v1/projects/{project}/apps/_search"),
                json!({"queries":[{"nameQuery":{"name":app_name,"method":"TEXT_QUERY_METHOD_EQUALS"}}]}),
            )
            .await?;
        for ptr in [
            "/result/0/oidcConfig/clientId",
            "/result/0/apiConfig/clientId",
        ] {
            if let Some(id) = found.pointer(ptr).and_then(Value::as_str) {
                return Ok(id.to_string());
            }
        }
        let (path, body) = match c.kind.as_str() {
            "backend" => (
                format!("/management/v1/projects/{project}/apps/api"),
                json!({"name":app_name,"authMethodType":"API_AUTH_METHOD_TYPE_PRIVATE_KEY_JWT"}),
            ),
            kind => (
                format!("/management/v1/projects/{project}/apps/oidc"),
                json!({
                    "name": app_name,
                    "redirectUris": c.redirect_uris,
                    "responseTypes": ["OIDC_RESPONSE_TYPE_CODE"],
                    "grantTypes": ["OIDC_GRANT_TYPE_AUTHORIZATION_CODE"],
                    "appType": if kind == "mobile" { "OIDC_APP_TYPE_NATIVE" } else { "OIDC_APP_TYPE_USER_AGENT" },
                    "authMethodType": "OIDC_AUTH_METHOD_TYPE_NONE",
                    "accessTokenType": "OIDC_TOKEN_TYPE_JWT",
                }),
            ),
        };
        let created = self.call(&path, body).await?;
        created
            .get("clientId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ProvisionError::Idp("client id missing in response".into()))
    }

    async fn provision(&self, req: &ProvisionRequest) -> Result<Provisioned, ProvisionError> {
        let project_id = match &req.existing_project_id {
            Some(p) => p.clone(),
            None => self.ensure_project(&req.slug).await?,
        };
        let mut clients = Vec::new();
        for c in &req.clients {
            let id = match &c.zitadel_client_id {
                Some(id) => id.clone(),
                None => {
                    self.ensure_client(&project_id, &format!("{}-{}", req.slug, c.kind), c)
                        .await?
                }
            };
            clients.push((c.kind.clone(), id));
        }
        Ok(Provisioned {
            project_id,
            clients,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Fake {
        projects: Vec<(String, String)>,
        apps: Vec<(String, String)>,
        creates: u32,
    }

    async fn serve() -> (String, Arc<Mutex<Fake>>) {
        let st = Arc::new(Mutex::new(Fake::default()));
        let app = Router::new()
            .route(
                "/management/v1/projects/_search",
                post(|State(s): State<Arc<Mutex<Fake>>>, Json(b): Json<Value>| async move {
                    let name = b.pointer("/queries/0/nameQuery/name").and_then(Value::as_str).unwrap().to_string();
                    let f = s.lock().unwrap();
                    Json(json!({"result": f.projects.iter().filter(|p| p.1 == name).map(|p| json!({"id": p.0})).collect::<Vec<_>>()}))
                }),
            )
            .route(
                "/management/v1/projects",
                post(|State(s): State<Arc<Mutex<Fake>>>, Json(b): Json<Value>| async move {
                    let mut f = s.lock().unwrap();
                    f.creates += 1;
                    let id = format!("proj-{}", f.projects.len() + 1);
                    f.projects.push((id.clone(), b["name"].as_str().unwrap().to_string()));
                    Json(json!({"id": id}))
                }),
            )
            .route(
                "/management/v1/projects/{p}/apps/_search",
                post(|State(s): State<Arc<Mutex<Fake>>>, Json(b): Json<Value>| async move {
                    let name = b.pointer("/queries/0/nameQuery/name").and_then(Value::as_str).unwrap().to_string();
                    let f = s.lock().unwrap();
                    Json(json!({"result": f.apps.iter().filter(|a| a.1 == name).map(|a| json!({"oidcConfig":{"clientId": a.0}})).collect::<Vec<_>>()}))
                }),
            )
            .route(
                "/management/v1/projects/{p}/apps/oidc",
                post(|State(s): State<Arc<Mutex<Fake>>>, Json(b): Json<Value>| async move {
                    let mut f = s.lock().unwrap();
                    f.creates += 1;
                    let id = format!("client-{}", f.apps.len() + 1);
                    f.apps.push((id.clone(), b["name"].as_str().unwrap().to_string()));
                    Json(json!({"clientId": id}))
                }),
            )
            .route(
                "/management/v1/projects/{p}/apps/api",
                post(|State(s): State<Arc<Mutex<Fake>>>, Json(b): Json<Value>| async move {
                    let mut f = s.lock().unwrap();
                    f.creates += 1;
                    let id = format!("client-{}", f.apps.len() + 1);
                    f.apps.push((id.clone(), b["name"].as_str().unwrap().to_string()));
                    Json(json!({"clientId": id}))
                }),
            )
            .with_state(st.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.ok() });
        (url, st)
    }

    #[tokio::test]
    async fn zitadel_provisioning_is_idempotent() {
        let (url, st) = serve().await;
        let p = Provisioner::Zitadel(ZitadelClient::new(&url, "t".into(), None));
        let req = ProvisionRequest {
            slug: "obroklik".into(),
            display_name: "Obroklik".into(),
            existing_project_id: None,
            clients: vec![
                ClientSpec {
                    kind: "web".into(),
                    redirect_uris: vec!["https://o.example/cb".into()],
                    zitadel_client_id: None,
                },
                ClientSpec {
                    kind: "backend".into(),
                    redirect_uris: vec![],
                    zitadel_client_id: None,
                },
            ],
        };
        let a = p.provision(&req).await.unwrap();
        let b = p.provision(&req).await.unwrap();
        assert_eq!(a.project_id, b.project_id);
        assert_eq!(a.clients, b.clients);
        assert_eq!(st.lock().unwrap().creates, 3, "second run must only search");
    }

    #[test]
    fn link_mode_requires_ids() {
        let req = ProvisionRequest {
            slug: "a".into(),
            display_name: "A".into(),
            existing_project_id: None,
            clients: vec![],
        };
        assert!(Provisioner::Link.validate(&req).is_err());
    }
}
