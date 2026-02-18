use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;

pub struct Config {
    pub hetzner_api_token: String,
}

#[derive(Deserialize)]
struct ConfigFile {
    hetzner: Option<HetznerConfig>,
}

#[derive(Deserialize)]
struct HetznerConfig {
    api_token: Option<String>,
}

pub fn load_config() -> Result<Config> {
    // Try env var first
    if let Ok(token) = std::env::var("HETZNER_API_TOKEN") {
        if !token.is_empty() {
            return Ok(Config {
                hetzner_api_token: token,
            });
        }
    }

    // Fall back to config file
    let config_dir = dirs::config_dir().context("Could not determine config directory")?;
    let config_path = config_dir.join("goblinmode").join("config.toml");

    if config_path.exists() {
        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        let config_file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse {}", config_path.display()))?;

        if let Some(hetzner) = config_file.hetzner {
            if let Some(token) = hetzner.api_token {
                if !token.is_empty() {
                    return Ok(Config {
                        hetzner_api_token: token,
                    });
                }
            }
        }
    }

    bail!(
        "Hetzner API token not found.\n\
         Set HETZNER_API_TOKEN env var or add to ~/.config/goblinmode/config.toml:\n\n\
         [hetzner]\n\
         api_token = \"your-token-here\""
    );
}
