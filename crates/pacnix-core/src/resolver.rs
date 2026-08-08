// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::backend::PackageBackend;
use crate::model::{Candidate, Source};

pub struct ResolutionResult {
    pub candidates: Vec<Candidate>,
    pub backend_errors: Vec<BackendError>,
}

impl ResolutionResult {
    pub fn is_ok(&self) -> bool {
        self.backend_errors.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty() && self.backend_errors.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub backend: String,
    pub message: String,
}

pub struct Resolver {
    backends: Vec<Box<dyn PackageBackend>>,
    priority: Vec<Source>,
}

impl Resolver {
    pub fn new(backends: Vec<Box<dyn PackageBackend>>) -> Self {
        let priority = vec![Source::Alpm, Source::Aur, Source::Nix];
        Self { backends, priority }
    }

    pub fn backends(&self) -> &[Box<dyn PackageBackend>] {
        &self.backends
    }

    pub fn resolve(&self, query: &str) -> ResolutionResult {
        let mut candidates = Vec::new();
        let mut backend_errors = Vec::new();
        for source in &self.priority {
            for backend in &self.backends {
                if backend.source() != *source {
                    continue;
                }
                match backend.search(query) {
                    Ok(mut found) => candidates.append(&mut found),
                    Err(message) => backend_errors.push(BackendError {
                        backend: backend.name().to_string(),
                        message,
                    }),
                }
            }
        }
        ResolutionResult {
            candidates,
            backend_errors,
        }
    }
}