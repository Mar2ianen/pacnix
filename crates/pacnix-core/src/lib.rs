// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

pub mod backend;
pub mod command;
pub mod executor;
pub mod interaction;
pub mod model;
pub mod parsers;
pub mod resolver;
pub mod storage;

pub use backend::{ExecutionContext, PackageBackend};
pub use command::Command;
pub use executor::{BackendPlan, BackendReport, ExecutionBatch, Executor};
pub use interaction::{Interaction, PrivilegeProvider, PrivilegedOperation};
pub use model::{
    Candidate, InstallReceipt, InstalledPackage, Provenance, Source, TargetSpec,
    TransactionOperation, TransactionPlan,
};
pub use resolver::{BackendError, RankedCandidate, Reason, ResolutionDecision, Resolver};
pub use storage::Storage;
