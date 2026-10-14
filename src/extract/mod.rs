//! Extractors (U3+): sources that populate the fact graph.
//!
//! Every extractor implements [`Extractor`] — it declares an id, the fact kinds
//! it emits, and a trust-rank that governs precedence in a scalar conflict
//! during merge (KTD5, E6). Extraction is deterministic and offline.
//!
//! File enumeration prefers git (KTD10): where there is a repository, the
//! canonical list of files is what git tracks, never a filesystem walk.
//! Git-ignored paths (`node_modules/`, `venv/`, per-user caches) differ across
//! developers and machines and are not part of the shared architecture, so they
//! never enter the graph.
//!
//! A tree with no repository is indexed through the crate-internal
//! `DirectorySource` (U15), which reproduces that property rather than abandoning it: it respects
//! the tree's own `.gitignore` files and no per-machine ignore source at all.
//! Which source a run got is not visible to any extractor — see the
//! crate-internal `source` module.

pub mod bodies;
mod body_secrets;
mod cache;
pub(crate) mod directory;
pub mod external;
pub mod filetree;
pub mod ids;
pub mod languages;
pub mod manifest_deps;
pub mod metrics;
pub(crate) mod name_ref;
pub mod readme;
mod reconcile;
pub mod resolve;
pub mod scip;
pub(crate) mod source;
pub mod test_pairing;
pub mod treesitter;
mod walk;
mod wire;

use crate::graph::{ExtractorId, FactKind, Graph, Ranks};
// `pub(crate)` re-export (review #3): `CacheStore` only appears in
// crate-internal signatures now (`default_extractors_with`,
// `run_all_with_cached`, and the two extractor structs below) — the public
// `run_all_with` never takes one, so nothing outside `extract` needs to name
// this type. The `cache` module stays private either way.
pub(crate) use cache::CacheStore;
// `pub(crate)` re-export: `activate`, `compute_fingerprint`, and
// `CACHE_DIR_NAME` are cache-internal (their types — `RunFingerprint` — stay
// `pub(crate)` too, unlike `CacheStore` above), but `generate_reporting`
// (`src/lib.rs`, U5) is a sibling module of `extract`, not a descendant of
// it, so it cannot name `cache::activate` through the private `cache`
// module even with `pub(crate)` visibility on the item itself — Rust module
// privacy gates the PATH, not just the item. Re-exporting here, at
// crate-wide visibility, is the narrowest fix: only these three names cross
// out of `cache`, everything else in it stays reachable solely from within
// `extract`.
pub(crate) use cache::{activate, compute_fingerprint, CACHE_DIR_NAME};
pub use resolve::rights::{DeleteRight, RightsTable};
use std::path::{Path, PathBuf};
pub(crate) use treesitter::NamesHandoff;

