// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::collections::HashMap;
use std::process::Command;

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::PackageBackend;

use crate::parsers;

const PACMAN: &str = "pacman";

pub struct AlpmBackend;

fn local_install_dates() -> HashMap<String, i64> {
    let mut dates = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/var/lib/pacman/local") else {
        return dates;
    };
    for entry in entries.flatten() {
        let Ok(content) = std::fs::read_to_string(entry.path().join("desc")) else {
            continue;
        };
        let name = parsers::desc_field(&content, "%NAME%");
        let date =
            parsers::desc_field(&content, "%INSTALLDATE%").and_then(|d| d.parse::<i64>().ok());
        if let (Some(name), Some(date)) = (name, date) {
            dates.insert(name, date);
        }
    }
    dates
}

impl PackageBackend for AlpmBackend {
    fn name(&self) -> &'static str {
        "alpm"
    }

    fn source(&self) -> Source {
        Source::Alpm
    }

    fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
        let output = run_pacman(&["-Ss", query])?;
        Ok(parsers::parse_search(&output))
    }

    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        let native = run_pacman(&["-Qn"])?;
        let foreign = run_pacman(&["-Qm"])?;
        let dates = local_install_dates();
        let mut pkgs = parsers::parse_installed(&native, pacnix_core::Provenance::SyncKnown);
        pkgs.extend(parsers::parse_installed(
            &foreign,
            pacnix_core::Provenance::Foreign,
        ));
        for pkg in &mut pkgs {
            pkg.installed_at = dates.get(&pkg.name).copied();
        }
        Ok(pkgs)
    }

    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::InstallPackage {
                package: target.name.clone(),
            }],
            requires_privilege: true,
        })
    }

    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::RemovePackage {
                package: target.name.clone(),
            }],
            requires_privilege: true,
        })
    }

    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::UpgradePackage {
                package: target.name.clone(),
            }],
            requires_privilege: true,
        })
    }
}

fn run_pacman(args: &[&str]) -> Result<String, String> {
    let output = Command::new(PACMAN)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run pacman: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Ok(String::new());
        }
        return Err(format!("pacman {} failed: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_output() {
        let out = "extra/firefox 122.0-1\n    Standalone web browser\nchaotic-aur/foo-bin 1.2-3\n    Foo binary\n";
        let candidates = parsers::parse_search(out);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].provider, "extra");
        assert_eq!(candidates[0].name, "firefox");
        assert_eq!(candidates[0].version.as_deref(), Some("122.0-1"));
        assert_eq!(
            candidates[0].description.as_deref(),
            Some("Standalone web browser")
        );
        assert_eq!(candidates[1].provider, "chaotic-aur");
        assert_eq!(candidates[1].name, "foo-bin");
    }

    #[test]
    fn parse_installed_output() {
        let parsed = parsers::parse_installed(
            "firefox 122.0-1\nfoo 1.2-1\n",
            pacnix_core::Provenance::SyncKnown,
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "firefox");
        assert_eq!(parsed[0].version.as_deref(), Some("122.0-1"));
        assert_eq!(parsed[0].backend_ref, "local/firefox");
        assert_eq!(parsed[1].name, "foo");
    }
}
