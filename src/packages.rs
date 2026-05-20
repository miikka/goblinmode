// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

/// A package to be installed on a VM, along with how to install it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PackageSpec {
    Apt { name: String },
    CurlInstaller { name: String, url: String },
    CargoBinstall { name: String },
    Npm { name: String, package: String },
}

impl PackageSpec {
    /// The canonical name used for deduplication and delta comparisons.
    pub fn name(&self) -> &str {
        match self {
            PackageSpec::Apt { name } => name,
            PackageSpec::CurlInstaller { name, .. } => name,
            PackageSpec::CargoBinstall { name } => name,
            PackageSpec::Npm { name, .. } => name,
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
            PackageSpec::Npm { package, .. } => {
                Some(format!("su - {username} -c 'npm install -g {package}'"))
            }
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
        "pi" => Some(PackageSpec::Npm {
            name: "pi".to_string(),
            package: "@mariozechner/pi-coding-agent".to_string(),
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
    }

    #[test]
    fn apt_has_no_runcmd() {
        let spec = PackageSpec::Apt {
            name: "vim".to_string(),
        };
        assert!(spec.cloud_init_runcmd("alice").is_none());
    }

    #[test]
    fn cargo_binstall_has_runcmd() {
        let spec = PackageSpec::CargoBinstall {
            name: "jj-cli".to_string(),
        };
        let runcmd = spec.cloud_init_runcmd("alice").unwrap();
        assert!(runcmd.contains("cargo-binstall"));
        assert!(runcmd.contains("jj-cli"));
    }

    #[test]
    fn resolve_coding_agent_known_agents() {
        let claude = resolve_coding_agent("claude-code").unwrap();
        assert_eq!(claude.name(), "claude-code");
        assert!(matches!(claude, PackageSpec::CurlInstaller { .. }));

        let opencode = resolve_coding_agent("opencode").unwrap();
        assert_eq!(opencode.name(), "opencode");

        let pi = resolve_coding_agent("pi").unwrap();
        assert_eq!(pi.name(), "pi");
        assert!(matches!(pi, PackageSpec::Npm { .. }));

        assert!(resolve_coding_agent("unknown-agent").is_none());
    }
}
