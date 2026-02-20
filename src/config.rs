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
    api_token_cmd: Option<String>,
}

#[derive(Deserialize)]
struct TailscaleConfig {
    auth_key: Option<String>,
    auth_key_cmd: Option<String>,
    api_key: Option<String>,
    api_key_cmd: Option<String>,
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

    let hetzner = config_file.as_ref().and_then(|c| c.hetzner.as_ref());
    let hetzner_api_token = resolve_secret(
        "HETZNER_API_TOKEN",
        hetzner.and_then(|h| h.api_token_cmd.as_deref()),
        hetzner.and_then(|h| h.api_token.as_deref()),
    )
    .context("Failed to retrieve Hetzner API token")?
    .context(format!(
        "Hetzner API token not found.\n\
         Set HETZNER_API_TOKEN env var or add to {config_path_display}:\n\n\
         [hetzner]\n\
         api_token = \"your-token-here\""
    ))?;

    let tailscale = config_file.as_ref().and_then(|c| c.tailscale.as_ref());
    let tailscale_api_key = resolve_secret(
        "TAILSCALE_API_KEY",
        tailscale.and_then(|t| t.api_key_cmd.as_deref()),
        tailscale.and_then(|t| t.api_key.as_deref()),
    )
    .context("Failed to retrieve Tailscale API key")?
    .context(format!(
        "Tailscale API key not found.\n\
         Set TAILSCALE_API_KEY env var or add to {config_path_display}:\n\n\
         [tailscale]\n\
         api_key = \"tskey-api-...\""
    ))?;

    let tailscale_auth_key = resolve_secret(
        "TAILSCALE_AUTH_KEY",
        tailscale.and_then(|t| t.auth_key_cmd.as_deref()),
        tailscale.and_then(|t| t.auth_key.as_deref()),
    )
    .context("Failed to retrieve Tailscale auth key")?;

    let tailscale_tags = tailscale
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

fn run_secret_cmd(cmd: &str) -> Result<String> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("Failed to run secret command: {cmd}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Secret command failed: {cmd}\n{stderr}");
    }
    Ok(String::from_utf8(output.stdout)
        .context("Secret command output is not valid UTF-8")?
        .trim()
        .to_string())
}

pub(crate) fn resolve_secret(
    env_var: &str,
    cmd: Option<&str>,
    file_value: Option<&str>,
) -> Result<Option<String>> {
    if let Ok(val) = std::env::var(env_var) {
        if !val.is_empty() {
            return Ok(Some(val));
        }
    }
    if let Some(cmd) = cmd {
        let val = run_secret_cmd(cmd)?;
        if !val.is_empty() {
            return Ok(Some(val));
        }
    }
    Ok(file_value.filter(|v| !v.is_empty()).map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_secret_env_var_takes_priority() {
        let key = "GOB_TEST_SECRET_PRIORITY";
        std::env::set_var(key, "from_env");
        let result = resolve_secret(key, Some("echo from_cmd"), Some("from_file")).unwrap();
        std::env::remove_var(key);
        assert_eq!(result, Some("from_env".to_string()));
    }

    #[test]
    fn resolve_secret_cmd_used_when_no_env_var() {
        let key = "GOB_TEST_SECRET_CMD";
        std::env::remove_var(key);
        let result = resolve_secret(key, Some("echo hello"), Some("from_file")).unwrap();
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn resolve_secret_falls_back_to_file_value() {
        let key = "GOB_TEST_SECRET_FILE";
        std::env::remove_var(key);
        let result = resolve_secret(key, None, Some("from_file")).unwrap();
        assert_eq!(result, Some("from_file".to_string()));
    }

    #[test]
    fn resolve_secret_all_absent_returns_none() {
        let key = "GOB_TEST_SECRET_NONE";
        std::env::remove_var(key);
        let result = resolve_secret(key, None, None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_secret_empty_env_skipped() {
        let key = "GOB_TEST_SECRET_EMPTY_ENV";
        std::env::set_var(key, "");
        let result = resolve_secret(key, None, Some("from_file")).unwrap();
        std::env::remove_var(key);
        assert_eq!(result, Some("from_file".to_string()));
    }

    #[test]
    fn resolve_secret_empty_file_value_returns_none() {
        let key = "GOB_TEST_SECRET_EMPTY_FILE";
        std::env::remove_var(key);
        let result = resolve_secret(key, None, Some("")).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_secret_cmd_output_is_trimmed() {
        let key = "GOB_TEST_SECRET_TRIM";
        std::env::remove_var(key);
        let result = resolve_secret(key, Some("printf '  trimmed  '"), None).unwrap();
        assert_eq!(result, Some("trimmed".to_string()));
    }

    #[test]
    fn resolve_secret_cmd_failure_returns_error() {
        let key = "GOB_TEST_SECRET_CMD_FAIL";
        std::env::remove_var(key);
        let result = resolve_secret(key, Some("exit 1"), None);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_secret_empty_cmd_output_falls_through_to_file() {
        let key = "GOB_TEST_SECRET_CMD_EMPTY";
        std::env::remove_var(key);
        let result = resolve_secret(key, Some("true"), Some("from_file")).unwrap();
        assert_eq!(result, Some("from_file".to_string()));
    }
}
