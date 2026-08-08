// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::model::TargetSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Install(Vec<TargetSpec>),
    Remove(Vec<TargetSpec>),
    Search(String),
    Info(TargetSpec),
    ListInstalled,
    Upgrade,
    Sync,
}
