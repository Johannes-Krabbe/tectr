//! Logging in to HTTP MCP servers, following the MCP authorization spec: OAuth 2.1 with PKCE,
//! the server and its authorization server found through their metadata, and tectr registered
//! as a client by itself where the server allows it.
//!
//! This module only talks HTTP. The daemon keeps the logins and decides when to use them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A token this close to expiring is refreshed before use, so a request doesn't carry one that
/// runs out on its way.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// What the daemon keeps for one server after a login. It never leaves the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// The server the tokens are for, sent with every token request.
    pub resource: String,
    pub redirect_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
}

impl Login {
    /// Whether the access token should be refreshed before it's used.
    pub fn is_stale(&self, now_ms: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|at| at - now_ms < REFRESH_MARGIN.as_millis() as i64)
    }
}

/// A login started in a browser and not finished yet.
#[derive(Debug, Clone)]
pub struct Pending {
    pub verifier: String,
    /// Everything but the tokens, which the code is exchanged for.
    pub login: Login,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoints {
    authorization: String,
    token: String,
    registration: Option<String>,
    scope: Option<String>,
    resource: String,
}

/// Finds where to log in, registers tectr if needed, and returns the address to open in the
/// browser with what finishing the login needs. `client_id` is one registered by hand, and
/// `previous` an earlier login whose registration can be used again.
pub async fn start(
    http: &reqwest::Client,
    server_url: &str,
    headers: &[(String, String)],
    client_id: Option<&str>,
    previous: Option<&Login>,
    redirect_uri: &str,
    state: &str,
) -> Result<(String, Pending)> {
    let endpoints = discover(http, server_url, headers).await?;
    let (client_id, client_secret) = match (client_id, previous) {
        (Some(id), _) => (id.to_owned(), None),
        (None, Some(previous))
            if previous.redirect_uri == redirect_uri
                && previous.token_endpoint == endpoints.token =>
        {
            (previous.client_id.clone(), previous.client_secret.clone())
        }
        (None, _) => {
            let Some(registration) = &endpoints.registration else {
                bail!(
                    "this server doesn't register clients by itself. Register tectr with it, with {redirect_uri} as the redirect address, and set the client ID"
                );
            };
            register(http, registration, redirect_uri).await?
        }
    };

    let verifier = format!("{}{}", crate::new_id(), crate::new_id());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&endpoints.authorization)
        .context("the authorization server gave an invalid address")?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("resource", &endpoints.resource);
        if let Some(scope) = &endpoints.scope {
            query.append_pair("scope", scope);
        }
    }
    let pending = Pending {
        verifier,
        login: Login {
            token_endpoint: endpoints.token,
            client_id,
            client_secret,
            resource: endpoints.resource,
            redirect_uri: redirect_uri.to_owned(),
            access_token: None,
            refresh_token: None,
            expires_at_ms: None,
        },
    };
    Ok((url.to_string(), pending))
}

/// Trades the code from the redirect for tokens.
pub async fn finish(http: &reqwest::Client, pending: Pending, code: &str) -> Result<Login> {
    let login = pending.login;
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code.to_owned()),
        ("redirect_uri", login.redirect_uri.clone()),
        ("client_id", login.client_id.clone()),
        ("code_verifier", pending.verifier),
        ("resource", login.resource.clone()),
    ];
    if let Some(secret) = &login.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let tokens = token_request(http, &login.token_endpoint, &form)
        .await
        .map_err(|error| anyhow!("{error}"))?;
    Ok(with_tokens(login, tokens))
}

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    /// The server won't refresh this login any more, so someone has to log in again.
    #[error("the login has expired")]
    Expired,
    #[error("{0}")]
    Failed(String),
}

pub async fn refresh(http: &reqwest::Client, login: &Login) -> Result<Login, RefreshError> {
    let Some(refresh_token) = &login.refresh_token else {
        return Err(RefreshError::Expired);
    };
    let mut form = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh_token.clone()),
        ("client_id", login.client_id.clone()),
        ("resource", login.resource.clone()),
    ];
    if let Some(secret) = &login.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let tokens = token_request(http, &login.token_endpoint, &form).await?;
    let mut refreshed = with_tokens(login.clone(), tokens);
    // A server that doesn't rotate refresh tokens leaves the old one out.
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = Some(refresh_token.clone());
    }
    Ok(refreshed)
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Some servers send this as a string.
    #[serde(default)]
    expires_in: Option<Value>,
}

