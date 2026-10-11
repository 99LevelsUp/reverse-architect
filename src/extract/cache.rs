//! Cross-run caching for extraction (U1+).
//!
//! Through U2 this file also held a manual [`Deserialize`] for
//! [`Provenance`], because decoding an extractor id back into a
//! `&'static str` needed [`crate::extract::trust_ranks()`] and `graph` must
//! never depend on `extract` (KTD2). U3 made the id owned, which removed the
//! lookup and the inversion with it — `graph` derives that impl itself now.
//! What the hand-written version *also* did, refusing an unknown extractor
//! id, moved to the cache-hit path in `extract::walk`; see the note where it
//! used to be.

use super::{ExtractError, ExtractOptions, Repo};
use crate::graph::{Ranks, Vocabulary};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// The manual `impl Deserialize for Provenance` that stood here was deleted in
// U3. It existed for one reason: an `ExtractorId` was a `&'static str`, serde
// cannot manufacture a `'static` borrow from a runtime buffer, and the only
// route back was a lookup in `trust_ranks()`. `graph` therefore could not own
// its own decoder without depending on `extract` — the dependency inversion
// this file was created to hold. An owned id removes the lookup, so
// `Provenance` derives `Deserialize` in `graph` now.
//
// Deleting it removed a second thing the derive cannot do: refuse an extractor
// id this build does not know. That check was doing real work — it turned a
// stale record into a decode error, which `get` swallows into a cache MISS, so
// the file was simply re-parsed. Left unreplaced, a stale record would now be
// SERVED. It moved to `walk_files`' cache-hit path (`WalkOutput`), which is
// where such a record actually enters and which can still answer it by
// re-parsing.
//
// It also called `trust_ranks()` once per deserialized provenance — building
// every extractor to decode one fact. Under runtime identity that would mean
// loading components per fact on a warm run, which is the cost KTD17 exists to
// remove.

/// One run's identity for cache validity (KTD1, U2): the exact tracked-file
/// content set plus the parts of [`ExtractOptions`] that can change what the
/// three `walk_files`-based passes (tree-sitter declarations, tree-sitter
/// usages, `external` — KTD4) produce for otherwise-unchanged input.
///
/// Equality is field-by-field and EXACT — derived [`PartialEq`]/[`Eq`], never
/// a single hashed integer. A hash collision could silently accept a stale
/// cache; comparing the plain data cannot collide. Two fingerprints compare
/// equal only when every field matches, so a caller (U5) gates validity with
/// plain `==` between a freshly computed fingerprint and the one stored on
/// disk — a mismatch anywhere means the ENTIRE on-disk cache is invalid
/// (KTD1: never a per-file partial invalidation; enforced by the caller that
/// acts on the comparison, not by this type).
///
/// Serializes with `serde_json` (KTD7) so it can be written as the on-disk
/// cache header — but this type does no file I/O itself; that header is read
/// and written by [`activate`] (U5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunFingerprint {
    /// [`super::CACHE_VERSION`] at the time this fingerprint was computed.
    /// Any difference here invalidates the cache even when the repo and
    /// options are byte-identical to a previous run — a reader must never
    /// match a header written under a different cache format.
    cache_version: u32,
    /// Every tracked file's `(graph key, blob id as hex)`, in the order
    /// [`Repo::tracked_files_with_paths`] returns them (already sorted and
    /// deduped by key — KTD10). Changes on any content change, addition, or
    /// removal of a tracked file.
    tracked_files: Vec<(String, String)>,
    /// Mirrors [`ExtractOptions::ambiguity_ceiling`].
    ambiguity_ceiling: usize,
    /// Mirrors [`ExtractOptions::secret_prefixes`].
    secret_prefixes: Vec<String>,
    /// `(id, SHA-256)` of every language plugin, in precedence order (U10).
    /// A plugin's output is a function of its bytes, so a rebuilt, replaced or
    /// reordered plugin makes every language record stale — the one change to a
    /// run that neither the tracked files nor the options above would show.
    plugins: Vec<(String, String)>,
    /// The shared resolver's revision (plan A, KTD15): every rule's id and
    /// revision and the rights table's fingerprint. A changed rule or a
    /// granted right changes what a `languages-usages` record holds for the
    /// same file, and this is the field that says so.
    ///
    /// Absent from a header written before it existed, which decodes with it
    /// empty and re-encodes to the same bytes — so such a header still
    /// digests to what it wrote, and is still never equal to a current one,
    /// whose revision is never empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    resolver: String,
}

impl RunFingerprint {
    /// Serialize to bytes (`serde_json`, KTD7) — a pure encode, no file I/O.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self)
            .expect("RunFingerprint holds only plain, always-serializable fields")
    }

    /// Deserialize a fingerprint previously produced by [`Self::to_bytes`].
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// A compact, deterministic digest of this fingerprint (KTD9): FNV1a64
    /// over this value's own JSON encoding.
    ///
    /// [`CacheStore`] carries this on every record it writes, alongside the
    /// existing `key` (U3) — not the full `RunFingerprint`, whose
    /// `tracked_files` list would otherwise be duplicated onto every one of
    /// the thousands of per-file records a large repo produces. [`get`]
    /// rejects a record whose digest does not match the store's own, closing
    /// the race two concurrent runs with DIFFERENT `ExtractOptions` could
    /// hit: a straggling write from one run landing under a header the other
    /// already rewrote, previously indistinguishable from a genuine hit
    /// because only `key` was checked.
    ///
    /// Reuses [`crate::graph::fnv1a64_hex`] rather than `std`'s
    /// `HashMap`/`Hasher` default (`RandomState`): that hasher reseeds per
    /// process, so two runs computing the SAME fingerprint would disagree on
    /// its digest and every warm-cache hit would silently become a miss.
    ///
    /// [`get`]: CacheStore::get
    pub(crate) fn digest(&self) -> String {
        let text = serde_json::to_string(self)
            .expect("RunFingerprint holds only plain, always-serializable fields");
        crate::graph::fnv1a64_hex(&text)
    }
}

/// Compute the fingerprint for the run that would extract `repo` with `opts`
/// right now. Pure computation over already-open handles — no file I/O
/// beyond what `repo` itself performs to read the git index.
///
/// `opts` is destructured field-by-field, never with a `..` catch-all, so a
/// field added to `ExtractOptions` later forces a conscious decision here:
/// fold it into the fingerprint, or name it `_` with a comment explaining why
/// it is safe to leave out, rather than silently defaulting to "not in the
/// fingerprint."
pub(crate) fn compute_fingerprint(
    repo: &Repo,
    opts: &ExtractOptions,
) -> Result<RunFingerprint, ExtractError> {
    let ExtractOptions {
        ambiguity_ceiling,
        // Only configures the `scip` extractor, which attaches to Symbol
        // nodes the tree-sitter pass already emitted and creates no node of
        // its own (KTD3) — it is not one of the three `walk_files`-based
        // passes this fingerprint gates (KTD4), so a different (or absent)
        // index path never changes what those three produce.
        scip_index: _,
        secret_prefixes,
        // Configures only `metrics` and `test_pairing` (U4), and those are two
        // of the five extractors that never call `walk_files` and never receive
        // a cache handle at all — so nothing this fingerprint gates can depend
        // on the registry, and a run that registers a new language still
        // recomputes both from scratch.
        //
        // Revisit the moment a plugin's own parse output becomes a cached
        // phase: at that point the registry decides what a cached record even
        // contains, and leaving it out here would serve records produced under
        // a different language set. It would also need `Serialize`, which
        // `Languages` deliberately does not derive yet.
        languages: _,
        plugins,
        rights,
        // A run that keeps the records neither reads nor writes a usage
        // record (`treesitter::Languages::run`) — a usage record holds only
        // the counters a run without them needs — and the declaration records
        // it does share do not depend on it. Leaving it out keeps one cache
        // for a run with `--sites` and one without, rather than each clearing
        // the other's.
        site_records: _,
        // Changes what a usage record holds, but such a run never reads or
        // writes a usage record (`treesitter::Languages::trace_over_ceiling`),
        // and the declaration records it does share do not depend on it.
        // Leaving it out keeps one cache for `ra measure` and generation over
        // the same corpus, rather than each clearing the other's.
        trace_over_ceiling: _,
    } = opts;

    let tracked_files = repo
        .tracked_files_with_paths()?
        .into_iter()
        .map(|tf| (tf.key, tf.blob_id.to_string()))
        .collect();

    Ok(RunFingerprint {
        cache_version: super::CACHE_VERSION,
        tracked_files,
        ambiguity_ceiling: *ambiguity_ceiling,
        secret_prefixes: secret_prefixes.clone(),
        plugins: plugins.identities(),
        resolver: super::resolve::rules::resolver_revision(super::resolve::rules::CATALOG, rights),
    })
}