/// Current on-disk cross-run cache format version (U2+).
///
/// Embedded in the fingerprint header ([`cache::RunFingerprint`], KTD1) a run
/// writes before extracting, so a build with a different `CACHE_VERSION`
/// never matches a header written by another build — even when the repo's
/// tracked files and [`ExtractOptions`] are otherwise byte-identical (KTD1:
/// cache validity is exact-match-or-invalid, never a partial invalidation).
///
/// Bump this whenever the cache FORMAT changes: the fingerprint's own field
/// set, the shape of a per-file cache entry, or the on-disk layout of the
/// cache store — AND whenever the host's own logic changes what it stores for
/// unchanged input. The fingerprint invalidates on any tracked-file or
/// relevant-option change and, since P3, on any plugin's digest, so a change
/// inside a plugin needs no bump here. A change in the HOST's per-file
/// computation does: the host's code is in no fingerprint field, so a record it
/// computed before the change would keep being served as a hit (v3 and v5
/// below are both this case).
///
/// NOT bumped for U15, deliberately, and the reason is the rule above applied
/// rather than skipped. `Provenance` gained `anchor: Option<SourceAnchor>`,
/// which touches the cache record's wire form — but it is the wire-compatible
/// half of the v3 story with none of the stale-output half. A record written by
/// an older binary decodes with `anchor: None`, which is correct rather than
/// merely tolerated: `None` is what every bundled extractor sets and will keep
/// setting, because a file in a tree has no inside to anchor into. Nothing about
/// a stored record became stale, so there is nothing a bump would rescue. The
/// day a source starts POPULATING it, that source's records genuinely differ
/// from a pre-U15 binary's and the bump belongs there.
///
/// v3: `Provenance`'s wire form gained `ambiguous_candidates: Option<usize>`
/// (fix/ambiguous-relation-visibility). This is NOT a wire-compatibility bump —
/// a missing `Option<T>` field decodes to `None` fine either way (serde's
/// built-in behavior, verified by this fix's own round-trip tests) — it is a
/// **stale-computed-output** bump. `CacheStore::get` (this file) returns a
/// cache HIT's stored `Out` value directly, skipping re-extraction entirely
/// (`walk_files`, `src/extract/walk.rs`): a per-file record written by the
/// pre-fix binary carries edges whose `Provenance` was computed by
/// `flush_usages` before `.ambiguous()` existed, and nothing about that
/// record's BYTES is wrong, so a wire-compatibility check alone would never
/// catch it. Left unbumped, upgrading this binary against an unmodified
/// target repo — the ordinary "regenerate the same corpus again" workflow —
/// would silently keep serving those pre-fix edges (`ambiguous_candidates:
/// None` on every one) forever, while the freshly-rendered card's
/// `CARD_SCHEMA_VERSION: 3` front matter falsely claims the new marker is
/// active for that data — confirmed against the `walk_files` cache-hit path
/// that returns a stored `Out` without ever calling back into extraction.
///
/// v4: language extraction moved into the bundled plugins (U11). Two things
/// changed at once, and either alone would need the bump. The phases are named
/// `languages-decl` / `languages-usages` instead of `treesitter-*`, so a v3
/// store's language records would simply never be read again — dead weight, not
/// danger. The danger is the other half: every language fact now names the
/// plugin that wrote it (`rust`, `ecmascript`, …) instead of `treesitter`, an id
/// this build no longer knows. A v3 record served as a hit would carry an
/// unknown extractor into the graph. Until the fingerprint carries plugin
/// digests (U10), this constant was also what separated two builds whose bundled
/// components differ; since P3 the fingerprint names every plugin's digest, so a
/// rebuilt component invalidates the cache on its own.
///
/// v5: the confidence rung and the rule (plan A, U1). `flush_usages` — host
/// code, which no fingerprint field covers — now rates every edge it emits
/// (`ambiguous` or `name_only`, by `name-match`), and its output is what the
/// `languages-usages` records store. The v3 story again: a v4 record decodes
/// cleanly, the two new `Provenance` fields simply absent, but served as a hit
/// it would put unrated edges into a graph that claims every name-resolved
/// relation carries its rung. Type relations are rated later, at the run-level
/// flush, so their cached form did not change.
///
/// v6: usage sites (plan A, U3). `flush_usages` now gives every edge the sorted
/// list of the sites it was read at, and keeps the provenance of the site with
/// the strongest rung instead of simply the first. The v3 story a third time,
/// and the reason this is a bump rather than a decode failure: a v5 record —
/// written only by this unreleased branch's earlier builds — decodes cleanly
/// with no `sites` key, and served as a hit it would put edges read nowhere
/// into a graph whose cards and manifest list every relation's sites. Making
/// the key required would have refused it too, but it would also have made
/// every older fixture record undecodable, and the cache tests rest on those
/// records still decoding (`a_record_written_before_u3_…`,
/// `a_pre_confidence_record_…`); this constant is the mechanism the rule above
/// names for a changed per-file entry.
///
/// Plugin contract 0.2 (plan A, U5) is NOT a bump, checked rather than
/// assumed: the `languages-usages` records store the resolved edges, not the
/// sites, so the site context they gain is never cached; a site id's column
/// was optional in v6 already; and a 0.1 plugin's output is byte-for-byte what
/// it was. The only records that change are those of a plugin rebuilt on 0.2,
/// and a rebuilt component changes its digest in the fingerprint, so a record
/// from before can never be served beside it.
///
/// v7: the shared resolver (plan A, U7). Three things at once, on this
/// unreleased branch. The `languages-usages` records gain the site records
/// (KTD7) and the `languages-decl` records the declaration-name positions —
/// a v6 record decodes cleanly with neither, and served as a hit it would hand
/// the run a symbol table that cannot tell two declarations on one line apart
/// and a ledger with sites missing. And the host's own computation changed:
/// rules now rate the edges a v6 record holds as name matches. The fingerprint
/// gained the resolver's revision in the same unit, which alone already makes
/// every v6 header unequal; the bump states the format change for what it is
/// rather than leaning on that. From here on a changed rule or a granted right
/// needs no bump: the resolver revision in the fingerprint carries it (KTD15).
///
/// The index pairing (plan A, U9) is NOT a bump, checked rather than assumed.
/// `SiteRecord` gained a serialized field, `index`, but it is skipped while
/// `None`, `flush_usages` always writes `None`, and only the pairing fills it
/// — in the scip extractor's run, outside every cached phase. A cached record
/// therefore decodes and re-serializes byte for byte as before, and the
/// declaration-name positions the pairing reads were cached since v7.
///
/// v8: the host's own computation changed again in ways no rule revision
/// names, on the same unreleased branch. A tie between backed candidates now
/// rates its edges `name-match` instead of the backing rule, and a site's
/// owner is picked by line AND column where declarations only meet on its
/// line (`SymbolTable::owner`) — both change what a `languages-usages`
/// record holds for unchanged input. The rebuilt C# component changes its
/// digest in the same unit, which alone would make every v7 header unequal;
/// the bump states the change for what it is rather than leaning on that.
///
/// v9: the `languages-usages` records stop holding site records and hold the
/// two ledger counters over the file's sites instead (`graph::SiteCounts`).
/// With a right to delete in force every C# site is judged, so a record held
/// every candidate of every site with its full trace — 6.5 GB over the
/// reference repository, for records a run without `--sites` drops unread.
/// Only a run that keeps no records reads or writes a usage record now. The v3
/// story once more: a v8 record decodes cleanly, its `sites` ignored and its
/// counters absent, and served as a hit it would report every site as neither
/// over the ceiling nor thinned by a deletion.
///
/// v10 (plan B, the one bump for the whole of area B — later units on that
/// branch extend this paragraph rather than bump again): the `external`
/// records gain a [`crate::graph::FactKind::DbAccess`] fact per database
/// access a SQL literal makes (U2). Host code computes them, so no
/// fingerprint field moves; a v9 record decodes cleanly with none, and served
/// as a hit it would leave every method of an unchanged file saying it reads,
/// writes and calls nothing — the v3 story once more. Within the same v10
/// (U3, U4): the records also gain [`crate::graph::FactKind::Anchor`],
/// `AnchorSkipped` and `AnchorsDropped` facts, the lexer reads C# raw and
/// interpolated literals and C and C++ files, and the secret filter withholds
/// more — all host code again, all of it missing from a v9 record served as a
/// hit. And within it too (U5): a C# file's record gains an
/// [`crate::graph::FactKind::EnumDecision`] per place its code decides by an
/// enum's value and an `EnumDecisionsUnmatched` count — checked against the
/// whole graph, which is safe in a per-file record only because the run
/// fingerprint covers every tracked blob. And within it once more (the
/// review of plan B): the secret filter withholds a quoted key (`"password":
/// "…"`), a markup element or setting, a `#define` of a secret name and the
/// literals of a key written one per line, and cuts a key-shaped word out of
/// a multi-word literal's anchor; a C or C++ literal continued over a CRLF
/// break is read whole. And within it after the first cross-model review: a
/// passphrase of several words assigned to a secret's name and a value
/// written on the line below its assignment are withheld; a JS/TS template
/// with no `${…}` hole is read like any string; a common-table expression's
/// `SELECT` reads its tables; a C# decision written through an alias of
/// another name is the enum's; and a `languages-declarations` record's doc
/// comment loses its key-shaped words, each cut leaving an `AnchorSkipped`.
/// And after the second: a passphrase under a quoted key (`{ "password":
/// "…" }`, `c["apiKey"] = "…"`) and a value at column zero below its
/// assignment are withheld; a key written right between doc tags
/// (`<c>KEY</c>`) is cut with the tags kept; a JS/TS template reads its
/// backslash escapes; and a statement reading the table it changes — in a
/// subquery or under another alias — reads it and writes it, its target
/// being the occurrence the verb names, not every one of its name.
/// A record written by an earlier commit of the same branch can still hold
/// the anchors these withhold — v10 is unreleased, and the one bump for
/// area B stands.
///
/// The second layer (plan 2a, U4) is NOT a bump, checked rather than
/// assumed. Its records live in a phase of their own, `languages-bodies`,
/// which no build before it wrote or reads; each is tagged with the digest
/// of the body component that made it and with the version of the host's
/// redaction of its texts ([`bodies::BODY_REDACTION_VERSION`], through
/// `CacheStore::derived`), so a rebuilt component or a changed redaction
/// misses exactly those records. The first layer's records are written as
/// before, and a run without `--detail-level` writes byte for byte what it
/// did — so no record of any phase could be served stale by this change.
///
/// v11 (the reviews of step 2c): the host now rewrites what a plugin hands
/// it for the first layer before the per-file record is written — a symbol
/// key whose local part the secret filter withholds becomes an opaque key
/// unique to its file, and a withheld name no longer resolves a reference.
/// The plugin's digest does not change for a third-party plugin, so a record
/// written before could still hold the raw key; the bump retires every one.
///
/// v12 (R-20): the host also withholds, in a member's signature, a literal
/// the first layer withholds by the name it is given (`const string
/// SmtpPassword = "…"`) — before the per-file record is written, and no
/// plugin's digest moves with it, so a v11 record would serve the value.
/// The same bump retires what a third-party plugin's v11 record holds from
/// before `66f1e99` (a source key a reference by name resolved to, stored
/// before that fix), which went unbumped on the branch of step 2c.
///
/// v13 (R-42): an opaque key's mark of its file grows from eight hex digits
/// to sixteen. A v12 record holds the short mark, and served beside fresh
/// records it would give one run keys of both widths.
pub(crate) const CACHE_VERSION: u32 = 13;

/// Errors an extractor can raise. All readable; never a panic.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("failed to open git repository at {path}: {source}")]
    OpenRepo {
        path: PathBuf,
        #[source]
        source: git2::Error,
    },
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    /// `root` is not something that can be indexed at all — it does not exist,
    /// or it is a file rather than a tree. Distinct from [`ExtractError::OpenRepo`]
    /// on purpose: since U15 a directory that is not a git repository is a
    /// perfectly good source, so "not a repository" stopped being a reason to
    /// refuse and "not a directory" became the reason that is left.
    #[error("{path} is not a directory that can be indexed")]
    NotASource { path: PathBuf },
    /// `path` has no repository of its own but sits INSIDE one. Refused rather
    /// than walked — see [`Repo::open`] for why guessing here would be unsafe.
    ///
    /// The message names the enclosing repository and stops there. An earlier
    /// version prescribed "run against `{workdir}` instead", which is advice
    /// that can be wrong in exactly the case an operator is most likely to hit:
    /// if the repository ignores this subtree, the prescribed run does not cover
    /// it. Saying what is true and what is not yet supported beats sending
    /// someone to a run that quietly answers a different question. The explicit
    /// `--source=directory` opt-in that would answer it is U21's, which is
    /// already building the one place a source gets resolved.
    #[error(
        "{path} has no repository of its own but is inside the one at {workdir}. \
         Index that repository instead — but if it ignores this subtree, the run \
         will not cover it, and indexing a subtree on its own is not yet supported."
    )]
    InsideRepository { path: PathBuf, workdir: PathBuf },
    /// Whether `path` sits inside a repository could not be determined. Stopping
    /// is the only safe answer: the alternative reading — "no enclosing
    /// repository" — is the one that walks a repository subtree as a flat tree
    /// and carries root-ignored files into the corpus.
    ///
    /// The realistic cause is libgit2's ownership check
    /// (`repository path ... is not owned by current user`), which fires while
    /// searching upward and is a machine-configuration problem
    /// (`safe.directory`), not a property of the tree. Saying so beats indexing
    /// something else.
    #[error("cannot tell whether {path} is inside a git repository: {source}")]
    UndeterminedEnclosure {
        path: PathBuf,
        #[source]
        source: git2::Error,
    },
    /// A non-git source could not read what it was enumerating or reading. The
    /// git source cannot produce this — it reads objects, not files.
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The bytes a source returned for an already-enumerated file no longer hash
    /// to the blob id enumeration recorded for it — the file changed under the
    /// run.
    ///
    /// Only a live-filesystem source can produce this; a git blob is immutable,
    /// so the git source cannot. It is an error rather than a silent
    /// re-hash because the whole per-file cache is keyed on the enumerated id
    /// (`walk_files`): parsing the new bytes and storing the facts under the old
    /// id would write a record that a later run serves as a hit for content it
    /// never described. Every caller treats this the way it treats an unreadable
    /// file — skip that one file, keep the run (E4).
    #[error(
        "{path} changed while the run was reading it (enumerated as {expected}, now {actual})"
    )]
    ContentChanged {
        path: PathBuf,
        expected: git2::Oid,
        actual: git2::Oid,
    },
    #[error("cannot read the SCIP index at {path}: {source}")]
    ScipRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// An index that exists but cannot be parsed fails the run. Degrading to a
    /// heuristic corpus instead would be indistinguishable from a project with no
    /// cross-references at all, and the caller asked for that index by name.
    #[error("the SCIP index at {path} is not a valid index: {source}")]
    ScipFormat {
        path: PathBuf,
        #[source]
        source: protobuf::Error,
    },
}

