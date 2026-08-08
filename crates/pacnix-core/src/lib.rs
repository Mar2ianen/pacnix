// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

pub mod backend;
pub mod command;
pub mod interaction;
pub mod model;
pub mod parsers;
pub mod resolver;
pub mod storage;

pub use backend::PackageBackend;
pub use command::Command;
pub use interaction::{Interaction, PrivilegeProvider, PrivilegedOperation};
pub use model::{Candidate, InstalledPackage, Provenance, Source, TargetSpec, TransactionPlan, TransactionOperation};
pub use resolver::{BackendError, ResolutionResult, Resolver};
pub use storage::Storage;