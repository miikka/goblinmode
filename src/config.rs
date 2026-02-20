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
    load_config_from(config_path)
}

fn load_config_from(config_path: std::path::PathBuf) -> Result<Config> {
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
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate shared env vars like HETZNER_API_TOKEN.
    /// Callers must acquire this before calling `with_env_locked`.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Set env vars, run f, then restore the original values.
    /// Caller must hold `ENV_MUTEX` before calling this (pass the guard to prove it).
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
        fs::write(&path, contents).unwrap();
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
                ("HETZNER_API_TOKEN", Some("htoken")),
                ("TAILSCALE_API_KEY", Some("tsapi")),
                ("TAILSCALE_AUTH_KEY", None),
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
        assert!(config.coding_agents.is_empty());
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
coding_agents = ["claude"]
"#;
        let path = write_temp_config(&dir, toml);
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER_API_TOKEN", None),
                ("TAILSCALE_API_KEY", None),
                ("TAILSCALE_AUTH_KEY", None),
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
        assert_eq!(config.vm_packages, vec!["vim", "tmux"]);
        assert_eq!(config.coding_agents, vec!["claude"]);
    }

    #[test]
    fn load_config_missing_hetzner_token_returns_error() {
        let guard = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = with_env_locked(
            &guard,
            &[
                ("HETZNER_API_TOKEN", None),
                ("TAILSCALE_API_KEY", None),
            ],
            || load_config_from(path),
        );
        assert!(result.is_err());
        let msg = format!("{:#}", result.err().unwrap());
        assert!(msg.contains("Hetzner API token not found"), "got: {msg}");
    }

    #[test]
    fn load_config_missing_tailscale_api_key_returns_error() {
        let guard = ENV_MUTEX.lock().unwrap();
        // Pre-set HETZNER_API_TOKEN so the restore branch hits Some(v) → line 203.
        std::env::set_var("HETZNER_API_TOKEN", "preexisting");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = with_env_locked(
            &guard,
            &[
                ("HETZNER_API_TOKEN", Some("htoken")),
                ("TAILSCALE_API_KEY", None),
            ],
            || load_config_from(path),
        );
        // HETZNER_API_TOKEN should be restored to "preexisting"
        assert_eq!(std::env::var("HETZNER_API_TOKEN").unwrap(), "preexisting");
        std::env::remove_var("HETZNER_API_TOKEN");
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
            &[
                ("HETZNER_API_TOKEN", None),
                ("TAILSCALE_API_KEY", None),
            ],
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
                ("HETZNER_API_TOKEN", None),
                ("TAILSCALE_API_KEY", None),
                ("TAILSCALE_AUTH_KEY", None),
            ],
            || load_config_from(path).unwrap(),
        );
        assert!(config.dotfiles_repo.is_none());
        assert!(config.dotfiles_install.is_none());
    }

    #[test]
    fn load_config_delegates_to_load_config_from() {
        // Exercises load_config() lines 51-57 (path resolution + delegation).
        // We provide both required tokens via env vars so it succeeds regardless
        // of whether a real config file exists (env vars take priority).
        let guard = ENV_MUTEX.lock().unwrap();
        let config = with_env_locked(
            &guard,
            &[
                ("HETZNER_API_TOKEN", Some("env-htoken")),
                ("TAILSCALE_API_KEY", Some("env-tsapi")),
                ("TAILSCALE_AUTH_KEY", None),
            ],
            || load_config().unwrap(),
        );
        assert_eq!(config.hetzner_api_token, "env-htoken");
        assert_eq!(config.tailscale_api_key, "env-tsapi");
    }

    // --- resolve_secret tests ---

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
