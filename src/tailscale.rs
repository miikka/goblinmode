use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::http_client::MockCall;
use crate::http_client::{is_success, ApiClient, AuthStrategy};

const BASE_URL: &str = "https://api.tailscale.com/api/v2";

pub struct TailscaleClient {
    api: ApiClient,
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
            api: ApiClient::new(
                AuthStrategy::BasicUsername(api_key),
                BASE_URL.to_string(),
                "tailscale_http",
            ),
        }
    }

    #[cfg(test)]
    fn with_mock_calls(api_key: String, base_url: String, calls: Vec<MockCall>) -> Self {
        Self {
            api: ApiClient::with_mock_calls(
                AuthStrategy::BasicUsername(api_key),
                base_url,
                "tailscale_http",
                calls,
            ),
        }
    }

    /// Delete a device from the tailnet by its hostname.
    /// Returns `true` if the device was found and deleted, `false` if not found.
    pub fn delete_device_by_hostname(&self, hostname: &str) -> Result<bool> {
        let device_id = self.find_device_by_hostname(hostname)?;
        match device_id {
            Some(id) => {
                self.delete_device_by_id(&id)?;
                Ok(true)
            }
            None => Ok(false),
        }
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

    fn list_devices(&self) -> Result<Vec<Device>> {
        let resp = self
            .api
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
            .api
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
            .api
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
        MockCall::new(method, path, status, body)
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
        assert!(client.delete_device_by_hostname("gob-one").unwrap());
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
        assert!(!client.delete_device_by_hostname("gob-one").unwrap());
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
        let mut c = call(
            "POST",
            "/tailnet/-/keys",
            200,
            r#"{"key":"tskey-auth-123"}"#,
        );
        c.body_contains = Some(r#""tags":["tag:gob"]"#.to_string());
        let client =
            TailscaleClient::with_mock_calls("api-key".to_string(), "mock://".to_string(), vec![c]);
        let key = client.create_auth_key(&["tag:gob".to_string()]).unwrap();
        assert_eq!(key, "tskey-auth-123");
    }

    #[test]
    fn create_auth_key_without_tags_omits_tags_field() {
        let mut c = call(
            "POST",
            "/tailnet/-/keys",
            200,
            r#"{"key":"tskey-auth-123"}"#,
        );
        c.body_contains = Some(r#""expirySeconds":300"#.to_string());
        c.body_not_contains = Some(r#""tags":"#.to_string());
        let client =
            TailscaleClient::with_mock_calls("api-key".to_string(), "mock://".to_string(), vec![c]);
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
