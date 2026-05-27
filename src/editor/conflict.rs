//! Git conflict-marker parsing and resolution.
//!
//! Recognizes the standard merge-conflict shape — produced by `git
//! merge`/`rebase`, by this editor's own `autoreload = "merge"` (see
//! [`super::merge`]), and by any other three-way merge tool:
//!
//! ```text
//! <<<<<<< ours
//! …our side…
//! ||||||| base          (diff3 style; optional)
//! …common ancestor…
//! =======
//! …their side…
//! >>>>>>> theirs
//! ```
//!
//! Two consumers read this: the buffer renderer highlights each marker
//! and side (via `ui::buffer`), and the `:conflict` command + `]c`/`[c`
//! keys navigate and resolve hunks. The parsing is pure (operates on
//! `&[String]`) so both paths share one definition and it stays unit-
//! testable without an `App`.
//!
//! Markers are matched only inside a well-formed `<<<<<<<` … `>>>>>>>`
//! run, so a stray `=======` (e.g. a Markdown setext underline) or a
//! lone `>>>>>>>` never lights up on its own.

use std::ops::Range;

/// Which side of a resolved conflict to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the top side (`<<<<<<<` … `=======`) — git's "ours", the
    /// merge feature's "local".
    Ours,
    /// Keep the bottom side (`=======` … `>>>>>>>`) — git's "theirs",
    /// the merge feature's "disk".
    Theirs,
    /// Keep both sides' content, dropping only the marker lines (and the
    /// diff3 base region, if present).
    Both,
    /// Drop the entire hunk.
    None,
}

/// One parsed conflict, by row index. All indices point at *whole lines*
/// of the source buffer; the marker rows (`start`, `base`, `sep`, `end`)
/// are the `<<<<<<<` / `|||||||` / `=======` / `>>>>>>>` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// Row of the `<<<<<<<` marker.
    pub start: usize,
    /// Row of the `|||||||` base marker (diff3 conflicts only).
    pub base: Option<usize>,
    /// Row of the `=======` separator.
    pub sep: usize,
    /// Row of the `>>>>>>>` marker.
    pub end: usize,
}

impl Hunk {
    /// Whether `row` falls anywhere within the hunk (markers included).
    pub fn contains(&self, row: usize) -> bool {
        row >= self.start && row <= self.end
    }

    /// Row range of the top ("ours") side's content — between `<<<<<<<`
    /// and the base marker (diff3) or the separator.
    pub fn ours(&self) -> Range<usize> {
        (self.start + 1)..self.base.unwrap_or(self.sep)
    }

    /// Row range of the bottom ("theirs") side's content — between
    /// `=======` and `>>>>>>>`.
    pub fn theirs(&self) -> Range<usize> {
        (self.sep + 1)..self.end
    }

    /// Row range of the diff3 base region, if this conflict carries one
    /// (between `|||||||` and `=======`).
    pub fn base_region(&self) -> Option<Range<usize>> {
        self.base.map(|b| (b + 1)..self.sep)
    }

    /// The lines this hunk collapses to under `res`, drawn from `lines`
    /// (the buffer the hunk was parsed from). The caller splices these in
    /// place of `start..=end`.
    pub fn replacement(&self, lines: &[String], res: Resolution) -> Vec<String> {
        match res {
            Resolution::Ours => lines[self.ours()].to_vec(),
            Resolution::Theirs => lines[self.theirs()].to_vec(),
            Resolution::Both => {
                let mut v = lines[self.ours()].to_vec();
                v.extend_from_slice(&lines[self.theirs()]);
                v
            }
            Resolution::None => Vec::new(),
        }
    }
}

fn is_start(l: &str) -> bool {
    l.starts_with("<<<<<<<")
}
fn is_base(l: &str) -> bool {
    l.starts_with("|||||||")
}
fn is_sep(l: &str) -> bool {
    l.starts_with("=======")
}
fn is_end(l: &str) -> bool {
    l.starts_with(">>>>>>>")
}

/// Every well-formed conflict in `lines`, in document order. A hunk is
/// only emitted once its full `<<<<<<<` → `=======` → `>>>>>>>` sequence
/// is seen; an unterminated or out-of-order run is skipped (so a partly
/// typed conflict doesn't flicker the whole tail of the buffer).
pub fn hunks(lines: &[String]) -> Vec<Hunk> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_start(&lines[i])
            && let Some((h, next)) = parse_one(lines, i)
        {
            out.push(h);
            i = next;
            continue;
        }
        i += 1;
    }
    out
}

