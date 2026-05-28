//! Fold command helpers on [`App`].
//!
//! Folding state lives on the per-pane [`crate::editor::Editor`]; the
//! *foldable regions* are derived from the active document (tree-sitter
//! `folds.scm` when the language has one, indentation otherwise). These
//! helpers bridge the two: resolve the region under the cursor, mutate
//! the view's collapse state, and keep the cursor off hidden rows.

use std::collections::HashSet;

use crate::app::App;

/// Innermost foldable region containing `row` — the one with the
/// smallest span among those whose `header ..= end` covers it. `za`/`zo`/
/// `zc` act on this region.
fn innermost_fold_at(regions: &[(usize, usize)], row: usize) -> Option<(usize, usize)> {
    regions
        .iter()
        .copied()
        .filter(|&(h, e)| h <= row && row <= e)
        .min_by_key(|&(h, e)| e - h)
}

impl App {
    /// Foldable regions for the active document: syntax folds when the
    /// language ships a `folds.scm`, indentation-based regions otherwise
    /// (and for buffers with no highlighter at all). Sorted by header
    /// row, one region per header.
    pub(crate) fn fold_regions(&self) -> Vec<(usize, usize)> {
        let tab_width = self.effective_editor().tab_width.max(1);
        crate::editor::fold::buffer_fold_regions(self.active_doc(), tab_width)
    }

    /// Rows hidden by the active view's collapsed folds. Empty (and
    /// cheap — no region scan) when nothing is collapsed. Used by
    /// folding-aware vertical motion and the cursor-snap invariant.
    pub(crate) fn hidden_fold_rows(&self) -> HashSet<usize> {
        let folds = self.editor.folds();
        if folds.is_empty() {
            return HashSet::new();
        }
        let regions = self.fold_regions();
        let mut hidden = HashSet::new();
        for &(h, e) in &regions {
            if folds.is_collapsed(h) {
                hidden.extend((h + 1)..=e);
            }
        }
        hidden
    }

    /// Post-dispatch guard: pull the cursor off any hidden row after a
    /// command runs. Cheap no-op when nothing is folded — the common
    /// case — so it's safe to call after every `evaluate`. Catches
    /// motions (`gg`/`G`/search/jumps) that don't go through the
    /// folding-aware vertical-move path.
    pub(crate) fn snap_cursor_after_motion(&mut self) {
        if self.editor.folds().is_empty() {
            return;
        }
        let regions = self.fold_regions();
        self.snap_cursor_out_of_fold(&regions);
    }

    /// If the cursor sits on a hidden row (inside a collapsed fold), move
    /// it up to the outermost collapsed header covering it. The single
    /// invariant the rest of the fold code relies on: the cursor is never
    /// on a row the renderer would skip. Pass the already-computed
    /// `regions` to avoid recomputing them.
    pub(crate) fn snap_cursor_out_of_fold(&mut self, regions: &[(usize, usize)]) {
        let row = self.editor.cursor.row;
        let folds = self.editor.folds();
        // Outermost (smallest header) collapsed region whose hidden body
        // `header + 1 ..= end` contains the cursor.
        let mut target: Option<usize> = None;
        for &(h, e) in regions {
            if row > h && row <= e && folds.is_collapsed(h) {
                target = Some(target.map_or(h, |t| t.min(h)));
            }
        }
        if let Some(h) = target {
            self.editor.cursor.row = h;
            ed_op_ref!(self, clamp_col(false));
        }
    }

    /// `za` — toggle the fold under the cursor.
    pub(crate) fn fold_toggle_at_cursor(&mut self) {
        let regions = self.fold_regions();
        let row = self.editor.cursor.row;
        let Some((header, _)) = innermost_fold_at(&regions, row) else {
            return;
        };
        self.editor.folds_mut().toggle(header);
        self.snap_cursor_out_of_fold(&regions);
    }

    /// `zo` — open the fold under the cursor.
    pub(crate) fn fold_open_at_cursor(&mut self) {
        let regions = self.fold_regions();
        let row = self.editor.cursor.row;
        if let Some((header, _)) = innermost_fold_at(&regions, row) {
            self.editor.folds_mut().open(header);
        }
    }

    /// `zc` — close the fold under the cursor.
    pub(crate) fn fold_close_at_cursor(&mut self) {
        let regions = self.fold_regions();
        let row = self.editor.cursor.row;
        if let Some((header, _)) = innermost_fold_at(&regions, row) {
            self.editor.folds_mut().close(header);
            self.snap_cursor_out_of_fold(&regions);
        }
    }

    /// `zR` — open every fold in the buffer.
    pub(crate) fn fold_open_all(&mut self) {
        self.editor.folds_mut().clear();
    }

    /// `zM` — close every fold in the buffer.
    pub(crate) fn fold_close_all(&mut self) {
        let regions = self.fold_regions();
        self.editor.folds_mut().close_all(&regions);
        self.snap_cursor_out_of_fold(&regions);
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::editor::Cursor;

    /// Minimal `App` over a scratch buffer with `src` as its content.
    /// The loader has no grammar dirs, so no highlighter attaches and
    /// folding exercises the indentation-based fallback.
    fn app_with_lines(src: &[&str]) -> App {
        let config = crate::config::Config::load(None).expect("default config loads");
        let loader =
            crate::syntax::Loader::new(std::path::PathBuf::new(), std::path::PathBuf::new());
        let (tx, _rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut app = App::new(config, loader, tx, cwd);
        app.active_doc_mut().lines = src.iter().map(|s| s.to_string()).collect();
        app
    }

    #[test]
    fn za_collapses_block_and_snaps_cursor_to_header() {
        let mut app = app_with_lines(&["fn f() {", "    a;", "    b;", "}"]);
        // Cursor inside the body; toggling should fold (0,2) and pull
        // the cursor up to the header.
        app.editor.cursor = Cursor { row: 2, col: 0 };
        app.fold_toggle_at_cursor();
        let hidden = app.hidden_fold_rows();
        assert!(hidden.contains(&1) && hidden.contains(&2), "body hidden");
        assert!(
            !hidden.contains(&0) && !hidden.contains(&3),
            "header/tail visible"
        );
        assert_eq!(app.editor.cursor.row, 0, "cursor snapped to header");

        // Toggling again expands.
        app.fold_toggle_at_cursor();
        assert!(app.hidden_fold_rows().is_empty());
    }

    #[test]
    fn close_all_then_open_all() {
        let mut app = app_with_lines(&["fn f() {", "    a;", "}", "fn g() {", "    b;", "}"]);
        app.fold_close_all();
        assert!(!app.hidden_fold_rows().is_empty(), "zM collapses folds");
        app.fold_open_all();
        assert!(app.hidden_fold_rows().is_empty(), "zR opens all folds");
    }

    #[test]
    fn move_down_skips_hidden_rows() {
        let mut app = app_with_lines(&["fn f() {", "    a;", "    b;", "}", "after"]);
        app.editor.cursor = Cursor { row: 0, col: 0 };
        app.fold_close_at_cursor(); // fold (0,2)
        let hidden = app.hidden_fold_rows();
        let r = app.editor.doc.clone();
        let doc = app.documents.get(&r).unwrap();
        // From the header, j should jump over the hidden body to row 3.
        app.editor.move_down_folding(doc, &hidden);
        assert_eq!(app.editor.cursor.row, 3);
    }
}
