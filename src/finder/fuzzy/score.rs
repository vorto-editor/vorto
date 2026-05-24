// Smith-Waterman-style scoring constants, matching nucleo/fzf v2 so
// the picker ranks results the way Helix users expect. Word-boundary
// hits dominate; long gaps are punished but not lethal.
const SCORE_MATCH: i32 = 16;
const SCORE_GAP_START: i32 = -3;
const SCORE_GAP_EXTEND: i32 = -1;
const BONUS_BOUNDARY: i32 = SCORE_MATCH / 2; // 8
const BONUS_CAMEL: i32 = BONUS_BOUNDARY - 1; // 7
const BONUS_CONSECUTIVE: i32 = -(SCORE_GAP_START + SCORE_GAP_EXTEND); // 4
const BONUS_FIRST_CHAR_MULT: i32 = 2;
const SCORE_NEG_INF: i32 = i32::MIN / 4;

#[derive(Copy, Clone, PartialEq)]
enum CharKind {
    NonWord,
    Lower,
    Upper,
    Number,
}

fn char_kind(c: char) -> CharKind {
    if c.is_ascii_lowercase() {
        CharKind::Lower
    } else if c.is_ascii_uppercase() {
        CharKind::Upper
    } else if c.is_ascii_digit() {
        CharKind::Number
    } else if c.is_alphanumeric() {
        // Treat non-ASCII letters (e.g. CJK) as word characters so a
        // transition from a separator into them still earns the
        // boundary bonus.
        CharKind::Lower
    } else {
        CharKind::NonWord
    }
}

fn boundary_bonus(prev: CharKind, curr: CharKind) -> i32 {
    use CharKind::*;
    match (prev, curr) {
        (NonWord, c) if c != NonWord => BONUS_BOUNDARY,
        (Lower, Upper) => BONUS_CAMEL,
        (Lower | Upper, Number) => BONUS_CAMEL,
        _ => 0,
    }
}

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