/// The per-file, per-phase on-disk cache (U3).
///
/// One record per `(phase_id, blob_id)`, each in its own file under `root`:
/// `<root>/<phase_id>/<blob_id-hex>.json`. `phase_id` names one of the three
/// `walk_files`-based passes (KTD4); `blob_id` is the git index blob a file's
/// content hashes to.
///
/// `blob_id` alone is NOT a safe cache key: two distinct tracked files with
/// byte-identical content (an empty `__init__.py`, a vendored duplicate) share
/// one blob id, but a `walk_files` output is a function of `(key, bytes)` —
/// declarations and usages both carry the file's [`crate::graph::NodeKey`]s and
/// its [`Provenance`] `file` field, neither of which the blob id determines.
/// Every record therefore also carries the graph key ([`CacheStore::put`]) that
/// produced it, and [`CacheStore::get`] treats a stored key that does not match
/// the one asked for as a miss, same as a missing or corrupt file (KTD6) — two
/// files sharing a blob simply thrash one record and always re-parse, which is
/// correct (if not a cache win) rather than one file silently receiving the
/// other's node keys and provenance.
///
/// Whole-store trustworthiness is decided once, by the caller (U5), via
/// [`RunFingerprint`] equality (KTD1) before `Some(&store)` is ever passed
/// into `walk_files` — `CacheStore` does not re-derive that decision. What it
/// DOES decide per record, since U7 (KTD9): whether the record on disk was
/// produced under THIS store's own fingerprint digest, not merely under
/// SOME valid-looking header — the defense a whole-store decision alone
/// cannot give against a straggling write from a differently-configured
/// concurrent run. "Is there a readable, correctly-tagged record here, for
/// this exact file" is the whole of what `CacheStore` answers; "should
/// there be" stays the caller's call.
///
/// Every fallible operation degrades rather than propagates (KTD6, the
/// [`Repo::blob_bytes`] idiom): a read that fails for any reason — missing
/// file, truncated write, JSON that no longer parses, a key mismatch — is a
/// cache MISS, not an error the caller has to handle; a write that fails is
/// logged to stderr and otherwise ignored, since losing one cache record costs
/// a re-parse next run, never correctness this run.
///
/// `pub(crate)`: only [`super::treesitter::Languages`],
/// [`super::external::External`], and `super::run_all_with_cached` need to
/// name this type, all crate-internal (review #3 -- U4 originally widened
/// this and `run_all_with` to plain `pub` purely so two integration tests
/// could typecheck a `None` argument; `run_all_with_cached` staying
/// crate-internal and the public `run_all_with` never taking a cache
/// parameter removes that need). U5 is still the only unit that will ever
/// construct one in production code.
#[derive(Debug)]
pub(crate) struct CacheStore {
    root: PathBuf,
    /// The active run's fingerprint digest (U7, KTD9) — set once by
    /// [`activate`] and carried on every clone. [`CacheStore::put`] tags
    /// every record it writes with this value; [`CacheStore::get`] rejects a
    /// record whose own digest does not match, even when `key` matches, so a
    /// record left behind by a DIFFERENT run's fingerprint is never served
    /// as a hit.
    digest: String,
    /// Phase subdirectories already known to exist, so [`CacheStore::put`]
    /// stops paying a `create_dir_all` syscall on every record write once a
    /// phase's directory has been created once. `Mutex`, not `RefCell`:
    /// `Languages`/`External` embed a `CacheStore` and are shared across
    /// rayon worker threads inside `walk_files`'s `parse` closure, so the
    /// type must stay `Sync` even though `put` itself only ever runs on the
    /// single serial apply thread. `Clone` starts a fresh, empty set — each
    /// extractor holds its own clone and only ever writes its own phase
    /// id(s), so re-learning "created" once per extractor costs nothing a
    /// per-file cost would (review #4).
    created_dirs: Mutex<HashSet<String>>,
    /// The extractor ids this run knows, as the run's own [`ExtractorId`]
    /// values (KTD17: resolved once, threaded — never rebuilt per fact).
    ///
    /// Two jobs, both on the cache-hit path. It rejects a record naming an
    /// extractor this build no longer has, which is the honesty guard the
    /// deleted manual `Deserialize` used to provide. And it hands back the
    /// run's own `Arc` for an id serde just allocated fresh, so a warm run
    /// holds one allocation per id rather than one per fact — serde's `rc`
    /// impls do not preserve sharing, and say so.
    known_ids: Ranks,
    /// The kinds this corpus declared (U7, R6), resolved once and threaded the
    /// same way `known_ids` is — and for the same reason. A record spells its
    /// kinds as names; turning a name back into a kind, and refusing one this
    /// corpus cannot name, needs the vocabulary at the point of the read.
    ///
    /// **Cloned, not borrowed.** Declarations happen before the walk and the
    /// walk holds `&mut Graph`, so a borrow of the graph's own vocabulary would
    /// fight the walk for the graph. A vocabulary is a few hundred entries (A1),
    /// so the copy is not worth a lifetime on `CacheStore`.
    ///
    /// Empty for every corpus that declares nothing, which is every corpus this
    /// tool builds today — the cache path is then exactly what it was before
    /// U7, names and all.
    vocabulary: Vocabulary,
}

impl Clone for CacheStore {
    fn clone(&self) -> Self {
        CacheStore {
            root: self.root.clone(),
            digest: self.digest.clone(),
            created_dirs: Mutex::new(HashSet::new()),
            known_ids: self.known_ids.clone(),
            vocabulary: self.vocabulary.clone(),
        }
    }
}

/// The on-disk envelope for one record: the graph key it was produced from,
/// alongside the payload. See [`CacheStore`]'s doc comment for why the key
/// travels with the value instead of the blob id standing in for it alone.
///
/// Read back as the owned [`Record`]; written from a borrowed [`RecordRef`] so
/// [`CacheStore::put`] never needs `T: Clone` just to tag a borrowed value with
/// its key.
#[derive(Deserialize)]
struct Record<T> {
    /// The run fingerprint's digest (U7, KTD9) that produced this record —
    /// checked by [`CacheStore::get`] alongside `key`, never trusted from
    /// `key`/`blob_id` alone.
    fingerprint: String,
    key: String,
    value: T,
}

#[derive(Serialize)]
struct RecordRef<'a, T> {
    fingerprint: &'a str,
    key: &'a str,
    value: &'a T,
}

impl CacheStore {
    /// Open a store rooted at `root`, tagged with `digest` — the active run's
    /// fingerprint digest (U7, KTD9), always [`activate`]'s single decision
    /// point in production. Pure — no I/O, no directory creation; directories
    /// are created lazily by [`CacheStore::put`] (or by [`activate`]'s
    /// [`write_header`] call) only when something is actually written.
    pub(crate) fn new(
        root: PathBuf,
        digest: String,
        known_ids: Ranks,
        vocabulary: Vocabulary,
    ) -> Self {
        CacheStore {
            root,
            digest,
            created_dirs: Mutex::new(HashSet::new()),
            known_ids,
            vocabulary,
        }
    }

    /// The ids this run knows — see the field's doc for what reads it.
    pub(crate) fn known_ids(&self) -> &Ranks {
        &self.known_ids
    }

    /// The kinds this corpus declared — see the field's doc for what reads it.
    pub(crate) fn vocabulary(&self) -> &Vocabulary {
        &self.vocabulary
    }

