use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Case-insensitive substring search for the ASCII fast path. Returns
/// the byte (= char, since haystack is ASCII) offset where the needle
/// first occurs. `needle_lower` must already be lower-cased; the
/// haystack is lower-cased inline byte-by-byte (no allocation).
pub(super) fn ascii_find_lower(hay: &str, needle_lower: &str) -> Option<usize> {
    let hay = hay.as_bytes();
    let ndl = needle_lower.as_bytes();
    if ndl.is_empty() {
        return Some(0);
    }
    if hay.len() < ndl.len() {
        return None;
    }
    'outer: for start in 0..=hay.len() - ndl.len() {
        for (k, &n) in ndl.iter().enumerate() {
            if hay[start + k].to_ascii_lowercase() != n {
                continue 'outer;
            }
        }
        return Some(start);
    }
    None
}

/// Helix's picker matcher: nucleo with its default query syntax
/// (whitespace-separated atoms that must all match, `^prefix`,
/// `suffix$`, `'substring`, `!negation`), smart case, and smart
/// Unicode normalization.
///
/// Built once per query and reused across every candidate so the
/// matcher's scratch allocations aren't paid per item.
pub struct FuzzyMatcher {
    matcher: Matcher,
    pattern: Pattern,
    buf: Vec<char>,
    indices: Vec<u32>,
}

impl FuzzyMatcher {
    /// `paths` selects nucleo's path config (`/` counts as a word
    /// boundary, matches after it earn the full boundary bonus) — Helix
    /// switches it on for every picker that previews a file.
    pub fn new(query: &str, paths: bool) -> Self {
        let config = if paths {
            Config::DEFAULT.match_paths()
        } else {
            Config::DEFAULT
        };
        Self {
            matcher: Matcher::new(config),
            pattern: Pattern::parse(query, CaseMatching::Smart, Normalization::Smart),
            buf: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// True when the query has no atoms (empty or whitespace only) —
    /// every item matches and Helix keeps the original order.
    pub fn is_empty(&self) -> bool {
        self.pattern.atoms.is_empty()
    }

    /// Score `haystack`, or `None` if it doesn't match.
    pub fn score(&mut self, haystack: &str) -> Option<u32> {
        let hay = Utf32Str::new(haystack, &mut self.buf);
        self.pattern.score(hay, &mut self.matcher)
    }

    /// Score `haystack` and return the matched char indices, sorted
    /// and deduplicated (atoms report their hits independently).
    pub fn indices(&mut self, haystack: &str) -> Option<(u32, Vec<usize>)> {
        self.indices.clear();
        let hay = Utf32Str::new(haystack, &mut self.buf);
        let score = self
            .pattern
            .indices(hay, &mut self.matcher, &mut self.indices)?;
        self.indices.sort_unstable();
        self.indices.dedup();
        Some((score, self.indices.iter().map(|&i| i as usize).collect()))
    }
}

/// Sort key reproducing nucleo's result order: score descending, then
/// shorter haystack (in chars) first, then original index.
pub fn rank_key(score: u32, haystack: &str, idx: usize) -> (Reverse<u32>, usize, usize) {
    (Reverse(score), haystack.chars().count(), idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(haystack: &str, needle: &str) -> Option<String> {
        let (_score, positions) = FuzzyMatcher::new(needle, true).indices(haystack)?;
        let hay: Vec<char> = haystack.chars().collect();
        Some(positions.iter().map(|&i| hay[i]).collect())
    }

    fn score(haystack: &str, needle: &str) -> Option<u32> {
        FuzzyMatcher::new(needle, true).score(haystack)
    }

    #[test]
    fn skips_separators() {
        assert_eq!(matched("xx.go", "xxgo").as_deref(), Some("xxgo"));
        assert_eq!(matched("foo_bar", "foobar").as_deref(), Some("foobar"));
        assert_eq!(matched("src/foo.rs", "foors").as_deref(), Some("foors"));
    }

    #[test]
    fn smart_case() {
        assert!(score("README.md", "readme").is_some());
        assert!(score("readme.md", "README").is_none());
        assert!(score("README.md", "README").is_some());
    }

    #[test]
    fn whitespace_atoms_all_must_match() {
        assert!(score("src/finder/fuzzy/walk.rs", "fuzzy walk").is_some());
        assert!(score("src/finder/fuzzy/walk.rs", "walk fuzzy").is_some());
        assert!(score("src/finder/fuzzy/walk.rs", "fuzzy zzz").is_none());
        assert!(FuzzyMatcher::new("   ", true).is_empty());
    }

    #[test]
    fn special_atoms() {
        assert!(score("src/main.rs", "^src").is_some());
        assert!(score("lib/src.rs", "^src").is_none());
        assert!(score("src/main.rs", ".rs$").is_some());
        assert!(score("src/main.rs.bak", ".rs$").is_none());
        assert!(score("src/main.rs", "main !test").is_some());
        assert!(score("tests/main.rs", "main !test").is_none());
        assert!(score("smain", "'s/m").is_none());
    }

    #[test]
    fn indices_are_sorted_and_unique() {
        let (_s, pos) = FuzzyMatcher::new("ma rs", true)
            .indices("src/main.rs")
            .unwrap();
        assert!(pos.windows(2).all(|w| w[0] < w[1]), "{pos:?}");
    }

    #[test]
    fn word_boundary_outranks_mid_word() {
        let boundary = score("src/foo", "foo").unwrap();
        let mid = score("srcafoo", "foo").unwrap();
        assert!(
            boundary > mid,
            "boundary {boundary} should outrank mid-word {mid}"
        );
    }

    #[test]
    fn rank_breaks_ties_by_length_then_index() {
        let mut items = [
            (10, "abcd", 0),
            (10, "ab", 2),
            (12, "abcdef", 3),
            (10, "ab", 1),
        ];
        items.sort_by_key(|&(s, h, i)| rank_key(s, h, i));
        let order: Vec<usize> = items.iter().map(|t| t.2).collect();
        assert_eq!(order, vec![3, 1, 2, 0]);
    }

    #[test]
    fn rejects_when_letters_missing() {
        assert!(score("xx.go", "xxrs").is_none());
        assert!(score("abc", "abcd").is_none());
    }
}