/// Sub-sequence fuzzy match, modelled on Helix's nucleo / fzf v2.
/// Each needle character must appear in the haystack in order; gaps
/// between matches are permitted but penalised. Returns `(score, char
/// positions)` where positions are the indices of the matched needle
/// characters in the haystack (used for highlighting).
///
/// Scoring rewards: word-boundary anchored matches (after `/`, `_`,
/// `-`, `.`, ` ` or at start), camelCase boundaries
/// (`fooBar`-style), consecutive matches, and exact-case characters.
/// Scoring penalises every skipped haystack character (affine gap:
/// the first skip costs `SCORE_GAP_START`, each subsequent skip costs
/// `SCORE_GAP_EXTEND`).
pub fn fuzzy_match(haystack: &str, needle: &str) -> Option<(i32, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let hay: Vec<char> = haystack.chars().collect();
    let ndl: Vec<char> = needle.chars().collect();
    let n = ndl.len();
    let m = hay.len();
    if n > m {
        return None;
    }

    // Per-position transition bonus: `bonus[j]` is what you earn for
    // matching at `hay[j]` given the kind of `hay[j-1]`. Position 0
    // is treated as if preceded by a NonWord character, so a match
    // there is always a boundary hit.
    let mut bonus = vec![0i32; m];
    let mut prev_kind = CharKind::NonWord;
    for (j, &c) in hay.iter().enumerate() {
        let k = char_kind(c);
        bonus[j] = boundary_bonus(prev_kind, k);
        prev_kind = k;
    }

    // Two DP tables. `mscore[i][j]` is the best score for matching
    // needle[..=i] with `hay[j]` taken as the final match. `gscore[i][j]`
    // is the best score for matching needle[..=i] when `hay[j]` is *not*
    // a match — i.e. we're currently extending a gap that follows the
    // last needle char. Parent tables record where the predecessor
    // needle character landed, so we can rebuild the position list.
    let cell = |i: usize, j: usize| i * m + j;
    let mut mscore = vec![SCORE_NEG_INF; n * m];
    let mut gscore = vec![SCORE_NEG_INF; n * m];
    let mut mparent = vec![usize::MAX; n * m];
    let mut gmatch = vec![usize::MAX; n * m]; // gscore traceback: where needle[i] matched

    for i in 0..n {
        for j in i..m {
            let nc = ndl[i];
            let hc = hay[j];
            let is_match = nc.eq_ignore_ascii_case(&hc);

            if is_match {
                let case_bonus = if nc == hc { 1 } else { 0 };
                let ms = if i == 0 {
                    SCORE_MATCH + bonus[j] * BONUS_FIRST_CHAR_MULT + case_bonus
                } else if j == 0 {
                    SCORE_NEG_INF
                } else {
                    let from_m = mscore[cell(i - 1, j - 1)];
                    let from_g = gscore[cell(i - 1, j - 1)];
                    let consec_bonus = BONUS_CONSECUTIVE.max(bonus[j]);
                    let via_m = from_m.saturating_add(SCORE_MATCH + consec_bonus + case_bonus);
                    let via_g = from_g.saturating_add(SCORE_MATCH + bonus[j] + case_bonus);
                    if from_m == SCORE_NEG_INF && from_g == SCORE_NEG_INF {
                        SCORE_NEG_INF
                    } else if via_m >= via_g {
                        mparent[cell(i, j)] = j - 1;
                        via_m
                    } else {
                        // Predecessor's match position lives in the gap-trace table.
                        mparent[cell(i, j)] = gmatch[cell(i - 1, j - 1)];
                        via_g
                    }
                };
                mscore[cell(i, j)] = ms;
            }

            if j > 0 {
                let from_m = mscore[cell(i, j - 1)];
                let from_g = gscore[cell(i, j - 1)];
                let start = from_m.saturating_add(SCORE_GAP_START);
                let extend = from_g.saturating_add(SCORE_GAP_EXTEND);
                if from_m == SCORE_NEG_INF && from_g == SCORE_NEG_INF {
                    // no predecessor yet
                } else if extend >= start {
                    gscore[cell(i, j)] = extend;
                    gmatch[cell(i, j)] = gmatch[cell(i, j - 1)];
                } else {
                    gscore[cell(i, j)] = start;
                    gmatch[cell(i, j)] = j - 1;
                }
            }
        }
    }

    // The match must end on an actual needle character — trailing
    // characters are unmatched and don't contribute. Pick the column
    // where the last needle row scores highest.
    let mut best_score = SCORE_NEG_INF;
    let mut best_j = usize::MAX;
    for j in (n - 1)..m {
        let s = mscore[cell(n - 1, j)];
        if s > best_score {
            best_score = s;
            best_j = j;
        }
    }
    if best_j == usize::MAX || best_score == SCORE_NEG_INF {
        return None;
    }

    // Walk parent pointers back from (n-1, best_j) collecting match
    // positions for highlighting.
    let mut positions = Vec::with_capacity(n);
    let mut i = n - 1;
    let mut j = best_j;
    positions.push(j);
    while i > 0 {
        let pj = mparent[cell(i, j)];
        if pj == usize::MAX {
            return None;
        }
        j = pj;
        i -= 1;
        positions.push(j);
    }
    positions.reverse();
    Some((best_score, positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(haystack: &str, needle: &str) -> Option<String> {
        let (_score, positions) = fuzzy_match(haystack, needle)?;
        let hay: Vec<char> = haystack.chars().collect();
        Some(positions.iter().map(|&i| hay[i]).collect())
    }

    #[test]
    fn skips_dot_separator() {
        let (_score, positions) = fuzzy_match("xx.go", "xxgo").expect("should match");
        assert_eq!(positions, vec![0, 1, 3, 4]);
    }

    #[test]
    fn skips_underscore_and_slash() {
        assert_eq!(matched("foo_bar", "foobar").as_deref(), Some("foobar"));
        assert_eq!(matched("src/foo.rs", "foors").as_deref(), Some("foors"));
        assert_eq!(matched("foo bar", "foobar").as_deref(), Some("foobar"));
    }

    #[test]
    fn case_insensitive() {
        let (_s, pos) = fuzzy_match("README.md", "readme").unwrap();
        assert_eq!(pos, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn camel_case_boundary_preferred() {
        // Both haystacks contain "foobar" as a subsequence, but
        // `fooBar` has a camelCase boundary at 'B' which should rank
        // higher than the flat `foobar`.
        let (camel, _) = fuzzy_match("fooBar", "foob").unwrap();
        let (flat, _) = fuzzy_match("flooba", "foob").unwrap();
        assert!(camel > flat, "camelCase {camel} should outrank flat {flat}");
    }

    #[test]
    fn word_boundary_outranks_mid_word() {
        // `src/foo` should rank higher than `srcafoo` for needle "foo"
        // because the match starts on a path-segment boundary.
        let (boundary, _) = fuzzy_match("src/foo", "foo").unwrap();
        let (mid, _) = fuzzy_match("srcafoo", "foo").unwrap();
        assert!(
            boundary > mid,
            "boundary {boundary} should outrank mid-word {mid}"
        );
    }

    #[test]
    fn subsequence_with_long_gap() {
        // Needle chars need only appear in order anywhere in the
        // haystack — pure fzf-style subsequence matching.
        let (_score, positions) = fuzzy_match("alphabet_soup", "asp").unwrap();
        assert_eq!(positions.len(), 3);
        // Match must be in order and use distinct positions.
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn rejects_when_letters_missing() {
        assert!(fuzzy_match("xx.go", "xxrs").is_none());
        assert!(fuzzy_match("abc", "abcd").is_none());
        // Needle character not present anywhere.
        assert!(fuzzy_match("src/foo", "srcz").is_none());
    }
}