fn with_tokens(mut login: Login, tokens: Tokens) -> Login {
    let expires_in = tokens.expires_in.and_then(|value| match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    });
    login.access_token = Some(tokens.access_token);
    login.refresh_token = tokens.refresh_token;
    login.expires_at_ms = expires_in.map(|seconds| now_ms() + seconds * 1000);
    login
}

async fn token_request(
    http: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, String)],
) -> Result<Tokens, RefreshError> {
    let response = http
        .post(endpoint)
        .header("accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|error| RefreshError::Failed(format!("the token request failed: {error}")))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return serde_json::from_value(body).map_err(|error| {
            RefreshError::Failed(format!("the server sent tokens tectr can't read: {error}"))
        });
    }
    let code = body.get("error").and_then(Value::as_str).unwrap_or("");
    if code == "invalid_grant" {
        return Err(RefreshError::Expired);
    }
    let detail = body
        .get("error_description")
        .and_then(Value::as_str)
        .unwrap_or(code);
    Err(RefreshError::Failed(format!(
        "the server refused the token request ({status}): {detail}"
    )))
}

async fn register(
    http: &reqwest::Client,
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>)> {
    let response = http
        .post(endpoint)
        .json(&json!({
            "client_name": "tectr",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .context("registering tectr with the server failed")?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        bail!("the server refused to register tectr ({status}): {detail}");
    }
    let client_id = body
        .get("client_id")
        .and_then(Value::as_str)
        .context("the server registered tectr without a client ID")?;
    let secret = body
        .get("client_secret")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((client_id.to_owned(), secret))
}

/// What the server says when asked without a login, as in the spec's first step.
#[derive(Debug, Default, PartialEq, Eq)]
struct Challenge {
    resource_metadata: Option<String>,
    scope: Option<String>,
}

/// The initialize request an MCP client starts with. Servers that need a login refuse it.
pub fn initialize_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "tectr", "version": env!("CARGO_PKG_VERSION") },
        },
    })
}

/// Asks the server without a login. `Some` means it wants one.
pub async fn challenge(
    http: &reqwest::Client,
    server_url: &str,
    headers: &[(String, String)],
) -> Result<Option<String>> {
    let mut request = http
        .post(server_url)
        .header("accept", "application/json, text/event-stream")
        .json(&initialize_body());
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.send().await.context("the server didn't answer")?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    Ok(Some(
        response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    ))
}

fn parse_challenge(header: &str) -> Challenge {
    let param = |name: &str| {
        let start = header.find(&format!("{name}=\""))? + name.len() + 2;
        let end = header[start..].find('"')?;
        Some(header[start..start + end].to_owned())
    };
    Challenge {
        resource_metadata: param("resource_metadata"),
        scope: param("scope"),
    }
}

/// `https://host/.well-known/<name>/path`, then without the path, as RFC 9728 and RFC 8414 put
/// it.
fn well_known(url: &Url, name: &str) -> Vec<String> {
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    let mut urls = vec![];
    if !path.is_empty() {
        urls.push(format!("{origin}/.well-known/{name}{path}"));
    }
    urls.push(format!("{origin}/.well-known/{name}"));
    urls
}

fn authorization_server_metadata(issuer: &Url) -> Vec<String> {
    let origin = issuer.origin().ascii_serialization();
    let path = issuer.path().trim_end_matches('/');
    if path.is_empty() {
        return vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ];
    }
    vec![
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
        format!("{origin}{path}/.well-known/openid-configuration"),
    ]
}