/// A handle to the tree under analysis: its root plus the source it reads
/// through. Every extractor reads through this.
///
/// The name predates U15 and is kept deliberately. `Repo` is the first parameter
/// of [`Extractor::run`], implemented by seven extractors, consumed by
/// `walk_files` and reached by roughly twenty integration tests — a rename, or a
/// change to `&dyn Source`, would ripple through all of them to say nothing new.
/// What changed is what it holds: a `Source` rather than a git
/// handle, so a tree with no commits is a different implementation behind the
/// same type instead of an unindexable one.
pub struct Repo {
    /// Tree root as supplied (used for I/O).
    pub root: PathBuf,
    /// Where files and their bytes come from — git, a plain directory, or (U21)
    /// a declared component standing in this seam.
    ///
    /// Private, unlike the `git: git2::Repository` it replaces. That field was
    /// public and, as it turned out, read by nothing outside this `impl`: making
    /// the replacement private costs no caller anything and stops a future one
    /// reaching past the trait for git-only behavior the other implementations
    /// cannot offer.
    source: Box<dyn source::Source>,
    /// The enumeration, taken **once** per `Repo` and reused by every caller.
    ///
    /// Under git this was only an optimization: the index is an immutable
    /// in-memory list, so asking it eight times gave eight identical answers.
    /// Under the directory source it is a correctness
    /// requirement. A walk of a live filesystem is not a pure function — a run
    /// takes minutes, and each of the ~8 callers (the fingerprint plus every
    /// default extractor) would otherwise take its own snapshot at its own
    /// moment. Two extractors could then describe different trees inside one
    /// corpus, and the coverage artifact would count files no fact mentions.
    /// Taking the snapshot once restores the single-consistent-tree property the
    /// git index provided for free, and incidentally stops re-reading and
    /// re-hashing every file eight times.
    ///
    /// [`std::cell::OnceCell`], not `OnceLock`: `Repo` is not `Sync` today
    /// (`git2::Repository` is `Send` but not `Sync`, and [`Source`](source::Source)
    /// carries no `Sync` bound for that reason), and every read of this cell is
    /// on the serial path. This is the field that would have to become a
    /// `OnceLock` first if `Repo` is ever made `Sync`.
    files: std::cell::OnceCell<Vec<TrackedFile>>,
}

/// A tracked file's two identities, deliberately kept distinct.
///
/// - `key` is the byte-preserving **graph key** ([`path_key_from_bytes`]): a
///   literal `%` is percent-escaped, so `a%main.rs` becomes `a%25main.rs`. This
///   is the identity filetree/git already use for File nodes and provenance.
/// - `path` is the **real** source-relative path as the source spells it on
///   disk — never the escaped key.
///
/// They diverge only for a path containing `%` or invalid UTF-8; for the common
/// case they are equal. Content extractors (tree-sitter, metrics, manifest-deps)
/// read the file *bytes* from [`Repo::blob_bytes`], use `path` **only** for
/// language detection by extension/filename, and emit graph nodes/facts under
/// `key`. `path` must not be used for byte I/O: a `%`-escaped key names a
/// non-existent file, and which bytes a given source considers authoritative is
/// the source's business, not the extractor's.
#[derive(Clone)]
pub struct TrackedFile {
    pub key: String,
    pub path: PathBuf,
    /// The file's mode in git's vocabulary. `0o100644`/`0o100755` for a regular
    /// blob, `0o120000` for a symlink. A source with no git index of its own
    /// reports the documented constants it would have had. Gitlinks (`0o160000`) never appear here — the
    /// git source filters them out at enumeration. Retained so a future
    /// explicit-submodule feature can re-derive from the index without changing
    /// this type.
    pub mode: u32,
    /// The file's content object id — a git blob hash over the bytes the source
    /// considers stored, whether it read them from an index or computed them
    /// from disk. Identical content therefore gets an identical id from either
    /// source, which is what lets one cache serve a tree indexed both ways. For
    /// a symlink the hashed bytes are the link *target path text*, not the
    /// pointed-to file, so content extractors skip symlinks (see `is_symlink`).
    pub blob_id: git2::Oid,
    /// Derived from `mode`: a symlink (`0o120000`). Its blob is the target path
    /// text rather than source content, so content extractors skip it (it still
    /// becomes a `File` node with git facts, like any non-source tracked file).
    pub is_symlink: bool,
}

/// Reconstruct the real on-disk relative path from a git index entry's raw path
/// bytes. Git stores paths as `/`-separated byte strings. On unix the bytes are
/// the exact `OsStr` (any byte sequence is a legal path); on windows git paths
/// are UTF-8, so a lossy decode is exact for real paths and never panics.
#[cfg(unix)]
pub(crate) fn disk_path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}
#[cfg(not(unix))]
pub(crate) fn disk_path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// The working directory of the repository `root` is inside, when `root` is not
/// that repository's own root. `Ok(None)` means there is certainly none.
///
/// `Repository::discover` is the upward search `Repository::open` deliberately
/// is not. Only its `NotFound` is read as "no enclosing repository"; **every
/// other error refuses the run rather than being swallowed**, and that
/// distinction is the whole value of this function. Written the obvious way —
/// `discover(root).ok()?` — any failure reads as an absence, and the failure
/// this project's own checkout actually produces is libgit2's ownership check
/// (`repository path ... is not owned by current user`), which fires *while
/// searching upward*, which is to say exactly when there IS a repository above.
/// That version passed its test on a temp-dir fixture and still walked
/// `<repo>/src` as a flat tree.
///
/// A bare repository has no working directory and so cannot enclose a tree. The
/// paths are compared canonicalized, because `root` arrives from a command line
/// and may be relative, `..`-laden, or spelled in a case or short form that
/// libgit2's answer is not; a canonicalization that fails falls back to the raw
/// comparison.
///
/// The decision itself lives in [`enclosing_from`], which takes the `discover`
/// result rather than producing it, so both branches are testable without a
/// machine that happens to reproduce libgit2's ownership refusal.
fn enclosing_repository(root: &Path) -> Result<Option<PathBuf>, ExtractError> {
    enclosing_from(git2::Repository::discover(root), root)
}

