//! Three-way text merge used by `autoreload = "merge"` to follow an
//! external edit (formatter, agent, `git checkout`, another editor) into a
//! buffer that still has unsaved changes.
//!
//! Runs in two passes. The first merges at *line* granularity: a run only
//! one side changed applies that side; a run both sides changed
//! differently is a conflict. The second pass retries each line-level
//! conflict at *character* granularity — so a formatter that reindents a
//! line whose body you also edited merges cleanly instead of conflicting.
//! Only when the character pass still overlaps do we emit `<<<<<<<`
//! markers.
//!
//! Both passes share one granularity-generic [`diff3`] core. Unlike a CRDT
//! this never silently interleaves competing edits: a genuine overlap is
//! surfaced as a conflict, not merged into arbitrary text.

use std::hash::Hash;

use similar::{Algorithm, DiffOp, capture_diff_slices};

/// Conflict-marker lines. 7-char markers, matching git's convention; the
/// labels use the editor's `local`/`disk` vocabulary.
const MARK_LOCAL: &str = "<<<<<<< local (your edits)";
const MARK_SEP: &str = "=======";
const MARK_DISK: &str = ">>>>>>> disk";

/// One run of a three-way comparison.
enum Region<T> {
    /// Already reconciled — unchanged, changed on only one side, or both
    /// sides made the identical change. `content` is the agreed result.
    Resolved(Vec<T>),
    /// Both sides changed this run, differently. `base` lets the caller
    /// retry the conflict at a finer granularity.
    Conflict {
        #[allow(dead_code)]
        base: Vec<T>,
        ours: Vec<T>,
        theirs: Vec<T>,
    },
}

/// Merge `ours` and `theirs` — both descended from `base` — at line
/// granularity, retrying any conflict at character granularity. Returns
/// the merged lines and whether any unresolved conflict markers were
/// emitted.
pub fn three_way(base: &[String], ours: &[String], theirs: &[String]) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut had_conflict = false;

    for region in diff3(base, ours, theirs) {
        match region {
            Region::Resolved(lines) => out.extend(lines),
            Region::Conflict {
                base, ours, theirs, ..
            } => match char_retry(&base, &ours, &theirs) {
                Some(merged) => out.extend(merged),
                None => {
                    had_conflict = true;
                    out.push(MARK_LOCAL.to_string());
                    out.extend(ours);
                    out.push(MARK_SEP.to_string());
                    out.extend(theirs);
                    out.push(MARK_DISK.to_string());
                }
            },
        }
    }
    // A merge that resolves to nothing still needs the one-empty-line shape
    // every buffer keeps.
    if out.is_empty() {
        out.push(String::new());
    }
    (out, had_conflict)
}

/// Retry a line-level conflict at character granularity. The three regions
/// are flattened to `char` sequences (lines rejoined with `\n`) and merged
/// with the same [`diff3`] core. Returns the merged lines when every run
/// reconciles, or `None` if any character run still overlaps — in which
/// case the caller keeps the coarse line-level conflict markers, which
/// read better than mid-line ones.
fn char_retry(base: &[String], ours: &[String], theirs: &[String]) -> Option<Vec<String>> {
    let b: Vec<char> = base.join("\n").chars().collect();
    let o: Vec<char> = ours.join("\n").chars().collect();
    let t: Vec<char> = theirs.join("\n").chars().collect();

    let mut merged: Vec<char> = Vec::new();
    for region in diff3(&b, &o, &t) {
        match region {
            Region::Resolved(chars) => merged.extend(chars),
            Region::Conflict { .. } => return None,
        }
    }
    let text: String = merged.into_iter().collect();
    Some(text.split('\n').map(str::to_string).collect())
}

/// Granularity-generic three-way diff (the classic diff3 over two
/// matching-block lists against a common `base`). Ported from Bazaar's
/// `merge3`: find runs equal in all three sequences (sync points), then
/// classify each divergent run between them.
fn diff3<T: Clone + Eq + Hash>(base: &[T], ours: &[T], theirs: &[T]) -> Vec<Region<T>> {
    let mut regions = Vec::new();
    let (mut ia, mut ib, mut iz) = (0usize, 0usize, 0usize);

    for (zmatch, zend, amatch, aend, bmatch, bend) in sync_regions(base, ours, theirs) {
        let divergent = amatch > ia || bmatch > ib;
        if divergent {
            let equal_a = slice_eq(ours, ia, amatch, base, iz, zmatch);
            let equal_b = slice_eq(theirs, ib, bmatch, base, iz, zmatch);
            let same = slice_eq(ours, ia, amatch, theirs, ib, bmatch);
            if same || (equal_b && !equal_a) {
                // Both sides agree, or only ours changed → take ours.
                regions.push(Region::Resolved(ours[ia..amatch].to_vec()));
            } else if equal_a && !equal_b {
                // Only theirs changed → take theirs.
                regions.push(Region::Resolved(theirs[ib..bmatch].to_vec()));
            } else {
                regions.push(Region::Conflict {
                    base: base[iz..zmatch].to_vec(),
                    ours: ours[ia..amatch].to_vec(),
                    theirs: theirs[ib..bmatch].to_vec(),
                });
            }
        }
        if zend > zmatch {
            regions.push(Region::Resolved(base[zmatch..zend].to_vec()));
        }
        ia = aend;
        ib = bend;
        iz = zend;
    }
    regions
}

