//! Diagnostic gutter and inline virtual-row rendering: per-row severity
//! lookup, per-row message summaries, and the styled virtual line.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use std::collections::HashMap;

use crate::app::App;
use crate::lsp::Severity;

use super::{GUTTER_SIGN_WIDTH, GUTTER_VCS_WIDTH};

/// Build a `row → highest severity` lookup for the visible window. Rows
/// outside `[scroll, last)` are skipped, multi-line diagnostics fill all
/// rows they span, and the most severe diagnostic wins per row.
pub(super) fn build_row_severity(
    app: &App,
    scroll: usize,
    last: usize,
) -> std::collections::HashMap<usize, Severity> {
    let mut map: std::collections::HashMap<usize, Severity> = std::collections::HashMap::new();
    let diags = match app.current_diagnostics() {
        Some(d) => d,
        None => return map,
    };
    for d in diags {
        let lo = d.range.start.line as usize;
        let hi = d.range.end.line as usize;
        for row in lo.max(scroll)..=hi.min(last.saturating_sub(1)) {
            map.entry(row)
                .and_modify(|s| {
                    if (d.severity as u8) < (*s as u8) {
                        *s = d.severity;
                    }
                })
                .or_insert(d.severity);
        }
    }
    map
}

/// Per-source-row diagnostic summary used for inline rendering. We
/// fold every diagnostic that *starts* on a row into a single virtual
/// line: the worst-severity message, with `(+N)` appended when more
/// than one diagnostic shares the row. Capping at one virtual row per
/// source row keeps the visual layout — and the cursor-y math — simple.
pub(super) struct RowDiag {
    pub severity: Severity,
    pub message: String,
    pub extra: usize,
}

/// Build the row → summary lookup, applying the cursor-vs-other-row
/// filter: the cursor's row shows any severity, every other row only
/// surfaces `Error` diagnostics inline. Keeps the buffer quiet when
/// the cursor is elsewhere — warnings/info/hints stay accessible via
/// the gutter sign and the status-bar toast.
pub(super) fn build_row_diag_summary(app: &App, cursor_row: usize) -> HashMap<usize, RowDiag> {
    let mut out: HashMap<usize, RowDiag> = HashMap::new();
    let Some(diags) = app.current_diagnostics() else {
        return out;
    };
    for d in diags {
        let row = d.range.start.line as usize;
        if row != cursor_row && d.severity != Severity::Error {
            continue;
        }
        // First line only — multi-line messages would blow past our
        // single-virtual-row budget.
        let msg = d.message.lines().next().unwrap_or("").to_string();
        match out.get_mut(&row) {
            None => {
                out.insert(
                    row,
                    RowDiag {
                        severity: d.severity,
                        message: msg,
                        extra: 0,
                    },
                );
            }
            Some(existing) => {
                if (d.severity as u8) < (existing.severity as u8) {
                    existing.severity = d.severity;
                    existing.message = msg;
                }
                existing.extra += 1;
            }
        }
    }
    out
}

/// Render one virtual diagnostic row. Layout mirrors a real source
/// row's gutter (sign + line-number column + vcs bar) but with blanks
/// so the message column-aligns with the source text above it.
pub(super) fn diagnostic_line(diag: &RowDiag, inner_text_width: usize) -> Line<'static> {
    let color = severity_color(diag.severity);
    // Blank gutter: 1 (sign) + 5 (line number column) + 1 (vcs bar).
    let gutter = " ".repeat((GUTTER_SIGN_WIDTH + 5 + GUTTER_VCS_WIDTH) as usize);
    let mut text = String::from("↳ ");
    text.push_str(&diag.message);
    if diag.extra > 0 {
        text.push_str(&format!(" (+{})", diag.extra));
    }
    if inner_text_width > 0 && text.chars().count() > inner_text_width {
        let mut t: String = text
            .chars()
            .take(inner_text_width.saturating_sub(1))
            .collect();
        t.push('…');
        text = t;
    }
    Line::from(vec![
        Span::raw(gutter),
        Span::styled(
            text,
            Style::default()
                .fg(color)
                .add_modifier(ratatui::style::Modifier::ITALIC),
        ),
    ])
}

pub(super) fn severity_color(sev: Severity) -> Color {
    match sev {
        Severity::Error => Color::Red,
        Severity::Warning => Color::Yellow,
        Severity::Info => Color::LightBlue,
        Severity::Hint => Color::DarkGray,
    }
}
