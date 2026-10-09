//! Shared fixture helpers for integration tests.
//!
//! Git fixtures are built at runtime via git2 (`Repository::init` + commits),
//! not stored as a nested `.git` — a nested repo could not be tracked or
//! reproduced in CI. Every commit uses a FIXED signature and time, so any
//! commit-derived output stays byte-identical across runs (determinism).

#![allow(dead_code)]

pub mod six_grammar;

use git2::{Repository, Signature, Time};
use protobuf::Message;
use reverse_architect::card::{Card, CardKind};
use reverse_architect::cli::LinkStyle;
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::graph::{FactKind, FactValue, Graph, NodeKey, NodeKind};
use scip::types::{Document, Index, Occurrence, SymbolRole};
use std::path::{Path, PathBuf};

// ---- SCIP index fixtures (U6, U7) ----
//
// Indexes are BUILT rather than recorded: the cases that matter most — an index
// whose declarations have moved, one written from the wrong working directory —
// cannot be produced by running a real indexer. The moniker syntax below is the
// real one, verified against a `scip-dotnet` index over a real C# library
// on 19.08.2026.

/// A SCIP moniker in `scip-dotnet`'s spelling. Overloads disambiguate by ordinal
/// (`Total(+1).`), never by parameter types.
pub fn scip_sym(descriptors: &str) -> String {
    format!("scip-dotnet nuget . . {descriptors}")
}

/// One occurrence at a 1-based source line. SCIP ranges are 0-based, so the
/// conversion happens here, in one place.
pub fn scip_occurrence(symbol: &str, line_1based: u32, definition: bool) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol.to_string();
    o.range = vec![line_1based as i32 - 1, 8, 20];
    if definition {
        o.symbol_roles = SymbolRole::Definition as i32;
    }
    o
}

/// One occurrence starting at a 1-based line AND column — what pairing a usage
/// site with the index needs (plan A, U9): two calls of one name on a line are
/// told apart by where each name starts. The column is in the index's own
/// unit, UTF-16 code units for `scip-dotnet` — the plugin's characters
/// wherever the line holds nothing outside the Basic Multilingual Plane;
/// SCIP's 0-based start is derived here, in one place.
pub fn scip_occurrence_at(
    symbol: &str,
    (line_1based, column_1based): (u32, u32),
    definition: bool,
) -> Occurrence {
    let mut o = scip_occurrence(symbol, line_1based, definition);
    let start = column_1based as i32 - 1;
    o.range = vec![line_1based as i32 - 1, start, start + 1];
    o
}

// Promoted from `tests/scip.rs` when `tests/measure.rs` became their second
// caller (U11).

/// 1-based `(line, column)` where the `nth` (0-based) `needle` starts in `src`,
/// the column in characters — where the plugin puts a used or declared name, so
/// an occurrence built at it is what a real indexer would have written there.
pub fn at(src: &str, needle: &str, nth: usize) -> (u32, u32) {
    let mut seen = 0;
    for (i, line) in src.lines().enumerate() {
        let mut from = 0;
        while let Some(pos) = line[from..].find(needle) {
            let column = from + pos;
            if seen == nth {
                return ((i + 1) as u32, (line[..column].chars().count() + 1) as u32);
            }
            seen += 1;
            from = column + needle.len();
        }
    }
    panic!("`{needle}` #{nth} is not in the fixture");
}

/// An occurrence of `symbol` where the `nth` `needle` starts. A needle may end
/// in a newline to pin a type name at the end of its line; only its start
/// matters.
pub fn occ(src: &str, symbol: &str, needle: &str, nth: usize, definition: bool) -> Occurrence {
    let needle = needle.trim_end_matches('\n');
    scip_occurrence_at(&scip_sym(symbol), at_end(src, needle, nth), definition)
}

/// [`at`], where a needle that names a type at the end of its line (`"A"`)
/// counts only whole-line-end matches — `A` alone would match every capital A.
pub fn at_end(src: &str, needle: &str, nth: usize) -> (u32, u32) {
    if needle.chars().all(char::is_alphanumeric) {
        let mut seen = 0;
        for (i, line) in src.lines().enumerate() {
            if let Some(prefix) = line.strip_suffix(needle) {
                if prefix.ends_with(' ') {
                    if seen == nth {
                        return ((i + 1) as u32, (prefix.chars().count() + 1) as u32);
                    }
                    seen += 1;
                }
            }
        }
    }
    at(src, needle, nth)
}

pub fn scip_document(relative_path: &str, occurrences: Vec<Occurrence>) -> Document {
    let mut d = Document::new();
    d.language = "c-sharp".to_string();
    d.relative_path = relative_path.to_string();
    d.occurrences = occurrences;
    d
}