    /// This store, tagged for records that depend on more than the run
    /// fingerprint says: the same root, ids and vocabulary, and a digest that
    /// is the run's own followed by `tag` (plan 2a, U4: KTD7, KTD8).
    ///
    /// The second layer's records (`languages-bodies`) are a function of the
    /// body component's bytes and of the host's redaction of what it
    /// returned, and neither is in [`RunFingerprint`] — on purpose: the
    /// component is loaded only for `--detail-level`, and putting it in the
    /// fingerprint would make every switch of the layer clear the first
    /// layer's records with it. The tag names both instead, so [`get`]'s
    /// per-record digest check turns a rebuilt component or a raised
    /// redaction version into a miss for exactly those records, and the
    /// `languages-decl` and `languages-usages` records, written under the
    /// untagged digest, never notice. The tag is written out rather than
    /// hashed into the digest, so a record says which component made it.
    ///
    /// [`get`]: CacheStore::get
    pub(crate) fn derived(&self, tag: &str) -> CacheStore {
        CacheStore::new(
            self.root.clone(),
            format!("{}+{tag}", self.digest),
            self.known_ids.clone(),
            self.vocabulary.clone(),
        )
    }

    fn path_for(&self, phase_id: &str, blob_id: git2::Oid) -> PathBuf {
        self.root.join(phase_id).join(format!("{blob_id}.json"))
    }

    /// Look up the record for `(phase_id, blob_id)`, requiring it to have been
    /// written for this exact `key` AND under this exact store's fingerprint
    /// digest (U7, KTD9). `None` on a cache miss for ANY reason — no record
    /// on disk, an unreadable file, JSON that fails to parse or fails to
    /// match `T`, a record written for a DIFFERENT key that happens to share
    /// this blob id, or a record written under a DIFFERENT run's fingerprint
    /// (KTD6). Never a panic, never a propagated error: to the caller every
    /// one of these looks the same — "parse it again."
    pub(crate) fn get<T: DeserializeOwned>(
        &self,
        phase_id: &str,
        blob_id: git2::Oid,
        key: &str,
    ) -> Option<T> {
        let bytes = std::fs::read(self.path_for(phase_id, blob_id)).ok()?;
        let record: Record<T> = serde_json::from_slice(&bytes).ok()?;
        if record.fingerprint != self.digest || record.key != key {
            return None;
        }
        Some(record.value)
    }

    /// Write the record for `(phase_id, blob_id)`, tagged with the `key` it
    /// was produced from, creating `root/phase_id` if it does not exist yet.
    /// A failure at any step — directory creation, serialization, the write
    /// itself — is logged to stderr and otherwise swallowed (KTD6): a lost
    /// write only costs a re-parse of this one file on the next run, and must
    /// never fail the run in progress.
    pub(crate) fn put<T: Serialize>(
        &self,
        phase_id: &str,
        blob_id: git2::Oid,
        key: &str,
        value: &T,
    ) {
        let path = self.path_for(phase_id, blob_id);
        let Some(parent) = path.parent() else {
            return;
        };
        // `create_dir_all` costs a syscall even when the directory already
        // exists — cheap once, wasteful once per record when there are only
        // ever a handful of phase directories total (review #4). Remember
        // which phases this store has already created and skip the call once
        // it has; a failed creation is never remembered, so the next `put`
        // retries it exactly as before.
        // A poisoned lock (a prior panic while held) degrades to "not known
        // created yet" rather than panicking here too (KTD6) -- worst case,
        // one redundant create_dir_all, never a failed run.
        let already_created = self
            .created_dirs
            .lock()
            .map(|dirs| dirs.contains(phase_id))
            .unwrap_or(false);
        if !already_created {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("cache: failed to create {}: {e}", parent.display());
                return;
            }
            if let Ok(mut dirs) = self.created_dirs.lock() {
                dirs.insert(phase_id.to_string());
            }
        }
        let record = RecordRef {
            fingerprint: &self.digest,
            key,
            value,
        };
        let bytes = match serde_json::to_vec(&record) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("cache: failed to serialize {phase_id}/{blob_id} ({key}): {e}");
                return;
            }
        };
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("cache: failed to write {}: {e}", path.display());
        }
    }

    /// Remove every record this store holds ([`activate`] calls this when a
    /// fresh [`RunFingerprint`] does not match the stored one — KTD1,
    /// whole-cache invalidation only). A missing root is `Ok(())`: there is
    /// nothing to clear, which is the state this leaves behind either way.
    ///
    /// Returns the underlying I/O error on any OTHER failure — unlike every
    /// other fallible operation on this type, the caller here ([`activate`])
    /// genuinely needs to know: a clear that silently failed but was treated
    /// as successful could go on to write a header claiming a store is empty
    /// when records from the PREVIOUS regime are still sitting on disk,
    /// which is exactly the "header lies about what's in the store"
    /// invariant KTD8 exists to prevent. Still logged to stderr here, same
    /// idiom as [`CacheStore::put`], so the failure is visible even though
    /// it is also propagated.
    pub(crate) fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                eprintln!("cache: failed to clear {}: {e}", self.root.display());
                Err(e)
            }
        }
    }
}

/// Hidden directory name for the cache root, nested under a run's
/// `corpus_dir` (U5: [`crate::config::Config::corpus_dir`]) — e.g.
/// `<corpus_dir>/.ra-cache/`.
///
/// Dot-prefixed and named distinctly from anything
/// [`crate::taxonomy::generated_filenames`] ever emits (none of those begin
/// with `.`, and none is named `ra-cache`), so a consumer scanning
/// `corpus_dir` for corpus content — or `main.rs`'s own
/// `generated_filenames`-driven cleanup pass before a re-run — cannot
/// mistake this directory for a corpus file or vice versa.
pub(crate) const CACHE_DIR_NAME: &str = ".ra-cache";

/// Reserved filename for the on-disk [`RunFingerprint`] header, directly
/// under a [`CacheStore`]'s root. Disjoint from every `phase_id` a store ever
/// fans out to a subdirectory (`languages-decl`, `languages-usages`,
/// `external`) — a header live IN the store's own root, rather than
/// alongside it, is what makes [`CacheStore::clear`] (a plain
/// `remove_dir_all` on that same root) also remove a stale header as one
/// atomic filesystem operation, instead of needing to separately delete it.
const HEADER_FILE_NAME: &str = "fingerprint.json";

fn header_path(root: &Path) -> PathBuf {
    root.join(HEADER_FILE_NAME)
}

/// Read the header stored at `root`, if there is a trustworthy one.
///
/// `None` for every "not authoritative" outcome — no file, an unreadable
/// file, JSON that no longer parses (KTD6, the same idiom as
/// [`CacheStore::get`]): to [`activate`] every one of these means exactly
/// the same thing, "treat this as a cold cache," never a distinct error path.
fn read_header(root: &Path) -> Option<RunFingerprint> {
    let bytes = std::fs::read(header_path(root)).ok()?;
    RunFingerprint::from_bytes(&bytes).ok()
}

/// Write `fingerprint` as the header at `root`, creating `root` if it does
/// not exist. Never called except immediately after a successful
/// [`CacheStore::clear`] of the very same root (see [`activate`]) — so this
/// always starts from an empty (or freshly-created) directory, never one
/// holding records from a different regime.
fn write_header(root: &Path, fingerprint: &RunFingerprint) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    std::fs::write(header_path(root), fingerprint.to_bytes())
}

