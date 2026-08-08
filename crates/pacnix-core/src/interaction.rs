// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::model::{Candidate, TransactionPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interaction {
    SelectCandidate(Vec<Candidate>),
    Confirm(TransactionPlan),
    RequestPrivilege(PrivilegedOperation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivilegedOperation {
    Install(String),
    Remove(String),
    Upgrade,
}

pub trait PrivilegeProvider {
    fn available(&self) -> bool;
    fn elevate(&self, operation: PrivilegedOperation) -> Result<(), String>;
}
