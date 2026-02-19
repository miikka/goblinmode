use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

const BASE_URL: &str = "https://api.tailscale.com/api/v2";

pub struct TailscaleClient {
    client: Client,
    api_key: String,
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
        }
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

    fn list_devices(&self) -> Result<Vec<Device>> {
        let resp = self
            .client
            .get(format!("{}/tailnet/-/devices", BASE_URL))
            .basic_auth(&self.api_key, Option::<&str>::None)
            .send()
            .context("Failed to list Tailscale devices")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to list Tailscale devices ({}): {}", status, body);
        }

        let devices: DevicesResponse = resp
            .json()
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
            .client
            .delete(format!("{}/device/{}", BASE_URL, device_id))
            .basic_auth(&self.api_key, Option::<&str>::None)
            .send()
            .context("Failed to delete Tailscale device")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("Failed to delete Tailscale device ({}): {}", status, body);
        }

        Ok(())
    }
}

pub struct DeviceInfo {
    pub id: String,
    pub hostname: String,
}
