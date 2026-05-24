//! Multi-server response-merge machinery: the per-request [`Group`]
//! accumulator and the free functions that fold each client's response
//! into it (`accumulate`), surface the merged outcome (`finalize`), and
//! the small JSON / dedup helpers those rely on.

use serde_json::Value;

use crate::editor::Cursor;
use crate::lsp::{
    self, CodeAction, CompletionItem, Diagnostic, Hover, Location, SignatureHelp, WorkspaceEdit,
};

use super::{LspEventOutcome, LspRequestKind};

/// Accumulator state for an in-flight fan-out. One `Group` is allocated
/// per user-initiated LSP request and lives until every client we
/// dispatched to has either responded, errored, or been declared dead.
pub(super) struct Group {
    /// How many client responses (or terminal errors) are still
    /// outstanding before we surface the merged outcome.
    pub(super) remaining: usize,
    pub(super) accum: GroupAccum,
}

pub(super) enum GroupAccum {
    Jump {
        label: &'static str,
        locations: Vec<Location>,
    },
    References(Vec<Location>),
    /// First non-empty edit wins. Rename across multiple servers in a
    /// single buffer is rare and trying to merge edit lists from two
    /// servers could double-apply.
    Rename {
        new_name: String,
        edit: Option<WorkspaceEdit>,
    },
    CodeAction(Vec<CodeAction>),
    /// Joined with blank lines on emit.
    Hover(Vec<String>),
    Completion {
        prefix_start: Cursor,
        items: Vec<CompletionItem>,
    },
    /// Resolve outcomes are inherently single-client; the group just
    /// carries the per-request context until the one response arrives.
    /// `item_index` distinguishes a popup-display resolve (with the
    /// item slot to update) from an accept-time resolve (`None` —
    /// pulls auto-import edits).
    CompletionResolve {
        uri: String,
        item_index: Option<usize>,
        item: Option<CompletionItem>,
    },
    CodeActionResolve {
        action: Option<CodeAction>,
    },
    /// Signature help is single-client; the group just carries the
    /// anchor row for stale-response detection and accumulates the one
    /// response.
    SignatureHelp {
        anchor_row: usize,
        help: Option<SignatureHelp>,
    },
}

/// Fold a single client's response into the group's accumulator.
pub(super) fn accumulate(
    accum: &mut GroupAccum,
    source: &str,
    result: &Value,
    kind: &LspRequestKind,
) {
    match (accum, kind) {
        (GroupAccum::Jump { locations, .. }, LspRequestKind::Jump) => {
            locations.extend(lsp::parse_locations(result));
        }
        (GroupAccum::References(locations), LspRequestKind::References) => {
            locations.extend(lsp::parse_locations(result));
        }
        (GroupAccum::Rename { edit, .. }, LspRequestKind::Rename) if edit.is_none() => {
            *edit = lsp::parse_workspace_edit(result);
        }
        (GroupAccum::CodeAction(actions), LspRequestKind::CodeAction) => {
            let mut parsed = lsp::parse_code_actions(result);
            for a in &mut parsed {
                a.source = source.to_string();
            }
            actions.extend(parsed);
        }
        (GroupAccum::Hover(parts), LspRequestKind::Hover) => {
            if let Some(h) = lsp::parse_hover(result) {
                parts.push(h.contents);
            }
        }
        (GroupAccum::Completion { items, .. }, LspRequestKind::Completion) => {
            let mut parsed = lsp::parse_completion(result);
            for it in &mut parsed {
                it.source = source.to_string();
            }
            items.extend(parsed);
        }
        (GroupAccum::CompletionResolve { item, .. }, LspRequestKind::CompletionResolve) => {
            // Servers that don't support resolve typically echo the
            // item back unchanged (or return null); both shapes parse
            // to either `None` or an item with no new fields, which
            // the handler treats as a no-op.
            *item = lsp::parse_completion_resolve(result);
            if let Some(it) = item.as_mut() {
                it.source = source.to_string();
            }
        }
        (GroupAccum::CodeActionResolve { action }, LspRequestKind::CodeActionResolve) => {
            let mut parsed = lsp::parse_code_action(result);
            if let Some(a) = parsed.as_mut() {
                a.source = source.to_string();
            }
            *action = parsed;
        }
        (GroupAccum::SignatureHelp { help, .. }, LspRequestKind::SignatureHelp)
            if help.is_none() =>
        {
            // First non-null response wins — fanning out to two servers
            // would otherwise need a merge strategy we don't have, and
            // signature help is inherently "one signature at a time".
            *help = lsp::parse_signature_help(result);
        }
        _ => {}
    }
}

