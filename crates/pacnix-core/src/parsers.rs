// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

pub fn parse_pairs(output: &str) -> Vec<(String, Option<String>)> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            Some(match line.split_once(' ') {
                Some((name, version)) => (name.to_string(), Some(version.to_string())),
                None => (line.to_string(), None),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_version_pairs() {
        let pairs = parse_pairs("firefox 122.0-1\nfoo\n  bar 1.0\n");
        assert_eq!(pairs.len(), 3);
        assert_eq!(
            pairs[0],
            ("firefox".to_string(), Some("122.0-1".to_string()))
        );
        assert_eq!(pairs[1], ("foo".to_string(), None));
        assert_eq!(pairs[2], ("bar".to_string(), Some("1.0".to_string())));
    }
}
