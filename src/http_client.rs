use anyhow::{Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use serde::Serialize;
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{info, instrument};

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// How to authenticate requests.
pub enum AuthStrategy {
    /// Bearer token (Authorization: Bearer <token>)
    Bearer(String),
    /// Basic auth (username only, no password)
    BasicUsername(String),
}

#[cfg(test)]
#[derive(Debug)]
pub struct MockCall {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub body: String,
    pub body_contains: Option<String>,
    pub body_not_contains: Option<String>,
}

#[cfg(test)]
impl MockCall {
    pub fn new(method: &str, path: &str, status: u16, body: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            status,
            body: body.to_string(),
            body_contains: None,
            body_not_contains: None,
        }
    }
}

pub struct ApiClient {
    client: Client,
    auth: AuthStrategy,
    base_url: String,
    log_target: &'static str,
    #[cfg(test)]
    mock_calls: Option<Arc<Mutex<VecDeque<MockCall>>>>,
}

impl ApiClient {
    pub fn new(auth: AuthStrategy, base_url: String, log_target: &'static str) -> Self {
        Self {
            client: Client::new(),
            auth,
            base_url,
            log_target,
            #[cfg(test)]
            mock_calls: None,
        }
    }

    #[cfg(test)]
    pub fn with_mock_calls(
        auth: AuthStrategy,
        base_url: String,
        log_target: &'static str,
        calls: Vec<MockCall>,
    ) -> Self {
        let mut client = Self::new(auth, base_url, log_target);
        client.mock_calls = Some(Arc::new(Mutex::new(calls.into())));
        client
    }

    fn apply_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.auth {
            AuthStrategy::Bearer(token) => builder.bearer_auth(token),
            AuthStrategy::BasicUsername(username) => {
                builder.basic_auth(username, Option::<&str>::None)
            }
        }
    }

    #[cfg(test)]
    fn maybe_mock(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<Option<HttpResponse>> {
        let Some(calls) = &self.mock_calls else {
            return Ok(None);
        };
        let mut calls = calls.lock().unwrap();
        let call = calls
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected request: {method} {path}"));
        assert_eq!(call.method, method);
        assert_eq!(call.path, path);
        let req_body = body.unwrap_or("");
        if let Some(fragment) = &call.body_contains {
            assert!(
                req_body.contains(fragment.as_str()),
                "request body missing fragment `{fragment}`: {req_body}"
            );
        }
        if let Some(fragment) = &call.body_not_contains {
            assert!(
                !req_body.contains(fragment.as_str()),
                "request body unexpectedly contains `{fragment}`: {req_body}"
            );
        }
        Ok(Some(HttpResponse {
            status: call.status,
            body: call.body.clone(),
        }))
    }

    #[cfg(not(test))]
    fn maybe_mock(
        &self,
        _method: &str,
        _path: &str,
        _body: Option<&str>,
    ) -> Result<Option<HttpResponse>> {
        Ok(None)
    }

    #[instrument(level = "debug", skip(self), fields(path = path))]
    pub fn get(&self, path: &str) -> Result<HttpResponse> {
        let start = Instant::now();
        if let Some(resp) = self.maybe_mock("GET", path, None)? {
            info!(
                method = "GET",
                path = path,
                status = resp.status,
                mock = true,
                duration_ms = start.elapsed().as_millis(),
                self.log_target
            );
            return Ok(resp);
        }
        let resp = self
            .apply_auth(self.client.get(format!("{}{}", self.base_url, path)))
            .send()
            .context("HTTP request failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        info!(
            method = "GET",
            path = path,
            status = status,
            duration_ms = start.elapsed().as_millis(),
            self.log_target
        );
        Ok(HttpResponse { status, body })
    }

    #[instrument(level = "debug", skip(self, body), fields(path = path))]
    pub fn post_json<T: Serialize>(&self, path: &str, body: &T) -> Result<HttpResponse> {
        let start = Instant::now();
        let request_body = serde_json::to_string(body)?;
        if let Some(resp) = self.maybe_mock("POST", path, Some(&request_body))? {
            info!(
                method = "POST",
                path = path,
                status = resp.status,
                mock = true,
                duration_ms = start.elapsed().as_millis(),
                self.log_target
            );
            return Ok(resp);
        }
        let resp = self
            .apply_auth(self.client.post(format!("{}{}", self.base_url, path)))
            .json(body)
            .send()
            .context("HTTP request failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        info!(
            method = "POST",
            path = path,
            status = status,
            duration_ms = start.elapsed().as_millis(),
            self.log_target
        );
        Ok(HttpResponse { status, body })
    }

    #[instrument(level = "debug", skip(self), fields(path = path))]
    pub fn delete(&self, path: &str) -> Result<HttpResponse> {
        let start = Instant::now();
        if let Some(resp) = self.maybe_mock("DELETE", path, None)? {
            info!(
                method = "DELETE",
                path = path,
                status = resp.status,
                mock = true,
                duration_ms = start.elapsed().as_millis(),
                self.log_target
            );
            return Ok(resp);
        }
        let resp = self
            .apply_auth(self.client.delete(format!("{}{}", self.base_url, path)))
            .send()
            .context("HTTP request failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        info!(
            method = "DELETE",
            path = path,
            status = status,
            duration_ms = start.elapsed().as_millis(),
            self.log_target
        );
        Ok(HttpResponse { status, body })
    }
}
