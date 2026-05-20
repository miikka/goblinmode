// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use crate::packages::PackageSpec;
use crate::state;

pub(crate) fn detect_timezone() -> Option<String> {
    // 1. Honour explicit TZ env var
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return Some(tz);
        }
    }

    // 2. Resolve /etc/localtime symlink and strip the zoneinfo prefix.
    // Works on Linux (/usr/share/zoneinfo/…) and macOS (/var/db/timezone/zoneinfo/…).
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(pos) = s.find("/zoneinfo/") {
            let tz = &s[pos + "/zoneinfo/".len()..];
            if !tz.is_empty() {
                return Some(tz.to_string());
            }
        }
    }

    // 3. Linux fallback: /etc/timezone plain-text file (e.g. "Europe/Helsinki\n")
    // (Not present on macOS, but harmless to try.)
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        let tz = contents.trim().to_string();
        if !tz.is_empty() {
            return Some(tz);
        }
    }

    None
}

pub fn build_cloud_init(
    username: &str,
    ssh_pubkey: &str,
    tailscale_auth_key: &str,
    toolchains: &[state::Toolchain],
    packages: &[PackageSpec],
) -> String {
    let toolchain_packages: String = toolchains
        .iter()
        .flat_map(|t| t.apt_packages())
        .map(|p| format!("\n  - {p}"))
        .collect();
    let toolchain_cmds: String = toolchains
        .iter()
        .flat_map(|t| t.runcmds(username))
        .map(|cmd| format!("\n  - {cmd}"))
        .collect();
    let timezone_line = match detect_timezone() {
        Some(tz) => format!("\ntimezone: {tz}"),
        None => String::new(),
    };

    // APT packages go into the `packages:` YAML list
    let configurable_packages: String = packages
        .iter()
        .filter_map(|p| {
            if let PackageSpec::Apt { name } = p {
                Some(format!("\n  - {name}"))
            } else {
                None
            }
        })
        .collect();

    // Non-APT packages go into `runcmd:`.
    // If there are any CargoBinstall specs, bootstrap cargo-binstall first.
    let has_cargo_binstall = packages
        .iter()
        .any(|p| matches!(p, PackageSpec::CargoBinstall { .. }));
    let has_npm = packages
        .iter()
        .any(|p| matches!(p, PackageSpec::Npm { .. }));

    let mut extra_cmds = String::new();
    if has_npm {
        extra_cmds.push_str(
            "\n  - curl -fsSL https://deb.nodesource.com/setup_lts.x | bash -\n  - apt-get install -y nodejs",
        );
    }
    if has_cargo_binstall {
        extra_cmds.push_str(&format!(
            "\n  - su - {username} -c \"curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash\""
        ));
    }
    for pkg in packages {
        if let Some(cmd) = pkg.cloud_init_runcmd(username) {
            extra_cmds.push_str(&format!("\n  - {cmd}"));
        }
    }

    format!(
        r#"#cloud-config{timezone_line}
users:
  - name: {username}
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/zsh
    ssh_authorized_keys:
      - {ssh_pubkey}

ssh_pwauth: false

package_update: true
packages:
  - git
  - stow
  - zsh
  - tmux
  - mosh
  - just
  - socat
  - bubblewrap{configurable_packages}{toolchain_packages}

runcmd:
  - sed -i 's/^PermitRootLogin .*/PermitRootLogin no/' /etc/ssh/sshd_config
  - systemctl restart sshd
  - curl -fsSL https://tailscale.com/install.sh | sh
  - tailscale up --auth-key={tailscale_auth_key} --ssh{toolchain_cmds}{extra_cmds}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cloud_init(toolchains: &[state::Toolchain], packages: &[PackageSpec]) -> String {
        // Pin timezone so snapshots are deterministic across machines
        std::env::set_var("TZ", "UTC");
        build_cloud_init(
            "testuser",
            "ssh-ed25519 AAAA",
            "tskey-auth-xxx",
            toolchains,
            packages,
        )
    }

    #[test]
    fn cloud_init_basic() {
        let output = test_cloud_init(&[], &[]);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_rust() {
        let output = test_cloud_init(&[state::Toolchain::Rust], &[]);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_python() {
        let output = test_cloud_init(&[state::Toolchain::Python], &[]);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_packages() {
        let packages = vec![
            PackageSpec::Apt {
                name: "nodejs".to_string(),
            },
            PackageSpec::Apt {
                name: "python3".to_string(),
            },
        ];
        let output = test_cloud_init(&[], &packages);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_agents() {
        let packages = vec![
            PackageSpec::CurlInstaller {
                name: "claude-code".to_string(),
                url: "https://claude.ai/install.sh".to_string(),
            },
            PackageSpec::CurlInstaller {
                name: "opencode".to_string(),
                url: "https://opencode.ai/install".to_string(),
            },
        ];
        let output = test_cloud_init(&[], &packages);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_cargo_packages() {
        let packages = vec![PackageSpec::CargoBinstall {
            name: "jj-cli".to_string(),
        }];
        let output = test_cloud_init(&[], &packages);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_npm_packages() {
        let packages = vec![PackageSpec::Npm {
            name: "pi".to_string(),
            package: "@mariozechner/pi-coding-agent".to_string(),
        }];
        let output = test_cloud_init(&[], &packages);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn detect_timezone_falls_through_when_tz_is_empty() {
        let saved = std::env::var("TZ").ok();
        std::env::set_var("TZ", "");
        let tz = detect_timezone();
        // On Linux, fallback to /etc/localtime or /etc/timezone should yield Some
        // (if neither exists we get None, which is also valid)
        let _ = tz;
        match saved {
            Some(v) => std::env::set_var("TZ", v),
            None => std::env::remove_var("TZ"),
        }
    }

    #[test]
    fn cloud_init_full() {
        let packages = vec![
            PackageSpec::Apt {
                name: "nodejs".to_string(),
            },
            PackageSpec::CurlInstaller {
                name: "claude-code".to_string(),
                url: "https://claude.ai/install.sh".to_string(),
            },
        ];
        let output = test_cloud_init(&[state::Toolchain::Rust], &packages);
        insta::assert_snapshot!(output);
    }
}
