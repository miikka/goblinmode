// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::instrument;

#[cfg(test)]
use crate::http_client::MockCall;
use crate::http_client::{is_success, ApiClient, AuthStrategy};

const BASE_URL: &str = "https://api.hetzner.cloud/v1";

pub struct HetznerClient {
    api: ApiClient,
}

#[derive(Debug, Deserialize)]
struct ServerResponse {
    server: Server,
}

#[derive(Debug, Deserialize)]
struct ServersResponse {
    servers: Vec<Server>,
}

#[derive(Debug, Deserialize)]
struct Server {
    id: u64,
    name: String,
    status: String,
    public_net: PublicNet,
    #[serde(default)]
    created: String,
}

#[derive(Debug, Deserialize)]
struct PublicNet {
    ipv4: Ipv4,
}

#[derive(Debug, Deserialize)]
struct Ipv4 {
    ip: String,
}

#[derive(Debug, Serialize)]
struct CreateServerRequest {
    name: String,
    server_type: String,
    image: String,
    location: String,
    labels: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh_keys: Option<Vec<u64>>,
}

#[derive(Debug, Deserialize)]
struct SshKeyResponse {
    ssh_key: SshKey,
}

#[derive(Debug, Deserialize)]
struct SshKeysResponse {
    ssh_keys: Vec<SshKey>,
}

#[derive(Debug, Deserialize)]
struct SshKey {
    id: u64,
    name: String,
}

impl HetznerClient {
    pub fn new(token: String) -> Self {
        Self {
            api: ApiClient::new(
                AuthStrategy::Bearer(token),
                BASE_URL.to_string(),
                "hetzner_http",
            ),
        }
    }

    #[cfg(test)]
    fn with_mock_calls(token: String, base_url: String, calls: Vec<MockCall>) -> Self {
        Self {
            api: ApiClient::with_mock_calls(
                AuthStrategy::Bearer(token),
                base_url,
                "hetzner_http",
                calls,
            ),
        }
    }

    /// Create a server. Returns (server_id, ipv4).
    pub fn create_server(
        &self,
        name: &str,
        server_type: &str,
        image: &str,
        location: &str,
        user_data: Option<&str>,
        ssh_keys: Option<Vec<u64>>,
    ) -> Result<(u64, String)> {
        let mut labels = HashMap::new();
        labels.insert("managed-by".to_string(), "goblinmode".to_string());
        let req = CreateServerRequest {
            name: name.to_string(),
            server_type: server_type.to_string(),
            image: image.to_string(),
            location: location.to_string(),
            labels,
            user_data: user_data.map(|s| s.to_string()),
            ssh_keys,
        };
        let resp = self.api.post_json("/servers", &req)?;

        if !is_success(resp.status) && resp.status != 201 {
            bail!("Failed to create server ({}): {}", resp.status, resp.body);
        }

        let created: ServerResponse =
            serde_json::from_str(&resp.body).context("Failed to parse server response")?;
        let id = created.server.id;
        let ip = created.server.public_net.ipv4.ip.clone();
        Ok((id, ip))
    }

