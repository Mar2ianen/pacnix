// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Source, TargetSpec, TransactionPlan};
use pacnix_core::PackageBackend;

pub struct NixBackend;

impl PackageBackend for NixBackend {
    fn name(&self) -> &'static str {
        "nix"
    }

    fn source(&self) -> Source {
        Source::Nix
    }

    fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
        Err("pacnix-backend-nix: not implemented yet".into())
    }

    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        Err("pacnix-backend-nix: not implemented yet".into())
    }

    fn plan_install(&self, _target: &TargetSpec) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-nix: not implemented yet".into())
    }

    fn plan_remove(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-nix: not implemented yet".into())
    }

    fn plan_upgrade(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("pacnix-backend-nix: not implemented yet".into())
    }
}