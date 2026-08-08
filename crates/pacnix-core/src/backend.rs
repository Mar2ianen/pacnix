// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::model::{Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContext {
    pub use_sudo: bool,
}

#[derive(Debug, Clone, Default)]
pub struct UpgradeImpact {
    pub entries: Vec<UpgradeImpactEntry>,
}

#[derive(Debug, Clone)]
pub struct UpgradeImpactEntry {
    pub name: String,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

pub trait PackageBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn source(&self) -> Source;
    fn search(&self, query: &str) -> Result<Vec<Candidate>, String>;
    fn installed(&self) -> Result<Vec<InstalledPackage>, String>;
    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String>;
    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String>;
    fn install_size_estimate(&self, _target: &Candidate) -> Result<Option<u64>, String> {
        Ok(None)
    }
    fn remove_size_estimate(&self, _target: &InstalledPackage) -> Result<Option<u64>, String> {
        Ok(None)
    }
    fn upgrade_impact_estimate(
        &self,
        _plan: &TransactionPlan,
    ) -> Result<Option<UpgradeImpact>, String> {
        Ok(None)
    }
    fn execute_operation(
        &self,
        op: &TransactionOperation,
        ctx: &ExecutionContext,
    ) -> Result<(), String>;
    fn receipt_instances(
        &self,
        plan: &TransactionPlan,
        before: &[InstalledPackage],
        after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String>;
}
