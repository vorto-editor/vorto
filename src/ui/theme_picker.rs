//! Screen-centered, filterable modal for `:theme`.
//!
//! Lists every available theme (bundled ∪ user, plus the synthesized
//! `ansi`) with a selection cursor and a live `/` filter, mirroring
//! [`super::grammar_list`]. The defining behavior is live preview: the
//! prompt layer swaps the active theme as the selection moves, so the
//! buffer *behind* this modal recolors in real time — this module only
//! paints the picker chrome. The currently-saved theme (`config.theme`)
//! is tagged so the user can tell which one Enter would replace.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::prompt::Prompt;

const MAX_WIDTH: u16 = 50;
const MAX_HEIGHT: u16 = 24;

pub(super) fn draw_theme_picker(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::ThemePicker {
        themes,
        selected,
        query,
        filtering,
    } = &app.prompt.state
    else {
        return;
    };
    if area.width < 12 || area.height < 6 {
        return;
    }

    let visible = crate::prompt::theme_visible_indices(themes, query);
    let show_filter = *filtering || !query.is_empty();

    let title = if show_filter {
        format!(
            " themes · {} match{} ",
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
        )
    } else {
        format!(" themes · {} ", themes.len())
    };
    let footer = "Enter apply · j/k move · / filter · Esc cancel";

    let inner_w = (footer.len() as u16)
        .max(title.len() as u16)
        .clamp(20, MAX_WIDTH);
    let popup_w = (inner_w + 2).min(area.width.saturating_sub(2));

    let filter_h = u16::from(show_filter);
    let body_h = (themes.len() as u16).min(MAX_HEIGHT);
    let box_h = (body_h + 2 + filter_h).min(area.height);
    let total_h = (box_h + 1).min(area.height);

    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(total_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: box_h,
    };

    // Colors follow the *previewed* theme (live preview already swapped the
    // active theme as the cursor moved here): the selection bar uses its
    // `ui.menu.selected`, normal rows its `ui.popup` text fg. Reading the
    // popup fg is what keeps text legible on a light theme's popup bg —
    // the terminal's own fg would vanish there.
    let theme = crate::theme::active();
    let text_fg = theme.ui_popup().fg;
    let dim_fg = theme.ui_linenr().fg.unwrap_or(Color::DarkGray);
    let sel_style = theme.ui_menu_selected();
    let normal_style = text_fg.map(|c| Style::default().fg(c)).unwrap_or_default();

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(Span::styled(title, normal_style))
        .style(Style::default().bg(super::panel_bg()));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);
    if show_filter {
        lines.push(filter_line(query, *filtering, normal_style));
    }

    let list_h = (inner.height as usize).saturating_sub(filter_h as usize);
    let scroll = super::grammar_list::scroll_offset(*selected, visible.len(), list_h);

    for (pos, &theme_idx) in visible.iter().enumerate().skip(scroll).take(list_h) {
        let name = &themes[theme_idx];
        let is_sel = pos == *selected;
        let is_applied = name == &app.config.theme;

        let row_style = if is_sel { sel_style } else { normal_style };
        // `>` marks the cursor row; other rows get two leading spaces so
        // names stay aligned.
        let marker = if is_sel { "> " } else { "  " };

        let mut spans = vec![
            Span::styled(marker, row_style),
            Span::styled(name.clone(), row_style),
        ];
        let mut used = 2 + name.chars().count();
        // Tag the theme that's actually applied/saved (what Enter would
        // replace). On the selection bar keep it in the bar style; off it,
        // dim it.
        if is_applied {
            let label = "  (applied)";
            used += label.chars().count();
            let label_style = if is_sel {
                row_style
            } else {
                Style::default().fg(dim_fg)
            };
            spans.push(Span::styled(label, label_style));
        }
        // Extend the selection bar across the full width.
        if is_sel {
            let pad = (inner.width as usize).saturating_sub(used);
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), row_style));
            }
        }
        lines.push(Line::from(spans).style(row_style));
    }

    if visible.is_empty() && list_h > 0 {
        lines.push(Line::from(Span::styled(
            "  no matching themes",
            Style::default().fg(dim_fg),
        )));
    }

    f.render_widget(Paragraph::new(lines), inner);

    let hint_y = y + box_h;
    if hint_y < area.y + area.height {
        let hint_rect = Rect {
            x,
            y: hint_y,
            width: popup_w,
            height: 1,
        };
        f.render_widget(Clear, hint_rect);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                footer,
                Style::default().fg(Color::DarkGray),
            ))),
            hint_rect,
        );
    }
}

/// Top filter row — a live yellow `/` prompt while typing, a dim
/// `filter:` indicator once dismissed. `text_style` carries the previewed
/// theme's popup fg so the typed query stays legible on its background.
fn filter_line(query: &str, filtering: bool, text_style: Style) -> Line<'static> {
    if filtering {
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Yellow)),
            Span::styled(query.to_string(), text_style),
            Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
        ])
    } else {
        Line::from(Span::styled(format!(" filter: {query}"), text_style))
    }
}
