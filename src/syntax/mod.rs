//! Syntax services backed by tree-sitter.
//!
//! Each per-buffer language facility lives behind [`Engine`] — the
//! single object the rest of the editor talks to. The engine itself
//! is a thin facade; the actual query handling is split into focused
//! modules so that "highlighting" doesn't end up owning indent,
//! text-object, bracket, and injection responsibilities the way the
//! old `Highlighter` struct did:
//!
//! - [`engine`] — `Engine` struct, parser/tree lifecycle, facade methods
//! - [`highlight`] — `highlights.scm` → spans
//! - [`indent`] — `indents.scm` → auto-indent + indent-guide scopes
//! - [`textobject`] — `textobjects.scm` → vim-style text objects
//! - [`bracket`] — tree-driven bracket pair matching (no query)
//! - [`injection`] — `injections.scm` → embed sub-language highlights /
//!   indent scopes (e.g. TS inside a Vue `<script lang="ts">`)
//! - [`loader`] — grammar `.so` / `.dylib` loading and query I/O
//!
//! Capture-name → terminal style mapping now lives in [`crate::theme`]
//! (themes are swappable at runtime); [`style_for`] re-exports the active
//! theme's resolver so existing call sites are unchanged.

mod bracket;
pub(crate) mod engine;
mod fold;
mod highlight;
mod indent;
pub(crate) mod injection;
mod loader;
mod textobject;

pub use crate::theme::style_for;
pub use engine::Engine;
pub use highlight::Capture;
pub use loader::Loader;
