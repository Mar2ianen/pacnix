// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

/// Compares valid ALPM package version strings according to the
/// alpm-pkgver comparison rules. Returns -1, 0 or 1 when `a` is older
/// than, equal to, or newer than `b`.
///
/// Pure Rust port of alpm's version comparison (epoch/version/release
/// splitting plus the segment algorithm from the alpm-pkgver specification),
/// so pacnix never needs the pacman-contrib `vercmp` binary. Non-ASCII
/// input is out of contract: valid `pkgver` values are ASCII only.
pub fn vercmp(a: &str, b: &str) -> i8 {
    if a == b {
        return 0;
    }
    let (epoch_a, ver_a, rel_a) = parse_evr(a);
    let (epoch_b, ver_b, rel_b) = parse_evr(b);
    let epoch_order = cmp_version(epoch_a, epoch_b);
    if epoch_order != 0 {
        return epoch_order;
    }
    let ver_order = cmp_version(ver_a, ver_b);
    if ver_order != 0 {
        return ver_order;
    }
    match (rel_a, rel_b) {
        (Some(x), Some(y)) => cmp_version(x, y),
        _ => 0,
    }
}

/// Splits `[epoch:]version[-release]` per alpm rules: epoch is the prefix up
/// to the first colon (default "0"), release is the suffix after the last
/// dash, and a release is only present when it is non-empty.
fn parse_evr(version: &str) -> (&str, &str, Option<&str>) {
    let (epoch, rest) = match version.split_once(':') {
        Some((e, r)) if !e.is_empty() => (e, r),
        _ => ("0", version),
    };
    match rest.rfind('-') {
        Some(pos) => (epoch, &rest[..pos], Some(&rest[pos + 1..])),
        None => (epoch, rest, None),
    }
}

/// A numeric or alphabetic run inside a version segment, e.g. "0" and "alpha"
/// for "0alpha". Numeric runs compare as integers (leading zeros ignored),
/// alphabetic runs case-insensitively, and numeric always beats alphabetic.
struct Sub {
    numeric: bool,
    text: String,
}

/// A version segment. `delims` counts the non-alphanumeric characters that
/// precede the segment; consecutive delimiters are not collapsed and, for two
/// non-empty segments, a larger count wins. A trailing delimiter produces a
/// final empty segment.
struct Seg {
    subs: Vec<Sub>,
    delims: usize,
}

/// Splits a version string into segments per the alpm-pkgver specification.
fn split_segments(s: &str) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    let mut delims = 0usize;
    let mut subs: Vec<Sub> = Vec::new();
    let mut cur = String::new();
    let mut cur_numeric = false;
    let mut cur_open = false;
    let mut ended_with_separator = false;
    for ch in s.chars() {
        if !ch.is_alphanumeric() {
            if cur_open {
                subs.push(Sub {
                    numeric: cur_numeric,
                    text: std::mem::take(&mut cur),
                });
                cur_open = false;
            }
            if !subs.is_empty() {
                segs.push(Seg {
                    subs: std::mem::take(&mut subs),
                    delims,
                });
            }
            delims += 1;
            ended_with_separator = true;
        } else {
            let numeric = ch.is_ascii_digit();
            if cur_open && numeric != cur_numeric {
                subs.push(Sub {
                    numeric: cur_numeric,
                    text: std::mem::take(&mut cur),
                });
            }
            cur.push(ch);
            cur_numeric = numeric;
            cur_open = true;
            ended_with_separator = false;
        }
    }
    if cur_open {
        subs.push(Sub {
            numeric: cur_numeric,
            text: cur,
        });
        segs.push(Seg { subs, delims });
    } else if ended_with_separator {
        segs.push(Seg {
            subs: vec![],
            delims,
        });
    }
    segs
}

fn trim_zeros(s: &str) -> &str {
    let trimmed = s.trim_start_matches('0');
    if trimmed.is_empty() {
        "0"
    } else {
        trimmed
    }
}

/// Compares two sub-segments; 0 when equal.
fn cmp_sub(a: &Sub, b: &Sub) -> i8 {
    match (a.numeric, b.numeric) {
        (true, true) => {
            let (x, y) = (trim_zeros(&a.text), trim_zeros(&b.text));
            match x.len().cmp(&y.len()) {
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => {
                    for (ca, cb) in x.bytes().zip(y.bytes()) {
                        match ca.cmp(&cb) {
                            std::cmp::Ordering::Greater => return 1,
                            std::cmp::Ordering::Less => return -1,
                            std::cmp::Ordering::Equal => {}
                        }
                    }
                    0
                }
            }
        }
        (true, false) => 1,
        (false, true) => -1,
        (false, false) => a.text.as_bytes().cmp(b.text.as_bytes()) as i8,
    }
}

