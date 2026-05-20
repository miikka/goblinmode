// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::warn;

use crate::packages::{resolve_coding_agent, PackageSpec};

pub struct Config {
    pub hetzner_api_token: String,
    pub tailscale_auth_key: Option<String>,
    pub tailscale_api_key: String,
    pub tailscale_tags: Vec<String>,
    pub dotfiles_repo: Option<String>,
    pub dotfiles_install: Option<String>,
    pub vm_packages: Vec<PackageSpec>,
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
    binstall_packages: Option<Vec<String>>,
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
    load_config_from(config_path)
}

fn load_config_from(config_path: std::path::PathBuf) -> Result<Config> {
    let raw = config::Config::builder()
        .add_source(
            config::File::from(config_path.as_path())
                .required(false)
                .format(config::FileFormat::Toml),
        )
        .add_source(config::Environment::default().separator("__"))
        .build()
        .with_context(|| format!("Failed to parse config from {}", config_path.display()))?;

    let config_file: ConfigFile = raw
        .try_deserialize()
        .with_context(|| format!("Failed to parse config from {}", config_path.display()))?;

    let config_path_display = config_path.display();

    let hetzner = config_file.hetzner.as_ref();
    let hetzner_api_token = resolve_value_with_cmd(
        hetzner.and_then(|h| h.api_token.as_deref()),
        hetzner.and_then(|h| h.api_token_cmd.as_deref()),
    )
    .context("Failed to retrieve Hetzner API token")?
    .context(format!(
        "Hetzner API token not found.\n\
         Set HETZNER__API_TOKEN env var or add to {config_path_display}:\n\n\
         [hetzner]\n\
         api_token = \"your-token-here\""
    ))?;

    let tailscale = config_file.tailscale.as_ref();
    let tailscale_api_key = resolve_value_with_cmd(
        tailscale.and_then(|t| t.api_key.as_deref()),
        tailscale.and_then(|t| t.api_key_cmd.as_deref()),
    )
    .context("Failed to retrieve Tailscale API key")?
    .context(format!(
        "Tailscale API key not found.\n\
         Set TAILSCALE__API_KEY env var or add to {config_path_display}:\n\n\
         [tailscale]\n\
         api_key = \"tskey-api-...\""
    ))?;

    let tailscale_auth_key = resolve_value_with_cmd(
        tailscale.and_then(|t| t.auth_key.as_deref()),
        tailscale.and_then(|t| t.auth_key_cmd.as_deref()),
    )
    .context("Failed to retrieve Tailscale auth key")?;

    let tailscale_tags = tailscale.and_then(|t| t.tags.clone()).unwrap_or_default();

    let dotfiles = config_file.dotfiles.as_ref();
    let dotfiles_repo = dotfiles
        .and_then(|d| d.repo.clone())
        .filter(|v| !v.is_empty());
    let dotfiles_install = dotfiles
        .and_then(|d| d.install.clone())
        .filter(|v| !v.is_empty());

    let vm = config_file.vm.as_ref();

    let mut vm_packages: Vec<PackageSpec> = vm
        .and_then(|v| v.packages.as_ref())
        .map(|pkgs| {
            pkgs.iter()
                .map(|n| PackageSpec::Apt { name: n.clone() })
                .collect()
        })
        .unwrap_or_default();

    if let Some(agents) = vm.and_then(|v| v.coding_agents.as_ref()) {
        for name in agents {
            match resolve_coding_agent(name) {
                Some(spec) => vm_packages.push(spec),
                None => warn!(agent = %name, "unknown coding_agent in config, skipping"),
            }
        }
    }

    if let Some(cargo_pkgs) = vm.and_then(|v| v.binstall_packages.as_ref()) {
        for name in cargo_pkgs {
            vm_packages.push(PackageSpec::CargoBinstall { name: name.clone() });
        }
    }

    Ok(Config {
        hetzner_api_token,
        tailscale_auth_key,
        tailscale_api_key,
        tailscale_tags,
        dotfiles_repo,
        dotfiles_install,
        vm_packages,
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

/// Resolves a config value: returns `value` if non-empty, otherwise executes `cmd` as a
/// shell command and returns its trimmed output. Returns `None` if both are absent/empty.
pub(crate) fn resolve_value_with_cmd(
    value: Option<&str>,
    cmd: Option<&str>,
) -> Result<Option<String>> {
    if let Some(val) = value.filter(|v| !v.is_empty()) {
        return Ok(Some(val.to_string()));
    }
    if let Some(cmd) = cmd {
        let val = run_secret_cmd(cmd)?;
        if !val.is_empty() {
            return Ok(Some(val));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate shared env vars.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Set env vars, run f, then restore the original values.
    fn with_env_locked<F: FnOnce() -> R, R>(
        _guard: &std::sync::MutexGuard<()>,
        vars: &[(&str, Option<&str>)],
        f: F,
    ) -> R {
        let originals: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let result = f();
        for (k, orig) in &originals {
            match orig {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        result
    }

    /// Write a temp config file and return its path.
    fn write_temp_config(dir: &tempfile::TempDir, contents: &str) -> std::path::PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    // --- load_config / load_config_from tests ---

    #[test]
    fn load_config_no_file_uses_env_vars() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER__API_TOKEN", Some("htoken")),
                ("TAILSCALE__API_KEY", Some("tsapi")),
                ("TAILSCALE__AUTH_KEY", None),
            ],
            || load_config_from(path).unwrap(),
        );
        assert_eq!(config.hetzner_api_token, "htoken");
        assert_eq!(config.tailscale_api_key, "tsapi");
        assert_eq!(config.tailscale_auth_key, None);
        assert!(config.tailscale_tags.is_empty());
        assert!(config.dotfiles_repo.is_none());
        assert!(config.dotfiles_install.is_none());
        assert!(config.vm_packages.is_empty());
    }

    #[test]
    fn load_config_from_full_toml() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[hetzner]
api_token = "hetz-token"

[tailscale]
api_key = "ts-api-key"
auth_key = "ts-auth-key"
tags = ["tag:server", "tag:dev"]

[dotfiles]
repo = "git@github.com:user/dotfiles"
install = "./install.sh"

[vm]
packages = ["vim", "tmux"]
coding_agents = ["claude-code"]
binstall_packages = ["jj-cli"]
"#;
        let path = write_temp_config(&dir, toml);
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER__API_TOKEN", None),
                ("TAILSCALE__API_KEY", None),
                ("TAILSCALE__AUTH_KEY", None),
            ],
            || load_config_from(path).unwrap(),
        );
        assert_eq!(config.hetzner_api_token, "hetz-token");
        assert_eq!(config.tailscale_api_key, "ts-api-key");
        assert_eq!(config.tailscale_auth_key, Some("ts-auth-key".to_string()));
        assert_eq!(config.tailscale_tags, vec!["tag:server", "tag:dev"]);
        assert_eq!(
            config.dotfiles_repo,
            Some("git@github.com:user/dotfiles".to_string())
        );
        assert_eq!(config.dotfiles_install, Some("./install.sh".to_string()));
        assert_eq!(
            config.vm_packages,
            vec![
                PackageSpec::Apt {
                    name: "vim".to_string()
                },
                PackageSpec::Apt {
                    name: "tmux".to_string()
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
                PackageSpec::CargoBinstall {
                    name: "jj-cli".to_string()
                },
            ]
        );
    }

    #[test]
    fn load_config_missing_hetzner_token_returns_error() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = with_env_locked(
            &guard,
            &[("HETZNER__API_TOKEN", None), ("TAILSCALE__API_KEY", None)],
            || load_config_from(path),
        );
        assert!(result.is_err());
        let msg = format!("{:#}", result.err().unwrap());
        assert!(msg.contains("Hetzner API token not found"), "got: {msg}");
    }

    #[test]
    fn load_config_missing_tailscale_api_key_returns_error() {
        let guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("HETZNER__API_TOKEN", "preexisting");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = with_env_locked(
            &guard,
            &[
                ("HETZNER__API_TOKEN", Some("htoken")),
                ("TAILSCALE__API_KEY", None),
            ],
            || load_config_from(path),
        );
        assert_eq!(std::env::var("HETZNER__API_TOKEN").unwrap(), "preexisting");
        std::env::remove_var("HETZNER__API_TOKEN");
        assert!(result.is_err());
        let msg = format!("{:#}", result.err().unwrap());
        assert!(msg.contains("Tailscale API key not found"), "got: {msg}");
    }

    #[test]
    fn load_config_invalid_toml_returns_error() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_config(&dir, "this is not valid toml ][[[");
        let result = with_env_locked(
            &guard,
            &[("HETZNER__API_TOKEN", None), ("TAILSCALE__API_KEY", None)],
            || load_config_from(path),
        );
        assert!(result.is_err());
        let msg = format!("{:#}", result.err().unwrap());
        assert!(msg.contains("Failed to parse"), "got: {msg}");
    }

    #[test]
    fn load_config_empty_dotfiles_filtered() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[hetzner]
api_token = "hetz-token"

[tailscale]
api_key = "ts-api-key"

[dotfiles]
repo = ""
install = ""
"#;
        let path = write_temp_config(&dir, toml);
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER__API_TOKEN", None),
                ("TAILSCALE__API_KEY", None),
                ("TAILSCALE__AUTH_KEY", None),
            ],
            || load_config_from(path).unwrap(),
        );
        assert!(config.dotfiles_repo.is_none());
        assert!(config.dotfiles_install.is_none());
    }

    #[test]
    fn load_config_delegates_to_load_config_from() {
        let guard = ENV_MUTEX.lock().unwrap();
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER__API_TOKEN", Some("env-htoken")),
                ("TAILSCALE__API_KEY", Some("env-tsapi")),
                ("TAILSCALE__AUTH_KEY", None),
            ],
            || load_config().unwrap(),
        );
        assert_eq!(config.hetzner_api_token, "env-htoken");
        assert_eq!(config.tailscale_api_key, "env-tsapi");
    }

    // --- resolve_value_with_cmd tests ---

    #[test]
    fn resolve_value_with_cmd_value_takes_priority() {
        let result = resolve_value_with_cmd(Some("from_value"), Some("echo from_cmd")).unwrap();
        assert_eq!(result, Some("from_value".to_string()));
    }

    #[test]
    fn resolve_value_with_cmd_cmd_used_when_no_value() {
        let result = resolve_value_with_cmd(None, Some("echo hello")).unwrap();
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn resolve_value_with_cmd_returns_value_when_no_cmd() {
        let result = resolve_value_with_cmd(Some("from_value"), None).unwrap();
        assert_eq!(result, Some("from_value".to_string()));
    }

    #[test]
    fn resolve_value_with_cmd_all_absent_returns_none() {
        let result = resolve_value_with_cmd(None, None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_value_with_cmd_empty_value_tries_cmd() {
        let result = resolve_value_with_cmd(Some(""), Some("echo fallback")).unwrap();
        assert_eq!(result, Some("fallback".to_string()));
    }

    #[test]
    fn resolve_value_with_cmd_empty_value_no_cmd_returns_none() {
        let result = resolve_value_with_cmd(Some(""), None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_value_with_cmd_output_is_trimmed() {
        let result = resolve_value_with_cmd(None, Some("printf '  trimmed  '")).unwrap();
        assert_eq!(result, Some("trimmed".to_string()));
    }

    #[test]
    fn resolve_value_with_cmd_cmd_failure_returns_error() {
        let result = resolve_value_with_cmd(None, Some("exit 1"));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_value_with_cmd_empty_cmd_output_returns_none() {
        let result = resolve_value_with_cmd(None, Some("true")).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn load_config_unknown_coding_agent_is_skipped() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[hetzner]
api_token = "token"

[tailscale]
api_key = "tskey"

[vm]
coding_agents = ["totally-unknown-agent-xyz"]
"#;
        let path = write_temp_config(&dir, toml);
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER__API_TOKEN", None),
                ("TAILSCALE__API_KEY", None),
                ("TAILSCALE__AUTH_KEY", None),
            ],
            || load_config_from(path).unwrap(),
        );
        assert!(config.vm_packages.is_empty());
    }

    #[test]
    fn load_config_type_mismatch_in_toml_returns_error() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // hetzner should be a table but is given as a string — build() succeeds
        // (valid TOML) but try_deserialize() fails on the type mismatch.
        let toml = r#"hetzner = "not_a_table""#;
        let path = write_temp_config(&dir, toml);
        let result = with_env_locked(
            &guard,
            &[("HETZNER__API_TOKEN", None), ("TAILSCALE__API_KEY", None)],
            || load_config_from(path),
        );
        assert!(result.is_err());
        let msg = format!("{:#}", result.err().unwrap());
        assert!(msg.contains("Failed to parse"), "got: {msg}");
    }
}
