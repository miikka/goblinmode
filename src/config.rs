use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;

pub struct Config {
    pub hetzner_api_token: String,
    pub tailscale_auth_key: Option<String>,
    pub tailscale_api_key: String,
    pub tailscale_tags: Vec<String>,
    pub dotfiles_repo: Option<String>,
    pub dotfiles_install: Option<String>,
    pub vm_packages: Vec<String>,
    pub coding_agents: Vec<String>,
}

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
    coding_agents: Option<Vec<String>>,
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
    tags: Option<Vec<String>>,
}

pub fn load_config() -> Result<Config> {
    let config_dir = dirs::home_dir()
        .context("Could not determine home directory")?
        .join(".config");
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
    );

    let tailscale_tags = config_file
        .as_ref()
        .and_then(|c| c.tailscale.as_ref())
        .and_then(|t| t.tags.clone())
        .unwrap_or_default();

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
        .unwrap_or_default();

    let coding_agents = config_file
        .as_ref()
        .and_then(|c| c.vm.as_ref())
        .and_then(|v| v.coding_agents.clone())
        .unwrap_or_default();

    Ok(Config {
        hetzner_api_token,
        tailscale_auth_key,
        tailscale_api_key,
        tailscale_tags,
        dotfiles_repo,
        dotfiles_install,
        vm_packages,
        coding_agents,
    })
}

pub(crate) fn resolve_value(env_var: &str, file_value: Option<&str>) -> Option<String> {
    if let Ok(val) = std::env::var(env_var) {
        if !val.is_empty() {
            return Some(val);
        }
    }
    file_value.filter(|v| !v.is_empty()).map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_value_env_var_takes_priority() {
        let key = "GOB_TEST_RESOLVE_PRIORITY";
        std::env::set_var(key, "from_env");
        let result = resolve_value(key, Some("from_file"));
        std::env::remove_var(key);
        assert_eq!(result, Some("from_env".to_string()));
    }

    #[test]
    fn resolve_value_falls_back_to_file() {
        let key = "GOB_TEST_RESOLVE_FALLBACK";
        std::env::remove_var(key);
        assert_eq!(
            resolve_value(key, Some("from_file")),
            Some("from_file".to_string())
        );
    }

    #[test]
    fn resolve_value_empty_env_treated_as_absent() {
        let key = "GOB_TEST_RESOLVE_EMPTY_ENV";
        std::env::set_var(key, "");
        let result = resolve_value(key, Some("from_file"));
        std::env::remove_var(key);
        assert_eq!(result, Some("from_file".to_string()));
    }

    #[test]
    fn resolve_value_both_absent_returns_none() {
        let key = "GOB_TEST_RESOLVE_NONE";
        std::env::remove_var(key);
        assert_eq!(resolve_value(key, None), None);
    }

    #[test]
    fn resolve_value_empty_file_value_returns_none() {
        let key = "GOB_TEST_RESOLVE_EMPTY_FILE";
        std::env::remove_var(key);
        assert_eq!(resolve_value(key, Some("")), None);
    }
}