/// Compares two version strings with the segment algorithm of the
/// alpm-pkgver specification, including its special cases (delimiter counts,
/// trailing delimiters, sub-segment vs new segment, trailing alpha runs).
fn cmp_version(a: &str, b: &str) -> i8 {
    if a == b {
        return 0;
    }
    let a = split_segments(a);
    let b = split_segments(b);
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        let sa = &a[i];
        let sb = &b[j];
        if sa.subs.is_empty() && sb.subs.is_empty() {
            i += 1;
            j += 1;
            continue;
        }
        if !sa.subs.is_empty() && !sb.subs.is_empty() && sa.delims != sb.delims {
            return if sa.delims > sb.delims { 1 } else { -1 };
        }
        if sa.subs.is_empty() {
            return if sb.subs[0].numeric { -1 } else { 1 };
        }
        if sb.subs.is_empty() {
            return if sa.subs[0].numeric { 1 } else { -1 };
        }
        let (mut k, mut l) = (0usize, 0usize);
        while k < sa.subs.len() && l < sb.subs.len() {
            let r = cmp_sub(&sa.subs[k], &sb.subs[l]);
            if r != 0 {
                return r;
            }
            k += 1;
            l += 1;
        }
        if k == sa.subs.len() && l == sb.subs.len() {
            i += 1;
            j += 1;
            continue;
        }
        if k == sa.subs.len() {
            if i + 1 < a.len() && !a[i + 1].subs.is_empty() {
                return 1;
            }
            if sb.subs[l].numeric {
                return -1;
            }
            return if sa.subs[k - 1].numeric { 1 } else { -1 };
        }
        if j + 1 < b.len() && !b[j + 1].subs.is_empty() {
            return -1;
        }
        if sa.subs[k].numeric {
            return 1;
        }
        return if sb.subs[l - 1].numeric { -1 } else { 1 };
    }
    if i == a.len() {
        if j == b.len() {
            return 0;
        }
        return -1;
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(pairs: &[(&str, &str, i8)]) {
        for (a, b, expected) in pairs {
            assert_eq!(vercmp(a, b), *expected, "{a} vs {b}");
            assert_eq!(vercmp(b, a), -*expected, "{b} vs {a}");
        }
    }

    #[test]
    fn matches_pacman_suite() {
        check(&[
            ("1.0.1", "1.0.1", 0),
            ("1.0.1", "1.0.2", -1),
            ("1.0.1", "1.0.1", 0),
            ("1.5", "1.5", 0),
            ("1.5", "1.6", -1),
            ("1.5", "1.5.1", -1),
            ("1.5.1", "1.5", 1),
            ("1.5.0", "1.5", 1),
            ("1.5.0", "1.5.0.0", -1),
            ("1.0", "1.0-1", 0),
            ("1.0-1", "1.0", 0),
            ("1.0-1", "1.0-2", -1),
            ("1.0-2", "1.0-1.1", 1),
            ("1.0-1.1", "1.0-2", -1),
            ("1.0.15-1", "1-1", 1),
            ("1.0.15", "1", 1),
            ("1.0", "1", 1),
            ("1.0", "1-1", 1),
            ("1.0", "1.0a", 1),
            ("1.0a", "1.0", -1),
            ("1.0a", "1.0b", -1),
            ("1.0b", "1.0a1", 1),
            ("1.0a1", "1.0b", -1),
            ("1.0a1", "1.0b1", -1),
            ("1.0b1", "1.0a1", 1),
            ("1.0a1", "1.0a2", -1),
            ("1.0a2", "1.0a1b", 1),
            ("1.0a1b", "1.0a2", -1),
            ("1.0a1b", "1.0a1b1", -1),
        ]);
    }

    #[test]
    fn handles_epochs() {
        check(&[
            ("1:1.0", "1:1.0", 0),
            ("1:1.0", "1:2.0", -1),
            ("1:2.0", "1:1.0", 1),
            ("0:1.0", "1.0", 0),
            ("0:1.0", "1:2.0", -1),
            ("1:2.0", "0:1.0", 1),
            ("1.0", "1:1.0", -1),
            ("1:1.0", "1.0", 1),
            ("1:2.0", "1.0", 1),
            ("1:1.0", "0:1.0", 1),
            ("0:1.0", "1:1.0", -1),
        ]);
    }

    #[test]
    fn release_only_compared_when_present_on_both_sides() {
        check(&[
            ("1.5-1", "1.5", 0),
            ("1.5", "1.5-1", 0),
            ("1.5-1", "1.5-2", -1),
            ("1.5-2", "1.5-1", 1),
            ("1.5-1", "1.6", -1),
            ("1.6", "1.5-99", 1),
            ("1.0-", "1.0", 0),
            ("1.0-", "1.0-1", -1),
            ("1.0-1-2", "1.0-1", 1),
        ]);
    }

    #[test]
    fn alpha_ordering_is_bytewise_and_leading_zeros_ignored() {
        check(&[
            ("1.0Alpha", "1.0alpha", -1),
            ("1.0beta", "1.0Alpha", 1),
            ("1.05", "1.5", 0),
            ("1.05", "1.5.1", -1),
            ("1.0.0", "1.0", 1),
            ("1.0", "1.0.0", -1),
            ("1a", "1", -1),
            ("1", "1a", 1),
            ("1-2", "1a", 1),
            ("1a", "1-2", -1),
            ("a1", "a", 1),
            ("a", "a1", -1),
            ("A", "a", -1),
            ("a", "A", 1),
            ("RC1", "rc1", -1),
        ]);
    }

    #[test]
    fn handles_git_style_and_release_candidates() {
        check(&[
            ("r1041.gb1c3d2f", "r1041.gb1c3d2f", 0),
            ("r1041.gb1c3d2f", "r1042.gb1c3d2f", -1),
            ("r1041.gb1c3d2f", "0.4.0-1", -1),
            ("0.4.0-1", "r1041.gb1c3d2f", 1),
            ("0.4.0-1", "0.5.0-1", -1),
            ("1.0rc1", "1.0", -1),
            ("1.0", "1.0rc1", 1),
            ("1.0", "1.0.0", -1),
            ("3.14.0+git", "3.14.0", 1),
            ("3.14.0", "3.14.0+git", -1),
        ]);
    }

    #[test]
    fn follows_specification_special_cases() {
        check(&[
            ("1...0", "1.2", 1),
            ("1...", "1.", 0),
            ("1.", "1.foo.2", 1),
            ("1.", "1.2", -1),
            ("1", "1.", -1),
            ("1.", "1", 1),
            ("alpha1", "alpha.0", -1),
            ("alpha2", "alpha.0", -1),
            ("alpha1", "alpha.", 1),
            ("1.foo.1", "1.foo2", 1),
            ("1.foo.", "1.foo2", -1),
            ("1.foo", "1.foo2", -1),
            ("1.0", "1.0foo.2", 1),
            ("1.0", "1.0alpha", 1),
            ("1.0alpha", "1.0", -1),
        ]);
    }

    #[test]
    fn differential_against_system_vercmp() {
        if std::env::var("PACNIX_DIFF_VERCMP").is_err() {
            return;
        }
        let mut versions: Vec<String> = vec![
            "0".into(),
            "1".into(),
            "1.0".into(),
            "1.0-1".into(),
            "1:1.0".into(),
            "1.0alpha".into(),
            "2.0rc1".into(),
            "0.1.0".into(),
            "3.14.0+git".into(),
            "r1041.gb1c3d2f".into(),
            "0.4.0-1".into(),
            "1.0.".into(),
            "1.0-1-2".into(),
            "1a".into(),
            "a1".into(),
            "1.0.15".into(),
            "2026.01".into(),
            "1.0rc1".into(),
            "0.0.1".into(),
            "1.".into(),
            "1..0".into(),
            "alpha1".into(),
            "alpha.0".into(),
            "alpha.".into(),
            "1.foo".into(),
            "1.foo2".into(),
            "1.foo.1".into(),
            "1.foo.".into(),
            "1-2".into(),
            "1.0ab".into(),
            "1.0Alpha".into(),
            "1.0alpha".into(),
            "A".into(),
            "a".into(),
            "RC1".into(),
            "rc1".into(),
        ];
        let local = std::path::Path::new("/var/lib/pacman/local");
        if let Ok(entries) = std::fs::read_dir(local) {
            for entry in entries.flatten() {
                let Ok(text) = std::fs::read_to_string(entry.path().join("desc")) else {
                    continue;
                };
                let mut lines = text.lines();
                while let Some(line) = lines.next() {
                    if line.trim() == "%VERSION%" {
                        if let Some(version) = lines.next() {
                            let version = version.trim();
                            if !version.is_empty() && !versions.iter().any(|v| v == version) {
                                versions.push(version.to_string());
                            }
                        }
                    }
                }
            }
        }
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let next = |state: &mut u64| {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *state >> 33
        };
        let check = |a: &str, b: &str| {
            let output = std::process::Command::new("vercmp")
                .args([a, b])
                .output()
                .expect("vercmp binary required for differential check");
            assert!(output.status.success(), "vercmp failed for {a} vs {b}");
            let expected: i64 = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .expect("vercmp must print -1/0/1");
            let got = vercmp(a, b) as i64;
            assert_eq!(got, expected, "{a} vs {b}");
        };
        for a in &versions {
            for b in &versions {
                check(a, b);
            }
        }
        for _ in 0..10_000 {
            let a = &versions[(next(&mut state) % versions.len() as u64) as usize];
            let b = &versions[(next(&mut state) % versions.len() as u64) as usize];
            check(a, b);
        }
    }
}