/// Open (or reinitialize) the on-disk cache at `root` for a run whose current
/// identity is `fresh` — the single decision point KTD8 requires
/// (U5, [`crate::generate_reporting`]'s only caller).
///
/// - **Header matches `fresh`:** the existing store is trusted exactly as it
///   sits — nothing is cleared, nothing is (re)written — and `Some(store)` is
///   handed back for extraction to read and write through.
/// - **No stored header, or one that does not match:** the store is cleared
///   ([`CacheStore::clear`]) and the new header is written
///   ([`write_header`]) — CLEAR THEN WRITE, in that order, both completing
///   BEFORE this function returns and before extraction ever starts. Never
///   the reverse: a header written before the clear would, if the process
///   died between the two steps, leave a header on disk claiming validity
///   for a store that still mixes old and new records. Clearing first means
///   the only states a crash can ever leave behind are "old header, old
///   records" (unchanged — the next run repeats this same decision) or "no
///   header, empty store" (a cold cache — safe, if wasteful) — never a
///   header lying about what is actually in the store.
/// - **Any I/O failure along the way** — the store won't clear, the header
///   won't write — degrades the WHOLE run to no cache at all (`None`),
///   logged to stderr, never a partial activation and never propagated as an
///   error a caller has to handle (KTD6): a cache-plumbing failure is not a
///   run failure.
///
/// `vocabulary` is the corpus's declared kinds (U7). It is **not** part of
/// `fresh`, and that is a decision rather than an omission: the fingerprint
/// answers "may these records be trusted at all", while a vocabulary change is
/// answered per record, by name, in [`crate::extract::walk::WireOps::into_ops`].
/// A renamed kind makes exactly the records that used it unreadable; a changed
/// role or cardinality invalidates nothing, because neither is ever written into
/// a record — both are read from the current vocabulary at merge and projection
/// time. Folding the vocabulary into the fingerprint would therefore throw away
/// every record in the store to answer a question the read path already answers
/// precisely. The hook is named here (KTD9) and the policy belongs to the
/// universal-plugins plan, where a plugin's own identity joins it.
pub(crate) fn activate(
    root: PathBuf,
    fresh: &RunFingerprint,
    known_ids: Ranks,
    vocabulary: Vocabulary,
) -> Option<CacheStore> {
    let digest = fresh.digest();
    if read_header(&root).as_ref() == Some(fresh) {
        return Some(CacheStore::new(root, digest, known_ids, vocabulary));
    }

    let store = CacheStore::new(root.clone(), digest, known_ids, vocabulary);
    store.clear().ok()?;
    if let Err(e) = write_header(&root, fresh) {
        eprintln!("cache: failed to write header at {}: {e}", root.display());
        return None;
    }
    Some(store)
}

#[cfg(test)]
mod tests {
    use super::{activate, compute_fingerprint, CacheStore, RunFingerprint, HEADER_FILE_NAME};
    use crate::extract::{ExtractOptions, Repo};
    use crate::graph::Provenance;
    use git2::{Repository, Signature, Time};
    use std::path::Path;

    /// Init a repo at `dir`, write `files` (relative path, contents), stage
    /// and commit them with a fixed signature — mirrors the fixture pattern
    /// `src/enrich/mod.rs` uses for the same purpose (a real extractor-facing
    /// `Repo`, not hand-built nodes).
    fn repo_with_files(dir: &Path, files: &[(&str, &str)]) -> Repo {
        for (rel, contents) in files {
            let full = dir.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, contents).unwrap();
        }

