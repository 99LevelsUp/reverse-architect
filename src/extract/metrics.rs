//! The metrics extractor (U7): lines-of-code per file and per module, via tokei
//! used as an embedded library (KTD-B4 — `default-features = false`, so no CLI
//! / clap / serialization surface, and — verified with `cargo tree -i
//! libgit2-sys` — no second `libgit2-sys` to conflict with our `git2`).
//!
//! LOC is tokei's total physical line count (`code + comments + blanks`), so
//! every non-empty tracked file carries a non-zero measure — a comment-only or
//! data file is still counted, not silently zeroed.
//!
//! KTD10: tokei classifies exactly the git-tracked file list, never a directory
//! walk — so git-ignored / vendored trees (`node_modules/`, `target/`) that are
//! deliberately absent from the graph can never be re-admitted.
//!
//! Determinism (KTD7): each file's bytes are read from its git **index blob**,
//! not the working tree, and counted in isolation via `parse_from_slice` — so the
//! measure is byte-identical for a given commit regardless of a dirty tree or
//! `core.autocrlf` line-ending differences across checkouts. Values are funneled
//! through a `BTreeMap` keyed by the node key before emission, and module sums are
//! derived from that ordered map, so emission order is stable too.

use super::ids;
use super::languages::Languages;
use super::{ExtractError, Extractor, Repo};
use crate::graph::{
    ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Tier,
};
use std::collections::BTreeMap;
use tokei::{Config, LanguageType};

/// Trust-rank 25 (KTD-B8): above filetree/git, below tree-sitter/manifest-deps.
const TRUST_RANK: u8 = 25;

pub struct Metrics {
    /// The run's language registry (U4). Carried on the extractor the same way
    /// `secret_prefixes` and the cache handle are — set once at construction —
    /// rather than widening every extractor's `run` signature.
    pub languages: Languages,
}

impl Extractor for Metrics {
    fn id(&self) -> ExtractorId {
        ids::metrics()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[FactKind::Loc]
    }

    fn trust_rank(&self) -> u8 {
        TRUST_RANK
    }

    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        _progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError> {
        let config = Config::default();

        // Per-file LOC from the git index blob, funneled through a sorted map for
        // deterministic emission order. Keys are `tf.key` (the byte-preserving
        // graph key filetree/git already use); they are never re-derived from a
        // path string, so tokei's own path normalization can no longer mis-key or
        // silently drop a `%`-named file (the old `resolve_key` fallback is gone).
        let mut file_loc: BTreeMap<String, i64> = BTreeMap::new();
        for tf in repo.tracked_files_with_paths()? {
            // A symlink's blob is its target path text, not source; skip it (E4).
            if tf.is_symlink {
                continue;
            }
            // Classify by EXTENSION only (pure). tokei's `from_path` also consults
            // a filename table and, for extensionless unknown files, falls back to
            // `from_shebang`, which OPENS THE FILE FROM DISK (cwd-relative, working
            // tree) — non-deterministic and an index-blob/KTD10 violation. Deriving
            // the language from the extension alone keeps LOC a pure function of the
            // blob. Extensionless files (Makefile, shebang scripts) get no LOC fact:
            // a deterministic, honest coverage gap, preferable to guessing from a
            // dirty or absent disk file. (E4: still a File node + git facts.)
            let Some(ext) = tf.path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            // The registry answers first (U4), so a plugin language is counted
            // with the counter its own entry names instead of being silently
            // zero-LOC. An entry with `loc: None` is a decision, not a gap: it
            // means "known language, not countable here", so it emits nothing
            // rather than falling through to a tokei guess about the extension.
            //
            // Behind it, tokei's own broad table still handles every extension
            // no language claims — `.md`, `.json`, `.toml` keep the LOC facts
            // they have today, which registry-only classification would drop.
            let lang = match self.languages.for_extension(ext) {
                Some(entry) => match entry.loc {
                    Some(lang) => lang,
                    None => continue,
                },
                None => match LanguageType::from_file_extension(ext) {
                    Some(lang) => lang,
                    None => continue,
                },
            };
            let Ok(bytes) = repo.blob_bytes(&tf) else {
                continue;
            };
            // `.lines()` on the parent CodeStats (code + comments + blanks),
            // EXCLUDING embedded-language child blobs — this matches the old
            // per-report sum, which also excluded embedded blobs. Do NOT call
            // `.summarise()` here: it folds child blobs in and inflates LOC for a
            // file with an embedded language (e.g. a fenced code block in Markdown),
            // breaking byte-identity with pre-branch output on a clean checkout.
            let loc = lang.parse_from_slice(&bytes, &config).lines() as i64;
            // Tracked keys are unique (deduped upstream), so a plain insert holds.
            file_loc.insert(tf.key.clone(), loc);
        }

        // Emit per-file facts (Measured) and accumulate module sums.
        let mut module_loc: BTreeMap<String, i64> = BTreeMap::new();
        for (file, loc) in &file_loc {
            let key = NodeKey::new(file.clone());
            // Only land on the File node filetree created (mirrors git.rs); a
            // key that is not a File is skipped rather than fabricated.
            match graph.node(&key) {
                Some(node) if node.kind == NodeKind::File => {}
                _ => continue,
            }
            graph.add_fact(
                &key,
                Fact::new(
                    FactKind::Loc,
                    FactValue::Int(*loc),
                    Provenance::new(file.clone(), None, ids::metrics(), Tier::Measured),
                ),
            );
            // Credit this file's LOC to every ancestor module, including the
            // root `.` — a module's LOC is the sum over every file beneath it
            // (recursive), matching how the overview ranks modules by file
            // count.
            for module in ancestor_modules(file) {
                *module_loc.entry(module).or_insert(0) += *loc;
            }
        }

        // Emit per-module facts as a clearly-derived sum (Tier::Derived).
        for (module, loc) in &module_loc {
            let key = NodeKey::new(module.clone());
            match graph.node(&key) {
                Some(node) if node.kind == NodeKind::Module => {}
                _ => continue,
            }
            graph.add_fact(
                &key,
                Fact::new(
                    FactKind::Loc,
                    FactValue::Int(*loc),
                    Provenance::new(module.clone(), None, ids::metrics(), Tier::Derived),
                ),
            );
        }

        Ok(())
    }
}

/// Every ancestor module key of a `/`-normalized file path, including the root
/// module `.`. `a/b/c.rs` → `.`, `a`, `a/b`; `README.md` → `.`.
fn ancestor_modules(file: &str) -> Vec<String> {
    let mut out = vec![".".to_string()];
    let parts: Vec<&str> = file.split('/').collect();
    for i in 1..parts.len() {
        out.push(parts[..i].join("/"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestor_modules_walks_every_prefix_including_root() {
        assert_eq!(ancestor_modules("README.md"), vec!["."]);
        assert_eq!(ancestor_modules("a/b/c.rs"), vec![".", "a", "a/b"]);
    }
}
