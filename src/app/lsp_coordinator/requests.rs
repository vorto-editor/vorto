//! Request-building methods on [`LspCoordinator`]: each public
//! `request_*` entry point plus the private dispatch helpers
//! (`text_document_position_params`, `fan_out_request`, `send_single`,
//! `alloc_group`) that turn a user action into one or more outgoing LSP
//! requests and register the per-request fan-out bookkeeping.

use anyhow::Result;
use serde_json::Value;

use crate::editor::Cursor;
use crate::lsp::{Diagnostic, SignatureHelp};

use super::{
    Group, GroupAccum, LspCoordinator, LspRequestKind, Pending, diagnostic_to_json,
    signature_help_to_json,
};
use crate::app::signature::SignatureTrigger;

impl LspCoordinator {
    pub fn request_jump(
        &mut self,
        method: &str,
        label: &'static str,
        cursor: Cursor,
    ) -> Result<()> {
        let params = self.text_document_position_params(cursor);
        self.fan_out_request(
            method,
            params,
            LspRequestKind::Jump,
            GroupAccum::Jump {
                label,
                locations: Vec::new(),
            },
        )
    }

    pub fn request_references(&mut self, cursor: Cursor) -> Result<()> {
        let mut params = self.text_document_position_params(cursor);
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "context".to_string(),
                serde_json::json!({ "includeDeclaration": true }),
            );
        }
        self.fan_out_request(
            "textDocument/references",
            params,
            LspRequestKind::References,
            GroupAccum::References(Vec::new()),
        )
    }

    pub fn request_code_action(
        &mut self,
        cursor: Cursor,
        diagnostics: &[Diagnostic],
    ) -> Result<()> {
        let uri = self.current_uri.clone().unwrap_or_default();
        let line = cursor.row as u64;
        let character = cursor.col as u64;
        let diagnostics_json = Value::Array(
            diagnostics
                .iter()
                .filter(|d| {
                    d.range.start.line <= cursor.row as u32 && cursor.row as u32 <= d.range.end.line
                })
                .map(diagnostic_to_json)
                .collect(),
        );
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": line, "character": character },
                "end":   { "line": line, "character": character },
            },
            "context": { "diagnostics": diagnostics_json },
        });
        self.fan_out_request(
            "textDocument/codeAction",
            params,
            LspRequestKind::CodeAction,
            GroupAccum::CodeAction(Vec::new()),
        )
    }

    pub fn request_hover(&mut self, cursor: Cursor) -> Result<()> {
        let params = self.text_document_position_params(cursor);
        self.fan_out_request(
            "textDocument/hover",
            params,
            LspRequestKind::Hover,
            GroupAccum::Hover(Vec::new()),
        )
    }

    /// `trigger` is `Some(c)` when the request was fired because the
    /// user typed `c` and the server declared it as a trigger character
    /// (`completionProvider.triggerCharacters`). `None` covers manual
    /// `<C-Space>` invocations and auto-fires on identifier chars. The
    /// distinction matters: rust-analyzer's path completion (`foo::|`)
    /// expects `triggerKind: 2 (TriggerCharacter)` with `triggerCharacter`
    /// set, and otherwise treats the request as a plain `Invoked`.
    pub fn request_completion(
        &mut self,
        cursor: Cursor,
        prefix_start: Cursor,
        trigger: Option<char>,
    ) -> Result<()> {
        let mut params = self.text_document_position_params(cursor);
        let context = match trigger {
            Some(c) => serde_json::json!({
                "triggerKind": 2,
                "triggerCharacter": c.to_string(),
            }),
            None => serde_json::json!({ "triggerKind": 1 }),
        };
        if let Some(obj) = params.as_object_mut() {
            obj.insert("context".to_string(), context);
        }
        self.fan_out_request(
            "textDocument/completion",
            params,
            LspRequestKind::Completion,
            GroupAccum::Completion {
                prefix_start,
                items: Vec::new(),
            },
        )
    }

    /// `textDocument/signatureHelp` — fans out to every attached client
    /// and the first non-null response wins. `trigger` maps onto LSP's
    /// `SignatureHelpContext`:
    /// - `Invoked` (programmatic, e.g. after accept-completion's
    ///   auto-`()`) sends `triggerKind: 1`.
    /// - `TriggerCharacter(c)` sends `triggerKind: 2` plus the actual
    ///   character — servers branch on this (e.g. `(` is "open from
    ///   scratch" vs `,` would arrive as `ContentChange` retrigger).
    /// - `ContentChange(c)` sends `triggerKind: 3` with `isRetrigger:
    ///   true` and the typed char when known. Used for the per-keystroke
    ///   refresh that keeps `activeParameter` aligned with the cursor.
    ///
    /// `active_help` is the currently-displayed help (when the popup is
    /// open) — passed back as `activeSignatureHelp` so the server can
    /// reconcile its view with what we're showing.
    pub fn request_signature_help(
        &mut self,
        cursor: Cursor,
        trigger: SignatureTrigger,
        active_help: Option<&SignatureHelp>,
    ) -> Result<()> {
        let mut params = self.text_document_position_params(cursor);
        let is_retrigger = matches!(trigger, SignatureTrigger::ContentChange(_));
        let mut context = match trigger {
            SignatureTrigger::Invoked => serde_json::json!({
                "triggerKind": 1,
                "isRetrigger": is_retrigger,
            }),
            SignatureTrigger::TriggerCharacter(c) => serde_json::json!({
                "triggerKind": 2,
                "triggerCharacter": c.to_string(),
                "isRetrigger": is_retrigger,
            }),
            SignatureTrigger::ContentChange(c) => {
                let mut o = serde_json::json!({
                    "triggerKind": 3,
                    "isRetrigger": true,
                });
                if let Some(c) = c {
                    o["triggerCharacter"] = Value::String(c.to_string());
                }
                o
            }
        };
        if let Some(help) = active_help {
            context["activeSignatureHelp"] = signature_help_to_json(help);
        }
        if let Some(obj) = params.as_object_mut() {
            obj.insert("context".to_string(), context);
        }
        self.fan_out_request(
            "textDocument/signatureHelp",
            params,
            LspRequestKind::SignatureHelp,
            GroupAccum::SignatureHelp {
                anchor_row: cursor.row,
                help: None,
            },
        )
    }

    /// `completionItem/resolve` — single-client. `source` is the
    /// `client_key` that originally produced the item; resolving via a
    /// different server would lose the opaque `data` context.
    ///
    /// `item_index` tags the call site: `Some(idx)` for popup-display
    /// resolves (the handler updates `CompletionState.items[idx]` with
    /// the returned detail / documentation); `None` for accept-time
    /// resolves (the handler applies the returned `additionalTextEdits`
    /// to the buffer).
    pub fn request_completion_resolve(
        &mut self,
        raw: Value,
        source: &str,
        item_index: Option<usize>,
    ) -> Result<()> {
        let uri = self.current_uri.clone().unwrap_or_default();
        self.send_single(
            source,
            "completionItem/resolve",
            raw,
            LspRequestKind::CompletionResolve,
            GroupAccum::CompletionResolve {
                uri,
                item_index,
                item: None,
            },
        )
    }

    /// `codeAction/resolve` — single-client. `source` is the `client_key`
    /// that originally produced the action.
    pub fn request_code_action_resolve(&mut self, action: Value, source: &str) -> Result<()> {
        self.send_single(
            source,
            "codeAction/resolve",
            action,
            LspRequestKind::CodeActionResolve,
            GroupAccum::CodeActionResolve { action: None },
        )
    }

    pub fn request_rename(&mut self, new_name: String, cursor: Cursor) -> Result<()> {
        let mut params = self.text_document_position_params(cursor);
        if let Some(obj) = params.as_object_mut() {
            obj.insert("newName".to_string(), Value::String(new_name.clone()));
        }
        let kind_new_name = new_name.clone();
        self.fan_out_request(
            "textDocument/rename",
            params,
            LspRequestKind::Rename,
            GroupAccum::Rename {
                new_name: kind_new_name,
                edit: None,
            },
        )
    }

    fn text_document_position_params(&self, cursor: Cursor) -> Value {
        let uri = self.current_uri.clone().unwrap_or_default();
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": cursor.row as u64,
                "character": cursor.col as u64,
            }
        })
    }

    /// Allocate a group, dispatch `params` as `method` to every current
    /// client, register a pending entry for each, and stash the group's
    /// accumulator. When every client has either responded or had its
    /// `Pending` cleared on error, the accumulated state is surfaced as
    /// an [`LspEventOutcome`].
    fn fan_out_request(
        &mut self,
        method: &str,
        params: Value,
        kind: LspRequestKind,
        accum: GroupAccum,
    ) -> Result<()> {
        let keys = self.current_clients.clone();
        if keys.is_empty() {
            return Ok(());
        }
        let group_id = self.alloc_group();
        let mut sent = 0usize;
        for key in &keys {
            if let Some(client) = self.clients.get_mut(key) {
                match client.request(method, params.clone()) {
                    Ok(id) => {
                        self.pending.insert(
                            (key.clone(), id),
                            Pending {
                                group: group_id,
                                kind,
                            },
                        );
                        sent += 1;
                    }
                    Err(_) => {
                        // The reader thread will surface the underlying
                        // error separately; here we just don't count
                        // this client toward the group.
                    }
                }
            }
        }
        if sent == 0 {
            return Ok(());
        }
        self.groups.insert(
            group_id,
            Group {
                remaining: sent,
                accum,
            },
        );
        Ok(())
    }

    /// Single-client dispatch (used for resolve round-trips). Falls
    /// back to the first attached client when `source` is unknown — a
    /// stale completion whose originating server was disabled between
    /// the popup opening and the user pressing accept.
    fn send_single(
        &mut self,
        source: &str,
        method: &str,
        params: Value,
        kind: LspRequestKind,
        accum: GroupAccum,
    ) -> Result<()> {
        let key = if self.clients.contains_key(source) {
            source.to_string()
        } else if let Some(first) = self.current_clients.first().cloned() {
            first
        } else {
            return Ok(());
        };
        let Some(client) = self.clients.get_mut(&key) else {
            return Ok(());
        };
        let id = client.request(method, params)?;
        let group_id = self.alloc_group();
        self.pending.insert(
            (key, id),
            Pending {
                group: group_id,
                kind,
            },
        );
        self.groups.insert(
            group_id,
            Group {
                remaining: 1,
                accum,
            },
        );
        Ok(())
    }

    fn alloc_group(&mut self) -> u64 {
        let id = self.next_group_id;
        self.next_group_id = self.next_group_id.wrapping_add(1);
        id
    }
}
