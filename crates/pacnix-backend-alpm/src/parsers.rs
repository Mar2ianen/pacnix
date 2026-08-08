// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Source};

pub fn parse_search(output: &str) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let mut lines = output.lines();
    while let Some(line) = lines.next() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let Some((prefix, rest)) = line.split_once(' ') else {
            continue;
        };
        let Some((provider, name)) = prefix.split_once('/') else {
            continue;
        };
        let description = lines
            .next()
            .map(|d| d.trim_start().to_string())
            .filter(|d| !d.is_empty());
        candidates.push(Candidate {
            source: Source::Alpm,
            provider: provider.to_string(),
            name: name.to_string(),
            version: Some(rest.to_string()),
            description,
        });
    }
    candidates
}

pub fn parse_installed(output: &str) -> Vec<InstalledPackage> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            if line.is_empty() {
                return None;
            }
            let (name, version) = line
                .split_once(' ')
                .map(|(n, v)| (n, Some(v.to_string())))
                .unwrap_or((line, None));
            Some(InstalledPackage {
                source: Source::Alpm,
                backend_ref: format!("local/{name}"),
                name: name.to_string(),
                version,
                scope: None,
                installed_at: None,
            })
        })
        .collect()
}