/// Emit the merged outcome once every fanned-out client has reported.
pub(super) fn finalize(accum: GroupAccum) -> LspEventOutcome {
    match accum {
        GroupAccum::Jump { label, locations } => LspEventOutcome::Jump { label, locations },
        GroupAccum::References(locations) => LspEventOutcome::References(locations),
        GroupAccum::Rename { new_name, edit } => LspEventOutcome::Rename { new_name, edit },
        GroupAccum::CodeAction(actions) => LspEventOutcome::CodeActions(actions),
        GroupAccum::Hover(parts) => {
            if parts.is_empty() {
                LspEventOutcome::Hover(None)
            } else {
                LspEventOutcome::Hover(Some(Hover {
                    contents: parts.join("\n\n---\n\n"),
                }))
            }
        }
        GroupAccum::Completion {
            prefix_start,
            items,
        } => {
            let items = dedup_completion(items);
            LspEventOutcome::Completion {
                prefix_start,
                items,
            }
        }
        GroupAccum::CompletionResolve {
            uri,
            item_index,
            item,
        } => LspEventOutcome::CompletionResolved {
            uri,
            item_index,
            item,
        },
        GroupAccum::CodeActionResolve { action } => LspEventOutcome::CodeActionResolved(action),
        GroupAccum::SignatureHelp { anchor_row, help } => {
            LspEventOutcome::SignatureHelp { anchor_row, help }
        }
    }
}

/// Round-trip our `SignatureHelp` back into the LSP wire shape so we
/// can echo it in `activeSignatureHelp` on retriggers. The server uses
/// this to reconcile its view against the popup the user is currently
/// looking at — without it, retrigger context is missing the "what
/// were we showing" half.
///
/// Documentation and parameter labels round-trip as plain text/offsets;
/// any per-parameter docs are dropped (servers don't need them back).
pub(super) fn signature_help_to_json(help: &SignatureHelp) -> Value {
    let signatures: Vec<Value> = help
        .signatures
        .iter()
        .map(|s| {
            let parameters: Vec<Value> = s
                .parameters
                .iter()
                .map(|p| match &p.label {
                    lsp::ParameterLabel::Text(t) => serde_json::json!({ "label": t }),
                    lsp::ParameterLabel::Offsets(start, end) => {
                        serde_json::json!({ "label": [start, end] })
                    }
                })
                .collect();
            let mut obj = serde_json::json!({
                "label": s.label,
                "parameters": parameters,
            });
            if let Some(ap) = s.active_parameter {
                obj["activeParameter"] = Value::from(ap);
            }
            obj
        })
        .collect();
    let mut obj = serde_json::json!({
        "signatures": signatures,
        "activeSignature": help.active_signature,
    });
    obj["activeParameter"] = match help.active_parameter {
        Some(n) => Value::from(n),
        None => Value::Null,
    };
    obj
}

/// Strip duplicate completion items that bubbled up from multiple
/// servers offering the same symbol. Keys on `(label, kind,
/// insert_text-or-newText)` so legitimately-distinct items (same name,
/// different signatures) survive.
pub(super) fn dedup_completion(items: Vec<CompletionItem>) -> Vec<CompletionItem> {
    use std::collections::HashSet;
    let mut seen: HashSet<(String, u8, String)> = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let text_key = it
            .text_edit
            .as_ref()
            .map(|te| te.new_text.clone())
            .or_else(|| it.insert_text.clone())
            .unwrap_or_else(|| it.label.clone());
        let key = (it.label.clone(), it.kind, text_key);
        if seen.insert(key) {
            out.push(it);
        }
    }
    out
}

pub(super) fn diagnostic_to_json(d: &Diagnostic) -> Value {
    serde_json::json!({
        "range": {
            "start": { "line": d.range.start.line, "character": d.range.start.character },
            "end":   { "line": d.range.end.line,   "character": d.range.end.character },
        },
        "severity": d.severity as u8 + 1,
        "message": d.message,
        "source": d.source,
    })
}