        let repository = Repository::init(dir).unwrap();
        let mut index = repository.index().unwrap();
        for (rel, _) in files {
            index.add_path(Path::new(rel)).unwrap();
        }
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::new("Ann", "ann@example.test", &Time::new(1_600_000_000, 0)).unwrap();
        repository
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        Repo::open(dir).unwrap()
    }

    #[test]
    fn same_commit_and_config_fingerprint_equal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let opts = ExtractOptions::default();

        let a = compute_fingerprint(&repo, &opts).unwrap();
        let b = compute_fingerprint(&repo, &opts).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn changing_file_content_changes_fingerprint() {
        let tmp_a = tempfile::TempDir::new().unwrap();
        let repo_a = repo_with_files(tmp_a.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let tmp_b = tempfile::TempDir::new().unwrap();
        let repo_b = repo_with_files(tmp_b.path(), &[("src/lib.rs", "pub fn g() {}\n")]);
        let opts = ExtractOptions::default();

        let a = compute_fingerprint(&repo_a, &opts).unwrap();
        let b = compute_fingerprint(&repo_b, &opts).unwrap();
        assert_ne!(a, b, "differing blob content for the same key must differ");
    }

    #[test]
    fn adding_a_tracked_file_changes_fingerprint_even_though_existing_files_are_unchanged() {
        let tmp_a = tempfile::TempDir::new().unwrap();
        let repo_a = repo_with_files(tmp_a.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let tmp_b = tempfile::TempDir::new().unwrap();
        let repo_b = repo_with_files(
            tmp_b.path(),
            &[
                ("src/lib.rs", "pub fn f() {}\n"),
                ("src/extra.rs", "pub fn h() {}\n"),
            ],
        );
        let opts = ExtractOptions::default();

        let a = compute_fingerprint(&repo_a, &opts).unwrap();
        let b = compute_fingerprint(&repo_b, &opts).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn removing_a_tracked_file_changes_fingerprint() {
        let tmp_a = tempfile::TempDir::new().unwrap();
        let repo_a = repo_with_files(
            tmp_a.path(),
            &[
                ("src/lib.rs", "pub fn f() {}\n"),
                ("src/extra.rs", "pub fn h() {}\n"),
            ],
        );
        let tmp_b = tempfile::TempDir::new().unwrap();
        let repo_b = repo_with_files(tmp_b.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let opts = ExtractOptions::default();

        let a = compute_fingerprint(&repo_a, &opts).unwrap();
        let b = compute_fingerprint(&repo_b, &opts).unwrap();
        assert_ne!(a, b);
    }

    /// A granted right to delete changes what a `languages-usages` record
    /// holds for the same file, so it must invalidate the cache like a changed
    /// file does (plan A, KTD15) — and nothing has to remember to bump a
    /// version for it.
    #[test]
    fn granting_a_right_changes_the_fingerprint_for_the_same_repo_state() {
        use crate::extract::resolve::rights::{DeleteRight, RightsTable};
        use crate::graph::Rule;
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let granted = ExtractOptions {
            rights: RightsTable::new(vec![DeleteRight {
                rule: Rule::DeclaredReceiver,
                revision: 1,
                language: "csharp".into(),
                component: "csharp".into(),
                component_sha256: "ab".repeat(32),
                contract: "0.2".into(),
                report: "docs/measurements/x.json".into(),
                report_sha256: "cd".repeat(32),
            }]),
            ..ExtractOptions::default()
        };
        let a = compute_fingerprint(&repo, &ExtractOptions::default()).unwrap();
        let b = compute_fingerprint(&repo, &granted).unwrap();
        assert_ne!(a, b);
        assert_ne!(a.digest(), b.digest());
    }

    /// A rule's revision is in the fingerprint: the run embeds the resolver
    /// revision of the catalog it was built with, and a catalog differing only
    /// in one rule's revision yields another revision string — so a changed
    /// rule can never be served a record its old behavior decided.
    #[test]
    fn a_rule_revision_change_changes_the_resolver_revision_in_the_fingerprint() {
        use crate::extract::resolve::rights::RightsTable;
        use crate::extract::resolve::rules::{resolver_revision, CATALOG};
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let fingerprint = compute_fingerprint(&repo, &ExtractOptions::default()).unwrap();
        let shipped = resolver_revision(CATALOG, &RightsTable::granted());
        assert_eq!(fingerprint.resolver, shipped);

        let mut revised = CATALOG.to_vec();
        revised[1].revision += 1;
        assert_ne!(
            resolver_revision(&revised, &RightsTable::granted()),
            shipped
        );
    }

    /// `--sites` decides only what the run keeps and which records it reads:
    /// a run that keeps the site records stays out of the usage records,
    /// which hold none, and shares the declaration records — one cache for
    /// both runs.
    #[test]
    fn keeping_site_records_does_not_change_the_fingerprint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let keeping = ExtractOptions {
            site_records: true,
            ..ExtractOptions::default()
        };
        assert_eq!(
            compute_fingerprint(&repo, &ExtractOptions::default()).unwrap(),
            compute_fingerprint(&repo, &keeping).unwrap()
        );
    }

    /// A measurement shares the declaration records with generation; it
    /// keeps out of the usage records instead (see `compute_fingerprint`).
    #[test]
    fn tracing_over_the_ceiling_does_not_change_the_fingerprint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let measuring = ExtractOptions {
            trace_over_ceiling: true,
            ..ExtractOptions::default()
        };
        assert_eq!(
            compute_fingerprint(&repo, &ExtractOptions::default()).unwrap(),
            compute_fingerprint(&repo, &measuring).unwrap()
        );
    }

    #[test]
    fn changing_ambiguity_ceiling_changes_fingerprint_for_the_same_repo_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let low = ExtractOptions {
            ambiguity_ceiling: 1,
            ..ExtractOptions::default()
        };
        let high = ExtractOptions {
            ambiguity_ceiling: 100,
            ..ExtractOptions::default()
        };

        let a = compute_fingerprint(&repo, &low).unwrap();
        let b = compute_fingerprint(&repo, &high).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn changing_secret_prefixes_changes_fingerprint_for_the_same_repo_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let none = ExtractOptions::default();
        let with_prefix = ExtractOptions {
            secret_prefixes: vec!["acmetok_".to_string()],
            ..ExtractOptions::default()
        };

        let a = compute_fingerprint(&repo, &none).unwrap();
        let b = compute_fingerprint(&repo, &with_prefix).unwrap();
        assert_ne!(a, b);
    }

    /// `scip_index` is deliberately excluded from the fingerprint
    /// (`compute_fingerprint`'s destructure comments why: it only
    /// configures the `scip` extractor, not one of the three
    /// `walk_files`-based passes this fingerprint gates) -- unlike
    /// `ambiguity_ceiling` and `secret_prefixes` above, changing it alone
    /// must NOT change the fingerprint. The exclusion is exercised here so a
    /// future refactor that starts folding it in by accident is caught.
    #[test]
    fn changing_scip_index_does_not_change_fingerprint_for_the_same_repo_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let none = ExtractOptions::default();
        let with_index = ExtractOptions {
            scip_index: Some(std::path::PathBuf::from("index.scip")),
            ..ExtractOptions::default()
        };

        let a = compute_fingerprint(&repo, &none).unwrap();
        let b = compute_fingerprint(&repo, &with_index).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn differing_only_in_cache_version_compares_unequal() {
        // `compute_fingerprint` always embeds the current `CACHE_VERSION`, so
        // to isolate this axis the two fingerprints are built directly
        // (fields are private to `cache`, but this test module is a
        // descendant of it) rather than by varying the crate constant, which
        // is not runtime-configurable.
        let same_files = vec![("src/lib.rs".to_string(), "deadbeef".to_string())];
        let a = RunFingerprint {
            cache_version: 1,
            tracked_files: same_files.clone(),
            ambiguity_ceiling: 8,
            secret_prefixes: Vec::new(),
            plugins: Vec::new(),
            resolver: String::new(),
        };
        let b = RunFingerprint {
            cache_version: 2,
            tracked_files: same_files,
            ambiguity_ceiling: 8,
            secret_prefixes: Vec::new(),
            plugins: Vec::new(),
            resolver: String::new(),
        };
        assert_ne!(a, b, "a version-only difference must still invalidate");
    }

    #[test]
    fn fingerprint_round_trips_through_bytes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let opts = ExtractOptions::default();

        let fingerprint = compute_fingerprint(&repo, &opts).unwrap();
        let bytes = fingerprint.to_bytes();
        let round_tripped = RunFingerprint::from_bytes(&bytes).unwrap();
        assert_eq!(fingerprint, round_tripped);
    }

    // `provenance_deserialize_recovers_the_same_static_extractor_id` was deleted
    // here in U3. It asserted `std::ptr::eq` against the `&'static str` from
    // `trust_ranks()`, and that premise is gone the moment `ExtractorId` becomes
    // an owned `Arc<str>` — there is no static to be identical to.
    //
    // The invariant underneath it is NOT gone, and is worth more now than it was:
    // "the decoded id is the one the run already knows, not a lookalike owned
    // string". Serde's `rc` impls allocate a fresh `Arc` per decoded value, so
    // without canonicalization a warm run would hold one allocation per fact
    // instead of one per id. The replacement assertion belongs with the
    // canonicalization that makes it true again, not here.

    /// An unknown extractor id now *decodes*, and that is the point of this
    /// test rather than a hole in it.
    ///
    /// Through U2 this asserted `serde_json::from_str::<Provenance>` returned
    /// an error. `Provenance` derives `Deserialize` as of U3, so the string
    /// decodes into an owned id like any other — there is no lookup left to
    /// fail. The honesty guard did not go away; it moved to where a stale
    /// record actually enters, and `walk.rs` owns the test that it still
    /// bites (`a_record_naming_an_unknown_extractor_is_not_served`).
    ///
    /// Pinning the decode here keeps the two halves of that story visible
    /// together: this is what the wire does, and the walk is what decides
    /// whether to trust it.
    #[test]
    fn an_unknown_extractor_id_decodes_and_is_refused_later_not_here() {
        let json = serde_json::json!({
            "file": "src/lib.rs",
            "span": "whole",
            "extractor": "not-a-real-extractor",
            "tier": "measured",
            "resolution": "heuristic",
        })
        .to_string();

        let decoded: Provenance =
            serde_json::from_str(&json).expect("an owned id decodes without a registry lookup");
        assert_eq!(&*decoded.extractor, "not-a-real-extractor");
        assert!(
            crate::extract::trust_ranks()
                .canonical(&decoded.extractor)
                .is_none(),
            "the run must still be able to tell this id is not one of its own"
        );
    }

    /// A record written by the PRE-U3 binary still decodes, and the run still
    /// recognizes the extractor it names.
    ///
    /// The fixture is a real artifact, not one this build produced: it was
    /// written by `reverse-architect` at 51dd4cb — the last commit before
    /// `ExtractorId` became an owned `Arc<str>` — by running a real generate
    /// over a two-declaration Rust file and copying one record out of
    /// `.ra-cache/languages-usages/`. Round-tripping a record written by *this*
    /// build would have assumed the thing under test — that the wire form did
    /// not move — rather than establishing it.
    ///
    /// Being precise about what that buys, since it is less than it looks: the
    /// wire form turned out to be unchanged, so a record written today over the
    /// same input would be byte-identical to this file, and nothing in the
    /// assertions below can tell the two apart. The value is in the capture,
    /// not the check — these bytes are evidence that the pre-U3 binary wrote
    /// exactly this. Regenerating the fixture from a current build would keep
    /// the test green while quietly destroying that evidence, so don't.
    ///
    /// It is also one `languages-usages` record, which holds two
    /// `GraphOp::Edge`s and an empty `relations` list — so it pins the pre-U3
    /// wire form for that shape only. Reachability across every other shape is
    /// covered in `walk.rs` and `treesitter.rs` instead.
    ///
    /// Two things have to hold, and the second is the one this change put at
    /// risk. The bytes must decode — they do, because the wire form never
    /// changed: an extractor id was always an owned string on disk, and only
    /// its in-memory type moved. And the id inside must still be one the run
    /// knows, because `walk_files` now refuses a record whose ids it cannot
    /// canonicalize. A guard that rejected every pre-existing record would
    /// throw away every warm cache on upgrade, silently, as a slow run.
    #[test]
    fn a_record_written_before_u3_decodes_but_names_an_id_this_build_retired() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("pre-u3-cache-record.json");
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("the pre-U3 fixture must be readable at {path:?}: {e}"));

        // Decoded through the U7 wire twin, which is what production reads
        // today. That makes this fixture carry a second claim it did not carry
        // before: the wire form the pre-U3 binary wrote is still exactly the one
        // the current binary reads. A twin that had changed the shape would show
        // up here as a decode failure, not as a silently cold cache.
        let record: super::Record<crate::extract::treesitter::FileOutputWire> =
            serde_json::from_slice(&bytes)
                .expect("a record written by the pre-U3 binary must still decode");
        assert_eq!(record.key, "src/lib.rs");

        use crate::extract::walk::WalkOutput;
        let mut ops = crate::extract::treesitter::FileOutput::from_wire(
            record.value,
            &crate::graph::Vocabulary::default(),
        )
        .expect("a pre-U3 record names only core kinds, which any vocabulary can name");
        let provenances = ops.provenances_mut();
        assert!(
            !provenances.is_empty(),
            "the fixture must carry provenance, or it proves nothing"
        );

        // Since U11 the facts name the plugin that wrote them, and `treesitter`
        // is no id this build runs. The record decodes, but served as a hit it
        // would bring an unknown extractor into the graph. That is the reason
        // `CACHE_VERSION` moved to 4: the fingerprint of every pre-U11 store
        // differs, so such a record is never read.
        let known = crate::extract::trust_ranks();
        for provenance in provenances {
            assert!(
                known.canonical(&provenance.extractor).is_none(),
                "{:?} is known again — the CACHE_VERSION 4 note no longer describes \
                 this build",
                provenance.extractor
            );
        }
        const { assert!(super::super::CACHE_VERSION >= 4) };
    }

    /// A cache record written before `ambiguous_candidates` existed has no such
    /// key in its bytes. It must still decode — as `None`, not an error — so
    /// adding the field never needed a `CACHE_VERSION` bump (KTD4).
    #[test]
    fn a_pre_field_cache_record_decodes_ambiguous_candidates_as_none() {
        let json = serde_json::json!({
            "file": "src/lib.rs",
            "span": "whole",
            "extractor": "filetree",
            "tier": "measured",
            "resolution": "heuristic",
        })
        .to_string();

        let provenance: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(provenance.ambiguous_candidates, None);
    }

    /// The new field itself round-trips like every other `Provenance` field.
    #[test]
    fn ambiguous_candidates_round_trips_through_the_cache_wire_format() {
        let provenance = Provenance::new(
            "src/lib.rs",
            Some(1),
            "treesitter".into(),
            crate::graph::Tier::Measured,
        )
        .ambiguous(3);
        let json = serde_json::to_string(&provenance).unwrap();
        let round_tripped: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.ambiguous_candidates, Some(3));
    }

    /// A record written before the confidence rung existed (plan A, U1) still
    /// DECODES — the rung and the rule come back absent — and is still never
    /// SERVED, because `CACHE_VERSION` moved to 5 and the record's fingerprint
    /// digest names the old version.
    ///
    /// Both halves matter, for the v3 reason in `CACHE_VERSION`'s history:
    /// the bytes are not wrong, the computation behind them is stale. The
    /// fixture's two `calls` edges are exactly the edges `flush_usages` now
    /// rates (`ambiguous` for the two `Save` overloads, `name_only` for
    /// `Load`), and served as a hit they would reach the graph with no rung at
    /// all.
    ///
    /// Both fixtures are real artifacts, captured at ac93cb3 — the last commit
    /// before the rung — by running that build's binary over a two-file C#
    /// repository (`Caller.cs` calling `Save()` and `Load()`, `Repo.cs`
    /// declaring two `Save` overloads and one `Load`) and copying the
    /// `languages-usages` record of `Caller.cs` and the store's
    /// `fingerprint.json` header. Don't regenerate them from a current build:
    /// that would keep the test green while destroying the evidence.
    #[test]
    fn a_pre_confidence_record_decodes_without_a_rung_but_the_new_version_refuses_it() {
        use crate::extract::treesitter::{FileOutput, FileOutputWire};
        use crate::extract::walk::WalkOutput;
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        let record_bytes = std::fs::read(fixtures.join("pre-confidence-cache-record.json"))
            .expect("the pre-confidence record fixture must be readable");
        let header_bytes = std::fs::read(fixtures.join("pre-confidence-cache-fingerprint.json"))
            .expect("the pre-confidence header fixture must be readable");

        // Half one: the bytes decode, through the wire twin production reads,
        // and no provenance in them carries a rung or a rule.
        let record: super::Record<FileOutputWire> = serde_json::from_slice(&record_bytes)
            .expect("a record written by the pre-confidence binary must still decode");
        assert_eq!(record.key, "Caller.cs");
        let mut ops = FileOutput::from_wire(record.value, &crate::graph::Vocabulary::default())
            .expect("the record names only core kinds");
        let provenances = ops.provenances_mut();
        assert!(
            provenances
                .iter()
                .any(|p| p.ambiguous_candidates == Some(2)),
            "precondition: the record holds the ambiguous `Save` edges"
        );
        for provenance in &provenances {
            assert_eq!(
                (provenance.confidence, provenance.rule),
                (None, None),
                "an absent key decodes as absent: {provenance:?}"
            );
        }

        // Half two: the header is the run that wrote the record, under the old
        // version, and the same run under the current version is a different
        // digest — so a store opened by this build never serves the record.
        let old = RunFingerprint::from_bytes(&header_bytes).expect("the header decodes");
        assert_eq!(
            old.digest(),
            record.fingerprint,
            "the header wrote this record"
        );
        assert_eq!(old.cache_version, 4);
        let current = RunFingerprint {
            cache_version: super::super::CACHE_VERSION,
            ..old.clone()
        };
        assert_ne!(current.digest(), record.fingerprint);

        let tmp = tempfile::TempDir::new().unwrap();
        let blob_id = git2::Oid::from_str(&old.tracked_files[0].1).unwrap();
        assert_eq!(old.tracked_files[0].0, "Caller.cs");
        let open = |digest: String| {
            CacheStore::new(
                tmp.path().to_path_buf(),
                digest,
                crate::extract::trust_ranks(),
                crate::graph::Vocabulary::default(),
            )
        };
        let path = open(String::new()).path_for("languages-usages", blob_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &record_bytes).unwrap();
        assert!(
            open(current.digest())
                .get::<FileOutputWire>("languages-usages", blob_id, "Caller.cs")
                .is_none(),
            "this build must not serve a record computed before the rung existed"
        );
        assert!(
            open(old.digest())
                .get::<FileOutputWire>("languages-usages", blob_id, "Caller.cs")
                .is_some(),
            "control: under its own digest the record is a hit, so the version alone refuses it"
        );
    }

    /// The whole basis for U15's "no `CACHE_VERSION` bump" decision, in one
    /// assertion: an unanchored provenance — which is every provenance any
    /// bundled extractor produces — writes no `anchor` key at all. The cache
    /// bytes a git-source run stores are therefore the same bytes the pre-U15
    /// binary stored, so no warm cache is invalidated and no stored record
    /// became stale.
    #[test]
    fn an_unanchored_provenance_writes_no_anchor_key() {
        let provenance = Provenance::new(
            "src/lib.rs",
            Some(1),
            "filetree".into(),
            crate::graph::Tier::Measured,
        );
        let json = serde_json::to_string(&provenance).unwrap();
        assert!(
            !json.contains("anchor"),
            "an absent anchor must be absent from the bytes, not written as null: {json}"
        );
    }

    /// And a record from before the field existed decodes as unanchored rather
    /// than failing — the other half of wire compatibility.
    #[test]
    fn a_pre_field_cache_record_decodes_its_anchor_as_none() {
        let json = serde_json::json!({
            "file": "src/lib.rs",
            "span": "whole",
            "extractor": "filetree",
            "tier": "measured",
            "resolution": "heuristic",
        })
        .to_string();

        let provenance: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(provenance.anchor, None);
    }

    /// An anchored provenance round-trips whole — container, entry and line —
    /// so the vocabulary U21 will populate is not merely declared but carried.
    #[test]
    fn an_anchor_round_trips_through_the_cache_wire_format() {
        let anchor = crate::graph::SourceAnchor {
            container: "vendor/lib.zip".to_string(),
            entry: "src/lib.rs".to_string(),
            line: Some(12),
        };
        let provenance = Provenance::new(
            "vendor/lib.zip",
            None,
            "filetree".into(),
            crate::graph::Tier::Measured,
        )
        .in_source(anchor.clone());
        let json = serde_json::to_string(&provenance).unwrap();
        let round_tripped: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.anchor, Some(anchor));
    }

    // ---- CacheStore (U3) ----

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Payload {
        value: String,
    }

    fn oid_for(bytes: &[u8]) -> git2::Oid {
        git2::Oid::hash_object(git2::ObjectType::Blob, bytes).unwrap()
    }

    /// A `CacheStore` with an arbitrary fixed digest (U7), for tests below
    /// that only exercise key/blob_id behavior — the fingerprint-matching
    /// tests further down construct stores with distinct digests directly.
    fn plain_store(root: std::path::PathBuf) -> CacheStore {
        CacheStore::new(
            root,
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        )
    }

    #[test]
    fn a_record_is_readable_immediately_after_it_is_written() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().to_path_buf());
        let blob_id = oid_for(b"pub fn f() {}\n");
        let payload = Payload {
            value: "hello".to_string(),
        };

        store.put("languages-decl", blob_id, "a.rs", &payload);
        let round_tripped: Option<Payload> = store.get("languages-decl", blob_id, "a.rs");

        assert_eq!(round_tripped, Some(payload));
    }

    #[test]
    fn a_missing_record_is_a_miss() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().to_path_buf());
        let blob_id = oid_for(b"never written");

        let result: Option<Payload> = store.get("languages-decl", blob_id, "a.rs");
        assert_eq!(result, None);
    }

    #[test]
    fn a_corrupted_record_is_a_miss_not_a_panic() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().to_path_buf());
        let blob_id = oid_for(b"pub fn f() {}\n");

        // Write valid content first, so the path/directory exist, then
        // corrupt it exactly the way a torn write would.
        store.put(
            "languages-decl",
            blob_id,
            "a.rs",
            &Payload {
                value: "hello".to_string(),
            },
        );
        let dir = tmp.path().join("languages-decl");
        let path = dir.join(format!("{blob_id}.json"));
        std::fs::write(&path, b"{ not valid json").unwrap();

        let result: Option<Payload> = store.get("languages-decl", blob_id, "a.rs");
        assert_eq!(result, None, "malformed JSON must degrade to a miss");
    }

    #[test]
    fn clear_removes_existing_records() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().to_path_buf());
        let blob_id = oid_for(b"pub fn f() {}\n");
        store.put(
            "languages-decl",
            blob_id,
            "a.rs",
            &Payload {
                value: "hello".to_string(),
            },
        );
        assert!(store
            .get::<Payload>("languages-decl", blob_id, "a.rs")
            .is_some());

        store.clear().unwrap();

        assert_eq!(
            store.get::<Payload>("languages-decl", blob_id, "a.rs"),
            None
        );
    }

    #[test]
    fn distinct_phase_ids_do_not_collide_for_the_same_blob() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().to_path_buf());
        let blob_id = oid_for(b"pub fn f() {}\n");
        store.put(
            "languages-decl",
            blob_id,
            "a.rs",
            &Payload {
                value: "decl".to_string(),
            },
        );
        store.put(
            "languages-usages",
            blob_id,
            "a.rs",
            &Payload {
                value: "usages".to_string(),
            },
        );

        assert_eq!(
            store.get::<Payload>("languages-decl", blob_id, "a.rs"),
            Some(Payload {
                value: "decl".to_string()
            })
        );
        assert_eq!(
            store.get::<Payload>("languages-usages", blob_id, "a.rs"),
            Some(Payload {
                value: "usages".to_string()
            })
        );
    }

    /// The collision case a blob-only key would miss entirely: two distinct
    /// files ("a.rs" and "b.rs") whose CONTENT is byte-identical share one
    /// blob id. A record `a.rs` wrote must never answer a lookup for `b.rs` —
    /// that would hand `b.rs` node keys and provenance that name `a.rs`.
    #[test]
    fn a_record_written_for_one_key_does_not_answer_a_lookup_for_another_key_sharing_its_blob() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().to_path_buf());
        let shared_blob = oid_for(b"pub fn f() {}\n"); // same bytes, two files

        store.put(
            "languages-decl",
            shared_blob,
            "a.rs",
            &Payload {
                value: "from a.rs".to_string(),
            },
        );

        let result: Option<Payload> = store.get("languages-decl", shared_blob, "b.rs");
        assert_eq!(
            result, None,
            "a record written for a.rs must not be handed back for b.rs, \
             even though they share a blob id"
        );
        // The record IS still readable under the key it was actually written
        // for — the mismatch degrades only the wrong-key lookup, nothing else.
        assert_eq!(
            store.get::<Payload>("languages-decl", shared_blob, "a.rs"),
            Some(Payload {
                value: "from a.rs".to_string()
            })
        );
    }

    // ---- Per-record fingerprint tag (U7, KTD9) ----

    /// The P0 `ce-code-review` finding this unit exists to close: two
    /// concurrent runs with DIFFERENT `ExtractOptions` sharing one
    /// `corpus_dir` could previously let a record written under one run's
    /// header be read as a valid hit by a store opened under a DIFFERENT
    /// header, because only `key` (never the fingerprint) was checked. A
    /// record's own digest must now match the reading store's digest, even
    /// when `key` and `blob_id` both match.
    #[test]
    fn a_record_from_a_different_fingerprint_digest_is_a_miss_even_when_key_and_blob_match() {
        let tmp = tempfile::TempDir::new().unwrap();
        let blob_id = oid_for(b"pub fn f() {}\n");
        let store_a = CacheStore::new(
            tmp.path().to_path_buf(),
            "digest-a".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        store_a.put(
            "languages-decl",
            blob_id,
            "a.rs",
            &Payload {
                value: "from run a".to_string(),
            },
        );

        // Same root, DIFFERENT digest — as if opened by a concurrently (or
        // subsequently) differently-configured run.
        let store_b = CacheStore::new(
            tmp.path().to_path_buf(),
            "digest-b".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        let result: Option<Payload> = store_b.get("languages-decl", blob_id, "a.rs");
        assert_eq!(
            result, None,
            "a record written under a different fingerprint digest must never \
             be served as a hit, even when key and blob_id both match"
        );
    }

    #[test]
    fn a_record_from_the_same_fingerprint_digest_is_still_a_hit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let blob_id = oid_for(b"pub fn f() {}\n");
        let store = CacheStore::new(
            tmp.path().to_path_buf(),
            "digest-a".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        store.put(
            "languages-decl",
            blob_id,
            "a.rs",
            &Payload {
                value: "from run a".to_string(),
            },
        );

        // A fresh handle over the SAME root and the SAME digest — the
        // ordinary warm-cache path — must still see the record.
        let reopened = CacheStore::new(
            tmp.path().to_path_buf(),
            "digest-a".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        let result: Option<Payload> = reopened.get("languages-decl", blob_id, "a.rs");
        assert_eq!(
            result,
            Some(Payload {
                value: "from run a".to_string()
            }),
            "the existing hit path must not regress"
        );
    }

    #[test]
    fn digest_is_deterministic_for_equal_fingerprints_and_differs_for_unequal_ones() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let low = ExtractOptions {
            ambiguity_ceiling: 1,
            ..ExtractOptions::default()
        };
        let high = ExtractOptions {
            ambiguity_ceiling: 100,
            ..ExtractOptions::default()
        };

        let a1 = compute_fingerprint(&repo, &low).unwrap();
        let a2 = compute_fingerprint(&repo, &low).unwrap();
        let b = compute_fingerprint(&repo, &high).unwrap();

        assert_eq!(
            a1.digest(),
            a2.digest(),
            "equal fingerprints must digest equal"
        );
        assert_ne!(
            a1.digest(),
            b.digest(),
            "differing fingerprints must digest differently"
        );
    }

    // ---- clear() failure signalling (U5) ----

    #[test]
    fn clear_on_a_root_that_never_existed_is_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = plain_store(tmp.path().join("never-written"));
        assert!(store.clear().is_ok(), "nothing to clear is not a failure");
    }

    #[test]
    fn clear_reports_failure_when_the_root_is_not_a_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("blocked");
        std::fs::write(&root, b"a plain file, not a directory").unwrap();

        let store = plain_store(root);
        assert!(
            store.clear().is_err(),
            "remove_dir_all on a non-directory must surface as an error, \
             not be swallowed like a missing root"
        );
    }

    // ---- activate() (U5, KTD8) ----

    fn read_header_bytes(root: &Path) -> Option<Vec<u8>> {
        std::fs::read(root.join(HEADER_FILE_NAME)).ok()
    }

    /// Test scenario 1: a clean run (no existing cache) leaves a readable
    /// header on disk whose fingerprint matches a freshly recomputed one for
    /// the same repo state.
    #[test]
    fn activation_on_a_cold_cache_writes_a_header_matching_a_fresh_fingerprint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let opts = ExtractOptions::default();
        let cache_root = tmp.path().join("cache");

        let fingerprint = compute_fingerprint(&repo, &opts).unwrap();
        let store = activate(
            cache_root.clone(),
            &fingerprint,
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        )
        .expect("a fresh, writable temp dir must activate");

        // The header on disk decodes to exactly the fingerprint just computed.
        let header_bytes = read_header_bytes(&cache_root).expect("a header was written");
        let header = RunFingerprint::from_bytes(&header_bytes).unwrap();
        assert_eq!(header, compute_fingerprint(&repo, &opts).unwrap());

        // And the store handed back is genuinely usable, not a dummy handle.
        let blob_id = repo
            .tracked_files_with_paths()
            .unwrap()
            .into_iter()
            .find(|tf| tf.key == "src/lib.rs")
            .unwrap()
            .blob_id;
        store.put(
            "languages-decl",
            blob_id,
            "src/lib.rs",
            &Payload {
                value: "hello".to_string(),
            },
        );
        assert!(store
            .get::<Payload>("languages-decl", blob_id, "src/lib.rs")
            .is_some());
    }

    /// The critical scenario this unit exists for (KTD8): the header must be
    /// visible and trusted by a NEXT activation even though the run that
    /// activated it never got as far as calling `run_all_with`/
    /// `generate_reporting` to completion — exactly the state an interrupted
    /// (killed, crashed) first run leaves behind. A process cannot literally
    /// be killed from within a test, so the interruption is simulated
    /// structurally: `activate` is called to reach the state a real run
    /// reaches the instant before extraction starts, ONE plausible per-file
    /// record is written directly via `CacheStore::put` (standing in for
    /// whatever partial extraction work an interrupted run left behind), and
    /// `run_all_with`/`generate_reporting` is never invoked at all for this
    /// "run". A second activation, over the identical unchanged repo state,
    /// must find the SAME header still valid (nothing gets cleared) and must
    /// still be able to read the seeded record back.
    #[test]
    fn a_header_written_before_extraction_survives_being_consulted_by_a_next_attempt_that_never_finished(
    ) {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let opts = ExtractOptions::default();
        let cache_root = tmp.path().join("cache");
        let blob_id = repo
            .tracked_files_with_paths()
            .unwrap()
            .into_iter()
            .find(|tf| tf.key == "src/lib.rs")
            .unwrap()
            .blob_id;

        // "Run 1" reaches the point right before extraction starts: the
        // header is written eagerly, and the store is (freshly) empty.
        let fingerprint = compute_fingerprint(&repo, &opts).unwrap();
        let store = activate(
            cache_root.clone(),
            &fingerprint,
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        )
        .expect("a fresh, writable temp dir must activate");
        let header_after_run1_setup =
            read_header_bytes(&cache_root).expect("run 1 must write a header eagerly");

        // Extraction starts writing per-file records as it goes (KTD5) — one
        // plausible record stands in for however far an interrupted run got.
        store.put(
            "languages-decl",
            blob_id,
            "src/lib.rs",
            &Payload {
                value: "seeded before the crash".to_string(),
            },
        );

        // "Run 1" is interrupted HERE. `run_all_with`/`generate_reporting` is
        // never called to completion for this run — no further code runs on
        // its behalf.

        // "Run 2": a retry over the SAME unchanged repo state.
        let fingerprint_again = compute_fingerprint(&repo, &opts).unwrap();
        assert_eq!(
            fingerprint, fingerprint_again,
            "precondition: the repo did not change between the two activations"
        );
        let store_again = activate(
            cache_root.clone(),
            &fingerprint_again,
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        )
        .expect("a fresh, writable temp dir must activate");

        // The header must be untouched — same bytes, meaning `activate` took
        // the "match" branch and cleared nothing.
        let header_after_run2_setup =
            read_header_bytes(&cache_root).expect("the header must still be there");
        assert_eq!(
            header_after_run1_setup, header_after_run2_setup,
            "an unchanged repo must not cause run 2's activation to clear \
             and rewrite the header"
        );

        // And the record run 1 seeded before being "interrupted" must still
        // be readable — proving a header written before extraction survives
        // being consulted by a next attempt without the original run ever
        // having finished.
        let seeded: Option<Payload> = store_again.get("languages-decl", blob_id, "src/lib.rs");
        assert_eq!(
            seeded,
            Some(Payload {
                value: "seeded before the crash".to_string()
            }),
            "a header written eagerly before extraction must survive being \
             consulted by a next attempt, without the original run needing \
             to have finished — this is the whole point of KTD8"
        );
    }

    /// Test scenario 4: changing the repo content between two activations
    /// over the SAME cache root causes the second activation to clear the
    /// store and write a new header — the header before/after differ, and a
    /// record seeded under the OLD fingerprint's regime is gone afterward.
    #[test]
    fn a_mismatched_fingerprint_clears_the_store_and_writes_a_new_header_before_returning() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache_root = tmp.path().join("cache");
        let opts = ExtractOptions::default();

        let repo_a_dir = tempfile::TempDir::new().unwrap();
        let repo_a = repo_with_files(repo_a_dir.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let fingerprint_a = compute_fingerprint(&repo_a, &opts).unwrap();
        let blob_a = repo_a.tracked_files_with_paths().unwrap()[0].blob_id;

        let store_a = activate(
            cache_root.clone(),
            &fingerprint_a,
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        )
        .unwrap();
        store_a.put(
            "languages-decl",
            blob_a,
            "src/lib.rs",
            &Payload {
                value: "from the old regime".to_string(),
            },
        );
        let header_before = read_header_bytes(&cache_root).unwrap();

        let repo_b_dir = tempfile::TempDir::new().unwrap();
        let repo_b = repo_with_files(repo_b_dir.path(), &[("src/lib.rs", "pub fn g() {}\n")]);
        let fingerprint_b = compute_fingerprint(&repo_b, &opts).unwrap();
        assert_ne!(
            fingerprint_a, fingerprint_b,
            "precondition: changed content must change the fingerprint"
        );

        let store_b = activate(
            cache_root.clone(),
            &fingerprint_b,
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        )
        .expect("a fresh, writable temp dir must activate");
        let header_after = read_header_bytes(&cache_root).unwrap();

        assert_ne!(
            header_before, header_after,
            "a mismatched fingerprint must rewrite the header"
        );
        assert_eq!(
            RunFingerprint::from_bytes(&header_after).unwrap(),
            fingerprint_b
        );

        let survived: Option<Payload> = store_b.get("languages-decl", blob_a, "src/lib.rs");
        assert_eq!(
            survived, None,
            "a fingerprint mismatch must clear every record from the prior regime"
        );
    }

    /// Test scenario 3 (unit-level half): activation degrades to `None`
    /// rather than panicking or propagating when the cache root cannot be
    /// created — here, because a plain file already occupies that path. The
    /// end-to-end proof that `generate_reporting` still returns
    /// `Ok(Generated { .. })` when this happens lives in
    /// `tests/cache_activation.rs`, which only has access to the public API.
    #[test]
    fn activation_degrades_to_none_when_the_cache_root_cannot_be_created() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("src/lib.rs", "pub fn f() {}\n")]);
        let opts = ExtractOptions::default();

        // A plain FILE sits where the cache root directory needs to be. No
        // header is readable at a path that treats a file as a directory
        // (so activation takes the clear+write branch), and then
        // `CacheStore::clear`'s `remove_dir_all` fails on a non-directory —
        // portably across platforms, unlike simulating a permissions error.
        let blocked_root = tmp.path().join("blocked-cache-root");
        std::fs::write(&blocked_root, b"not a directory").unwrap();

        let fingerprint = compute_fingerprint(&repo, &opts).unwrap();
        assert!(
            activate(
                blocked_root,
                &fingerprint,
                crate::extract::trust_ranks(),
                crate::graph::Vocabulary::default(),
            )
            .is_none(),
            "an unusable cache root must degrade to no cache, never panic \
             or propagate an error"
        );
    }
}