/// The half of [`enclosing_repository`] that decides what an answer *means*,
/// split from the half that goes to disk.
///
/// The split exists so the classification can be tested without a machine that
/// happens to reproduce the failure. The only test that ever exercised the
/// non-`NotFound` branch ran against this crate's own checkout and depended on
/// libgit2 refusing it — true on the machine the bug was found on, not
/// something a suite can rely on, and silently a no-op anywhere else. Handing
/// the `discover` result in means both branches can be asked for directly with
/// `git2::Error::new(ErrorCode::NotFound, …)` and its opposite.
fn enclosing_from(
    discovered: Result<git2::Repository, git2::Error>,
    root: &Path,
) -> Result<Option<PathBuf>, ExtractError> {
    let discovered = match discovered {
        Ok(discovered) => discovered,
        Err(err) if err.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(source) => {
            return Err(ExtractError::UndeterminedEnclosure {
                path: root.to_path_buf(),
                source,
            })
        }
    };
    let Some(workdir) = discovered.workdir().map(Path::to_path_buf) else {
        return Ok(None);
    };
    let same = match (std::fs::canonicalize(root), std::fs::canonicalize(&workdir)) {
        (Ok(a), Ok(b)) => a == b,
        _ => root == workdir,
    };
    Ok((!same).then_some(workdir))
}

impl Repo {
    /// Open the tree at `root`, preferring git and falling back to a plain
    /// directory walk (U15, R15).
    ///
    /// Git is tried first and always wins where it is available, because its
    /// index is a better answer than any walk: one canonical file list, blob ids
    /// for free, and bytes that do not move under a dirty working tree (KTD10).
    /// The fallback exists for the trees that have no commits — a decompiled
    /// project, an unpacked archive — which previously could only be indexed
    /// after a `git init` the operator had no reason to want.
    ///
    /// The fallback is narrow on purpose, in two ways.
    ///
    /// Only `NotFound` — libgit2's "there is no repository here" — degrades to a
    /// directory. Every other git error (a corrupt repository, a permission
    /// failure, an unreadable config) still fails as
    /// [`ExtractError::OpenRepo`]: those name a repository that exists and is
    /// broken, and silently indexing the working tree instead would answer a
    /// question nobody asked, from bytes the operator did not vouch for.
    ///
    /// And a directory that sits INSIDE a repository without being its root is
    /// refused, not walked ([`ExtractError::InsideRepository`]). `git2`'s `open`
    /// does not search upward, so `<repo>/src` reports `NotFound` exactly as a
    /// standalone tree does — and taking the fallback there would be wrong three
    /// times over. The walk is rooted at `src`, so the repository's own
    /// `.gitignore` is never consulted: `<repo>/src/.env`, ignored at the root
    /// and full of credentials, would enter the corpus, which is precisely the
    /// R22 hazard the ignore-file handling exists to prevent. `repo_revision`
    /// would report `null` for a tree that is in fact at a known commit. And
    /// every graph key would be relative to `src`, silently disagreeing with the
    /// keys a full run of the same repository produces.
    ///
    /// Indexing a subtree of a repository is a coherent feature and this is not
    /// it — which revision it reports and what its keys are relative to are real
    /// design questions. Refusing with the repository's path in the message
    /// leaves the operator one obvious move; a wrong guess would leave them an
    /// artifact that looks right.
    pub fn open(root: &Path) -> Result<Self, ExtractError> {
        if !root.is_dir() {
            return Err(ExtractError::NotASource {
                path: root.to_path_buf(),
            });
        }
        let source: Box<dyn source::Source> = match git2::Repository::open(root) {
            Ok(git) => Box::new(source::GitSource { git }),
            Err(err) if err.code() == git2::ErrorCode::NotFound => {
                if let Some(workdir) = enclosing_repository(root)? {
                    return Err(ExtractError::InsideRepository {
                        path: root.to_path_buf(),
                        workdir,
                    });
                }
                Box::new(directory::DirectorySource::new(root))
            }
            Err(source) => {
                return Err(ExtractError::OpenRepo {
                    path: root.to_path_buf(),
                    source,
                })
            }
        };
        Ok(Repo {
            root: root.to_path_buf(),
            source,
            files: std::cell::OnceCell::new(),
        })
    }

    /// The run's one enumeration: taken from the source on first use, ordered,
    /// and kept for every later caller.
    ///
    /// The ordering is here, not in the [`Source`](source::Source)
    /// implementations, and that placement is the point. Determinism (KTD7) is
    /// the property the whole artifact rests on, and every corpus byte depends on
    /// this order; a source that returned files in filesystem order — which a
    /// directory walk does — would break it silently, and the byte-identity gate
    /// would only catch it if the machine that ran it happened to disagree with
    /// the machine that wrote it. One choke point makes forgetting impossible
    /// instead of merely unlikely.
    ///
    /// The comparison is **total**: by key, then by path. Sorting on the key
    /// alone leaves entries that share a key in whatever order the source
    /// produced them, and `dedup_by` then keeps the first — which for a
    /// filesystem walk is readdir order, a per-machine fact. Two distinct paths
    /// can share a key: [`path_key_from_bytes`] folds `\` to `/`, so on unix a
    /// file literally named `a\b` collides with `a/b`. Rare, but the survivor
    /// must not be the machine's choice. A collision still drops one file
    /// silently; whether it should instead refuse the run is a design question
    /// left open, and one no bundled source can currently reach on Windows.
    ///
    /// Caching it is a correctness requirement, not an optimization — see the
    /// `files` field.
    fn files(&self) -> Result<&[TrackedFile], ExtractError> {
        if let Some(files) = self.files.get() {
            return Ok(files);
        }
        let mut files = self.source.tracked_files_with_paths()?;
        files.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.path.cmp(&b.path)));
        files.dedup_by(|a, b| a.key == b.key);
        Ok(self.files.get_or_init(|| files))
    }

    /// The tracked files as [`TrackedFile`]s (graph key + real disk path),
    /// **sorted and deduped by key**.
    ///
    /// Content extractors use this so they read the real path while keying facts
    /// under the byte-preserving graph key (see [`TrackedFile`]). Every caller
    /// receives a copy of the same enumeration — see the `files` field.
    pub fn tracked_files_with_paths(&self) -> Result<Vec<TrackedFile>, ExtractError> {
        Ok(self.files()?.to_vec())
    }

    /// The bytes of a tracked file, checked against the id it was enumerated
    /// under.
    ///
    /// The check is here rather than in any one source, for the same reason the
    /// ordering is: it is a property of the run, so no implementation can forget
    /// it. `tf.blob_id` is what the per-file cache is keyed on, and a source
    /// reading a live filesystem can hand back bytes that no longer match it — a
    /// file rewritten between enumeration and read. Storing facts parsed from
    /// those bytes under the enumerated id would poison the cache for every later
    /// run. Returning [`ExtractError::ContentChanged`] instead drops that one
    /// file the way an unreadable one is dropped (E4).
    ///
    /// For the git source the check is redundant — a blob is
    /// immutable and libgit2 verifies it on read — and costs one hash per file.
    /// It is paid anyway: an invariant enforced for every source is worth more
    /// than the microseconds, and a per-source exemption would be the kind of
    /// thing U21's component source inherits by accident.
    ///
    /// A symlink's bytes are its target path text rather than source content, so
    /// content extractors skip symlinks before calling this. Nothing here ever
    /// panics.
    pub fn blob_bytes(&self, tf: &TrackedFile) -> Result<Vec<u8>, ExtractError> {
        let bytes = self.source.blob_bytes(tf)?;
        let actual = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)?;
        if actual != tf.blob_id {
            return Err(ExtractError::ContentChanged {
                path: tf.path.clone(),
                expected: tf.blob_id,
                actual,
            });
        }
        Ok(bytes)
    }

    /// The revision this tree is a snapshot of, as a full hex id (R6), or `None`
    /// when it is not a snapshot of one — a repository with no commits yet, or a
    /// source with no commits at all. The coverage artifact reports the absence
    /// rather than omitting the field.
    pub fn head_revision(&self) -> Option<String> {
        self.source.head_revision()
    }

    /// The tracked file **graph keys**, sorted and deduped — gitlink-free for a
    /// git source, since it derives from the same enumeration as
    /// [`Repo::tracked_files_with_paths`]. Consumers that only need keys (e.g.
    /// test-pairing, which classifies by filename stem) never touch the disk, so
    /// the key alone is enough.
    ///
    /// Projected from the one memoized enumeration rather than asked of the
    /// source separately.
    /// A keys-only question used to be routed to the source on the theory that an
    /// implementation might answer it more cheaply; once the enumeration is taken
    /// once per run, asking again is not cheaper but *wrong* — it would be a
    /// second snapshot of a tree that may have moved.
    pub fn tracked_files(&self) -> Result<Vec<String>, ExtractError> {
        Ok(self.files()?.iter().map(|tf| tf.key.clone()).collect())
    }
}

