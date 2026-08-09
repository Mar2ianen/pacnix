// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::process::Command;

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::{ExecutionContext, PackageBackend};

use crate::parsers;

const NIX: &str = "nix";
const EXPERIMENTAL: &str = "nix-command flakes";

pub struct NixBackend;

impl PackageBackend for NixBackend {
    fn name(&self) -> &'static str {
        "nix"
    }

    fn source(&self) -> Source {
        Source::Nix
    }

    fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
        let output = run_nix(&["search", "nixpkgs", query, "--json"])?;
        parsers::parse_search(&output)
    }

    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        let output = run_nix(&["profile", "list", "--json"])?;
        parsers::parse_profile_list(&output)
    }

    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::ProfileInstall {
                profile: "default".into(),
                attr: target.backend_ref.clone(),
            }],
            requires_privilege: false,
        })
    }

    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::ProfileRemove {
                profile: "default".into(),
                attr: target.backend_ref.clone(),
            }],
            requires_privilege: false,
        })
    }

    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::ProfileUpgrade {
                profile: "default".into(),
                element: target.name.clone(),
            }],
            requires_privilege: false,
        })
    }

    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: "system".into(),
            name: "nix profile upgrade".into(),
            operations: vec![TransactionOperation::SystemUpgrade {
                system: "nix".into(),
            }],
            requires_privilege: false,
        })
    }

    fn receipt_instances(
        &self,
        _plan: &TransactionPlan,
        before: &[InstalledPackage],
        after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String> {
        let before_names: std::collections::HashSet<&str> =
            before.iter().map(|p| p.name.as_str()).collect();
        Ok(after
            .iter()
            .filter(|p| !before_names.contains(p.name.as_str()))
            .cloned()
            .collect())
    }

    fn execute_operation(
        &self,
        op: &TransactionOperation,
        _ctx: &ExecutionContext,
    ) -> Result<(), String> {
        match op {
            TransactionOperation::ProfileInstall { profile, attr } => {
                let mut args = vec!["profile", "install"];
                profile_args(&mut args, profile);
                args.push(attr);
                run_nix_mutating(&args)
            }
            TransactionOperation::ProfileRemove { profile, attr } => {
                let mut args = vec!["profile", "remove"];
                profile_args(&mut args, profile);
                args.push(attr);
                run_nix_mutating(&args)
            }
            TransactionOperation::ProfileUpgrade { profile, element } => {
                let mut args = vec!["profile", "upgrade"];
                profile_args(&mut args, profile);
                args.push(element);
                run_nix_mutating(&args)
            }
            TransactionOperation::SystemUpgrade { system } if system == "nix" => {
                run_nix_mutating(&["profile", "upgrade"])
            }
            _ => Err(format!("nix: unsupported operation {op:?}")),
        }
    }
}

fn profile_args<'a>(args: &mut Vec<&'a str>, profile: &'a str) {
    if profile != "default" {
        args.push("--profile");
        args.push(profile);
    }
}

fn run_nix(args: &[&str]) -> Result<String, String> {
    let output = Command::new(NIX)
        .arg("--extra-experimental-features")
        .arg(EXPERIMENTAL)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run nix: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stderr_empty = stderr.is_empty();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stderr_empty && stdout.is_empty() {
            return Ok(String::new());
        }
        if stderr.contains("no results") {
            return Ok(String::new());
        }
        return Err(format!("nix {} failed: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_nix_mutating(args: &[&str]) -> Result<(), String> {
    let status = Command::new(NIX)
        .arg("--extra-experimental-features")
        .arg(EXPERIMENTAL)
        .args(args)
        .status()
        .map_err(|e| format!("failed to run nix: {e}"))?;
    if !status.success() {
        return Err(format!(
            "nix {} failed with status {status}",
            args.join(" ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_use_profile_operations() {
        let backend = NixBackend;
        let cand = Candidate {
            source: Source::Nix,
            provider: "nixpkgs".into(),
            backend_ref: "nixpkgs#legacyPackages.x86_64-linux.ripgrep".into(),
            name: "ripgrep".into(),
            version: Some("14.1.1".into()),
            description: None,
            package_base: None,
            url_path: None,
        };
        let plan = backend.plan_install(&cand).unwrap();
        assert_eq!(
            plan.backend_ref,
            "nixpkgs#legacyPackages.x86_64-linux.ripgrep"
        );
        assert_eq!(
            plan.operations,
            vec![TransactionOperation::ProfileInstall {
                profile: "default".into(),
                attr: "nixpkgs#legacyPackages.x86_64-linux.ripgrep".into(),
            }]
        );
        assert!(!plan.requires_privilege);

        let target = InstalledPackage {
            source: Source::Nix,
            backend_ref: "/nix/store/000-ripgrep-14.1.1".into(),
            name: "ripgrep".into(),
            version: Some("14.1.1".into()),
            scope: None,
            installed_at: None,
            provenance: pacnix_core::Provenance::Unknown,
        };
        let upgrade = backend.plan_upgrade(&target).unwrap();
        assert_eq!(
            upgrade.operations,
            vec![TransactionOperation::ProfileUpgrade {
                profile: "default".into(),
                element: "ripgrep".into(),
            }]
        );
    }
}
