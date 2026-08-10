// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::{ExecutionContext, PackageBackend, UpgradeImpact, UpgradeImpactEntry};

use crate::parsers;

const PACMAN: &str = "pacman";

pub struct AlpmBackend;

fn local_install_dates() -> HashMap<String, i64> {
    let mut dates = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/var/lib/pacman/local") else {
        return dates;
    };
    let dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    if dirs.is_empty() {
        return dates;
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in dirs.chunks(dirs.len().div_ceil(workers)) {
            let chunk = chunk.to_vec();
            handles.push(scope.spawn(move || {
                let mut sub = HashMap::new();
                for path in chunk {
                    let Ok(content) = std::fs::read_to_string(path.join("desc")) else {
                        continue;
                    };
                    let name = parsers::desc_field(&content, "%NAME%");
                    let date = parsers::desc_field(&content, "%INSTALLDATE%")
                        .and_then(|d| d.parse::<i64>().ok());
                    if let (Some(name), Some(date)) = (name, date) {
                        sub.insert(name, date);
                    }
                }
                sub
            }));
        }
        for handle in handles {
            if let Ok(sub) = handle.join() {
                dates.extend(sub);
            }
        }
    });
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
                package: target.backend_ref.clone(),
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

    /// Candidates among the repos that have a newer version installed
    /// upstream, listed by `pacman -Su --print-format %r/%n`. The version
    /// is resolved by pacman itself, so it stays unknown here.
    fn outdated(&self, _installed: &[String]) -> Result<Vec<Candidate>, String> {
        let output = run_pacman(&["-Su", "--print-format", "%r/%n"])?;
        Ok(parsers::parse_upgrade_candidates(&output)
            .into_iter()
            .map(|(repo, name)| Candidate {
                source: Source::Alpm,
                provider: repo.clone(),
                backend_ref: format!("{repo}/{name}"),
                name,
                version: None,
                description: None,
                package_base: None,
                url_path: None,
            })
            .collect())
    }

    /// One system upgrade plan covering the given targets; an empty target
    /// list plans nothing, so `-Syu` stays silent when there is nothing to
    /// do. The name lists the concrete packages for the summary.
    fn plan_upgrade_chain(&self, targets: &[Candidate]) -> Result<Vec<TransactionPlan>, String> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        Ok(vec![TransactionPlan {
            backend_ref: "system".into(),
            name: names.join(", "),
            operations: vec![TransactionOperation::SystemUpgrade {
                system: "pacman".into(),
            }],
            requires_privilege: true,
        }])
    }

    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: "system".into(),
            name: "alpm system upgrade".into(),
            operations: vec![TransactionOperation::SystemUpgrade {
                system: "pacman".into(),
            }],
            requires_privilege: true,
        })
    }

    fn install_size_estimate(&self, target: &Candidate) -> Result<Option<u64>, String> {
        let output = run_pacman(&["-Si", &target.backend_ref])?;
        Ok(parsers::parse_installed_size(&output))
    }

    fn remove_size_estimate(&self, target: &InstalledPackage) -> Result<Option<u64>, String> {
        let output = match run_pacman(&["-Qi", &target.name]) {
            Ok(out) => out,
            Err(_) => return Ok(None),
        };
        Ok(parsers::parse_installed_size(&output))
    }

    fn upgrade_impact_estimate(
        &self,
        _plan: &TransactionPlan,
    ) -> Result<Option<UpgradeImpact>, String> {
        let output = run_pacman(&["-Su", "--print-format", "%r/%n"])?;
        let mut entries = Vec::new();
        for (repo, name) in parsers::parse_upgrade_candidates(&output) {
            let new_size = run_pacman(&["-Si", &format!("{repo}/{name}")])
                .ok()
                .as_deref()
                .and_then(parsers::parse_installed_size);
            let old_size = run_pacman(&["-Qi", &name])
                .ok()
                .as_deref()
                .and_then(parsers::parse_installed_size);
            entries.push(UpgradeImpactEntry {
                name,
                old_size,
                new_size,
            });
        }
        Ok(Some(UpgradeImpact { entries }))
    }

    fn receipt_instances(
        &self,
        plan: &TransactionPlan,
        _before: &[InstalledPackage],
        after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String> {
        Ok(after
            .iter()
            .filter(|p| p.name == plan.name)
            .cloned()
            .collect())
    }

    fn execute_operation(
        &self,
        op: &TransactionOperation,
        ctx: &ExecutionContext,
    ) -> Result<(), String> {
        match op {
            TransactionOperation::InstallPackage { package } => {
                run_pacman_elevated(ctx, &["-S", "--noconfirm", package])
            }
            TransactionOperation::RemovePackage { package } => {
                run_pacman_elevated(ctx, &["-R", "--noconfirm", package])
            }
            TransactionOperation::UpgradePackage { package } => {
                run_pacman_elevated(ctx, &["-S", "--noconfirm", "--needed", package])
            }
            TransactionOperation::SystemUpgrade { system } if system == "pacman" => {
                run_pacman_elevated(ctx, &["-Syu", "--noconfirm"])
            }
            _ => Err(format!("alpm: unsupported operation {op:?}")),
        }
    }
}

