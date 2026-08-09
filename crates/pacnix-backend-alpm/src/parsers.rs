// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Provenance, Source};
use pacnix_core::parsers::parse_pairs;

pub fn desc_field(content: &str, field: &str) -> Option<String> {
    let marker = format!("{field}\n");
    let pos = content.find(&marker)?;
    let value = content[pos + marker.len()..].lines().next()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn parse_upgrade_candidates(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (repo, name) = line.split_once('/')?;
            if name.contains('/') || name.contains(' ') {
                return None;
            }
            Some((repo.to_string(), name.to_string()))
        })
        .collect()
}

pub fn parse_installed_size(output: &str) -> Option<u64> {
    let value = output.lines().find_map(|line| {
        let (key, rest) = line.split_once(':')?;
        if key.trim_end() != "Installed Size" {
            return None;
        }
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        Some(rest)
    })?;
    let (amount, unit) = value.split_once(' ')?;
    let amount = amount.parse::<f64>().ok()?;
    let bytes = match unit {
        "B" => amount,
        "KiB" => amount * 1024.0,
        "MiB" => amount * 1024.0 * 1024.0,
        "GiB" => amount * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(bytes.round() as u64)
}

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
            backend_ref: prefix.to_string(),
            name: name.to_string(),
            version: Some(rest.to_string()),
            description,
            package_base: None,
            url_path: None,
        });
    }
    candidates
}

pub fn parse_installed(output: &str, provenance: Provenance) -> Vec<InstalledPackage> {
    parse_pairs(output)
        .into_iter()
        .map(|(name, version)| InstalledPackage {
            source: Source::Alpm,
            backend_ref: format!("local/{name}"),
            name,
            version,
            scope: None,
            installed_at: None,
            provenance: provenance.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desc_field_extracts_name_and_install_date() {
        let desc = "%NAME%\nfoo\n\n%VERSION%\n1.0-1\n\n%INSTALLDATE%\n1700000000\n";
        assert_eq!(desc_field(desc, "%NAME%"), Some("foo".into()));
        assert_eq!(desc_field(desc, "%INSTALLDATE%"), Some("1700000000".into()));
        assert_eq!(desc_field(desc, "%MISSING%"), None);
    }

    #[test]
    fn parses_search_output() {
        let out = "extra/firefox 122.0-1\n    Standalone web browser\nchaotic-aur/foo-bin 1.2-3\n    Foo binary\n";
        let candidates = parse_search(out);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].provider, "extra");
        assert_eq!(candidates[0].name, "firefox");
        assert_eq!(candidates[0].version.as_deref(), Some("122.0-1"));
        assert_eq!(
            candidates[0].description.as_deref(),
            Some("Standalone web browser")
        );
        assert_eq!(candidates[1].provider, "chaotic-aur");
    }

    #[test]
    fn parses_upgrade_print_list() {
        let out = "extra/blender\nextra/ffmpeg\n";
        let candidates = parse_upgrade_candidates(out);
        assert_eq!(
            candidates,
            vec![
                ("extra".to_string(), "blender".to_string()),
                ("extra".to_string(), "ffmpeg".to_string()),
            ]
        );
        assert!(parse_upgrade_candidates("").is_empty());
        assert!(
            parse_upgrade_candidates("https://mirror/extra/blender.pkg.tar.zst").is_empty(),
            "url-style default %l output must not produce repo/name pairs"
        );
    }

    #[test]
    fn parses_size_from_si_and_qi_output() {
        let si = "Repository      : extra\nName            : blender\nVersion         : 17:5.2.0-4\nInstalled Size  : 381.68 MiB\n";
        let qi = "Name            : a52dec\nVersion         : 0.8.0-3\nInstalled Size  : 132.34 KiB\nBuild Date      : ...\n";
        assert_eq!(parse_installed_size(si), Some(400_220_488));
        assert_eq!(parse_installed_size(qi), Some(135516));
        assert_eq!(parse_installed_size("Name : foo\n"), None);
    }
}
