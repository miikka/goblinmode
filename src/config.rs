use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;

pub struct Config {
    pub hetzner_api_token: String,
    pub tailscale_auth_key: String,
    pub tailscale_api_key: String,
    pub dotfiles_repo: Option<String>,
    pub dotfiles_install: Option<String>,
    pub vm_packages: Vec<String>,
}

/// Default extra packages installed on the VM (on top of core packages).
/// Users can override this list via `[vm] packages = [...]` in config.toml.
const DEFAULT_VM_PACKAGES: &[&str] = &["atuin", "starship", "git-delta"];

#[derive(Deserialize)]
struct ConfigFile {
    hetzner: Option<HetznerConfig>,
    tailscale: Option<TailscaleConfig>,
    dotfiles: Option<DotfilesConfig>,
    vm: Option<VmConfig>,
}

#[derive(Deserialize)]
struct VmConfig {
    packages: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct DotfilesConfig {
    repo: Option<String>,
    install: Option<String>,
}

#[derive(Deserialize)]
struct HetznerConfig {
    api_token: Option<String>,
}

#[derive(Deserialize)]
struct TailscaleConfig {
    auth_key: Option<String>,
    api_key: Option<String>,
}

pub fn load_config() -> Result<Config> {
    let config_dir = dirs::config_dir().context("Could not determine config directory")?;
    let config_path = config_dir.join("goblinmode").join("config.toml");

    let config_file: Option<ConfigFile> = if config_path.exists() {
        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        Some(
            toml::from_str(&contents)
                .with_context(|| format!("Failed to parse {}", config_path.display()))?,
        )
    } else {
        None
    };

    let config_path_display = config_path.display();

    let hetzner_api_token = resolve_value(
        "HETZNER_API_TOKEN",
        config_file
            .as_ref()
            .and_then(|c| c.hetzner.as_ref())
            .and_then(|h| h.api_token.as_deref()),
    )
    .context(format!(
        "Hetzner API token not found.\n\
         Set HETZNER_API_TOKEN env var or add to {config_path_display}:\n\n\
         [hetzner]\n\
         api_token = \"your-token-here\""
    ))?;

    let tailscale_api_key = resolve_value(
        "TAILSCALE_API_KEY",
        config_file
            .as_ref()
            .and_then(|c| c.tailscale.as_ref())
            .and_then(|t| t.api_key.as_deref()),
    )
    .context(format!(
        "Tailscale API key not found.\n\
         Set TAILSCALE_API_KEY env var or add to {config_path_display}:\n\n\
         [tailscale]\n\
         api_key = \"tskey-api-...\""
    ))?;

    let tailscale_auth_key = resolve_value(
        "TAILSCALE_AUTH_KEY",
        config_file
            .as_ref()
            .and_then(|c| c.tailscale.as_ref())
            .and_then(|t| t.auth_key.as_deref()),
    )
    .context(format!(
        "Tailscale auth key not found.\n\
         Set TAILSCALE_AUTH_KEY env var or add to {config_path_display}:\n\n\
         [tailscale]\n\
         auth_key = \"tskey-auth-...\""
    ))?;

    let dotfiles = config_file.as_ref().and_then(|c| c.dotfiles.as_ref());
    let dotfiles_repo = dotfiles
        .and_then(|d| d.repo.clone())
        .filter(|v| !v.is_empty());
    let dotfiles_install = dotfiles
        .and_then(|d| d.install.clone())
        .filter(|v| !v.is_empty());

    let vm_packages = config_file
        .as_ref()
        .and_then(|c| c.vm.as_ref())
        .and_then(|v| v.packages.clone())
        .unwrap_or_else(|| DEFAULT_VM_PACKAGES.iter().map(|s| s.to_string()).collect());

    Ok(Config {
        hetzner_api_token,
        tailscale_auth_key,
        tailscale_api_key,
        dotfiles_repo,
        dotfiles_install,
        vm_packages,
    })
}

fn resolve_value(env_var: &str, file_value: Option<&str>) -> Option<String> {
    if let Ok(val) = std::env::var(env_var) {
        if !val.is_empty() {
            return Some(val);
        }
    }
    file_value.filter(|v| !v.is_empty()).map(|v| v.to_string())
}