fn run_pacman(args: &[&str]) -> Result<String, String> {
    let output = Command::new(PACMAN)
        .args(args)
        .env("LC_ALL", "C")
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

fn run_pacman_elevated(ctx: &ExecutionContext, args: &[&str]) -> Result<(), String> {
    let mut command = ctx.build_command(PACMAN)?;
    let status = command
        .args(args)
        .status()
        .map_err(|e| format!("failed to run pacman: {e}"))?;
    if !status.success() {
        return Err(format!(
            "pacman {} failed with status {status}",
            args.join(" ")
        ));
    }
    Ok(())
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

    #[test]
    fn receipt_instances_exclude_dependencies() {
        let backend = AlpmBackend;
        let plan = TransactionPlan {
            backend_ref: "extra/foo".into(),
            name: "foo".into(),
            operations: vec![TransactionOperation::InstallPackage {
                package: "extra/foo".into(),
            }],
            requires_privilege: true,
        };
        let pkg = |name: &str| InstalledPackage {
            source: Source::Alpm,
            backend_ref: format!("local/{name}"),
            name: name.into(),
            version: Some("1.0-1".into()),
            scope: None,
            installed_at: None,
            provenance: pacnix_core::Provenance::SyncKnown,
        };
        let after = vec![pkg("foo"), pkg("libfoo")];
        let receipts = backend.receipt_instances(&plan, &[], &after).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].name, "foo");
    }

    #[test]
    fn upgrade_all_is_single_system_upgrade() {
        let backend = AlpmBackend;
        let plan = backend.plan_upgrade_all().unwrap();
        assert_eq!(
            plan.operations,
            vec![TransactionOperation::SystemUpgrade {
                system: "pacman".into(),
            }]
        );
        assert!(plan.requires_privilege);
    }

    #[test]
    fn upgrade_chain_plans_system_upgrade_only_with_targets() {
        let backend = AlpmBackend;
        assert_eq!(backend.plan_upgrade_chain(&[]).unwrap(), Vec::new());
        let plan = backend
            .plan_upgrade_chain(&[
                Candidate {
                    source: Source::Alpm,
                    provider: "extra".into(),
                    backend_ref: "extra/foo".into(),
                    name: "foo".into(),
                    version: None,
                    description: None,
                    package_base: None,
                    url_path: None,
                },
                Candidate {
                    source: Source::Alpm,
                    provider: "extra".into(),
                    backend_ref: "extra/bar".into(),
                    name: "bar".into(),
                    version: None,
                    description: None,
                    package_base: None,
                    url_path: None,
                },
            ])
            .unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].name, "foo, bar");
        assert_eq!(
            plan[0].operations,
            vec![TransactionOperation::SystemUpgrade {
                system: "pacman".into(),
            }]
        );
        assert!(plan[0].requires_privilege);
    }

    #[test]
    fn install_plan_targets_repo_qualified_package() {
        let backend = AlpmBackend;
        let cand = Candidate {
            source: Source::Alpm,
            provider: "chaotic-aur".into(),
            backend_ref: "chaotic-aur/foo-bin".into(),
            name: "foo-bin".into(),
            version: Some("1.2-3".into()),
            description: None,
            package_base: None,
            url_path: None,
        };
        let plan = backend.plan_install(&cand).unwrap();
        assert_eq!(plan.backend_ref, "chaotic-aur/foo-bin");
        assert_eq!(
            plan.operations,
            vec![TransactionOperation::InstallPackage {
                package: "chaotic-aur/foo-bin".into(),
            }],
            "install must pin the chosen repo, not let pacman pick by pacman.conf order"
        );
    }
}
