// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::backend::PackageBackend;
use crate::model::{Candidate, Source};

pub struct Resolver<'a> {
    backends: Vec<&'a dyn PackageBackend>,
    priority: Vec<Source>,
}

impl<'a> Resolver<'a> {
    pub fn new(backends: Vec<&'a dyn PackageBackend>) -> Self {
        let priority = vec![Source::Alpm, Source::Aur, Source::Nix];
        Self { backends, priority }
    }

    pub fn resolve(&self, query: &str) -> Result<Vec<Candidate>, String> {
        let mut candidates = Vec::new();
        for source in &self.priority {
            for backend in &self.backends {
                if backend.source() == *source {
                    candidates.extend(backend.search(query)?);
                }
            }
        }
        Ok(candidates)
    }
}