/// Write an index carrying real tool info into `dir`, and hand back its path.
pub fn write_scip_index(dir: &Path, documents: Vec<Document>) -> PathBuf {
    let mut index = Index::new();
    let mut info = scip::types::ToolInfo::new();
    info.name = "scip-dotnet".to_string();
    info.version = "0.2.14".to_string();
    index.metadata.mut_or_insert_default().tool_info = Some(info).into();
    index.documents = documents;

    let path = dir.join("index.scip");
    std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
    path
}

/// The record `reverse-architect index` would write beside `index` for a clean,
/// complete run over `repo` at its HEAD (plan A, KTD9): the index's SHA-256,
/// the commit, a clean tree, one indexed project. A test that needs a record
/// `measure` must refuse changes one field of it.
pub fn index_record(index: &Path, repo: &Path) -> reverse_architect::index::Record {
    use reverse_architect::index::{
        ProjectOutcome, ProjectRecord, Record, ToolVersions, RECORD_SCHEMA,
    };
    use sha2::{Digest, Sha256};
    Record {
        schema: RECORD_SCHEMA,
        index: index.file_name().unwrap().to_string_lossy().into_owned(),
        index_sha256: format!("{:x}", Sha256::digest(std::fs::read(index).unwrap())),
        commit: reverse_architect::extract::Repo::open(repo)
            .unwrap()
            .head_revision(),
        dirty: Some(false),
        partial: false,
        restore: false,
        tools: ToolVersions {
            dotnet: Some("10.0.401".to_string()),
            scip_dotnet: Some("0.2.14".to_string()),
        },
        projects: vec![ProjectRecord {
            path: "src/App.csproj".to_string(),
            outcome: ProjectOutcome::Indexed,
            detail: None,
            log: "index.logs/src_App.csproj.log".to_string(),
            untracked_documents: Vec::new(),
        }],
        conflicts: Vec::new(),
    }
}

/// Write `record` where `reverse-architect index` puts it: `<stem>.record.json`
/// beside `index`.
pub fn write_index_record(index: &Path, record: &reverse_architect::index::Record) {
    std::fs::write(
        reverse_architect::measure::record_path(index),
        serde_json::to_string_pretty(record).unwrap(),
    )
    .unwrap();
}

/// A fixed author signature at a fixed instant — so golden output never flaps.
pub fn fixed_signature(name: &str) -> Signature<'static> {
    let email = format!("{}@example.test", name.to_lowercase());
    // Fixed epoch (2020-09-13T12:26:40Z), zero offset.
    Signature::new(name, &email, &Time::new(1_600_000_000, 0)).unwrap()
}

/// Write `files` (relative path, contents) under `dir`, stage them, and commit
/// with a fixed signature. Creates parent directories as needed.
pub fn write_and_commit(
    repo: &Repository,
    dir: &Path,
    author: &str,
    message: &str,
    files: &[(&str, &str)],
) {
    for (rel, contents) in files {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, contents).unwrap();
    }

    let mut index = repo.index().unwrap();
    for (rel, _) in files {
        index.add_path(Path::new(rel)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();

    let sig = fixed_signature(author);
    let parent = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|oid| repo.find_commit(oid).ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap();
}

/// Init a repo at `dir` and commit `files` in a single initial commit.
pub fn init_repo_with_files(dir: &Path, author: &str, files: &[(&str, &str)]) -> Repository {
    let repo = Repository::init(dir).unwrap();
    write_and_commit(&repo, dir, author, "initial", files);
    repo
}

/// A minimal `Config` pointing at `repo` for input and `out` for output —
/// portable link style, no profile, default ambiguity ceiling, no secret
/// prefixes, no SCIP index, English prose. The shared shape every test that
/// just needs *a* runnable config reaches for (cache tests included), so it
/// exists once instead of once per file.
pub fn config_for(repo: &Path, out: &Path) -> Config {
    Config {
        repo_root: repo.to_path_buf(),
        corpus_dir: out.to_path_buf(),
        links: LinkStyle::Portable,
        profile: None,
        ambiguity_ceiling: DEFAULT_AMBIGUITY_CEILING,
        secret_prefixes: Vec::new(),
        plugins: Vec::new(),
        scip_index: None,
        lang: "en".to_string(),
        sites: false,
        rights: reverse_architect::extract::RightsTable::granted(),
        model_verification: false,
        model_purpose: false,
        trace_over_ceiling: false,
    }
}

/// A right for `declared-receiver` on the bundled C# component, at the
/// catalog's current revision — what a measurement would record (plan A,
/// KTD11). Handed to a run through [`Config::rights`].
///
/// Promoted here from `resolver.rs` when `truthful_graph.rs` became its second
/// caller.
pub fn declared_receiver_right() -> reverse_architect::extract::RightsTable {
    use reverse_architect::extract::resolve::rights::current_revision;
    use reverse_architect::extract::{DeleteRight, RightsTable};
    use reverse_architect::graph::Rule;

    let csharp = reverse_architect::plugin::bundled::bundled()
        .iter()
        .find(|p| p.id() == "csharp")
        .expect("the bundled C# plugin");
    RightsTable::new(vec![DeleteRight {
        rule: Rule::DeclaredReceiver,
        revision: current_revision(Rule::DeclaredReceiver).expect("a catalog rule"),
        language: "csharp".to_string(),
        component: csharp.id().to_string(),
        component_sha256: csharp.digest().to_string(),
        contract: csharp.contract().word().to_string(),
        report: "docs/measurements/fixture.json".to_string(),
        report_sha256: "0".repeat(64),
    }])
}

/// The git index blob id `repo_root`'s CURRENT tracked-file set has for
/// `key`. Opens a FRESH `Repo` rather than taking an existing handle: libgit2
/// caches an already-opened repository's index in memory, so a handle opened
/// before a later commit can still report a stale blob id for it.
pub fn blob_id_for(repo_root: &Path, key: &str) -> git2::Oid {
    reverse_architect::extract::Repo::open(repo_root)
        .unwrap()
        .tracked_files_with_paths()
        .unwrap()
        .into_iter()
        .find(|tf| tf.key == key)
        .unwrap_or_else(|| panic!("{key} not tracked"))
        .blob_id
}

/// Find the card of `kind` titled `title`, or panic with every card's
/// kind/title so a failing assertion is diagnosable without a debugger.
/// Shared across every test file that builds cards (`card.rs`, `card_index.rs`,
/// `acceptance.rs`) — promoted here after the third copy converged on the
/// same code (`ce-simplify-code` code-reuse finding).
pub fn find<'a>(cards: &'a [Card], kind: CardKind, title: &str) -> &'a Card {
    cards
        .iter()
        .find(|c| c.kind == kind && c.title == title)
        .unwrap_or_else(|| {
            let seen: Vec<String> = cards
                .iter()
                .map(|c| format!("{:?}/{}", c.kind, c.title))
                .collect();
            panic!("no {kind:?} card titled {title:?}; saw: {seen:?}")
        })
}

