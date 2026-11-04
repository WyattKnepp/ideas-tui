//! Typed client for the pipeline's client API. UI-free so it can be tested
//! against a mock HTTP server.

use std::fmt;
use std::time::Duration;

use reqwest::{Method, StatusCode, redirect};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::config::Config;
use crate::types::*;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// 401/403, or a redirect to a login page (wrong prefix).
    Unauthorized,
    /// Any other non-2xx answer, with the server's `{error}` message.
    Http { status: u16, message: String },
    Network(String),
    /// A 2xx answer that was not the JSON we expected.
    Decode(String),
}

impl ApiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Unauthorized => Some(401),
            ApiError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "Unauthorized: check CLIENT_TOKEN and API_PREFIX in your config"),
            ApiError::Http { status, message } if *status == 409 => write!(f, "{message}"),
            ApiError::Http { status, message } if *status >= 500 => write!(f, "Server error ({status}): {message}"),
            ApiError::Http { status, message } => write!(f, "{message} ({status})"),
            ApiError::Network(e) => write!(f, "Can't reach the server: {e}"),
            ApiError::Decode(e) => write!(f, "Unexpected response ({e}); check SERVER_URL and API_PREFIX"),
        }
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    pub fn new(config: &Config) -> Client {
        Client::with_base(config.base_url(), config.token.clone())
    }

    pub fn with_base(base: String, token: String) -> Client {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // Grilling turns shell out to Claude and can take minutes.
            .timeout(Duration::from_secs(600))
            // A redirect means a login page got in the way; report it, don't follow it.
            .redirect(redirect::Policy::none())
            .user_agent(concat!("ideas-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Client { http, base: base.trim_end_matches('/').to_string(), token }
    }

    async fn call<T: DeserializeOwned>(&self, method: Method, path: &str, body: Option<Value>) -> ApiResult<T> {
        let url = format!("{}{}", self.base, path);
        let mut req = self.http.request(method, &url).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| ApiError::Network(describe(&e)))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| ApiError::Network(describe(&e)))?;
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN || status.is_redirection() {
            return Err(ApiError::Unauthorized);
        }
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str().map(str::to_string)))
                .unwrap_or_else(|| {
                    let t = text.trim();
                    if t.is_empty() || t.starts_with('<') {
                        status.canonical_reason().unwrap_or("request failed").to_string()
                    } else {
                        t.chars().take(200).collect()
                    }
                });
            return Err(ApiError::Http { status: status.as_u16(), message });
        }
        serde_json::from_str(&text).map_err(|e| ApiError::Decode(e.to_string()))
    }

    pub async fn list_ideas(&self) -> ApiResult<Vec<Idea>> {
        self.call(Method::GET, "/api/ideas", None).await
    }

    pub async fn get_idea(&self, id: &str) -> ApiResult<Idea> {
        self.call(Method::GET, &format!("/api/ideas/{id}"), None).await
    }

    pub async fn create_idea(&self, text: &str) -> ApiResult<Idea> {
        self.call(Method::POST, "/api/ideas", Some(json!({ "text": text }))).await
    }

    pub async fn set_archived(&self, id: &str, archived: bool) -> ApiResult<Idea> {
        self.call(Method::PATCH, &format!("/api/ideas/{id}"), Some(json!({ "archived": archived }))).await
    }

    pub async fn delete_idea(&self, id: &str) -> ApiResult<()> {
        self.call::<Value>(Method::DELETE, &format!("/api/ideas/{id}"), None).await.map(|_| ())
    }

    /// Starts the grill (`message` empty on a fresh idea) or sends a turn.
    pub async fn grill(&self, id: &str, message: &str) -> ApiResult<Idea> {
        self.call(Method::POST, &format!("/api/ideas/{id}/grill"), Some(json!({ "message": message }))).await
    }

    pub async fn cancel_grill(&self, id: &str) -> ApiResult<Idea> {
        self.call(Method::POST, &format!("/api/ideas/{id}/cancel-grill"), Some(json!({}))).await
    }

    /// Explanation of one open question.
    pub async fn elaborate(&self, id: &str, qid: &str) -> ApiResult<String> {
        let v: Value = self.call(Method::POST, &format!("/api/ideas/{id}/elaborate"), Some(json!({ "qid": qid }))).await?;
        Ok(v.get("text").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    pub async fn write_handoff(&self, id: &str) -> ApiResult<Idea> {
        self.call(Method::POST, &format!("/api/ideas/{id}/handoff"), Some(json!({}))).await
    }

    pub async fn send(&self, id: &str, req: &SendRequest) -> ApiResult<Idea> {
        let body = serde_json::to_value(req).map_err(|e| ApiError::Decode(e.to_string()))?;
        self.call(Method::POST, &format!("/api/ideas/{id}/send"), Some(body)).await
    }

    pub async fn unsend(&self, id: &str) -> ApiResult<Idea> {
        self.call(Method::POST, &format!("/api/ideas/{id}/unsend"), Some(json!({}))).await
    }

    pub async fn job_action(&self, id: &str, action: JobAction) -> ApiResult<Idea> {
        self.call(Method::POST, &format!("/api/ideas/{id}/job/actions"), Some(json!({ "action": action.as_str() }))).await
    }

    /// Names of the projects the server knows (decides `isNew`).
    pub async fn projects(&self) -> ApiResult<Vec<String>> {
        let p: Projects = self.call(Method::GET, "/api/projects", None).await?;
        Ok(p.projects.into_iter().map(|p| p.name).collect())
    }
}

fn describe(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "request timed out".into()
    } else if e.is_connect() {
        "connection failed".into()
    } else {
        // Walk to the root cause; reqwest's top-level text is often just "error sending request".
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            msg = s.to_string();
            src = s.source();
        }
        msg
    }
}
