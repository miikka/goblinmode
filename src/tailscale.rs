use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::{Arc, Mutex};

const BASE_URL: &str = "https://api.tailscale.com/api/v2";

pub struct TailscaleClient {
    client: Client,
    api_key: String,
    base_url: String,
    #[cfg(test)]
    mock_calls: Option<Arc<Mutex<VecDeque<MockCall>>>>,
}

struct HttpResponse {
    status: u16,
    body: String,
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

#[cfg(test)]
#[derive(Debug)]
struct MockCall {
    method: String,
    path: String,
    status: u16,
    body: String,
    body_contains: Option<String>,
    body_not_contains: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DevicesResponse {
    devices: Vec<Device>,
}

#[derive(Debug, Deserialize)]
struct Device {
    id: String,
    hostname: String,
}

impl TailscaleClient {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: BASE_URL.to_string(),
            #[cfg(test)]
            mock_calls: None,
        }
    }

    #[cfg(test)]
    fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url,
            mock_calls: None,
        }
    }

    #[cfg(test)]
    fn with_mock_calls(api_key: String, base_url: String, calls: Vec<MockCall>) -> Self {
        let mut client = Self::with_base_url(api_key, base_url);
        client.mock_calls = Some(Arc::new(Mutex::new(calls.into())));
        client
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
        if let Some(fragment) = call.body_contains {
            assert!(
                req_body.contains(&fragment),
                "request body missing fragment `{fragment}`: {req_body}"
            );
        }
        if let Some(fragment) = call.body_not_contains {
            assert!(
                !req_body.contains(&fragment),
                "request body unexpectedly contains `{fragment}`: {req_body}"
            );
        }
        Ok(Some(HttpResponse {
            status: call.status,
            body: call.body,
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

    /// Delete a device from the tailnet by its hostname.
    pub fn delete_device_by_hostname(&self, hostname: &str) -> Result<()> {
        let device_id = self.find_device_by_hostname(hostname)?;
        match device_id {
            Some(id) => {
                self.delete_device_by_id(&id)?;
                println!("Tailscale device '{}' deleted.", hostname);
            }
            None => {
                println!(
                    "Tailscale device '{}' not found (already removed?).",
                    hostname
                );
            }
        }
        Ok(())
    }

    /// List all devices whose hostname starts with "gob-".
    pub fn list_gob_devices(&self) -> Result<Vec<DeviceInfo>> {
        let devices = self.list_devices()?;
        Ok(devices
            .into_iter()
            .filter(|d| d.hostname.starts_with("gob-"))
            .map(|d| DeviceInfo {
                id: d.id,
                hostname: d.hostname,
            })
            .collect())
    }

    fn get(&self, path: &str) -> Result<HttpResponse> {
        if let Some(resp) = self.maybe_mock("GET", path, None)? {
            return Ok(resp);
        }
        let resp = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .basic_auth(&self.api_key, Option::<&str>::None)
            .send()
            .context("HTTP request failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(HttpResponse { status, body })
    }

    fn delete(&self, path: &str) -> Result<HttpResponse> {
        if let Some(resp) = self.maybe_mock("DELETE", path, None)? {
            return Ok(resp);
        }
        let resp = self
            .client
            .delete(format!("{}{}", self.base_url, path))
            .basic_auth(&self.api_key, Option::<&str>::None)
            .send()
            .context("HTTP request failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(HttpResponse { status, body })
    }

    fn post_json<T: Serialize>(&self, path: &str, body: &T) -> Result<HttpResponse> {
        let request_body = serde_json::to_string(body)?;
        if let Some(resp) = self.maybe_mock("POST", path, Some(&request_body))? {
            return Ok(resp);
        }
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .basic_auth(&self.api_key, Option::<&str>::None)
            .json(body)
            .send()
            .context("HTTP request failed")?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(HttpResponse { status, body })
    }

    fn list_devices(&self) -> Result<Vec<Device>> {
        let resp = self
            .get("/tailnet/-/devices")
            .context("Failed to list Tailscale devices")?;

        if !is_success(resp.status) {
            bail!(
                "Failed to list Tailscale devices ({}): {}",
                resp.status,
                resp.body
            );
        }

        let devices: DevicesResponse = serde_json::from_str(&resp.body)
            .context("Failed to parse Tailscale devices response")?;

        Ok(devices.devices)
    }

    fn find_device_by_hostname(&self, hostname: &str) -> Result<Option<String>> {
        let devices = self.list_devices()?;
        Ok(devices
            .into_iter()
            .find(|d| d.hostname == hostname)
            .map(|d| d.id))
    }

    pub fn delete_device_by_id(&self, device_id: &str) -> Result<()> {
        let resp = self
            .delete(&format!("/device/{}", device_id))
            .context("Failed to delete Tailscale device")?;

        if !is_success(resp.status) {
            bail!(
                "Failed to delete Tailscale device ({}): {}",
                resp.status,
                resp.body
            );
        }

        Ok(())
    }
}

pub struct DeviceInfo {
    pub id: String,
    pub hostname: String,
}

impl TailscaleClient {
    /// Create a one-time preauthorized auth key via the Tailscale API.
    /// Tags are applied to the created key; if empty, the device is a user device.
    pub fn create_auth_key(&self, tags: &[String]) -> Result<String> {
        #[derive(Serialize)]
        struct DeviceCreate<'a> {
            reusable: bool,
            ephemeral: bool,
            preauthorized: bool,
            #[serde(skip_serializing_if = "slice_is_empty")]
            tags: &'a [String],
        }

        fn slice_is_empty(s: &&[String]) -> bool {
            s.is_empty()
        }

        #[derive(Serialize)]
        struct DeviceCapabilities<'a> {
            create: DeviceCreate<'a>,
        }

        #[derive(Serialize)]
        struct Capabilities<'a> {
            devices: DeviceCapabilities<'a>,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CreateKeyRequest<'a> {
            capabilities: Capabilities<'a>,
            expiry_seconds: u32,
        }

        #[derive(Deserialize)]
        struct CreateKeyResponse {
            key: String,
        }

        let request = CreateKeyRequest {
            capabilities: Capabilities {
                devices: DeviceCapabilities {
                    create: DeviceCreate {
                        reusable: false,
                        ephemeral: false,
                        preauthorized: true,
                        tags,
                    },
                },
            },
            expiry_seconds: 300, // 5 minutes — enough to provision and connect
        };

        let resp = self
            .post_json("/tailnet/-/keys", &request)
            .context("Failed to create Tailscale auth key")?;

        if !is_success(resp.status) {
            bail!(
                "Failed to create Tailscale auth key ({}): {}",
                resp.status,
                resp.body
            );
        }

        let response: CreateKeyResponse = serde_json::from_str(&resp.body)
            .context("Failed to parse Tailscale create key response")?;

        Ok(response.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(method: &str, path: &str, status: u16, body: &str) -> MockCall {
        MockCall {
            method: method.to_string(),
            path: path.to_string(),
            status,
            body: body.to_string(),
            body_contains: None,
            body_not_contains: None,
        }
    }

    #[test]
    fn list_gob_devices_filters_prefix() {
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![call(
                "GET",
                "/tailnet/-/devices",
                200,
                r#"{"devices":[{"id":"1","hostname":"gob-one"},{"id":"2","hostname":"laptop"}]}"#,
            )],
        );
        let devices = client.list_gob_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].hostname, "gob-one");
    }

    #[test]
    fn delete_device_by_hostname_deletes_when_found() {
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![
                call(
                    "GET",
                    "/tailnet/-/devices",
                    200,
                    r#"{"devices":[{"id":"dev-1","hostname":"gob-one"}]}"#,
                ),
                call("DELETE", "/device/dev-1", 200, "{}"),
            ],
        );
        client.delete_device_by_hostname("gob-one").unwrap();
    }

    #[test]
    fn delete_device_by_hostname_noop_when_missing() {
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![call(
                "GET",
                "/tailnet/-/devices",
                200,
                r#"{"devices":[{"id":"dev-1","hostname":"something-else"}]}"#,
            )],
        );
        client.delete_device_by_hostname("gob-one").unwrap();
    }

    #[test]
    fn delete_device_by_id_error_contains_body() {
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![call("DELETE", "/device/dev-1", 500, r#"{"error":"boom"}"#)],
        );
        let err = client.delete_device_by_id("dev-1").unwrap_err();
        assert!(format!("{err:#}").contains("boom"));
    }

    #[test]
    fn create_auth_key_with_tags() {
        let mut c = call("POST", "/tailnet/-/keys", 200, r#"{"key":"tskey-auth-123"}"#);
        c.body_contains = Some(r#""tags":["tag:gob"]"#.to_string());
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![c],
        );
        let key = client
            .create_auth_key(&["tag:gob".to_string()])
            .unwrap();
        assert_eq!(key, "tskey-auth-123");
    }

    #[test]
    fn create_auth_key_without_tags_omits_tags_field() {
        let mut c = call("POST", "/tailnet/-/keys", 200, r#"{"key":"tskey-auth-123"}"#);
        c.body_contains = Some(r#""expirySeconds":300"#.to_string());
        c.body_not_contains = Some(r#""tags":"#.to_string());
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![c],
        );
        let key = client.create_auth_key(&[]).unwrap();
        assert_eq!(key, "tskey-auth-123");
    }

    #[test]
    fn create_auth_key_parse_error() {
        let client = TailscaleClient::with_mock_calls(
            "api-key".to_string(),
            "mock://".to_string(),
            vec![call("POST", "/tailnet/-/keys", 200, r#"{"not_key":true}"#)],
        );
        let err = client.create_auth_key(&[]).unwrap_err();
        assert!(format!("{err:#}").contains("Failed to parse Tailscale create key response"));
    }
}
