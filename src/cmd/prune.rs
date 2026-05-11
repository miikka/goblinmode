use anyhow::Result;
use std::io::{self, Write};

use crate::config;
use crate::hetzner::HetznerClient;
use crate::tailscale::TailscaleClient;

trait PruneRunner {
    fn load_config_keys(&mut self) -> Result<(String, String)>;
    fn list_servers(&mut self, hetzner_token: &str) -> Result<Vec<crate::hetzner::ServerInfo>>;
    fn list_snapshots(&mut self, hetzner_token: &str) -> Result<Vec<crate::hetzner::SnapshotInfo>>;
    fn list_devices(
        &mut self,
        tailscale_api_key: &str,
    ) -> Result<Vec<crate::tailscale::DeviceInfo>>;
    fn confirm_delete_all(&mut self) -> Result<bool>;
    fn delete_server(&mut self, hetzner_token: &str, server_id: u64) -> Result<()>;
    fn delete_image(&mut self, hetzner_token: &str, image_id: u64) -> Result<()>;
    fn delete_device(&mut self, tailscale_api_key: &str, device_id: &str) -> Result<()>;
    fn line(&mut self, message: String);
}

struct RealPruneRunner;

impl PruneRunner for RealPruneRunner {
    fn load_config_keys(&mut self) -> Result<(String, String)> {
        let cfg = config::load_config()?;
        Ok((cfg.hetzner_api_token, cfg.tailscale_api_key))
    }

    fn list_servers(&mut self, hetzner_token: &str) -> Result<Vec<crate::hetzner::ServerInfo>> {
        let hetzner = HetznerClient::new(hetzner_token.to_string());
        hetzner.list_goblinmode_servers()
    }

    fn list_snapshots(&mut self, hetzner_token: &str) -> Result<Vec<crate::hetzner::SnapshotInfo>> {
        let hetzner = HetznerClient::new(hetzner_token.to_string());
        hetzner.list_goblinmode_snapshots()
    }

    fn list_devices(
        &mut self,
        tailscale_api_key: &str,
    ) -> Result<Vec<crate::tailscale::DeviceInfo>> {
        let tailscale = TailscaleClient::new(tailscale_api_key.to_string());
        tailscale.list_gob_devices()
    }

