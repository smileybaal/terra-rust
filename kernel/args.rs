//! Launch-argument parsing, hand-written (D4; listed in `sheets/kernel.tsv`).
//!
//! Mirrors `Terraria.Utils.ParseArguements`. A token beginning with `-` or `+`
//! starts a new entry and is itself the KEY, lowercased and with its prefix still
//! on it (`-savedirectory`, not `savedirectory`). Every following token that does
//! not begin with a prefix is appended to that entry's value, space-separated.
//!
//! ONE DELIBERATE DIFFERENCE. C# calls `Dictionary.Add`, which THROWS on a repeated
//! key, so `-port 1 -port 2` crashes the server before it can start. Reproducing a
//! crash would be reproducing a bug, so the first value wins and the repeat is
//! recorded in `duplicates` instead of being swallowed (dec020). Everything else,
//! including the trailing key with an empty value, behaves as the original does.

use std::collections::BTreeMap;

/// The parsed launch parameters, keyed exactly as C# keys them: lowercased,
/// prefix included.
#[derive(Debug, Default, Clone)]
pub struct LaunchParameters {
    map: BTreeMap<String, String>,
    duplicates: Vec<String>,
}

impl LaunchParameters {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|s| s.as_str())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// Flags whose value came from a repeat of an already-seen flag. Empty on any
    /// command line the original would have accepted.
    pub fn duplicates(&self) -> &[String] {
        &self.duplicates
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }
}

/// `Utils.ParseArguements`, with the duplicate behaviour noted above.
pub fn parse(args: &[String]) -> LaunchParameters {
    let mut out = LaunchParameters::default();
    let mut key: Option<String> = None;
    let mut value = String::new();

    let flush = |out: &mut LaunchParameters, key: &Option<String>, value: &mut String| {
        if let Some(k) = key {
            // the original's Add() would have thrown here; see the module note
            if out.map.contains_key(k) {
                out.duplicates.push(k.clone());
            } else {
                out.map.insert(k.clone(), value.clone());
            }
            value.clear();
        }
    };

    for a in args {
        if a.is_empty() {
            continue;
        }
        let first = a.chars().next().unwrap_or(' ');
        if first == '-' || first == '+' {
            flush(&mut out, &key, &mut value);
            key = Some(a.to_lowercase());
        } else if key.is_some() {
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(a);
        }
        // a non-flag token BEFORE any flag is ignored, as in the original
    }
    flush(&mut out, &key, &mut value);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: &[&str]) -> Vec<String> {
        x.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_flag_keeps_its_prefix_and_is_lowercased() {
        let p = parse(&v(&["-SaveDirectory", "C:\\save"]));
        assert_eq!(p.get("-savedirectory"), Some("C:\\save"));
    }

    #[test]
    fn a_bare_flag_has_an_empty_value() {
        let p = parse(&v(&["-logerrors"]));
        assert!(p.contains("-logerrors"));
        assert_eq!(p.get("-logerrors"), Some(""));
    }

    #[test]
    fn several_value_tokens_join_with_spaces() {
        let p = parse(&v(&["-world", "My", "World.wld"]));
        assert_eq!(p.get("-world"), Some("My World.wld"));
    }

    #[test]
    fn the_last_flag_is_flushed_even_with_no_value() {
        let p = parse(&v(&["-a", "1", "-b"]));
        assert_eq!(p.get("-a"), Some("1"));
        assert_eq!(p.get("-b"), Some(""));
    }

    #[test]
    fn tokens_before_any_flag_are_ignored() {
        let p = parse(&v(&["stray", "-a", "1"]));
        assert_eq!(p.len(), 1);
        assert_eq!(p.get("-a"), Some("1"));
    }

    #[test]
    fn a_repeated_flag_is_recorded_rather_than_throwing() {
        // the C# would throw ArgumentException here
        let p = parse(&v(&["-port", "1", "-port", "2"]));
        assert_eq!(p.get("-port"), Some("1"));
        assert_eq!(p.duplicates(), ["-port".to_string()]);
    }

    #[test]
    fn plus_is_a_prefix_too() {
        let p = parse(&v(&["+flag", "x"]));
        assert_eq!(p.get("+flag"), Some("x"));
    }
}