async fn fetch_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http
        .get(url)
        .header("accept", "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

async fn discover(
    http: &reqwest::Client,
    server_url: &str,
    headers: &[(String, String)],
) -> Result<Endpoints> {
    let server = Url::parse(server_url).context("the server's address isn't valid")?;
    let challenge = challenge(http, server_url, headers)
        .await?
        .map(|header| parse_challenge(&header))
        .unwrap_or_default();

    let mut resource_urls: Vec<String> = challenge.resource_metadata.clone().into_iter().collect();
    resource_urls.extend(well_known(&server, "oauth-protected-resource"));
    let mut resource_metadata = None;
    for url in &resource_urls {
        if let Some(found) = fetch_json(http, url).await
            && found.get("authorization_servers").is_some()
        {
            resource_metadata = Some(found);
            break;
        }
    }

    let strings = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    // Without resource metadata, the 2025-03-26 spec has the server's own origin authorize.
    let (issuer, scopes, resource) = match &resource_metadata {
        Some(found) => {
            let issuer = strings(found.get("authorization_servers"))
                .into_iter()
                .next()
                .context("the server names no authorization server")?;
            let resource = found
                .get("resource")
                .and_then(Value::as_str)
                .unwrap_or(server_url)
                .to_owned();
            (issuer, strings(found.get("scopes_supported")), resource)
        }
        None => (
            server.origin().ascii_serialization(),
            vec![],
            server_url.to_owned(),
        ),
    };
    let scope = challenge
        .scope
        .or_else(|| (!scopes.is_empty()).then(|| scopes.join(" ")));
    let issuer_url =
        Url::parse(&issuer).context("the authorization server's address isn't valid")?;

    for url in authorization_server_metadata(&issuer_url) {
        let Some(found) = fetch_json(http, &url).await else {
            continue;
        };
        let text = |key| found.get(key).and_then(Value::as_str).map(str::to_owned);
        if let (Some(authorization), Some(token)) =
            (text("authorization_endpoint"), text("token_endpoint"))
        {
            return Ok(Endpoints {
                authorization,
                token,
                registration: text("registration_endpoint"),
                scope,
                resource,
            });
        }
    }
    if resource_metadata.is_some() {
        bail!("the authorization server {issuer} doesn't describe itself");
    }
    let origin = server.origin().ascii_serialization();
    Ok(Endpoints {
        authorization: format!("{origin}/authorize"),
        token: format!("{origin}/token"),
        registration: Some(format!("{origin}/register")),
        scope,
        resource,
    })
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_names_its_metadata_and_scope() {
        let header = r#"Bearer error="invalid_token", resource_metadata="https://mcp.example/.well-known/oauth-protected-resource", scope="read write""#;
        assert_eq!(
            parse_challenge(header),
            Challenge {
                resource_metadata: Some(
                    "https://mcp.example/.well-known/oauth-protected-resource".to_owned()
                ),
                scope: Some("read write".to_owned()),
            }
        );
        assert_eq!(parse_challenge("Bearer"), Challenge::default());
    }

    #[test]
    fn metadata_is_looked_for_with_the_path_first() {
        let url = Url::parse("https://mcp.example/v1/mcp").unwrap();
        assert_eq!(
            well_known(&url, "oauth-protected-resource"),
            [
                "https://mcp.example/.well-known/oauth-protected-resource/v1/mcp",
                "https://mcp.example/.well-known/oauth-protected-resource",
            ]
        );
        let issuer = Url::parse("https://auth.example/tenant/").unwrap();
        assert_eq!(
            authorization_server_metadata(&issuer),
            [
                "https://auth.example/.well-known/oauth-authorization-server/tenant",
                "https://auth.example/.well-known/openid-configuration/tenant",
                "https://auth.example/tenant/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn a_token_is_refreshed_shortly_before_it_expires() {
        let login = Login {
            token_endpoint: String::new(),
            client_id: String::new(),
            client_secret: None,
            resource: String::new(),
            redirect_uri: String::new(),
            access_token: Some("a".into()),
            refresh_token: None,
            expires_at_ms: Some(100_000),
        };
        assert!(!login.is_stale(0));
        assert!(login.is_stale(50_000));
        assert!(
            !Login {
                expires_at_ms: None,
                ..login
            }
            .is_stale(i64::MAX)
        );
    }
}