/// Runs equal across all three sequences, as
/// `(base_start, base_end, ours_start, ours_end, theirs_start, theirs_end)`.
/// Intersects base↔ours and base↔theirs matching blocks; the trailing
/// entry is a zero-length sentinel at each sequence's end so [`diff3`]
/// flushes the tail.
fn sync_regions<T: Eq + Hash>(
    base: &[T],
    ours: &[T],
    theirs: &[T],
) -> Vec<(usize, usize, usize, usize, usize, usize)> {
    let am = matching_blocks(base, ours);
    let bm = matching_blocks(base, theirs);
    let mut sl = Vec::new();
    let (mut ia, mut ib) = (0usize, 0usize);

    while ia < am.len() && ib < bm.len() {
        let (abase, amatch, alen) = am[ia];
        let (bbase, bmatch, blen) = bm[ib];
        // Overlap of the two matches projected onto `base`.
        let i = abase.max(bbase);
        let j = (abase + alen).min(bbase + blen);
        if i < j {
            let asub = amatch + (i - abase);
            let bsub = bmatch + (i - bbase);
            let len = j - i;
            sl.push((i, j, asub, asub + len, bsub, bsub + len));
        }
        // Advance whichever match ends first in `base`.
        if abase + alen < bbase + blen {
            ia += 1;
        } else {
            ib += 1;
        }
    }
    sl.push((
        base.len(),
        base.len(),
        ours.len(),
        ours.len(),
        theirs.len(),
        theirs.len(),
    ));
    sl
}

/// Equal runs between `base` and `side` as `(base_index, side_index, len)`,
/// with a zero-length `(len, len, 0)` sentinel appended.
fn matching_blocks<T: Eq + Hash>(base: &[T], side: &[T]) -> Vec<(usize, usize, usize)> {
    let mut m: Vec<(usize, usize, usize)> = capture_diff_slices(Algorithm::Myers, base, side)
        .into_iter()
        .filter_map(|op| match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => Some((old_index, new_index, len)),
            _ => None,
        })
        .collect();
    m.push((base.len(), side.len(), 0));
    m
}

/// Whether `x[xlo..xhi]` equals `y[ylo..yhi]`.
fn slice_eq<T: Eq>(x: &[T], xlo: usize, xhi: usize, y: &[T], ylo: usize, yhi: usize) -> bool {
    xhi - xlo == yhi - ylo && x[xlo..xhi] == y[ylo..yhi]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.split('\n').map(str::to_string).collect()
    }

    fn merge_str(base: &str, ours: &str, theirs: &str) -> (String, bool) {
        let (out, c) = three_way(&lines(base), &lines(ours), &lines(theirs));
        (out.join("\n"), c)
    }

    #[test]
    fn disjoint_line_edits_merge_clean() {
        let (out, conflict) = merge_str("a\nb\nc", "A\nb\nc", "a\nb\nC");
        assert!(!conflict);
        assert_eq!(out, "A\nb\nC");
    }

    #[test]
    fn one_sided_change_takes_that_side() {
        // Only theirs changed.
        let (out, conflict) = merge_str("a\nb\nc", "a\nb\nc", "a\nB\nc");
        assert!(!conflict);
        assert_eq!(out, "a\nB\nc");
    }

    #[test]
    fn identical_change_on_both_sides_is_not_conflict() {
        let (out, conflict) = merge_str("a\nb\nc", "a\nX\nc", "a\nX\nc");
        assert!(!conflict);
        assert_eq!(out, "a\nX\nc");
    }

    #[test]
    fn same_line_disjoint_columns_merge_via_char_pass() {
        // ours renames calc→compute; theirs reindents (4→8 spaces). Same
        // line, disjoint character regions — the char pass resolves it.
        let (out, conflict) = merge_str(
            "    foo = calc(x)",
            "    foo = compute(x)",
            "        foo = calc(x)",
        );
        assert!(!conflict, "expected clean merge, got:\n{out}");
        assert_eq!(out, "        foo = compute(x)");
    }

    #[test]
    fn true_overlap_conflicts_with_markers() {
        let (out, conflict) = merge_str("a\nb\nc", "a\nlocal\nc", "a\ndisk\nc");
        assert!(conflict);
        assert!(out.contains("<<<<<<< local (your edits)"), "{out}");
        assert!(out.contains("local"), "{out}");
        assert!(out.contains("======="), "{out}");
        assert!(out.contains("disk"), "{out}");
        assert!(out.contains(">>>>>>> disk"), "{out}");
    }

    #[test]
    fn insertions_on_both_sides_merge() {
        let (out, conflict) = merge_str("a\nb", "a\nNEW\nb", "a\nb\nTAIL");
        assert!(!conflict);
        assert_eq!(out, "a\nNEW\nb\nTAIL");
    }
}
