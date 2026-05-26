//! In-editor `:theme` command — the filterable theme picker with live
//! preview.
//!
//! Flow:
//! * `:theme` opens [`Prompt::ThemePicker`](crate::prompt::Prompt) over
//!   the available theme list (bundled ∪ user, plus the synthesized
//!   `ansi`), cursor on the active theme. Opening records
//!   [`App::theme_origin`] — the theme to fall back to on cancel.
//! * Moving the selection emits `PreviewTheme`, handled by
//!   [`Self::preview_theme`]: it swaps the *global* active theme so the
//!   buffer (and chrome) recolor instantly, without writing anything.
//! * Enter emits `SelectTheme` → [`Self::commit_theme`]: keep it active
//!   and persist `theme = "..."` to the config.
//! * Esc/Ctrl-C cancels → [`Self::revert_theme`]: restore `theme_origin`.

use super::{App, Toast, root_cause};

impl App {
    /// `:theme` — open the picker. `rest` is ignored (the picker is the
    /// whole interface); a future `:theme <name>` could set directly, but
    /// the live-preview picker is the point.
    pub(super) fn run_theme_command(&mut self, _rest: &str) {
        let themes = crate::theme::available();
        if themes.is_empty() {
            self.push_toast(Toast::error("no themes available"));
            return;
        }
        // Remember what to restore on cancel; live preview will mutate the
        // global active theme freely from here.
        self.theme_origin = Some(crate::theme::active_name());
        let current = crate::theme::active_name();
        self.prompt.open_theme_picker(themes, &current);
    }

    /// Live preview: swap the active theme to `name` without persisting.
    /// A load failure (corrupt/just-deleted file) is surfaced but leaves
    /// the previous preview in place so the picker keeps working.
    pub(super) fn preview_theme(&mut self, name: &str) {
        match crate::theme::load_by_name(name) {
            Ok(theme) => crate::theme::set_active(theme),
            Err(e) => self.push_toast(Toast::error(format!("theme {name}: {}", root_cause(&e)))),
        }
    }

    /// Commit `name`: keep it active (it's already previewed) and write
    /// `theme = "..."` to the user config. Clears the cancel fallback.
    pub(super) fn commit_theme(&mut self, name: &str) {
        self.theme_origin = None;
        // Make sure the committed theme is the active one even if the
        // commit arrives without a preceding preview (e.g. Enter on the
        // already-selected current theme).
        self.preview_theme(name);
        match crate::config::persist_theme(name) {
            Ok(path) => {
                self.config.theme = name.to_string();
                self.push_toast(Toast::info(format!(
                    "theme set to {name} ({})",
                    path.display()
                )));
            }
            Err(e) => self.push_toast(Toast::error(format!(
                "theme {name} applied but not saved: {}",
                root_cause(&e)
            ))),
        }
    }

    /// Cancel: restore the theme that was active when the picker opened.
    /// No-op (beyond clearing the flag) if nothing was previewed.
    pub(super) fn revert_theme(&mut self) {
        if let Some(origin) = self.theme_origin.take() {
            match crate::theme::load_by_name(&origin) {
                Ok(theme) => crate::theme::set_active(theme),
                Err(e) => self.push_toast(Toast::error(format!(
                    "restoring theme {origin}: {}",
                    root_cause(&e)
                ))),
            }
        }
    }
}
