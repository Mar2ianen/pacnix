// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::backend::{ExecutionContext, PackageBackend};
use crate::model::{InstallReceipt, TransactionOperation, TransactionPlan};
use crate::Storage;

pub struct Executor<'a> {
    storage: &'a Storage,
}

pub struct BackendPlan<'a> {
    pub backend: &'a dyn PackageBackend,
    pub plan: &'a TransactionPlan,
    pub ctx: ExecutionContext,
}

pub struct ExecutionBatch<'a> {
    pub plans: Vec<BackendPlan<'a>>,
}

pub struct BackendReport {
    pub backend: &'static str,
    pub receipts: Vec<InstallReceipt>,
    pub error: Option<String>,
}

static PACMAN_DB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn op_uses_pacman_db(op: &TransactionOperation) -> bool {
    matches!(
        op,
        TransactionOperation::InstallPackage { .. }
            | TransactionOperation::RemovePackage { .. }
            | TransactionOperation::UpgradePackage { .. }
            | TransactionOperation::SystemUpgrade { .. }
    )
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
        let batch = ExecutionBatch {
            plans: vec![BackendPlan {
                backend,
                plan,
                ctx: ctx.clone(),
            }],
        };
        let mut reports = self.execute_batch(&batch);
        let report = reports.pop().expect("single lane must report");
        match report.error {
            Some(e) => Err(e),
            None => Ok(report.receipts),
        }
    }

    pub fn execute_batch(&self, batch: &ExecutionBatch<'_>) -> Vec<BackendReport> {
        let mut reports = Vec::with_capacity(batch.plans.len());
        if batch.plans.len() > 1 {
            std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .plans
                    .iter()
                    .map(|plan| scope.spawn(move || run_lane(plan)))
                    .collect();
                for handle in handles {
                    reports.push(handle.join().unwrap_or_else(|_| BackendReport {
                        backend: "unknown",
                        receipts: Vec::new(),
                        error: Some("lane panicked".into()),
                    }));
                }
            });
        } else if let Some(plan) = batch.plans.first() {
            reports.push(run_lane(plan));
        }
        for report in &reports {
            for receipt in &report.receipts {
                if let Err(e) = self.storage.record_receipt(receipt) {
                    eprintln!("pacnix: storage: {e}");
                }
            }
        }
        reports
    }
}

