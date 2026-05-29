//! First-run bootstrap of grammars embedded in the binary.
//!
//! When built with the `bundled-grammars` feature, `build.rs` embeds the
//! pre-built, gzip-compressed grammar libraries for the host target (from
//! `assets/grammars-prebuilt/<target>/`) into [`BUNDLED_GRAMMARS`]. On
//! startup [`bootstrap`] decompresses each into `grammar_dir` and writes
//! its vendored `.scm` queries into `query_dir`, so a freshly-unpacked
//! release binary highlights every built-in language out of the box with
//! no `vorto grammar install` and no network access.
//!
//! Placement target is the normal `~/.config/vorto/{grammars,queries}` —
//! identical to what `grammar install` produces — so the loader, the
//! `grammar list` command, and a later manual reinstall all see the same
//! files. Embedding is the *delivery* mechanism; `.config` stays the
//! single source of truth the editor reads from.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;

use crate::config::GrammarSource;

use super::build;

// `(name, dylib-ext, gzip-compressed bytes)`, emitted by `build.rs`.
// Empty when the feature is off or no artifacts were present at build.
include!(concat!(env!("OUT_DIR"), "/bundled_grammars.rs"));

/// Marker file recording which vorto version last extracted the bundled
/// set, so an upgraded binary re-extracts (picking up refreshed grammars)
/// without re-extracting on every launch.
const MARKER: &str = ".bundled_version";

/// Extract embedded grammars into `grammar_dir` and their queries into
/// `query_dir`, skipping any grammar the user has repointed via a
/// `[grammars.<name>]` config entry (we never clobber a user's own
/// choice). Returns the number of grammars newly written.
///
/// Idempotent and cheap on the steady-state path: when the version marker
/// already matches and every bundled grammar is fully installed, it does
/// nothing. Best-effort by design — the caller logs and continues on
/// error, since a missing grammar degrades to "no highlighting for that
/// language", never a failed startup.
pub fn bootstrap(
    grammar_dir: &Path,
    query_dir: &Path,
    user_grammars: &[GrammarSource],
) -> Result<usize> {
    if BUNDLED_GRAMMARS.is_empty() {
        return Ok(0);
    }

    let current = env!("CARGO_PKG_VERSION");
    let marker_path = grammar_dir.join(MARKER);
    // On a version bump we force a refresh so updated grammar binaries
    // replace the old ones even when a same-named file already exists.
    let refresh = std::fs::read_to_string(&marker_path)
        .map(|v| v.trim() != current)
        .unwrap_or(true);

    let overridden: HashSet<&str> = user_grammars.iter().map(|g| g.name.as_str()).collect();

    std::fs::create_dir_all(grammar_dir)
        .with_context(|| format!("creating grammar dir {}", grammar_dir.display()))?;

    let mut written = 0;
    for (name, ext, gz) in BUNDLED_GRAMMARS {
        // Respect a user's repointed grammar — their `grammar install`
        // output is authoritative for that name.
        if overridden.contains(name) {
            continue;
        }
        if !refresh && build::is_fully_installed(name, grammar_dir, query_dir) {
            continue;
        }
        if let Err(e) = extract_one(name, ext, gz, grammar_dir, query_dir) {
            // Skip the single bad grammar; keep going for the rest.
            crate::vlog!("bundled grammar `{name}` extract failed: {e:#}");
            continue;
        }
        written += 1;
    }

    // Record the version even on partial success: `is_fully_installed`
    // still gates per-grammar work next launch, so a re-run after a
    // transient failure self-heals without re-extracting everything.
    let _ = std::fs::write(&marker_path, current);
    Ok(written)
}

/// Decompress one embedded library into `grammar_dir/<name>.<ext>` and
/// materialize its bundled queries.
fn extract_one(
    name: &str,
    ext: &str,
    gz: &[u8],
    grammar_dir: &Path,
    query_dir: &Path,
) -> Result<()> {
    let mut decoder = GzDecoder::new(gz);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .with_context(|| format!("decompressing bundled grammar `{name}`"))?;

    let lib_path = grammar_dir.join(format!("{name}.{ext}"));
    // Write to a temp sibling then rename so a crash mid-write can't leave
    // a truncated `.so` that the loader would choke on.
    let tmp = grammar_dir.join(format!(".{name}.{ext}.tmp"));
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &lib_path)
        .with_context(|| format!("installing {}", lib_path.display()))?;

    build::write_vendored_queries(query_dir, name)
        .with_context(|| format!("writing queries for `{name}`"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "vorto-bundled-test-{}-{}",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn bootstrap_extracts_then_is_idempotent() {
        // These tests only mean anything when the feature actually
        // embedded artifacts; otherwise the table is empty by design.
        assert!(
            !BUNDLED_GRAMMARS.is_empty(),
            "run `vorto grammar bundle` for this target before testing with --features bundled-grammars"
        );

        let root = tmp("extract");
        let _ = std::fs::remove_dir_all(&root);
        let gdir = root.join("grammars");
        let qdir = root.join("queries");

        let n = bootstrap(&gdir, &qdir, &[]).unwrap();
        assert_eq!(n, BUNDLED_GRAMMARS.len(), "first run extracts everything");

        // A known grammar's decompressed library landed and looks real
        // (bigger than the gzip header, i.e. actually inflated).
        let (name, ext, _) = BUNDLED_GRAMMARS[0];
        let lib = gdir.join(format!("{name}.{ext}"));
        assert!(lib.exists(), "{} should exist", lib.display());
        assert!(std::fs::metadata(&lib).unwrap().len() > 1024);
        // Its vendored queries came along too.
        assert!(qdir.join(name).join("highlights.scm").exists());
        // Version marker recorded.
        assert!(gdir.join(MARKER).exists());

        // Second run: marker matches and everything is installed → no-op.
        let again = bootstrap(&gdir, &qdir, &[]).unwrap();
        assert_eq!(again, 0, "steady state extracts nothing");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_override_is_not_clobbered() {
        let root = tmp("override");
        let _ = std::fs::remove_dir_all(&root);
        let gdir = root.join("grammars");
        let qdir = root.join("queries");

        let (name, ext, _) = BUNDLED_GRAMMARS[0];
        let overrides = vec![GrammarSource {
            name: name.to_string(),
            source: "https://example.com/fork".into(),
            rev: None,
            subpath: None,
        }];
        bootstrap(&gdir, &qdir, &overrides).unwrap();

        // The user-repointed grammar must NOT have been written by the
        // bundle bootstrap.
        assert!(
            !gdir.join(format!("{name}.{ext}")).exists(),
            "bundle clobbered a user-overridden grammar"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
