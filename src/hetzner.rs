use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.hetzner.cloud/v1";

pub struct HetznerClient {
    client: Client,
    token: String,
}

#[derive(Debug, Deserialize)]
struct SshKeysResponse {
    ssh_keys: Vec<SshKey>,
}

#[derive(Debug, Deserialize)]
struct SshKeyResponse {
    ssh_key: SshKey,
}

#[derive(Debug, Deserialize)]
struct SshKey {
    id: u64,
    fingerprint: String,
}

#[derive(Debug, Serialize)]
struct CreateSshKeyRequest {
    name: String,
    public_key: String,
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
    ssh_keys: Vec<u64>,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ApiError,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    message: String,
    code: String,
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

    /// Ensure SSH key is registered in Hetzner. Returns the key ID.
    pub fn ensure_ssh_key(&self, name: &str, public_key: &str) -> Result<u64> {
        // List existing keys and check by fingerprint
        let resp = self.get("/ssh_keys")?;
        if !resp.status().is_success() {
            bail!("Failed to list SSH keys: {}", resp.status());
        }
        let keys: SshKeysResponse = resp.json().context("Failed to parse SSH keys response")?;

        // Compute fingerprint to match against existing keys
        // We'll try to create and handle uniqueness_error if key already exists
        let req = CreateSshKeyRequest {
            name: name.to_string(),
            public_key: public_key.to_string(),
        };
        let resp = self.post_json("/ssh_keys", &req)?;

        if resp.status().is_success() || resp.status().as_u16() == 201 {
            let created: SshKeyResponse =
                resp.json().context("Failed to parse SSH key response")?;
            println!("  SSH key uploaded: {}", created.ssh_key.fingerprint);
            return Ok(created.ssh_key.id);
        }

        // If uniqueness error, find the existing key by matching public key content
        let err_body: Result<ErrorResponse, _> = resp.json();
        if let Ok(err) = err_body {
            if err.error.code == "uniqueness_error" {
                for key in &keys.ssh_keys {
                    let detail_resp = self.get(&format!("/ssh_keys/{}", key.id))?;
                    if detail_resp.status().is_success() {
                        let detail: SshKeyDetailResponse = detail_resp
                            .json()
                            .context("Failed to parse SSH key detail")?;
                        if normalize_pubkey(&detail.ssh_key.public_key)
                            == normalize_pubkey(public_key)
                        {
                            println!(
                                "  SSH key already registered: {}",
                                detail.ssh_key.fingerprint
                            );
                            return Ok(detail.ssh_key.id);
                        }
                    }
                }
                bail!("SSH key uniqueness error but could not find matching key");
            }
            bail!(
                "Failed to create SSH key: {} ({})",
                err.error.message,
                err.error.code
            );
        }

        bail!("Failed to create SSH key: unexpected error");
    }

    /// Create a server. Returns (server_id, ipv4).
    pub fn create_server(
        &self,
        name: &str,
        server_type: &str,
        image: &str,
        location: &str,
        ssh_key_ids: &[u64],
    ) -> Result<(u64, String)> {
        let req = CreateServerRequest {
            name: name.to_string(),
            server_type: server_type.to_string(),
            image: image.to_string(),
            location: location.to_string(),
            ssh_keys: ssh_key_ids.to_vec(),
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

#[derive(Debug, Deserialize)]
struct SshKeyDetailResponse {
    ssh_key: SshKeyDetail,
}

#[derive(Debug, Deserialize)]
struct SshKeyDetail {
    id: u64,
    fingerprint: String,
    public_key: String,
}

fn normalize_pubkey(key: &str) -> String {
    // Strip comments and whitespace for comparison
    let parts: Vec<&str> = key.trim().split_whitespace().collect();
    if parts.len() >= 2 {
        format!("{} {}", parts[0], parts[1])
    } else {
        key.trim().to_string()
    }
}
