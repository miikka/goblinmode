use serde::{Deserialize, Serialize};

/// A package to be installed on a VM, along with how to install it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PackageSpec {
    Apt { name: String },
    CurlInstaller { name: String, url: String },
    CargoBinstall { name: String },
}

impl PackageSpec {
    /// The canonical name used for deduplication and delta comparisons.
    pub fn name(&self) -> &str {
        match self {
            PackageSpec::Apt { name } => name,
            PackageSpec::CurlInstaller { name, .. } => name,
            PackageSpec::CargoBinstall { name } => name,
        }
    }

    /// Returns the cloud-init `runcmd` entry for this package, if any.
    /// `Apt` packages are installed via the `packages:` list, not `runcmd`.
    /// Callers are responsible for emitting the cargo-binstall bootstrap
    /// command before any `CargoBinstall` entries.
    pub fn cloud_init_runcmd(&self, username: &str) -> Option<String> {
        match self {
            PackageSpec::Apt { .. } => None,
            PackageSpec::CurlInstaller { url, .. } => {
                Some(format!("su - {username} -c 'curl -fsSL {url} | bash'"))
            }
            PackageSpec::CargoBinstall { name } => Some(format!(
                "su - {username} -c \"/home/{username}/.cargo/bin/cargo-binstall --no-confirm --strategies crate-meta-data {name}\""
            )),
        }
    }

    /// Returns the shell command to install this package at runtime on an
    /// already-provisioned VM.  Returns `None` for `CargoBinstall` because
    /// that requires cargo-binstall to already be set up — use `--reset`.
    pub fn runtime_install_cmd(&self, username: &str) -> Option<String> {
        match self {
            PackageSpec::Apt { name } => Some(format!(
                "sudo DEBIAN_FRONTEND=noninteractive apt-get update && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y {name}"
            )),
            PackageSpec::CurlInstaller { url, .. } => {
                Some(format!("su - {username} -c 'curl -fsSL {url} | bash'"))
            }
            PackageSpec::CargoBinstall { .. } => None,
        }
    }
}

/// Resolve a known coding-agent short name to its `PackageSpec`.
/// Returns `None` for unknown names so callers can warn and skip.
pub fn resolve_coding_agent(name: &str) -> Option<PackageSpec> {
    match name {
        "claude-code" => Some(PackageSpec::CurlInstaller {
            name: "claude-code".to_string(),
            url: "https://claude.ai/install.sh".to_string(),
        }),
        "opencode" => Some(PackageSpec::CurlInstaller {
            name: "opencode".to_string(),
            url: "https://opencode.ai/install".to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apt_name_returns_package_name() {
        let spec = PackageSpec::Apt {
            name: "vim".to_string(),
        };
        assert_eq!(spec.name(), "vim");
    }

    #[test]
    fn curl_installer_name_and_runcmd() {
        let spec = PackageSpec::CurlInstaller {
            name: "claude-code".to_string(),
            url: "https://claude.ai/install.sh".to_string(),
        };
        assert_eq!(spec.name(), "claude-code");
        let cmd = spec.cloud_init_runcmd("alice").unwrap();
        assert!(cmd.contains("https://claude.ai/install.sh"));
        assert!(cmd.contains("su - alice"));
        let runtime = spec.runtime_install_cmd("alice").unwrap();
        assert_eq!(cmd, runtime);
    }

    #[test]
    fn apt_has_no_runcmd() {
        let spec = PackageSpec::Apt {
            name: "vim".to_string(),
        };
        assert!(spec.cloud_init_runcmd("alice").is_none());
    }

    #[test]
    fn apt_has_runtime_install_cmd() {
        let spec = PackageSpec::Apt {
            name: "vim".to_string(),
        };
        let cmd = spec.runtime_install_cmd("alice").unwrap();
        assert!(cmd.contains("apt-get install -y vim"));
    }

    #[test]
    fn cargo_binstall_has_runcmd_but_no_runtime_cmd() {
        let spec = PackageSpec::CargoBinstall {
            name: "jj-cli".to_string(),
        };
        let runcmd = spec.cloud_init_runcmd("alice").unwrap();
        assert!(runcmd.contains("cargo-binstall"));
        assert!(runcmd.contains("jj-cli"));
        assert!(spec.runtime_install_cmd("alice").is_none());
    }

    #[test]
    fn resolve_coding_agent_known_agents() {
        let claude = resolve_coding_agent("claude-code").unwrap();
        assert_eq!(claude.name(), "claude-code");
        assert!(matches!(claude, PackageSpec::CurlInstaller { .. }));

        let opencode = resolve_coding_agent("opencode").unwrap();
        assert_eq!(opencode.name(), "opencode");

        assert!(resolve_coding_agent("unknown-agent").is_none());
    }
}
