// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Source, TargetSpec, TransactionPlan};
use pacnix_core::PackageBackend;

pub struct AlpmBackend;

impl PackageBackend for AlpmBackend {
    fn name(&self) -> &'static str {
        "alpm"
    }

    fn source(&self) -> Source {
        Source::Alpm
    }

    fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
        Err("pacnix-backend-alpm: not implemented yet".into())
    }

    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        Err("pacnix-backend-alpm: not implemented yet".into())
    }

    fn plan_install(&self, _target: &TargetSpec) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-alpm: not implemented yet".into())
    }

    fn plan_remove(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-alpm: not implemented yet".into())
    }

    fn plan_upgrade(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-alpm: not implemented yet".into())
    }
}