    fn confirm_delete_all(&mut self) -> Result<bool> {
        print!("\nDelete all of them? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        Ok(answer.trim().eq_ignore_ascii_case("y"))
    }

    fn delete_server(&mut self, hetzner_token: &str, server_id: u64) -> Result<()> {
        let hetzner = HetznerClient::new(hetzner_token.to_string());
        hetzner.delete_server(server_id)
    }

    fn delete_image(&mut self, hetzner_token: &str, image_id: u64) -> Result<()> {
        let hetzner = HetznerClient::new(hetzner_token.to_string());
        hetzner.delete_image(image_id)
    }

    fn delete_device(&mut self, tailscale_api_key: &str, device_id: &str) -> Result<()> {
        let tailscale = TailscaleClient::new(tailscale_api_key.to_string());
        tailscale.delete_device_by_id(device_id)
    }

    fn line(&mut self, message: String) {
        println!("{}", message);
    }
}

pub fn run() -> Result<()> {
    let mut runner = RealPruneRunner;
    run_with(&mut runner)
}

fn run_with<R: PruneRunner>(runner: &mut R) -> Result<()> {
    let (hetzner_token, tailscale_api_key) = runner.load_config_keys()?;
    let servers = runner.list_servers(&hetzner_token)?;
    let snapshots = runner.list_snapshots(&hetzner_token)?;
    let devices = runner.list_devices(&tailscale_api_key)?;

    if servers.is_empty() && snapshots.is_empty() && devices.is_empty() {
        runner.line("No goblinmode resources found.".to_string());
        return Ok(());
    }

    if !servers.is_empty() {
        runner.line(format!("Found {} server(s):", servers.len()));
        for s in &servers {
            runner.line(format!(
                "  {} (id: {}, status: {}, ip: {})",
                s.name, s.id, s.status, s.ipv4
            ));
        }
    }

    if !snapshots.is_empty() {
        runner.line(format!("Found {} snapshot(s):", snapshots.len()));
        for s in &snapshots {
            runner.line(format!(
                "  {} (id: {}, created: {})",
                s.description, s.id, s.created
            ));
        }
    }

    if !devices.is_empty() {
        runner.line(format!("Found {} Tailscale device(s):", devices.len()));
        for d in &devices {
            runner.line(format!("  {}", d.hostname));
        }
    }

    if !runner.confirm_delete_all()? {
        runner.line("Aborted.".to_string());
        return Ok(());
    }

    for s in &servers {
        runner.line(format!("Deleting server {} (id: {})...", s.name, s.id));
        match runner.delete_server(&hetzner_token, s.id) {
            Ok(()) => runner.line("done".to_string()),
            Err(e) => runner.line(format!("failed: {}", e)),
        }
    }

    for s in &snapshots {
        runner.line(format!(
            "Deleting snapshot {} (id: {})...",
            s.description, s.id
        ));
        match runner.delete_image(&hetzner_token, s.id) {
            Ok(()) => runner.line("done".to_string()),
            Err(e) => runner.line(format!("failed: {}", e)),
        }
    }

    for d in &devices {
        runner.line(format!("Deleting Tailscale device {}...", d.hostname));
        match runner.delete_device(&tailscale_api_key, &d.id) {
            Ok(()) => runner.line("done".to_string()),
            Err(e) => runner.line(format!("failed: {}", e)),
        }
    }

    runner.line("Prune complete.".to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockRunner {
        hetzner_token: String,
        tailscale_api_key: String,
        servers: Vec<crate::hetzner::ServerInfo>,
        snapshots: Vec<crate::hetzner::SnapshotInfo>,
        devices: Vec<crate::tailscale::DeviceInfo>,
        confirm: bool,
        calls: RefCell<Vec<String>>,
        lines: RefCell<Vec<String>>,
    }

    impl PruneRunner for MockRunner {
        fn load_config_keys(&mut self) -> Result<(String, String)> {
            Ok((self.hetzner_token.clone(), self.tailscale_api_key.clone()))
        }

        fn list_servers(
            &mut self,
            _hetzner_token: &str,
        ) -> Result<Vec<crate::hetzner::ServerInfo>> {
            Ok(std::mem::take(&mut self.servers))
        }

        fn list_snapshots(
            &mut self,
            _hetzner_token: &str,
        ) -> Result<Vec<crate::hetzner::SnapshotInfo>> {
            Ok(std::mem::take(&mut self.snapshots))
        }

        fn list_devices(
            &mut self,
            _tailscale_api_key: &str,
        ) -> Result<Vec<crate::tailscale::DeviceInfo>> {
            Ok(std::mem::take(&mut self.devices))
        }

        fn confirm_delete_all(&mut self) -> Result<bool> {
            self.calls.borrow_mut().push("confirm".to_string());
            Ok(self.confirm)
        }

        fn delete_server(&mut self, _hetzner_token: &str, server_id: u64) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("delete_server:{server_id}"));
            Ok(())
        }

        fn delete_image(&mut self, _hetzner_token: &str, image_id: u64) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("delete_image:{image_id}"));
            Ok(())
        }

        fn delete_device(&mut self, _tailscale_api_key: &str, device_id: &str) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("delete_device:{device_id}"));
            Ok(())
        }

        fn line(&mut self, message: String) {
            self.lines.borrow_mut().push(message);
        }
    }

    fn server(id: u64, name: &str) -> crate::hetzner::ServerInfo {
        crate::hetzner::ServerInfo {
            id,
            name: name.to_string(),
            status: "running".to_string(),
            ipv4: "1.2.3.4".to_string(),
            created: "2026-02-20T00:00:00+00:00".to_string(),
        }
    }

    fn snapshot(id: u64, description: &str) -> crate::hetzner::SnapshotInfo {
        crate::hetzner::SnapshotInfo {
            id,
            description: description.to_string(),
            created: "2026-02-20".to_string(),
        }
    }

    fn device(id: &str, hostname: &str) -> crate::tailscale::DeviceInfo {
        crate::tailscale::DeviceInfo {
            id: id.to_string(),
            hostname: hostname.to_string(),
        }
    }

    #[test]
    fn run_with_exits_early_when_no_resources() {
        let mut runner = MockRunner {
            hetzner_token: "h".to_string(),
            tailscale_api_key: "t".to_string(),
            servers: vec![],
            snapshots: vec![],
            devices: vec![],
            confirm: true,
            calls: RefCell::new(Vec::new()),
            lines: RefCell::new(Vec::new()),
        };
        run_with(&mut runner).unwrap();
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(
            runner.lines.borrow().as_slice(),
            &["No goblinmode resources found."]
        );
    }

    #[test]
    fn run_with_aborts_when_confirmation_is_no() {
        let mut runner = MockRunner {
            hetzner_token: "h".to_string(),
            tailscale_api_key: "t".to_string(),
            servers: vec![server(1, "gob-a")],
            snapshots: vec![snapshot(2, "snap-a")],
            devices: vec![device("d1", "gob-a")],
            confirm: false,
            calls: RefCell::new(Vec::new()),
            lines: RefCell::new(Vec::new()),
        };
        run_with(&mut runner).unwrap();
        assert_eq!(runner.calls.borrow().as_slice(), &["confirm"]);
        assert!(runner.lines.borrow().contains(&"Aborted.".to_string()));
    }

    #[test]
    fn run_with_deletes_all_resources_when_confirmed() {
        let mut runner = MockRunner {
            hetzner_token: "h".to_string(),
            tailscale_api_key: "t".to_string(),
            servers: vec![server(1, "gob-a"), server(2, "gob-b")],
            snapshots: vec![snapshot(3, "snap-a")],
            devices: vec![device("d1", "gob-a"), device("d2", "gob-b")],
            confirm: true,
            calls: RefCell::new(Vec::new()),
            lines: RefCell::new(Vec::new()),
        };
        run_with(&mut runner).unwrap();
        assert_eq!(
            runner.calls.borrow().as_slice(),
            &[
                "confirm",
                "delete_server:1",
                "delete_server:2",
                "delete_image:3",
                "delete_device:d1",
                "delete_device:d2",
            ]
        );
        assert!(runner
            .lines
            .borrow()
            .contains(&"Prune complete.".to_string()));
    }

    struct FailingMockRunner {
        hetzner_token: String,
        tailscale_api_key: String,
        servers: Vec<crate::hetzner::ServerInfo>,
        snapshots: Vec<crate::hetzner::SnapshotInfo>,
        devices: Vec<crate::tailscale::DeviceInfo>,
        lines: RefCell<Vec<String>>,
    }

    impl PruneRunner for FailingMockRunner {
        fn load_config_keys(&mut self) -> Result<(String, String)> {
            Ok((self.hetzner_token.clone(), self.tailscale_api_key.clone()))
        }

        fn list_servers(&mut self, _: &str) -> Result<Vec<crate::hetzner::ServerInfo>> {
            Ok(std::mem::take(&mut self.servers))
        }

        fn list_snapshots(&mut self, _: &str) -> Result<Vec<crate::hetzner::SnapshotInfo>> {
            Ok(std::mem::take(&mut self.snapshots))
        }

        fn list_devices(&mut self, _: &str) -> Result<Vec<crate::tailscale::DeviceInfo>> {
            Ok(std::mem::take(&mut self.devices))
        }

        fn confirm_delete_all(&mut self) -> Result<bool> {
            Ok(true)
        }

        fn delete_server(&mut self, _: &str, _: u64) -> Result<()> {
            anyhow::bail!("server delete failed")
        }

        fn delete_image(&mut self, _: &str, _: u64) -> Result<()> {
            anyhow::bail!("image delete failed")
        }

        fn delete_device(&mut self, _: &str, _: &str) -> Result<()> {
            anyhow::bail!("device delete failed")
        }

        fn line(&mut self, message: String) {
            self.lines.borrow_mut().push(message);
        }
    }

    #[test]
    fn run_with_reports_failed_deletions_and_continues() {
        let mut runner = FailingMockRunner {
            hetzner_token: "h".to_string(),
            tailscale_api_key: "t".to_string(),
            servers: vec![server(1, "gob-a")],
            snapshots: vec![snapshot(2, "snap-a")],
            devices: vec![device("d1", "gob-a")],
            lines: RefCell::new(Vec::new()),
        };
        run_with(&mut runner).unwrap();
        let lines = runner.lines.borrow();
        assert!(lines.iter().any(|l| l.contains("failed")));
        assert!(lines.contains(&"Prune complete.".to_string()));
    }
}
