// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::backend::{ExecutionContext, PackageBackend};
use crate::model::{InstallReceipt, TransactionPlan};
use crate::Storage;

pub struct Executor<'a> {
    storage: &'a Storage,
}

impl<'a> Executor<'a> {
    pub fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    pub fn execute(
        &self,
        plan: &TransactionPlan,
        backend: &dyn PackageBackend,
        ctx: &ExecutionContext,
    ) -> Result<Vec<InstallReceipt>, String> {
        let before = backend.installed()?;
        for op in &plan.operations {
            backend
                .execute_operation(op, ctx)
                .map_err(|e| format!("{}: {e}", backend.name()))?;
        }
        let after = backend.installed()?;
        let mut receipts = Vec::new();
        for pkg in backend.receipt_instances(plan, &before, &after)? {
            let receipt = InstallReceipt {
                package_name: pkg.name.clone(),
                installed_backend: pkg.source.as_str().to_string(),
                installed_backend_ref: pkg.backend_ref.clone(),
                source: pkg.source.as_str().to_string(),
                source_ref: plan.backend_ref.clone(),
                version: pkg.version.clone(),
                installed_at: pkg.installed_at.unwrap_or_else(now),
            };
            self.storage.record_receipt(&receipt)?;
            receipts.push(receipt);
        }
        Ok(receipts)
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::model::{Candidate, InstalledPackage, Source, TransactionOperation};

    struct ScriptBackend {
        store: RefCell<Vec<InstalledPackage>>,
    }

    impl PackageBackend for ScriptBackend {
        fn name(&self) -> &'static str {
            "script"
        }
        fn source(&self) -> Source {
            Source::Nix
        }
        fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
            Err("unused in executor tests".into())
        }
        fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
            Ok(self.store.borrow().clone())
        }
        fn plan_install(&self, _target: &Candidate) -> Result<TransactionPlan, String> {
            Err("unused in executor tests".into())
        }
        fn plan_remove(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
            Err("unused in executor tests".into())
        }
        fn plan_upgrade(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
            Err("unused in executor tests".into())
        }
        fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
            Err("unused in executor tests".into())
        }
        fn execute_operation(
            &self,
            op: &TransactionOperation,
            _ctx: &ExecutionContext,
        ) -> Result<(), String> {
            match op {
                TransactionOperation::InstallPackage { package } => {
                    self.store.borrow_mut().push(InstalledPackage {
                        source: Source::Nix,
                        backend_ref: format!("/nix/store/000-{package}-1.0"),
                        name: package.clone(),
                        version: Some("1.0".into()),
                        scope: None,
                        installed_at: None,
                        provenance: crate::Provenance::Unknown,
                    });
                    Ok(())
                }
                _ => Err("scripted failure".into()),
            }
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
    }

    fn ctx() -> ExecutionContext {
        ExecutionContext { use_sudo: false }
    }

    fn tmp_storage(name: &str) -> Storage {
        let path = format!("/tmp/pacnix-exec-test-{}-{}.db", name, std::process::id());
        let _ = std::fs::remove_file(&path);
        Storage::open(&path).unwrap()
    }

    #[test]
    fn new_instance_produces_receipt() {
        let storage = tmp_storage("new_instance");
        let backend = ScriptBackend {
            store: RefCell::new(vec![]),
        };
        let plan = TransactionPlan {
            backend_ref: "nixpkgs#foo".into(),
            name: "foo".into(),
            operations: vec![TransactionOperation::InstallPackage {
                package: "foo".into(),
            }],
            requires_privilege: false,
        };
        let receipts = Executor::new(&storage)
            .execute(&plan, &backend, &ctx())
            .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].package_name, "foo");
        assert_eq!(receipts[0].source_ref, "nixpkgs#foo");
        assert_eq!(receipts[0].installed_backend_ref, "/nix/store/000-foo-1.0");
        let known = storage
            .known_source_for("foo", "nix", "/nix/store/000-foo-1.0", Some("1.0"), None)
            .unwrap();
        assert_eq!(known.as_deref(), Some("nix"));
    }

    #[test]
    fn failed_operation_writes_no_receipt() {
        let storage = tmp_storage("failed_operation");
        let backend = ScriptBackend {
            store: RefCell::new(vec![]),
        };
        let plan = TransactionPlan {
            backend_ref: "nixpkgs#foo".into(),
            name: "foo".into(),
            operations: vec![
                TransactionOperation::InstallPackage {
                    package: "foo".into(),
                },
                TransactionOperation::RemovePackage {
                    package: "foo".into(),
                },
            ],
            requires_privilege: false,
        };
        let err = Executor::new(&storage)
            .execute(&plan, &backend, &ctx())
            .unwrap_err();
        assert!(
            err.contains("scripted failure"),
            "failure must propagate as a backend error, got: {err}"
        );
        let known = storage
            .known_source_for("foo", "nix", "/nix/store/000-foo-1.0", Some("1.0"), None)
            .unwrap();
        assert_eq!(known, None, "no receipt must be recorded on failure");
    }

    #[test]
    fn unchanged_state_skips_receipt() {
        let storage = tmp_storage("unchanged_state");
        let backend = ScriptBackend {
            store: RefCell::new(vec![]),
        };
        let plan = TransactionPlan {
            backend_ref: "nixpkgs#bar".into(),
            name: "bar".into(),
            operations: vec![],
            requires_privilege: false,
        };
        let receipts = Executor::new(&storage)
            .execute(&plan, &backend, &ctx())
            .unwrap();
        assert!(receipts.is_empty());
    }
}