/// The extractor contract (KTD5).
pub trait Extractor {
    /// Stable identifier, surfaced in provenance.
    fn id(&self) -> ExtractorId;
    /// Fact kinds this extractor may emit (declared, for documentation/merge).
    fn emitted_kinds(&self) -> &'static [FactKind];
    /// Precedence in a scalar conflict — higher wins (E6).
    fn trust_rank(&self) -> u8;
    /// Every extractor id this extractor writes into provenance, with its
    /// rank. One pair for an ordinary extractor; the language extractor writes
    /// each plugin's own id, and merge must know every one of them.
    fn provenance_ids(&self) -> Vec<(ExtractorId, u8)> {
        vec![(self.id(), self.trust_rank())]
    }
    /// Populate the graph from the repository.
    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError>;
}

/// The run tunables an extractor needs from [`crate::config::Config`].
///
/// A narrow struct rather than `Config` itself: an extractor has no business
/// knowing where the corpus is written or how links are rendered, and `extract`
/// must not start depending on the whole configuration surface. It carries a
/// [`Default`] so every existing call site — the enrich path, the tests — keeps
/// working on the documented defaults without threading configuration it does not
/// care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractOptions {
    /// Most name candidates a heuristic identifier resolution may still turn into
    /// named edges (R23) — see [`crate::config::DEFAULT_AMBIGUITY_CEILING`].
    pub ambiguity_ceiling: usize,
    /// Path to an externally produced SCIP index, when the caller named one (R7).
    ///
    /// Deliberately explicit: there is no conventional location RA looks in. A
    /// stale `index.scip` left lying in a repository must never be able to change
    /// a run's output by accident, and `resolved` facts are exactly the ones that
    /// license absence claims (R20).
    pub scip_index: Option<PathBuf>,
    /// Extra token prefixes the R22 filter should treat as a credential, on top
    /// of the built-in public ones (`sk-`, `ghp_`, `AKIA`, …).
    ///
    /// For a company token shape that no public list knows. Additive on purpose:
    /// naming your own must never turn the public ones off.
    pub secret_prefixes: Vec<String>,
    /// The run's language registry (U4, R26).
    ///
    /// It lives here rather than as a parameter on `default_extractors_with`
    /// and `run_all_with_cached` because languages will arrive from the plugin
    /// set, and R30 already puts that set on these options so every entry point
    /// that builds a graph — `run_enrich` included — narrates the same fact set
    /// the corpus was built from. One field now, instead of a parameter on
    /// every function in the chain and then a second migration later.
    pub languages: languages::Languages,
    /// The language plugins of this run, in precedence order (U8): the ones
    /// the operator declared, then the bundled ones they did not replace.
    pub plugins: crate::plugin::PluginSet,
    /// The rights to delete the shared resolver applies (plan A, KTD11) —
    /// [`RightsTable::granted`] unless a library caller hands it another.
    pub rights: RightsTable,
    /// Keep every usage-site record in the graph's ledger (plan A, KTD7), not
    /// only the counters every run keeps — for `--sites`.
    pub site_records: bool,
    /// Run the resolver's rules at every site with context, also one over the
    /// ambiguity ceiling that no rule with a right could thin — for `ra
    /// measure` (plan A, U11). A right granted later makes the rules run at
    /// such a site, so the measurement that grants it must have judged them
    /// there. The graph is the same; the site records carry the verdicts.
    pub trace_over_ceiling: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        ExtractOptions {
            ambiguity_ceiling: crate::config::DEFAULT_AMBIGUITY_CEILING,
            scip_index: None,
            secret_prefixes: Vec::new(),
            languages: languages::Languages::bundled(),
            plugins: crate::plugin::PluginSet::bundled(),
            rights: RightsTable::granted(),
            site_records: false,
            trace_over_ceiling: false,
        }
    }
}

/// The Phase A extractor floor on the default options.
pub fn default_extractors() -> Vec<Box<dyn Extractor>> {
    default_extractors_with(&ExtractOptions::default(), None)
}

/// The Phase A extractor floor, in run order. filetree runs first so that every
/// later extractor's facts land on File nodes that already exist.
///
/// (This used to say "filetree runs before git". There is no git extractor in
/// this build and there has not been one — no `FactKind` carries an author, a
/// date or a churn count. Corrected in U15 rather than left describing a
/// component a reader would go looking for.)
///
/// `cache`, when present, is cloned into the two extractors whose `run` calls
/// `walk_files` — `treesitter::Languages` and `external::External` — exactly
/// the way `secret_prefixes` already is: a value threaded into just the
/// extractors that need it, set once at construction, rather than a
/// parameter widening every extractor's `run` signature. `CacheStore` is a
/// cheap `Clone` — a `PathBuf`, a digest string, the run's [`Ranks`], and a
/// fresh empty created-dirs set — so each extractor gets its own owned handle
/// instead of a borrow, keeping `Extractor` object-safe with no new lifetime.
/// The five extractors that never call `walk_files` — `FileTree`,
/// `ManifestDeps`, `Metrics`, `Scip`, `TestPairing` — never see a cache handle
/// at all (U4).
pub(crate) fn default_extractors_with(
    opts: &ExtractOptions,
    cache: Option<&CacheStore>,
) -> Vec<Box<dyn Extractor>> {
    default_extractors_handing_names(opts, cache, None)
}

/// [`default_extractors_with`], with the languages extractor handing its
/// declaration-name positions to `names` instead of dropping them — for a run
/// with the second layer (plan 2a, U4), whose pairing reads them after every
/// extractor ran. `None` is exactly [`default_extractors_with`].
pub(crate) fn default_extractors_handing_names(
    opts: &ExtractOptions,
    cache: Option<&CacheStore>,
    names: Option<NamesHandoff>,
) -> Vec<Box<dyn Extractor>> {
    // Compiled once and cloned into each extractor that redacts, rather than
    // rebuilt per file: the regex is the same for the whole run.
    let secret_prefixes = external::SecretPrefixes::new(&opts.secret_prefixes);
    // Filled by the languages extractor, read by `external` after it (U5).
    let usings_handoff = resolve::project::UsingsHandoff::default();
    vec![
        Box::new(filetree::FileTree),
        Box::new(treesitter::Languages {
            secret_prefixes: secret_prefixes.clone(),
            secret_prefix_list: opts.secret_prefixes.clone(),
            ambiguity_ceiling: opts.ambiguity_ceiling,
            cache: cache.cloned(),
            plugins: opts.plugins.clone(),
            rights: opts.rights.clone(),
            keep_sites: opts.site_records,
            pair_with_index: opts.scip_index.is_some(),
            trace_over_ceiling: opts.trace_over_ceiling,
            usings_handoff: usings_handoff.clone(),
            names_handoff: names,
        }),
        Box::new(manifest_deps::ManifestDeps),
        Box::new(metrics::Metrics {
            languages: opts.languages.clone(),
        }),
        // A directory's README paragraph, for the module card's summary.
        Box::new(readme::Readme),
        // external runs after BOTH tree-sitter and manifest-deps, and needs both:
        // the Symbol nodes' declaration ranges tell it which member encloses a
        // usage site, and the `dep:` nodes are its authority on which imports leave
        // the repository at all (U5).
        Box::new(external::External {
            secret_prefixes: secret_prefixes.clone(),
            cache: cache.cloned(),
            project_usings: usings_handoff,
        }),
        // scip runs after tree-sitter because it attaches to the Symbol nodes the
        // grammar already emitted and checks the index against their declaration
        // ranges — it creates no node of its own (KTD3).
        Box::new(scip::Scip {
            index_path: opts.scip_index.clone(),
            secret_prefixes,
        }),
        // test-pairing runs last: its coverage fact reads the tree-sitter
        // structural facts every earlier extractor has already emitted.
        Box::new(test_pairing::TestPairing {
            languages: opts.languages.clone(),
        }),
    ]
}