/// Every `Symbol` node whose `SymbolName` fact equals `name`.
pub fn symbol_nodes(graph: &Graph, name: &str) -> Vec<NodeKey> {
    graph
        .nodes_sorted()
        .into_iter()
        .filter(|n| n.kind == NodeKind::Symbol)
        .filter(|n| {
            n.facts.iter().any(|f| {
                f.kind == FactKind::SymbolName && f.value == FactValue::Text(name.to_string())
            })
        })
        .map(|n| n.key.clone())
        .collect()
}

/// The declaration line tree-sitter recorded for the named symbol — so a SCIP
/// fixture index can be built against the grammar's real ranges instead of
/// guessed ones (shared by `scip.rs` and `acceptance.rs`).
pub fn decl_line(graph: &Graph, name: &str) -> u32 {
    symbol_nodes(graph, name)
        .first()
        .and_then(|key| {
            graph
                .node(key)?
                .facts
                .iter()
                .find(|f| f.kind == FactKind::SymbolName)?
                .provenance
                .span
                .start()
        })
        .unwrap_or_else(|| panic!("no declaration line for {name}"))
}

/// Walk `dir` recursively and fold every file's `(path relative to `dir`,
/// bytes)` into one digest, in a stable sorted order — the standard this
/// project already holds itself to for proving two directory trees agree
/// byte-for-byte (see the perf-refactor and card-corpus work this repo's own
/// `CLAUDE.md` records). `DefaultHasher` is deterministic within one process,
/// which is all two digests computed in the same test run ever need to be.
///
/// Promoted here from `cache.rs` when `six_grammar_baseline.rs` became its
/// second caller, following the same rule `find`/`symbol_nodes` above did.
pub fn hash_dir(dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};

    fn collect(base: &Path, current: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(read_dir) = std::fs::read_dir(current) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(base, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, bytes));
            }
        }
    }

    let mut entries = Vec::new();
    collect(dir, dir, &mut entries);
    entries.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut hasher);
    hasher.finish()
}

/// Init a repo at `dir` and stage `files` WITHOUT committing — a repo with no
/// history (for E4 degradation tests).
pub fn init_repo_staged_no_commit(dir: &Path, files: &[(&str, &str)]) -> Repository {
    let repo = Repository::init(dir).unwrap();
    for (rel, contents) in files {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, contents).unwrap();
    }
    let mut index = repo.index().unwrap();
    for (rel, _) in files {
        index.add_path(Path::new(rel)).unwrap();
    }
    index.write().unwrap();
    repo
}

// Promoted from `tests/cli.rs` when `tests/search_card.rs` became its second
// caller (plan B, U14).

/// Every file under `dir` as `(relative path, bytes)`, sorted, the cache left
/// out — it is the run's scratch state, not its output.
pub fn output_files(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with(".ra-cache") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}
