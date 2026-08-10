// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::{ExecutionContext, PackageBackend};

use crate::parsers;

const NIX: &str = "nix";
const EXPERIMENTAL: &str = "nix-command flakes";

/// Nix profile backend. A configured profile path (None = default profile)
/// is part of the backend identity: install/remove/upgrade and outdated
/// detection all operate on exactly that profile and never mix others in.
pub struct NixBackend {
    profile: Option<PathBuf>,
}

impl NixBackend {
    pub fn new(profile: Option<PathBuf>) -> Self {
        Self { profile }
    }

    fn profile_label(&self) -> String {
        self.profile
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "default".to_string())
    }

    fn list_args(&self) -> Vec<&str> {
        let mut args = vec!["profile", "list", "--json"];
        if let Some(path) = &self.profile {
            args.push("--profile");
            args.push(path.to_str().expect("nix profile path must be UTF-8"));
        }
        args
    }
}

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
        let output = run_nix(&self.list_args())?;
        parsers::parse_profile_list(&output)
    }

    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::ProfileInstall {
                profile: self.profile_label(),
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
                profile: self.profile_label(),
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
                profile: self.profile_label(),
                element: target.name.clone(),
            }],
            requires_privilege: false,
        })
    }

    /// Candidates among the profile's elements whose pinned flake revision
    /// is older than the latest revision of the original URL. The target
    /// version is not knowable without building, so `version` stays `None`
    /// and the impact estimate is unavailable.
    fn outdated(&self, _installed: &[String]) -> Result<Vec<Candidate>, String> {
        let output = run_nix(&self.list_args())?;
        let elements = parsers::parse_profile_elements(&output)?;
        let mut outdated = Vec::new();
        for element in elements {
            let (Some(original_url), Some(locked_url)) = (
                element.original_url.as_deref(),
                element.locked_url.as_deref(),
            ) else {
                continue;
            };
            let Some(current_rev) = parsers::locked_rev_of(locked_url) else {
                continue;
            };
            let metadata = run_nix(&["flake", "metadata", original_url, "--json"])?;
            let latest_rev = parsers::flake_locked_rev(&metadata)?;
            if latest_rev != current_rev {
                outdated.push(Candidate {
                    source: Source::Nix,
                    provider: "nix".into(),
                    backend_ref: element.name.clone(),
                    name: element.name.clone(),
                    version: None,
                    description: None,
                    package_base: None,
                    url_path: None,
                });
            }
        }
        Ok(outdated)
    }

    /// One plan per profile covering every outdated element, so a single
    /// `nix profile upgrade <elements...>` executes for the whole profile.
    fn plan_upgrade_chain(&self, targets: &[Candidate]) -> Result<Vec<TransactionPlan>, String> {
        let elements: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
        if elements.is_empty() {
            return Ok(Vec::new());
        }
        let name = elements.join(", ");
        Ok(vec![TransactionPlan {
            backend_ref: self.profile_label(),
            name,
            operations: vec![TransactionOperation::ProfileUpgradeMany {
                profile: self.profile_label(),
                elements,
            }],
            requires_privilege: false,
        }])
    }

    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: self.profile_label(),
            name: "nix profile upgrade".into(),
            operations: vec![TransactionOperation::SystemUpgrade {
                system: "nix".into(),
            }],
            requires_privilege: false,
        })
    }

    /// Receipts only for elements that really changed: an element of this
    /// plan whose store path differs between before and after. Elements that
    /// vanished or were renamed are skipped — `nix profile list --json`
    /// after the operation is authoritative over plan assumptions.
    fn receipt_instances(
        &self,
        plan: &TransactionPlan,
        before: &[InstalledPackage],
        after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String> {
        let before_by_name: HashMap<&str, &InstalledPackage> =
            before.iter().map(|p| (p.name.as_str(), p)).collect();
        let after_by_name: HashMap<&str, &InstalledPackage> =
            after.iter().map(|p| (p.name.as_str(), p)).collect();
        let mut receipts = Vec::new();
        for op in &plan.operations {
            let elements: Vec<&str> = match op {
                TransactionOperation::ProfileUpgrade { element, .. } => {
                    vec![element.as_str()]
                }
                TransactionOperation::ProfileUpgradeMany { elements, .. } => {
                    elements.iter().map(String::as_str).collect()
                }
                _ => continue,
            };
            for element in elements {
                let Some(before_pkg) = before_by_name.get(element) else {
                    continue;
                };
                let Some(after_pkg) = after_by_name.get(element) else {
                    continue;
                };
                if after_pkg.backend_ref != before_pkg.backend_ref {
                    receipts.push((*after_pkg).clone());
                }
            }
        }
        Ok(receipts)
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
            TransactionOperation::ProfileUpgradeMany { profile, elements } => {
                let mut args = vec!["profile", "upgrade"];
                profile_args(&mut args, profile);
                args.extend(elements.iter().map(String::as_str));
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

    fn backend() -> NixBackend {
        NixBackend::new(None)
    }

    fn candidate(name: &str) -> Candidate {
        Candidate {
            source: Source::Nix,
            provider: "nix".into(),
            backend_ref: name.into(),
            name: name.into(),
            version: None,
            description: None,
            package_base: None,
            url_path: None,
        }
    }

    fn pkg(name: &str, store: &str) -> InstalledPackage {
        InstalledPackage {
            source: Source::Nix,
            backend_ref: store.into(),
            name: name.into(),
            version: None,
            scope: None,
            installed_at: None,
            provenance: pacnix_core::Provenance::Unknown,
        }
    }

    #[test]
    fn plans_use_profile_operations() {
        let backend = backend();
        let mut cand = candidate("ripgrep");
        cand.backend_ref = "nixpkgs#legacyPackages.x86_64-linux.ripgrep".into();
        cand.version = Some("14.1.1".into());
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

        let upgrade = backend
            .plan_upgrade(&pkg("ripgrep", "/nix/store/000-ripgrep-14.1.1"))
            .unwrap();
        assert_eq!(
            upgrade.operations,
            vec![TransactionOperation::ProfileUpgrade {
                profile: "default".into(),
                element: "ripgrep".into(),
            }]
        );
    }

    #[test]
    fn custom_profile_is_part_of_every_plan() {
        let backend = NixBackend::new(Some(PathBuf::from("/tmp/pacnix-profile")));
        let plan = backend.plan_install(&candidate("foo")).unwrap();
        assert_eq!(
            plan.operations,
            vec![TransactionOperation::ProfileInstall {
                profile: "/tmp/pacnix-profile".into(),
                attr: "foo".into(),
            }]
        );
        let plan = backend
            .plan_upgrade_chain(&[candidate("foo"), candidate("bar")])
            .unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(
            plan[0].operations,
            vec![TransactionOperation::ProfileUpgradeMany {
                profile: "/tmp/pacnix-profile".into(),
                elements: vec!["foo".into(), "bar".into()],
            }]
        );
    }

    #[test]
    fn upgrade_chain_is_one_plan_per_profile() {
        let plan = backend()
            .plan_upgrade_chain(&[candidate("foo"), candidate("bar"), candidate("baz")])
            .unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].name, "foo, bar, baz");
        assert_eq!(
            plan[0].operations,
            vec![TransactionOperation::ProfileUpgradeMany {
                profile: "default".into(),
                elements: vec!["foo".into(), "bar".into(), "baz".into()],
            }]
        );
        let single = backend().plan_upgrade_chain(&[candidate("foo")]).unwrap();
        assert_eq!(single[0].name, "foo");
        assert_eq!(
            backend().plan_upgrade_chain(&[]).unwrap(),
            Vec::new() as Vec<TransactionPlan>
        );
    }

    #[test]
    fn receipts_only_for_elements_that_changed() {
        let plan = TransactionPlan {
            backend_ref: "default".into(),
            name: "upgrade".into(),
            operations: vec![
                TransactionOperation::ProfileUpgradeMany {
                    profile: "default".into(),
                    elements: vec!["changed".into(), "same".into(), "vanished".into()],
                },
                TransactionOperation::ProfileUpgrade {
                    profile: "default".into(),
                    element: "single".into(),
                },
            ],
            requires_privilege: false,
        };
        let before = vec![
            pkg("changed", "/nix/store/aaa-changed-1.0"),
            pkg("same", "/nix/store/bbb-same-1.0"),
            pkg("vanished", "/nix/store/ccc-vanished-1.0"),
            pkg("single", "/nix/store/ddd-single-1.0"),
            pkg("untouched", "/nix/store/eee-untouched-1.0"),
        ];
        let after = vec![
            pkg("changed", "/nix/store/fff-changed-2.0"),
            pkg("same", "/nix/store/bbb-same-1.0"),
            pkg("single", "/nix/store/ggg-single-2.0"),
            pkg("untouched", "/nix/store/eee-untouched-1.0"),
        ];
        let receipts = backend().receipt_instances(&plan, &before, &after).unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0].name, "changed");
        assert_eq!(receipts[0].backend_ref, "/nix/store/fff-changed-2.0");
        assert_eq!(receipts[1].name, "single");
        assert_eq!(receipts[1].backend_ref, "/nix/store/ggg-single-2.0");
    }
}
