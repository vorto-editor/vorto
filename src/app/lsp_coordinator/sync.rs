//! Document-synchronisation methods on [`LspCoordinator`]: the
//! `didOpen` / `didChange` / `didSave` notifications that keep every
//! attached client's view of the current document in step with the
//! editor.

use std::path::Path;

use anyhow::Result;

use crate::lsp;

use super::LspCoordinator;

impl LspCoordinator {
    /// Send `didOpen` for `path` against `client_key` and mark it as
    /// the current document. When `client_key` already holds this URI
    /// open (the user is switching back to an already-visited buffer)
    /// the LSP notification is skipped — re-sending `didOpen` for an
    /// already-open document is forbidden by the spec and would make
    /// servers like tsserver reject the request. Either way the
    /// "current document" pointers are repointed so subsequent
    /// `did_change` / requests target this URI.
    pub fn did_open(
        &mut self,
        client_key: &str,
        lang_name: &str,
        path: &Path,
        text: &str,
    ) -> Result<()> {
        let uri = lsp::path_to_uri(path);
        let already_open = self
            .open_uris
            .get(&uri)
            .is_some_and(|keys| keys.iter().any(|k| k == client_key));
        if !already_open && let Some(client) = self.clients.get_mut(client_key) {
            client.did_open(&uri, text)?;
            self.open_uris
                .entry(uri.clone())
                .or_default()
                .push(client_key.to_string());
        }
        self.current_uri = Some(uri);
        self.current_language = Some(lang_name.to_string());
        self.add_current_client(client_key);
        Ok(())
    }

    /// Fan out `didChange` to every client attached to the current
    /// document. No-op when nothing is attached.
    pub fn did_change(&mut self, text: &str) -> Result<()> {
        let Some(uri) = self.current_uri.clone() else {
            return Ok(());
        };
        let keys = self.current_clients.clone();
        for key in &keys {
            if let Some(client) = self.clients.get_mut(key) {
                client.did_change(&uri, text)?;
            }
        }
        Ok(())
    }

    /// Fan out `didSave` to every client attached to the current
    /// document.
    pub fn did_save(&mut self, text: &str) -> Result<()> {
        let Some(uri) = self.current_uri.clone() else {
            return Ok(());
        };
        let keys = self.current_clients.clone();
        for key in &keys {
            if let Some(client) = self.clients.get_mut(key) {
                client.did_save(&uri, text)?;
            }
        }
        Ok(())
    }
}