    /// Poll until server is running. Returns the IPv4 address.
    pub fn wait_for_server(&self, server_id: u64) -> Result<String> {
        loop {
            let resp = self.api.get(&format!("/servers/{}", server_id))?;
            if !is_success(resp.status) {
                bail!("Failed to get server status: {}", resp.status);
            }
            let server: ServerResponse =
                serde_json::from_str(&resp.body).context("Failed to parse server response")?;

            match server.server.status.as_str() {
                "running" => {
                    let ip = server.server.public_net.ipv4.ip;
                    if ip != "0.0.0.0" {
                        return Ok(ip);
                    }
                }
                "initializing" | "starting" | "migrating" => {}
                status => {
                    bail!("Server entered unexpected status: {}", status);
                }
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    /// Delete a server.
    pub fn delete_server(&self, server_id: u64) -> Result<()> {
        let resp = self.api.delete(&format!("/servers/{}", server_id))?;
        if resp.status == 404 {
            bail!("Server {} not found", server_id);
        }
        if !is_success(resp.status) {
            bail!("Failed to delete server ({}): {}", resp.status, resp.body);
        }
        Ok(())
    }

    /// Find an SSH key by name, returns its ID if found.
    pub fn find_ssh_key_by_name(&self, name: &str) -> Result<Option<u64>> {
        let resp = self.api.get("/ssh_keys")?;
        if !is_success(resp.status) {
            bail!("Failed to list SSH keys: {}", resp.status);
        }
        let keys: SshKeysResponse =
            serde_json::from_str(&resp.body).context("Failed to parse SSH keys response")?;
        Ok(keys
            .ssh_keys
            .into_iter()
            .find(|k| k.name == name)
            .map(|k| k.id))
    }

    /// Upload an SSH public key. Returns the key ID.
    pub fn create_ssh_key(&self, name: &str, public_key: &str) -> Result<u64> {
        let resp = self.api.post_json(
            "/ssh_keys",
            &serde_json::json!({
                "name": name,
                "public_key": public_key
            }),
        )?;
        if !is_success(resp.status) && resp.status != 201 {
            bail!("Failed to create SSH key ({}): {}", resp.status, resp.body);
        }
        let created: SshKeyResponse =
            serde_json::from_str(&resp.body).context("Failed to parse SSH key response")?;
        Ok(created.ssh_key.id)
    }

    /// Ensure an SSH key exists in Hetzner by name, uploading it if not. Returns the key ID.
    pub fn ensure_ssh_key(&self, name: &str, public_key: &str) -> Result<u64> {
        if let Some(id) = self.find_ssh_key_by_name(name)? {
            return Ok(id);
        }
        self.create_ssh_key(name, public_key)
    }

    /// Gracefully shutdown a server.
    pub fn shutdown_server(&self, server_id: u64) -> Result<()> {
        let resp = self.api.post_json(
            &format!("/servers/{}/actions/shutdown", server_id),
            &serde_json::json!({}),
        )?;
        if !is_success(resp.status) {
            bail!("Failed to shutdown server ({}): {}", resp.status, resp.body);
        }
        Ok(())
    }

    /// Poll until server status is "off".
    pub fn wait_for_server_off(&self, server_id: u64) -> Result<()> {
        loop {
            let resp = self.api.get(&format!("/servers/{}", server_id))?;
            if !is_success(resp.status) {
                bail!("Failed to get server status: {}", resp.status);
            }
            let server: ServerResponse =
                serde_json::from_str(&resp.body).context("Failed to parse server response")?;
            if server.server.status == "off" {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    /// Create a snapshot image of a server. Returns the image ID.
    pub fn create_image(&self, server_id: u64, description: &str) -> Result<u64> {
        let resp = self.api.post_json(
            &format!("/servers/{}/actions/create_image", server_id),
            &serde_json::json!({
                "description": description,
                "type": "snapshot",
                "labels": {
                    "managed-by": "goblinmode"
                }
            }),
        )?;
        if !is_success(resp.status) && resp.status != 201 {
            bail!("Failed to create image ({}): {}", resp.status, resp.body);
        }
        let body: serde_json::Value =
            serde_json::from_str(&resp.body).context("Failed to parse create_image response")?;
        let image_id = body["image"]["id"]
            .as_u64()
            .context("Missing image id in create_image response")?;
        Ok(image_id)
    }

    /// Poll until an image is available.
    pub fn wait_for_image(&self, image_id: u64) -> Result<()> {
        loop {
            let resp = self.api.get(&format!("/images/{}", image_id))?;
            if !is_success(resp.status) {
                bail!("Failed to get image status: {}", resp.status);
            }
            let body: serde_json::Value =
                serde_json::from_str(&resp.body).context("Failed to parse image response")?;
            let status = body["image"]["status"].as_str().unwrap_or("unknown");
            if status == "available" {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    /// List all snapshots with the managed-by=goblinmode label.
    pub fn list_goblinmode_snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let resp = self
            .api
            .get("/images?type=snapshot&label_selector=managed-by%3Dgoblinmode")?;
        if !is_success(resp.status) {
            bail!("Failed to list images: {}", resp.status);
        }
        let body: serde_json::Value =
            serde_json::from_str(&resp.body).context("Failed to parse images response")?;
        let images = body["images"].as_array().context("Missing images array")?;
        let mut result = Vec::new();
        for img in images {
            let id = img["id"].as_u64().unwrap_or(0);
            let description = img["description"].as_str().unwrap_or("").to_string();
            let created = img["created"].as_str().unwrap_or("").to_string();
            if id != 0 {
                result.push(SnapshotInfo {
                    id,
                    description,
                    created,
                });
            }
        }
        Ok(result)
    }

    /// Delete an image/snapshot.
    #[instrument(level = "info", skip(self), fields(image_id = image_id))]
    pub fn delete_image(&self, image_id: u64) -> Result<()> {
        let resp = self.api.delete(&format!("/images/{}", image_id))?;
        if !is_success(resp.status) && resp.status != 404 {
            bail!("Failed to delete image ({}): {}", resp.status, resp.body);
        }
        Ok(())
    }

    /// List all servers with the managed-by=goblinmode label.
    pub fn list_goblinmode_servers(&self) -> Result<Vec<ServerInfo>> {
        let resp = self
            .api
            .get("/servers?label_selector=managed-by%3Dgoblinmode")?;
        if !is_success(resp.status) {
            bail!("Failed to list servers: {}", resp.status);
        }
        let body: ServersResponse =
            serde_json::from_str(&resp.body).context("Failed to parse servers response")?;
        Ok(body
            .servers
            .into_iter()
            .map(|s| ServerInfo {
                id: s.id,
                name: s.name,
                status: s.status,
                ipv4: s.public_net.ipv4.ip,
                created: s.created,
            })
            .collect())
    }

    /// Check if a server still exists and is running.
    pub fn get_server_status(&self, server_id: u64) -> Result<Option<(String, String)>> {
        let resp = self.api.get(&format!("/servers/{}", server_id))?;
        if resp.status == 404 {
            return Ok(None);
        }
        if !is_success(resp.status) {
            bail!("Failed to get server status: {}", resp.status);
        }
        let server: ServerResponse =
            serde_json::from_str(&resp.body).context("Failed to parse server response")?;
        Ok(Some((
            server.server.status,
            server.server.public_net.ipv4.ip,
        )))
    }
}

pub struct ServerInfo {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub ipv4: String,
    /// RFC3339 creation timestamp from Hetzner (e.g. "2024-01-15T10:30:00+00:00").
    pub created: String,
}

pub struct SnapshotInfo {
    pub id: u64,
    pub description: String,
    pub created: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(method: &str, path: &str, status: u16, body: &str) -> MockCall {
        MockCall::new(method, path, status, body)
    }

    #[test]
    fn create_server_success() {
        let mut c = call(
            "POST",
            "/servers",
            201,
            r#"{"server":{"id":42,"name":"gob-x","status":"running","public_net":{"ipv4":{"ip":"1.2.3.4"}}}}"#,
        );
        c.body_contains = Some(r#""managed-by":"goblinmode""#.to_string());
        let client =
            HetznerClient::with_mock_calls("token".to_string(), "mock://".to_string(), vec![c]);

        let (id, ip) = client
            .create_server(
                "gob-x",
                "cx23",
                "debian-13",
                "hel1",
                Some("cloud"),
                Some(vec![1]),
            )
            .unwrap();
        assert_eq!(id, 42);
        assert_eq!(ip, "1.2.3.4");
    }

    #[test]
    fn create_server_error_includes_body() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call("POST", "/servers", 400, r#"{"error":"bad request"}"#)],
        );
        let err = client
            .create_server("gob-x", "cx23", "debian-13", "hel1", None, None)
            .unwrap_err();
        assert!(format!("{err:#}").contains("bad request"));
    }

    #[test]
    fn create_server_parse_error() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call("POST", "/servers", 201, r#"{"not_server":true}"#)],
        );
        let err = client
            .create_server("gob-x", "cx23", "debian-13", "hel1", None, None)
            .unwrap_err();
        assert!(format!("{err:#}").contains("Failed to parse server response"));
    }

    #[test]
    fn wait_for_server_returns_ip_when_running() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call(
                "GET",
                "/servers/42",
                200,
                r#"{"server":{"id":42,"name":"gob-x","status":"running","public_net":{"ipv4":{"ip":"5.6.7.8"}}}}"#,
            )],
        );
        let ip = client.wait_for_server(42).unwrap();
        assert_eq!(ip, "5.6.7.8");
    }

    #[test]
    fn wait_for_server_unexpected_status_errors() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call(
                "GET",
                "/servers/42",
                200,
                r#"{"server":{"id":42,"name":"gob-x","status":"off","public_net":{"ipv4":{"ip":"5.6.7.8"}}}}"#,
            )],
        );
        let err = client.wait_for_server(42).unwrap_err();
        assert!(format!("{err:#}").contains("unexpected status"));
    }

    #[test]
    fn delete_server_404_errors() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call("DELETE", "/servers/99", 404, "{}")],
        );
        let err = client.delete_server(99).unwrap_err();
        assert!(format!("{err:#}").contains("not found"));
    }

    #[test]
    fn ensure_ssh_key_uses_existing_key() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call(
                "GET",
                "/ssh_keys",
                200,
                r#"{"ssh_keys":[{"id":777,"name":"goblinmode"}]}"#,
            )],
        );
        let id = client
            .ensure_ssh_key("goblinmode", "ssh-ed25519 AAAA")
            .unwrap();
        assert_eq!(id, 777);
    }

    #[test]
    fn ensure_ssh_key_creates_when_missing() {
        let mut create = call(
            "POST",
            "/ssh_keys",
            201,
            r#"{"ssh_key":{"id":888,"name":"goblinmode"}}"#,
        );
        create.body_contains = Some(r#""name":"goblinmode""#.to_string());
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call("GET", "/ssh_keys", 200, r#"{"ssh_keys":[]}"#), create],
        );
        let id = client
            .ensure_ssh_key("goblinmode", "ssh-ed25519 AAAA")
            .unwrap();
        assert_eq!(id, 888);
    }

    #[test]
    fn get_server_status_404_returns_none() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call("GET", "/servers/555", 404, "{}")],
        );
        let status = client.get_server_status(555).unwrap();
        assert!(status.is_none());
    }

    #[test]
    fn list_servers_and_snapshots_parse() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![
                call(
                    "GET",
                    "/servers?label_selector=managed-by%3Dgoblinmode",
                    200,
                    r#"{"servers":[{"id":1,"name":"gob-a","status":"running","public_net":{"ipv4":{"ip":"10.0.0.1"}}}]}"#,
                ),
                call(
                    "GET",
                    "/images?type=snapshot&label_selector=managed-by%3Dgoblinmode",
                    200,
                    r#"{"images":[{"id":2,"description":"snap","created":"2026-02-20T00:00:00Z"},{"id":0,"description":"skip","created":"x"}]}"#,
                ),
            ],
        );

        let servers = client.list_goblinmode_servers().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "gob-a");

        let snapshots = client.list_goblinmode_snapshots().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, 2);
    }

    #[test]
    fn wait_for_image_returns_when_available() {
        let client = HetznerClient::with_mock_calls(
            "token".to_string(),
            "mock://".to_string(),
            vec![call(
                "GET",
                "/images/2",
                200,
                r#"{"image":{"status":"available"}}"#,
            )],
        );
        client.wait_for_image(2).unwrap();
    }
}