/// Parse a single hunk opening at `start` (`lines[start]` is `<<<<<<<`).
/// Returns the hunk and the row just past its `>>>>>>>`, or `None` if the
/// run is malformed — a nested `<<<<<<<` before it closes, a `>>>>>>>`
/// with no preceding `=======`, or end-of-buffer first. On `None` the
/// caller advances one row, so a nested opener gets its own attempt.
fn parse_one(lines: &[String], start: usize) -> Option<(Hunk, usize)> {
    let mut base = None;
    let mut sep: Option<usize> = None;
    let mut j = start + 1;
    while j < lines.len() {
        let l = &lines[j];
        if is_start(l) {
            return None;
        }
        match sep {
            // Before the separator: the first `|||||||` marks the diff3
            // base, the first `=======` closes the top half.
            None => {
                if base.is_none() && is_base(l) {
                    base = Some(j);
                } else if is_sep(l) {
                    sep = Some(j);
                }
            }
            // After it: the first `>>>>>>>` closes the conflict.
            Some(sep) if is_end(l) => {
                return Some((
                    Hunk {
                        start,
                        base,
                        sep,
                        end: j,
                    },
                    j + 1,
                ));
            }
            Some(_) => {}
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn parses_a_basic_conflict() {
        let l = lines(
            "keep\n\
             <<<<<<< ours\n\
             mine\n\
             =======\n\
             yours\n\
             >>>>>>> theirs\n\
             tail",
        );
        let hs = hunks(&l);
        assert_eq!(hs.len(), 1);
        let h = hs[0];
        assert_eq!((h.start, h.sep, h.end), (1, 3, 5));
        assert_eq!(h.base, None);
        assert_eq!(&l[h.ours()], &["mine".to_string()]);
        assert_eq!(&l[h.theirs()], &["yours".to_string()]);
    }

    #[test]
    fn parses_diff3_base_region() {
        let l = lines(
            "<<<<<<< ours\n\
             mine\n\
             ||||||| base\n\
             orig\n\
             =======\n\
             yours\n\
             >>>>>>> theirs",
        );
        let h = hunks(&l)[0];
        assert_eq!(h.base, Some(2));
        assert_eq!(h.sep, 4);
        assert_eq!(&l[h.ours()], &["mine".to_string()]);
        assert_eq!(
            l[h.base_region().unwrap()].to_vec(),
            vec!["orig".to_string()]
        );
        assert_eq!(&l[h.theirs()], &["yours".to_string()]);
    }

    #[test]
    fn finds_multiple_conflicts() {
        let l = lines(
            "<<<<<<<\na\n=======\nb\n>>>>>>>\n\
             mid\n\
             <<<<<<<\nc\n=======\nd\n>>>>>>>",
        );
        let hs = hunks(&l);
        assert_eq!(hs.len(), 2);
        assert_eq!(hs[0].start, 0);
        assert_eq!(hs[1].start, 6);
    }

    #[test]
    fn ignores_unterminated_conflict() {
        let l = lines("<<<<<<<\nmine\n=======\nyours\nno end here");
        assert!(hunks(&l).is_empty());
    }

    #[test]
    fn ignores_stray_separator_and_end() {
        // A lone `=======` (Markdown underline) or `>>>>>>>` outside an
        // open `<<<<<<<` run is not a conflict.
        let l = lines("Title\n=======\nbody\n>>>>>>> not a marker");
        assert!(hunks(&l).is_empty());
    }

    #[test]
    fn nested_opener_restarts() {
        // The inner `<<<<<<<` aborts the outer parse; the inner one then
        // forms the real, well-formed conflict.
        let l = lines("<<<<<<<\nouter\n<<<<<<<\ninner\n=======\nyours\n>>>>>>>");
        let hs = hunks(&l);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].start, 2);
    }

    #[test]
    fn replacement_keeps_requested_side() {
        let l = lines("<<<<<<<\nmine\n=======\nyours\n>>>>>>>");
        let h = hunks(&l)[0];
        assert_eq!(
            h.replacement(&l, Resolution::Ours),
            vec!["mine".to_string()]
        );
        assert_eq!(
            h.replacement(&l, Resolution::Theirs),
            vec!["yours".to_string()]
        );
        assert_eq!(
            h.replacement(&l, Resolution::Both),
            vec!["mine".to_string(), "yours".to_string()]
        );
        assert!(h.replacement(&l, Resolution::None).is_empty());
    }

    #[test]
    fn both_drops_diff3_base() {
        let l = lines("<<<<<<<\nmine\n|||||||\norig\n=======\nyours\n>>>>>>>");
        let h = hunks(&l)[0];
        assert_eq!(
            h.replacement(&l, Resolution::Both),
            vec!["mine".to_string(), "yours".to_string()]
        );
    }
}
