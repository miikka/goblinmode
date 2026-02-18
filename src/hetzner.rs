use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.hetzner.cloud/v1";

pub struct HetznerClient {
    client: Client,
    token: String,
}

#[derive(Debug, Deserialize)]
struct ServerResponse {
    server: Server,
}

#[derive(Debug, Deserialize)]
struct Server {
    id: u64,
    status: String,
    public_net: PublicNet,
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
            client: Client::new(),
            token,
        }
    }

    fn get(&self, path: &str) -> Result<reqwest::blocking::Response> {
        let resp = self
            .client
            .get(format!("{}{}", BASE_URL, path))
            .bearer_auth(&self.token)
            .send()
            .context("HTTP request failed")?;
        Ok(resp)
    }

    fn post_json<T: Serialize>(&self, path: &str, body: &T) -> Result<reqwest::blocking::Response> {
        let resp = self
            .client
            .post(format!("{}{}", BASE_URL, path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .context("HTTP request failed")?;
        Ok(resp)
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
        let req = CreateServerRequest {
            name: name.to_string(),
            server_type: server_type.to_string(),
            image: image.to_string(),
            location: location.to_string(),
            user_data: user_data.map(|s| s.to_string()),
            ssh_keys,
        };
        let resp = self.post_json("/servers", &req)?;

        if !resp.status().is_success() && resp.status().as_u16() != 201 {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to create server ({}): {}", status, body);
        }

        let created: ServerResponse = resp.json().context("Failed to parse server response")?;
        let id = created.server.id;
        let ip = created.server.public_net.ipv4.ip.clone();
        Ok((id, ip))
    }

    /// Poll until server is running. Returns the IPv4 address.
    pub fn wait_for_server(&self, server_id: u64) -> Result<String> {
        loop {
            let resp = self.get(&format!("/servers/{}", server_id))?;
            if !resp.status().is_success() {
                bail!("Failed to get server status: {}", resp.status());
            }
            let server: ServerResponse = resp.json().context("Failed to parse server response")?;

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
        let resp = self
            .client
            .delete(format!("{}/servers/{}", BASE_URL, server_id))
            .bearer_auth(&self.token)
            .send()
            .context("HTTP request failed")?;

        if resp.status().as_u16() == 404 {
            bail!("Server {} not found", server_id);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to delete server ({}): {}", status, body);
        }
        Ok(())
    }

    /// Find an SSH key by name, returns its ID if found.
    pub fn find_ssh_key_by_name(&self, name: &str) -> Result<Option<u64>> {
        let resp = self.get("/ssh_keys")?;
        if !resp.status().is_success() {
            bail!("Failed to list SSH keys: {}", resp.status());
        }
        let keys: SshKeysResponse = resp.json().context("Failed to parse SSH keys response")?;
        Ok(keys.ssh_keys.into_iter().find(|k| k.name == name).map(|k| k.id))
    }

    /// Upload an SSH public key. Returns the key ID.
    pub fn create_ssh_key(&self, name: &str, public_key: &str) -> Result<u64> {
        let resp = self.post_json(
            "/ssh_keys",
            &serde_json::json!({
                "name": name,
                "public_key": public_key
            }),
        )?;
        if !resp.status().is_success() && resp.status().as_u16() != 201 {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to create SSH key ({}): {}", status, body);
        }
        let created: SshKeyResponse = resp.json().context("Failed to parse SSH key response")?;
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
        let resp = self.post_json(
            &format!("/servers/{}/actions/shutdown", server_id),
            &serde_json::json!({}),
        )?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to shutdown server ({}): {}", status, body);
        }
        Ok(())
    }

    /// Poll until server status is "off".
    pub fn wait_for_server_off(&self, server_id: u64) -> Result<()> {
        loop {
            let resp = self.get(&format!("/servers/{}", server_id))?;
            if !resp.status().is_success() {
                bail!("Failed to get server status: {}", resp.status());
            }
            let server: ServerResponse = resp.json().context("Failed to parse server response")?;
            if server.server.status == "off" {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    /// Create a snapshot image of a server. Returns the image ID.
    pub fn create_image(&self, server_id: u64, description: &str) -> Result<u64> {
        let resp = self.post_json(
            &format!("/servers/{}/actions/create_image", server_id),
            &serde_json::json!({
                "description": description,
                "type": "snapshot"
            }),
        )?;
        if !resp.status().is_success() && resp.status().as_u16() != 201 {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to create image ({}): {}", status, body);
        }
        let body: serde_json::Value = resp.json().context("Failed to parse create_image response")?;
        let image_id = body["image"]["id"]
            .as_u64()
            .context("Missing image id in create_image response")?;
        Ok(image_id)
    }

    /// Poll until an image is available.
    pub fn wait_for_image(&self, image_id: u64) -> Result<()> {
        loop {
            let resp = self.get(&format!("/images/{}", image_id))?;
            if !resp.status().is_success() {
                bail!("Failed to get image status: {}", resp.status());
            }
            let body: serde_json::Value = resp.json().context("Failed to parse image response")?;
            let status = body["image"]["status"]
                .as_str()
                .unwrap_or("unknown");
            if status == "available" {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    /// Delete an image/snapshot.
    pub fn delete_image(&self, image_id: u64) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/images/{}", BASE_URL, image_id))
            .bearer_auth(&self.token)
            .send()
            .context("HTTP request failed")?;
        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to delete image ({}): {}", status, body);
        }
        Ok(())
    }

    /// Check if a server still exists and is running.
    pub fn get_server_status(&self, server_id: u64) -> Result<Option<(String, String)>> {
        let resp = self.get(&format!("/servers/{}", server_id))?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("Failed to get server status: {}", resp.status());
        }
        let server: ServerResponse = resp.json().context("Failed to parse server response")?;
        Ok(Some((
            server.server.status,
            server.server.public_net.ipv4.ip,
        )))
    }
}