/// The run's registry (KTD17): **one** structure answering both extractor
/// identity and language identity, resolved once per run and threaded.
///
/// The plan's U3 step 2 asks for exactly this and names the reason two separate
/// registries would be worse: two threading efforts through overlapping call
/// paths, and two chances for the loaded plugin set and the registered language
/// set to disagree. That disagreement is not hypothetical once a plugin
/// supplies both an extractor id AND a language (U8, U9) — it is the same
/// declaration, and nothing but a shared structure makes the two views provably
/// come from it.
///
/// The two views are deliberately asymmetric in how they are handed out.
/// `graph` receives only [`Registry::ranks`], the id-and-rank view, which is
/// what keeps `graph` free of any dependency on `extract` (no module cycle).
/// [`Registry::languages`] goes to the coverage projection.
///
/// Languages are read from `opts` rather than resolved here, because the
/// extractors themselves need them at construction (they classify by language)
/// and `ExtractOptions` is what already reaches every entry point that builds a
/// graph, `run_enrich` included (R30). Taking a COPY here rather than a second
/// source is the whole point: `for_run` is the single place the two views are
/// paired, so a caller cannot pair them differently.
pub struct Registry {
    ranks: Ranks,
    languages: languages::Languages,
}

impl Registry {
    /// Resolve the registry for a run configured by `opts`.
    ///
    /// Constructing it instantiates every extractor, so resolve it **once** per
    /// run and thread it — under plugins this is what stops every component
    /// being loaded twice.
    pub fn for_run(opts: &ExtractOptions) -> Self {
        Registry {
            ranks: Ranks::new(
                default_extractors_with(opts, None)
                    .iter()
                    .flat_map(|e| e.provenance_ids())
                    .collect(),
            ),
            languages: opts.languages.clone(),
        }
    }

    /// The `id → trust_rank` view — the only view `graph` ever sees.
    pub fn ranks(&self) -> &Ranks {
        &self.ranks
    }

    /// The extension → language view the coverage projection reports from.
    pub fn languages(&self) -> &languages::Languages {
        &self.languages
    }
}

/// The `id → trust_rank` view for the DEFAULT extractor set (KTD-B5, KTD-B8).
///
/// Prefer [`Registry::for_run`] on any path that has an [`ExtractOptions`]:
/// this one hardcodes the defaults, so it cannot see a plugin. It remains for
/// the callers that genuinely run on defaults — the enrich path and the tests
/// that build a graph without options.
///
/// Built here, in the layer that already depends on `graph`, and handed to
/// `graph::merge` — so `graph` never depends on `extract` (no module cycle). A
/// `Fact` carries only its extractor id in provenance, and `run_all` drops the
/// extractor objects, so merge cannot read `trust_rank()` off a live instance.
///
/// Returns [`Ranks`] rather than the bare map it used to (U3): the unknown-id
/// decision belongs on a type with a doc, not on an `unwrap_or` at the point of
/// use. Constructing it instantiates every extractor, so a caller resolves it
/// **once per run** and threads it (KTD17) — the reason the cache no longer
/// reaches for it while decoding.
pub fn trust_ranks() -> Ranks {
    Ranks::new(
        default_extractors()
            .iter()
            .flat_map(|e| e.provenance_ids())
            .collect(),
    )
}

/// Run every default extractor against the repo, in order, on the default
/// [`ExtractOptions`] and no cache.
pub fn run_all(repo: &Repo, graph: &mut Graph) -> Result<(), ExtractError> {
    run_all_with(
        repo,
        graph,
        &ExtractOptions::default(),
        &crate::progress::Silent,
    )
}

/// Run every default extractor against the repo, in order, with the run's own
/// resolved options — what `generate` calls so a user-supplied `--ambiguity-ceiling`
/// actually reaches the tree-sitter extractor. Never consults a cache; use
/// this from outside the crate (every integration test under `tests/` that
/// exercises extraction end to end without caching calls this).
pub fn run_all_with(
    repo: &Repo,
    graph: &mut Graph,
    opts: &ExtractOptions,
    progress: &dyn crate::progress::Progress,
) -> Result<(), ExtractError> {
    run_all_with_cached(repo, graph, opts, progress, None)
}

/// [`run_all_with`], plus an optional cache (review #3: `CacheStore` and this
/// cache-aware entry point stay crate-internal — nothing outside `extract`
/// needs to name `CacheStore` or pass one in; every external caller wants the
/// plain, always-uncached `run_all_with` above).
///
/// `cache`, when `Some`, is handed to [`default_extractors_with`], which
/// threads it into the two extractors that call `walk_files`. This function
/// itself makes no decision about validity — that is entirely
/// `cache::activate`'s job, called once by `generate_reporting` (U5) before
/// this function ever runs.
pub(crate) fn run_all_with_cached(
    repo: &Repo,
    graph: &mut Graph,
    opts: &ExtractOptions,
    progress: &dyn crate::progress::Progress,
    cache: Option<&CacheStore>,
) -> Result<(), ExtractError> {
    run_all_handing_names(repo, graph, opts, progress, cache, None)
}

/// [`run_all_with_cached`], handing the declaration-name positions to
/// `names` for the second layer ([`default_extractors_handing_names`]).
pub(crate) fn run_all_handing_names(
    repo: &Repo,
    graph: &mut Graph,
    opts: &ExtractOptions,
    progress: &dyn crate::progress::Progress,
    cache: Option<&CacheStore>,
    names: Option<&NamesHandoff>,
) -> Result<(), ExtractError> {
    for extractor in default_extractors_handing_names(opts, cache, names.cloned()) {
        // The extractor's own id is the phase name — one vocabulary for the
        // operator watching, for the coverage artifact and for the trust-rank
        // table, so a slow phase can be named in a bug report without a lookup.
        progress.phase(&extractor.id(), None);
        extractor.run(repo, graph, progress)?;
        progress.phase_done();
    }
    Ok(())
}

/// The repo-relative parent directory key for a `/`-normalized path.
/// Top-level entries have the root module `.` as their parent.
pub(crate) fn parent_key(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, _)) => parent.to_string(),
        None => ".".to_string(),
    }
}

