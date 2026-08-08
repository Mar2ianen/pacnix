// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::model::{Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContext {
    pub use_sudo: bool,
}

pub trait PackageBackend {
    fn name(&self) -> &'static str;
    fn source(&self) -> Source;
    fn search(&self, query: &str) -> Result<Vec<Candidate>, String>;
    fn installed(&self) -> Result<Vec<InstalledPackage>, String>;
    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String>;
    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    fn execute_operation(
        &self,
        op: &TransactionOperation,
        ctx: &ExecutionContext,
    ) -> Result<(), String>;
}
