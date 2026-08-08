// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Source, TargetSpec, TransactionPlan};
use pacnix_core::PackageBackend;

pub struct AurBackend;

impl PackageBackend for AurBackend {
    fn name(&self) -> &'static str {
        "aur"
    }

    fn source(&self) -> Source {
        Source::Aur
    }

    fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
        Err("pacnix-backend-aur: not implemented yet".into())
    }

    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        Err("pacnix-backend-aur: not implemented yet".into())
    }

    fn plan_install(&self, _target: &TargetSpec) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-aur: not implemented yet".into())
    }

    fn plan_remove(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-aur: not implemented yet".into())
    }

    fn plan_upgrade(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-aur: not implemented yet".into())
    }
}