/// Convert git's raw path bytes into a stable, byte-preserving node key.
///
/// Git stores paths as byte strings, `/`-separated even on Windows. Valid UTF-8
/// with no `%` passes through unchanged — the overwhelmingly common case, so
/// existing keys and snapshots are untouched. Anything else (invalid UTF-8, or
/// a literal `%`, our escape marker) is percent-escaped byte by byte. That
/// makes the mapping injective: two distinct byte sequences can never collapse
/// onto one key. This closes the `from_utf8_lossy` collision where different
/// non-UTF-8 paths both decoded to the same `U+FFFD` string and one file
/// silently vanished from the corpus. `\` folds to `/` to match [`NodeKey`].
pub(crate) fn path_key_from_bytes(bytes: &[u8]) -> String {
    if let Some(s) = std::str::from_utf8(bytes).ok().filter(|s| !s.contains('%')) {
        return s.replace('\\', "/");
    }
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' | b'/' => out.push('/'),
            // Printable ASCII passes through, except `%` (0x25), which must be
            // escaped so it can't be confused with an escape sequence.
            0x20..=0x24 | 0x26..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// KTD17: both views come from one resolution of one structure.
    ///
    /// The failure this guards is not a crash — it is a run whose merge ranks
    /// describe one extractor set while coverage reports the languages of
    /// another. Nothing downstream can detect that; the artifact just quietly
    /// describes a run that never happened.
    #[test]
    fn the_registry_pairs_the_two_views_it_was_resolved_from() {
        let mut languages = super::languages::Languages::bundled();
        languages
            .register(
                "ktd",
                super::languages::LanguageEntry {
                    name: "otevrenost".to_string(),
                    loc: None,
                },
            )
            .expect("the run's registry accepts a plugin language");
        let opts = super::ExtractOptions {
            languages: languages.clone(),
            ..super::ExtractOptions::default()
        };

        let registry = super::Registry::for_run(&opts);

        assert_eq!(
            registry.languages(),
            &languages,
            "the language view must be the one the run was configured with, \
             not a second `bundled()` that silently drops every plugin"
        );
        for id in super::ids::all() {
            assert!(
                registry.ranks().rank(&id) > crate::graph::UNKNOWN_RANK,
                "`{id}` runs in this build but merge would treat it as unknown"
            );
        }
    }
    /// libgit2's "there is no repository here" is the ONE answer that may read
    /// as an absence, because it is the only one that means the search finished
    /// and found nothing.
    #[test]
    fn a_not_found_from_the_upward_search_means_there_is_no_enclosing_repository() {
        let answer = super::enclosing_from(
            Err(git2::Error::new(
                git2::ErrorCode::NotFound,
                git2::ErrorClass::Repository,
                "could not find repository",
            )),
            std::path::Path::new("/somewhere"),
        );

        assert!(
            matches!(answer, Ok(None)),
            "NotFound is the answer that means the search finished empty"
        );
    }

    /// Every other answer refuses the run, and this is the branch that matters.
    /// Read as an absence it walks a repository subtree as a flat tree, never
    /// consulting the repository's own `.gitignore` — the R22 hazard. The
    /// failure that actually happens here is libgit2's ownership check, which
    /// fires *while searching upward*, which is exactly when there IS a
    /// repository above.
    ///
    /// The integration test that first caught this could only fire on a machine
    /// whose git configuration reproduces that refusal; this one fires
    /// everywhere.
    #[test]
    fn any_other_failure_of_the_upward_search_refuses_rather_than_reading_as_absence() {
        let err = super::enclosing_from(
            Err(git2::Error::new(
                git2::ErrorCode::Owner,
                git2::ErrorClass::Config,
                "repository path is not owned by current user",
            )),
            std::path::Path::new("/somewhere"),
        )
        .expect_err("an undecidable answer must not read as 'no repository'");

        let super::ExtractError::UndeterminedEnclosure { path, source } = err else {
            panic!("expected UndeterminedEnclosure, got {err}");
        };
        assert_eq!(path, std::path::PathBuf::from("/somewhere"));
        assert!(
            source.message().contains("not owned by current user"),
            "the operator must be told what git actually said: {source}"
        );
    }

    /// A source that counts how often it is enumerated and hands back exactly
    /// what it was built with, in that order.
    struct CountingSource {
        files: Vec<super::TrackedFile>,
        enumerations: std::cell::Cell<usize>,
        bytes: Vec<u8>,
    }

    impl CountingSource {
        fn tracked(key: &str, path: &str, content: &[u8]) -> super::TrackedFile {
            super::TrackedFile {
                key: key.to_string(),
                path: std::path::PathBuf::from(path),
                mode: 0o100644,
                blob_id: git2::Oid::hash_object(git2::ObjectType::Blob, content).unwrap(),
                is_symlink: false,
            }
        }
    }

    impl super::source::Source for CountingSource {
        fn tracked_files_with_paths(&self) -> Result<Vec<super::TrackedFile>, super::ExtractError> {
            self.enumerations.set(self.enumerations.get() + 1);
            Ok(self.files.clone())
        }

        fn blob_bytes(&self, _tf: &super::TrackedFile) -> Result<Vec<u8>, super::ExtractError> {
            Ok(self.bytes.clone())
        }

        fn head_revision(&self) -> Option<String> {
            None
        }
    }

    fn repo_over(source: CountingSource) -> super::Repo {
        super::Repo {
            root: std::path::PathBuf::from("."),
            source: Box::new(source),
            files: std::cell::OnceCell::new(),
        }
    }

    /// The defect this guards is not the wasted I/O, it is the second snapshot.
    /// A directory walk of a live tree answers a different question at a
    /// different moment, so two extractors in one run could describe two trees.
    #[test]
    fn a_source_is_enumerated_once_however_many_times_the_run_asks() {
        let counter = std::rc::Rc::new(std::cell::Cell::new(0usize));
        struct Shared {
            counter: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl super::source::Source for Shared {
            fn tracked_files_with_paths(
                &self,
            ) -> Result<Vec<super::TrackedFile>, super::ExtractError> {
                self.counter.set(self.counter.get() + 1);
                Ok(vec![CountingSource::tracked("a.rs", "a.rs", b"a")])
            }
            fn blob_bytes(&self, _tf: &super::TrackedFile) -> Result<Vec<u8>, super::ExtractError> {
                Ok(b"a".to_vec())
            }
            fn head_revision(&self) -> Option<String> {
                None
            }
        }
        let repo = super::Repo {
            root: std::path::PathBuf::from("."),
            source: Box::new(Shared {
                counter: std::rc::Rc::clone(&counter),
            }),
            files: std::cell::OnceCell::new(),
        };

        for _ in 0..8 {
            repo.tracked_files_with_paths().unwrap();
        }
        repo.tracked_files().unwrap();

        assert_eq!(
            counter.get(),
            1,
            "nine questions must reach the source as one enumeration"
        );
    }

    /// A file rewritten between enumeration and read must not be parsed under the
    /// id it was enumerated with — that id is the cache key, and the record would
    /// answer for content it never described.
    #[test]
    fn bytes_that_no_longer_hash_to_the_enumerated_id_are_refused() {
        let tf = CountingSource::tracked("src/lib.rs", "src/lib.rs", b"before");
        let repo = repo_over(CountingSource {
            files: vec![tf.clone()],
            enumerations: std::cell::Cell::new(0),
            bytes: b"after".to_vec(),
        });

        let err = repo
            .blob_bytes(&tf)
            .expect_err("changed bytes must not pass");
        let super::ExtractError::ContentChanged {
            path,
            expected,
            actual,
        } = err
        else {
            panic!("a changed file must report ContentChanged, got {err}");
        };
        assert_eq!(path, std::path::PathBuf::from("src/lib.rs"));
        assert_eq!(expected, tf.blob_id);
        assert_ne!(actual, tf.blob_id);
    }

    /// And the unchanged case still passes through, so the check cannot be
    /// mistaken for "always refuses".
    #[test]
    fn bytes_that_match_the_enumerated_id_are_returned_unchanged() {
        let tf = CountingSource::tracked("src/lib.rs", "src/lib.rs", b"same");
        let repo = repo_over(CountingSource {
            files: vec![tf.clone()],
            enumerations: std::cell::Cell::new(0),
            bytes: b"same".to_vec(),
        });

        assert_eq!(repo.blob_bytes(&tf).unwrap(), b"same".to_vec());
    }

    /// Two paths that fold onto one graph key must not let the source's order
    /// pick the survivor: on a filesystem walk that order is readdir order, which
    /// is a property of the machine rather than of the tree.
    ///
    /// The real collision route is the `\`-to-`/` fold in
    /// [`path_key_from_bytes`] — a unix file literally named `a\b` and a real
    /// `a/b`. It cannot be written portably (`\` is a separator on Windows, and
    /// `Path`'s ordering folds it there too), so the entries are built directly:
    /// the mechanism under test is the tie-break, not the route that reaches it.
    #[test]
    fn a_key_collision_resolves_the_same_way_whichever_order_the_source_found_it() {
        let one = CountingSource::tracked("a/b", "a-first.rs", b"one");
        let other = CountingSource::tracked("a/b", "z-second.rs", b"other");

        let survivors: Vec<_> = [
            vec![one.clone(), other.clone()],
            vec![other.clone(), one.clone()],
        ]
        .into_iter()
        .map(|files| {
            let repo = repo_over(CountingSource {
                files,
                enumerations: std::cell::Cell::new(0),
                bytes: Vec::new(),
            });
            let listed = repo.tracked_files_with_paths().unwrap();
            assert_eq!(listed.len(), 1, "one key must yield one entry");
            listed[0].path.clone()
        })
        .collect();

        assert_eq!(
            survivors[0], survivors[1],
            "the surviving path must not depend on the order the source enumerated"
        );
        assert_eq!(
            survivors[0],
            std::path::PathBuf::from("a-first.rs"),
            "with the order total, the lower path wins deterministically"
        );
    }

    use super::path_key_from_bytes;

    #[test]
    fn valid_ascii_passes_through_unchanged() {
        assert_eq!(
            path_key_from_bytes(b"src/extract/git.rs"),
            "src/extract/git.rs"
        );
    }

    #[test]
    fn backslashes_fold_to_forward_slash() {
        assert_eq!(
            path_key_from_bytes(b"src\\extract\\git.rs"),
            "src/extract/git.rs"
        );
    }

    #[test]
    fn distinct_non_utf8_paths_do_not_collide() {
        // Two different invalid-UTF-8 byte sequences that `from_utf8_lossy`
        // collapses to the same "a\u{FFFD}b" string — the bug that dropped one
        // file. Byte-preserving keys must keep them apart.
        assert_eq!(
            String::from_utf8_lossy(b"a\xC0b"),
            String::from_utf8_lossy(b"a\xFFb"),
            "precondition: lossy decoding collides these two paths"
        );
        assert_ne!(
            path_key_from_bytes(b"a\xC0b"),
            path_key_from_bytes(b"a\xFFb"),
            "byte-preserving keys must not collide"
        );
    }

    #[test]
    fn literal_percent_is_escaped_to_stay_injective() {
        // A real path with '%' must not be mistaken for an escape sequence.
        assert_eq!(path_key_from_bytes(b"a%2Fb"), "a%252Fb");
    }
}

/// U4: `run_all_with`/`default_extractors_with` correctly thread a shared
/// `CacheStore` from the top of the extractor pipeline down to the real
/// phase ids the three `walk_files`-based passes use (`languages-decl`,
/// `languages-usages`, `external`) — plumbing only, no validity decision.
/// `walk_files` itself (hit/miss behaviour, corruption, blob collisions,
/// ordering) is already exhaustively tested in `walk.rs`'s own tests against
/// a synthetic `Out` type; these tests exercise the NEW wiring introduced by
/// this unit — that the real extractors reach `walk_files` with a real,
/// shared cache handle — which those tests cannot see.
#[cfg(test)]
mod cache_wiring_tests {
    use super::{run_all_with_cached, CacheStore, ExtractOptions, Repo};
    use crate::graph::Graph;
    use crate::progress::Silent;

    /// Write `contents` at `rel` and commit, initializing the repo on the
    /// first call. Mirrors the fixture pattern already used by `cache.rs`'s
    /// and `walk.rs`'s own tests (a real git commit, not hand-built
    /// `TrackedFile`s, so `blob_id` is the actual blob a `CacheStore` record
    /// is keyed on) but allows a SECOND commit on the same repo, needed to
    /// exercise a file changing blob id across runs.
    fn commit_file(dir: &std::path::Path, rel: &str, contents: &str, message: &str) {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, contents).unwrap();

        let repository = git2::Repository::open(dir)
            .or_else(|_| git2::Repository::init(dir))
            .unwrap();
        let mut index = repository.index().unwrap();
        index.add_path(std::path::Path::new(rel)).unwrap();
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::new(
            "Ann",
            "ann@example.test",
            &git2::Time::new(1_600_000_000, 0),
        )
        .unwrap();
        let parent_commit = repository
            .head()
            .ok()
            .and_then(|h| h.target())
            .and_then(|oid| repository.find_commit(oid).ok());
        let parents: Vec<&git2::Commit> = parent_commit.iter().collect();
        repository
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap();
    }

    fn blob_id_of(repo: &Repo, key: &str) -> git2::Oid {
        repo.tracked_files_with_paths()
            .unwrap()
            .into_iter()
            .find(|tf| tf.key == key)
            .unwrap_or_else(|| panic!("{key} not tracked"))
            .blob_id
    }

    /// The point of this whole unit: two SEPARATE `run_all_with` calls over
    /// the same commit, sharing one `CacheStore`, each against its own fresh
    /// `Graph` (mirrors what `generate_reporting` does per run) — the second
    /// run must find records the first run left for all three real phase ids,
    /// and must not add or replace any of them.
    #[test]
    fn a_second_run_over_the_same_commit_finds_every_phases_records_already_there() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        commit_file(repo_dir.path(), "src/lib.rs", "pub fn f() {}\n", "initial");
        let repo = Repo::open(repo_dir.path()).unwrap();
        let blob_id = blob_id_of(&repo, "src/lib.rs");

        let cache_dir = tempfile::TempDir::new().unwrap();
        let store = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            super::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );

        let mut graph = Graph::new();
        run_all_with_cached(
            &repo,
            &mut graph,
            &ExtractOptions::default(),
            &Silent,
            Some(&store),
        )
        .unwrap();

        for phase in ["languages-decl", "languages-usages"] {
            assert!(
                store
                    .get::<crate::extract::treesitter::FileOutputWire>(phase, blob_id, "src/lib.rs")
                    .is_some(),
                "run 1 must leave a {phase} record for the tracked file"
            );
        }
        assert!(
            store
                .get::<crate::extract::wire::WireOps>("external", blob_id, "src/lib.rs")
                .is_some(),
            "run 1 must leave an external record for the tracked file"
        );

        let count_files = |phase: &str| -> usize {
            std::fs::read_dir(cache_dir.path().join(phase))
                .map(|entries| entries.count())
                .unwrap_or(0)
        };
        let before = (
            count_files("languages-decl"),
            count_files("languages-usages"),
            count_files("external"),
        );

        // A SECOND, independent run over the identical commit — a fresh
        // graph, the same repo, the same store.
        let mut graph2 = Graph::new();
        run_all_with_cached(
            &repo,
            &mut graph2,
            &ExtractOptions::default(),
            &Silent,
            Some(&store),
        )
        .unwrap();

        let after = (
            count_files("languages-decl"),
            count_files("languages-usages"),
            count_files("external"),
        );
        assert_eq!(
            before, after,
            "a second run over an unchanged commit must not add or replace cache files"
        );
        for phase in ["languages-decl", "languages-usages"] {
            assert!(
                store
                    .get::<crate::extract::treesitter::FileOutputWire>(phase, blob_id, "src/lib.rs")
                    .is_some(),
                "the {phase} record must still be readable after run 2"
            );
        }
        assert!(store
            .get::<crate::extract::wire::WireOps>("external", blob_id, "src/lib.rs")
            .is_some());
    }

    /// A sanity check on the plumbing this unit adds, not on invalidation
    /// (U5's job, not implemented here): a file changed between two commits
    /// gets records under its NEW blob id, and — since this unit implements
    /// no invalidation — the OLD blob's records are simply left behind,
    /// never cleared.
    #[test]
    fn a_changed_files_two_blob_ids_each_get_their_own_records() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        commit_file(repo_dir.path(), "src/lib.rs", "pub fn f() {}\n", "v1");
        let repo = Repo::open(repo_dir.path()).unwrap();
        let old_blob = blob_id_of(&repo, "src/lib.rs");

        let cache_dir = tempfile::TempDir::new().unwrap();
        let store = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            super::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        let mut graph = Graph::new();
        run_all_with_cached(
            &repo,
            &mut graph,
            &ExtractOptions::default(),
            &Silent,
            Some(&store),
        )
        .unwrap();

        commit_file(repo_dir.path(), "src/lib.rs", "pub fn g() {}\n", "v2");
        // A fresh `Repo::open` rather than reusing `repo`: libgit2 caches an
        // already-opened repository's index in memory, so the same handle
        // can still see the pre-v2 index after `commit_file` rewrote it
        // through a SEPARATE `git2::Repository` handle on disk.
        let repo2 = Repo::open(repo_dir.path()).unwrap();
        let new_blob = blob_id_of(&repo2, "src/lib.rs");
        assert_ne!(
            old_blob, new_blob,
            "precondition: changed content must change the blob id"
        );

        let mut graph2 = Graph::new();
        run_all_with_cached(
            &repo2,
            &mut graph2,
            &ExtractOptions::default(),
            &Silent,
            Some(&store),
        )
        .unwrap();

        for phase in ["languages-decl", "languages-usages"] {
            assert!(
                store
                    .get::<crate::extract::treesitter::FileOutputWire>(
                        phase,
                        old_blob,
                        "src/lib.rs"
                    )
                    .is_some(),
                "the old blob's {phase} record must survive — this unit implements no invalidation"
            );
            assert!(
                store
                    .get::<crate::extract::treesitter::FileOutputWire>(
                        phase,
                        new_blob,
                        "src/lib.rs"
                    )
                    .is_some(),
                "the new blob must get its own {phase} record"
            );
        }
        assert!(store
            .get::<crate::extract::wire::WireOps>("external", old_blob, "src/lib.rs")
            .is_some());
        assert!(store
            .get::<crate::extract::wire::WireOps>("external", new_blob, "src/lib.rs")
            .is_some());
    }
}