fn run_lane(plan: &BackendPlan<'_>) -> BackendReport {
    let result = (|| -> Result<Vec<InstallReceipt>, String> {
        let before = plan.backend.installed()?;
        for op in &plan.plan.operations {
            if op_uses_pacman_db(op) {
                let _guard = PACMAN_DB_LOCK
                    .lock()
                    .map_err(|_| "pacman db lock poisoned".to_string())?;
                plan.backend
                    .execute_operation(op, &plan.ctx)
                    .map_err(|e| format!("{}: {e}", plan.backend.name()))?;
            } else {
                plan.backend
                    .execute_operation(op, &plan.ctx)
                    .map_err(|e| format!("{}: {e}", plan.backend.name()))?;
            }
        }
        let after = plan.backend.installed()?;
        let mut receipts = Vec::new();
        for pkg in plan.backend.receipt_instances(plan.plan, &before, &after)? {
            receipts.push(InstallReceipt {
                package_name: pkg.name.clone(),
                installed_backend: pkg.source.as_str().to_string(),
                installed_backend_ref: pkg.backend_ref.clone(),
                source: plan.backend.source().as_str().to_string(),
                source_ref: plan.plan.backend_ref.clone(),
                version: pkg.version.clone(),
                installed_at: pkg.installed_at.unwrap_or_else(now),
            });
        }
        Ok(receipts)
    })();
    match result {
        Ok(receipts) => BackendReport {
            backend: plan.backend.name(),
            receipts,
            error: None,
        },
        Err(e) => BackendReport {
            backend: plan.backend.name(),
            receipts: Vec::new(),
            error: Some(e),
        },
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
    use std::sync::Mutex;

    use super::*;
    use crate::model::{Candidate, InstalledPackage, Source, TransactionOperation};

    struct ScriptBackend {
        store: Mutex<Vec<InstalledPackage>>,
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
            Ok(self.store.lock().unwrap().clone())
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
                    self.store.lock().unwrap().push(InstalledPackage {
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

    fn plan(name: &str, ops: Vec<TransactionOperation>) -> TransactionPlan {
        TransactionPlan {
            backend_ref: format!("nixpkgs#{name}"),
            name: name.into(),
            operations: ops,
            requires_privilege: false,
        }
    }

    #[test]
    fn new_instance_produces_receipt() {
        let storage = tmp_storage("new_instance");
        let backend = ScriptBackend {
            store: Mutex::new(vec![]),
        };
        let plan = plan(
            "foo",
            vec![TransactionOperation::InstallPackage {
                package: "foo".into(),
            }],
        );
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
            store: Mutex::new(vec![]),
        };
        let plan = plan(
            "foo",
            vec![
                TransactionOperation::InstallPackage {
                    package: "foo".into(),
                },
                TransactionOperation::RemovePackage {
                    package: "foo".into(),
                },
            ],
        );
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
            store: Mutex::new(vec![]),
        };
        let plan = plan("bar", vec![]);
        let receipts = Executor::new(&storage)
            .execute(&plan, &backend, &ctx())
            .unwrap();
        assert!(receipts.is_empty());
    }

    #[test]
    fn batch_two_lanes_record_receipts_from_both() {
        let storage = tmp_storage("batch_lanes");
        let a = ScriptBackend {
            store: Mutex::new(vec![]),
        };
        let b = ScriptBackend {
            store: Mutex::new(vec![]),
        };
        let plan_a = plan(
            "foo",
            vec![TransactionOperation::InstallPackage {
                package: "foo".into(),
            }],
        );
        let plan_b = plan(
            "bar",
            vec![TransactionOperation::InstallPackage {
                package: "bar".into(),
            }],
        );
        let batch = ExecutionBatch {
            plans: vec![
                BackendPlan {
                    backend: &a,
                    plan: &plan_a,
                    ctx: ctx(),
                },
                BackendPlan {
                    backend: &b,
                    plan: &plan_b,
                    ctx: ctx(),
                },
            ],
        };
        let reports = Executor::new(&storage).execute_batch(&batch);
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|r| r.error.is_none()));
        let known = storage
            .known_source_for("foo", "nix", "/nix/store/000-foo-1.0", Some("1.0"), None)
            .unwrap();
        assert_eq!(known.as_deref(), Some("nix"));
        let known = storage
            .known_source_for("bar", "nix", "/nix/store/000-bar-1.0", Some("1.0"), None)
            .unwrap();
        assert_eq!(known.as_deref(), Some("nix"));
    }

    #[test]
    fn batch_reports_partial_failure_per_lane() {
        let storage = tmp_storage("batch_partial");
        let good = ScriptBackend {
            store: Mutex::new(vec![]),
        };
        let failing = ScriptBackend {
            store: Mutex::new(vec![]),
        };
        let plan_good = plan(
            "foo",
            vec![TransactionOperation::InstallPackage {
                package: "foo".into(),
            }],
        );
        let plan_bad = plan(
            "bar",
            vec![TransactionOperation::RemovePackage {
                package: "bar".into(),
            }],
        );
        let batch = ExecutionBatch {
            plans: vec![
                BackendPlan {
                    backend: &good,
                    plan: &plan_good,
                    ctx: ctx(),
                },
                BackendPlan {
                    backend: &failing,
                    plan: &plan_bad,
                    ctx: ctx(),
                },
            ],
        };
        let reports = Executor::new(&storage).execute_batch(&batch);
        assert_eq!(reports.len(), 2);
        assert!(
            reports.iter().any(|r| r
                .error
                .as_deref()
                .is_some_and(|e| e.contains("scripted failure"))),
            "failing lane must surface its error"
        );
    }
}
