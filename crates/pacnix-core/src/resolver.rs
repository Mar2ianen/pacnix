// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::backend::PackageBackend;
use crate::model::{Candidate, Source};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub backend: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    Prefix,
    Substring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    ExactName,
    PrefixName,
    SubstringName,
    BackendMatch,
    BackendPriority(&'static str),
    PreferredBackend,
    PreviousPreference,
}

impl Reason {
    pub fn label(&self) -> String {
        match self {
            Reason::ExactName => "exact name match".into(),
            Reason::PrefixName => "prefix match".into(),
            Reason::SubstringName => "substring match".into(),
            Reason::BackendMatch => "matched by backend".into(),
            Reason::BackendPriority(b) => format!("{b} backend priority"),
            Reason::PreferredBackend => "preferred backend".into(),
            Reason::PreviousPreference => "previous preference".into(),
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

fn weight(reason: &Reason) -> i32 {
    match reason {
        Reason::ExactName => 150,
        Reason::PrefixName => 80,
        Reason::SubstringName => 40,
        Reason::BackendMatch => 5,
        Reason::BackendPriority(b) => match *b {
            "alpm" => 30,
            "aur" => 20,
            _ => 10,
        },
        Reason::PreferredBackend => 60,
        Reason::PreviousPreference => 1000,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedCandidate {
    pub candidate: Candidate,
    pub score: i32,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionDecision {
    Selected(RankedCandidate),
    Ambiguous(Vec<RankedCandidate>),
    NotFound { errors: Vec<BackendError> },
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

    pub fn resolve(&self, query: &str) -> ResolutionDecision {
        self.resolve_with_preference(query, None)
    }

    pub fn resolve_ranked(&self, query: &str) -> (Vec<RankedCandidate>, Vec<BackendError>) {
        let (raw, errors) = self.collect(query);
        let mut ranked: Vec<RankedCandidate> = raw
            .into_iter()
            .map(|candidate| self.rank(query, candidate, None))
            .collect();
        Self::sort_ranked(&mut ranked);
        (ranked, errors)
    }

    pub fn resolve_with_preference(
        &self,
        query: &str,
        preference: Option<(&str, &str)>,
    ) -> ResolutionDecision {
        let (raw, errors) = self.collect(query);
        if raw.is_empty() {
            return ResolutionDecision::NotFound { errors };
        }
        let mut ranked: Vec<RankedCandidate> = raw
            .into_iter()
            .map(|candidate| self.rank(query, candidate, preference))
            .collect();
        Self::sort_ranked(&mut ranked);
        if ranked.len() == 1 {
            let top = ranked.remove(0);
            return ResolutionDecision::Selected(top);
        }
        let top = &ranked[0];
        let second = &ranked[1];
        if top.score > second.score {
            let winner = ranked.remove(0);
            ResolutionDecision::Selected(winner)
        } else {
            ResolutionDecision::Ambiguous(ranked)
        }
    }

    fn collect(&self, query: &str) -> (Vec<Candidate>, Vec<BackendError>) {
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
        (candidates, backend_errors)
    }

    fn rank(
        &self,
        query: &str,
        candidate: Candidate,
        preference: Option<(&str, &str)>,
    ) -> RankedCandidate {
        let mut reasons: Vec<Reason> = Vec::new();
        reasons.push(Self::match_reason(query, &candidate));
        reasons.push(Reason::BackendPriority(candidate.source.as_str()));
        if let Some((pref_source, pref_ref)) = preference {
            if candidate.source.as_str() == pref_source && candidate.backend_ref == pref_ref {
                reasons.push(Reason::PreviousPreference);
            } else if candidate.source.as_str() == pref_source {
                reasons.push(Reason::PreferredBackend);
            }
        }
        let score: i32 = reasons.iter().map(weight).sum();
        RankedCandidate {
            candidate,
            score,
            reasons,
        }
    }

    fn match_reason(query: &str, candidate: &Candidate) -> Reason {
        if query == candidate.name {
            return Reason::ExactName;
        }
        if !query.is_empty() && candidate.name.starts_with(query) {
            return Reason::PrefixName;
        }
        if !query.is_empty() && candidate.name.contains(query) {
            return Reason::SubstringName;
        }
        Reason::BackendMatch
    }

    fn sort_ranked(ranked: &mut [RankedCandidate]) {
        ranked.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| Self::ordinal(&a.candidate).cmp(&Self::ordinal(&b.candidate)))
        });
    }

    fn ordinal(candidate: &Candidate) -> usize {
        match &candidate.source {
            Source::Alpm => 0,
            Source::Aur => 1,
            Source::Nix => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ExecutionContext;
    use crate::model::{Candidate, InstalledPackage, TransactionOperation, TransactionPlan};

    struct MockBackend {
        source: Source,
        name: &'static str,
        results: Result<Vec<Candidate>, String>,
    }

    impl PackageBackend for MockBackend {
        fn name(&self) -> &'static str {
            self.name
        }
        fn source(&self) -> Source {
            self.source.clone()
        }
        fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
            self.results.clone()
        }
        fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
            Ok(Vec::new())
        }
        fn plan_install(&self, _target: &Candidate) -> Result<TransactionPlan, String> {
            Err("unused in tests".into())
        }
        fn plan_remove(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
            Err("unused in tests".into())
        }
        fn plan_upgrade(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
            Err("unused in tests".into())
        }
        fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
            Err("unused in tests".into())
        }
        fn execute_operation(
            &self,
            _op: &TransactionOperation,
            _ctx: &ExecutionContext,
        ) -> Result<(), String> {
            Err("unused in tests".into())
        }
        fn receipt_instances(
            &self,
            _plan: &TransactionPlan,
            _before: &[InstalledPackage],
            _after: &[InstalledPackage],
        ) -> Result<Vec<InstalledPackage>, String> {
            Err("unused in tests".into())
        }
    }

    fn candidate(source: Source, provider: &str, name: &str) -> Candidate {
        Candidate {
            source,
            provider: provider.to_string(),
            backend_ref: format!("{provider}/{name}"),
            name: name.to_string(),
            version: None,
            description: None,
            package_base: None,
            url_path: None,
        }
    }

    #[test]
    fn exact_name_beats_substring() {
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Nix,
            name: "nix",
            results: Ok(vec![
                candidate(Source::Nix, "nixpkgs", "firefox-esr"),
                candidate(Source::Nix, "nixpkgs", "firefox"),
            ]),
        })]);
        match resolver.resolve("firefox") {
            ResolutionDecision::Selected(winner) => {
                assert_eq!(winner.candidate.name, "firefox");
                assert!(winner.reasons.contains(&Reason::ExactName));
            }
            other => panic!("expected Best, got {other:?}"),
        }
    }

    #[test]
    fn backend_priority_breaks_score_ties() {
        let resolver = Resolver::new(vec![
            Box::new(MockBackend {
                source: Source::Aur,
                name: "aur",
                results: Ok(vec![candidate(Source::Aur, "aur", "firefox")]),
            }),
            Box::new(MockBackend {
                source: Source::Alpm,
                name: "alpm",
                results: Ok(vec![candidate(Source::Alpm, "extra", "firefox")]),
            }),
        ]);
        match resolver.resolve("firefox") {
            ResolutionDecision::Selected(winner) => {
                assert_eq!(winner.candidate.source, Source::Alpm);
            }
            other => panic!("expected Best, got {other:?}"),
        }
    }

    #[test]
    fn equal_scores_are_ambiguous() {
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Aur,
            name: "aur",
            results: Ok(vec![
                candidate(Source::Aur, "aur", "foo-bin"),
                candidate(Source::Aur, "aur", "foo-git"),
            ]),
        })]);
        match resolver.resolve("foo") {
            ResolutionDecision::Ambiguous(ranked) => {
                assert_eq!(ranked.len(), 2);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn previous_preference_wins() {
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Alpm,
            name: "alpm",
            results: Ok(vec![
                candidate(Source::Alpm, "extra", "firefox"),
                candidate(Source::Alpm, "extra", "firefox-esr"),
            ]),
        })]);
        match resolver.resolve_with_preference("firefox", Some(("alpm", "extra/firefox"))) {
            ResolutionDecision::Selected(winner) => {
                assert_eq!(winner.candidate.name, "firefox");
                assert!(winner.reasons.contains(&Reason::PreviousPreference));
            }
            other => panic!("expected Best, got {other:?}"),
        }
    }

    #[test]
    fn backend_errors_surface_as_not_found() {
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Nix,
            name: "nix",
            results: Err("flake evaluation exploded".into()),
        })]);
        match resolver.resolve("zzz") {
            ResolutionDecision::NotFound { errors } => {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].backend, "nix");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn previous_preference_and_preferred_backend() {
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Alpm,
            name: "alpm",
            results: Ok(vec![candidate(Source::Alpm, "extra", "firefox")]),
        })]);
        match resolver.resolve_with_preference("firefox", Some(("alpm", "extra/firefox"))) {
            ResolutionDecision::Selected(winner) => {
                assert!(winner.reasons.contains(&Reason::PreviousPreference));
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Alpm,
            name: "alpm",
            results: Ok(vec![
                candidate(Source::Alpm, "extra", "firefox"),
                candidate(Source::Alpm, "extra", "firefox-esr"),
            ]),
        })]);
        match resolver.resolve_with_preference("firefox", Some(("alpm", "extra/firefox-esr"))) {
            ResolutionDecision::Selected(winner) => {
                assert_eq!(winner.candidate.name, "firefox-esr");
                assert!(winner
                    .reasons
                    .iter()
                    .any(|r| matches!(r, Reason::PreviousPreference)));
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Alpm,
            name: "alpm",
            results: Ok(vec![candidate(Source::Alpm, "extra", "firefox")]),
        })]);
        match resolver.resolve_with_preference("firefox", Some(("alpm", "extra/other-firefox"))) {
            ResolutionDecision::Selected(winner) => {
                assert_eq!(winner.candidate.name, "firefox");
                assert!(winner
                    .reasons
                    .iter()
                    .any(|r| matches!(r, Reason::PreferredBackend)));
            }
            other => panic!("expected Selected, got {other:?}"),
        }
    }

    #[test]
    fn non_name_match_is_backend_match_not_substring() {
        let resolver = Resolver::new(vec![Box::new(MockBackend {
            source: Source::Aur,
            name: "aur",
            results: Ok(vec![Candidate {
                source: Source::Aur,
                provider: "aur".into(),
                backend_ref: "aur/hiddify-bin".into(),
                name: "hiddify-bin".into(),
                version: None,
                description: Some("proxy for hiddify".into()),
                package_base: Some("hiddify-bin".into()),
                url_path: None,
            }]),
        })]);
        match resolver.resolve("hiddify proxy") {
            ResolutionDecision::Selected(winner) => {
                assert!(winner.reasons.contains(&Reason::BackendMatch));
                assert!(!winner.reasons.contains(&Reason::SubstringName));
            }
            other => panic!("expected Selected, got {other:?}"),
        }
    }
}
