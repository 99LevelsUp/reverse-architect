//! The shape every per-file walk shares: read, parse in parallel, apply in
//! order (U1/KTD1).
//!
//! Three extractors walk every tracked file — tree-sitter twice, once for
//! declarations and once for usages, and `external` once. Together they are
//! four fifths of a run over the R17 target, and they were four fifths spent on
//! one core.
//!
//! ## What is shared, and what deliberately is not
//!
//! [`walk_files`] is shared: batching, blob reads, file order, the progress
//! cadence. The **reducer is not**. Each walk decides for itself what its
//! workers may compute and what only a thread that can see the graph may decide
//! — see [`GraphOp::External`] for the case that forced the distinction.
//!
//! ## Why the walks buffer instead of mutating
//!
//! A walk that holds `&mut Graph` cannot leave its thread. More to the point, a
//! walk that can READ the graph can depend on how much of it is built when it
//! runs, and that dependence is invisible until the day the file order changes.
//! Buffering removes the possibility rather than auditing for it: a worker
//! given no graph has nothing to depend on.
//!
//! What a worker genuinely cannot decide alone is deferred to the reducer, not
//! guessed at — that is the whole content of [`GraphOp`].

use super::cache::CacheStore;
use super::wire::WireOps;
use super::{ExtractError, Repo, TrackedFile};
use crate::graph::{Edge, Fact, Graph, NodeKey, NodeKind, Provenance, Vocabulary};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
// The test fixtures below are their own wire form, so they derive both.
#[cfg(test)]
use serde::Deserialize;

/// One buffered change to the graph.
///
/// Deliberately not a flat list of `add_fact`/`add_edge` calls: three of the
/// four variants exist because something about the change cannot be settled
/// without the graph, and flattening them would settle it wrongly and silently.
///
/// **Not serializable, deliberately** (U7). A kind is an identity in memory and
/// a name on disk, and the translation needs the corpus's [`Vocabulary`], which
/// serde cannot be handed. The cache therefore goes through [`WireOps`]; taking
/// the derives off this type is what stops a future caller from re-adding the
/// path that would have written a raw digest into a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GraphOp {
    /// A fact on a node someone else created.
    Fact { key: NodeKey, fact: Fact },

    /// A symbol node and everything hung off it, as one group.
    ///
    /// Grouped because creating the node GUARDS the rest. [`Graph::ensure_node`]
    /// rejects a key already held by a node of another kind, and the walk
    /// answers that by abandoning the whole symbol rather than attaching its
    /// facts to the wrong node (E4). A flat op list would apply those facts
    /// anyway: the guard would still fire and the thing it guards would happen.
    Symbol {
        key: NodeKey,
        facts: Vec<Fact>,
        edges: Vec<(NodeKey, NodeKey, Edge)>,
    },

    /// An external node, whose identity facts are emitted **only if this
    /// observation is the first one**.
    ///
    /// This is the variant that proves a reducer cannot be shared. Two files
    /// referencing one external must not stack duplicate scalar claims, so the
    /// name and kind are emitted once, carrying the provenance of whichever file
    /// touched the external FIRST. A worker cannot know whether it is first —
    /// that is a fact about the graph, not about the file — so it emits the
    /// identity unconditionally and the reducer decides. Order of application is
    /// therefore load-bearing: it picks which file's provenance the fact keeps
    /// (KTD2).
    External {
        key: NodeKey,
        /// Applied only when the node did not exist before this op.
        identity: Vec<Fact>,
        /// The `Uses` edge from the enclosing declaration, if the file wants one.
        uses: Option<(NodeKey, Edge)>,
    },

    /// An edge between nodes this op does not create. It survives only if both
    /// endpoints already exist — [`Graph::add_edge`] returns `false` otherwise —
    /// which is another reason application order is a correctness requirement
    /// and not a tidiness preference (KTD2).
    Edge {
        from: NodeKey,
        to: NodeKey,
        edge: Edge,
    },
}

impl GraphOp {
    /// Attach a fact to an open symbol group. A no-op on any other variant,
    /// which cannot occur: the only caller holds a handle it just opened.
    pub(crate) fn push_fact(&mut self, fact: Fact) {
        if let GraphOp::Symbol { facts, .. } = self {
            facts.push(fact);
        }
    }

    /// Attach an edge to an open symbol group, on the same terms as
    /// [`GraphOp::push_fact`].
    pub(crate) fn push_edge(&mut self, from: NodeKey, to: NodeKey, edge: Edge) {
        if let GraphOp::Symbol { edges, .. } = self {
            edges.push((from, to, edge));
        }
    }
}

/// What `walk_files` needs of a cached output beyond serializing it.
///
/// One method today. U10 adds a second (`is_failure`, so a placeholder for a
/// failed unit of work is never written to the cache) — the seam is here, and
/// the bound on `Out` already exists, so that unit adds a method rather than a
/// trait plus a bound on three call sites.
pub(crate) trait WalkOutput: Sized + Send + Sync {
    /// This output's on-disk twin — the only form the cache ever sees (U7).
    ///
    /// An associated type rather than a free pair of functions because
    /// `walk_files` is generic over the output and has to name what it reads
    /// back before it has one. It is also the seam the vocabulary enters
    /// through: everything below takes `&Vocabulary` because the wire form is
    /// where a kind stops being an identity and becomes a name.
    type Wire: Serialize + DeserializeOwned;

    /// This output in wire form, for [`CacheStore::put`] — or `None` when this
    /// vocabulary cannot name a kind it carries, in which case the record is not
    /// written at all. Refusing to write beats writing a record that can never
    /// be read back (RG2).
    fn to_wire(&self, vocabulary: &Vocabulary) -> Option<Self::Wire>;

    /// This output read back from wire form, or `None` when this corpus cannot
    /// name a kind the record uses — see [`WireOps::into_ops`].
    fn from_wire(wire: Self::Wire, vocabulary: &Vocabulary) -> Option<Self>;

    /// Every provenance this output carries, in no particular order.
    ///
    /// A `Vec` rather than an `impl Iterator`: the implementations chain
    /// borrows across several nested collections, and one allocation per
    /// cache hit is not worth the lifetime gymnastics that would avoid.
    fn provenances_mut(&mut self) -> Vec<&mut Provenance>;

    /// Whether this output stands for a unit of work that failed (a plugin
    /// trap, exhausted fuel). A failed output is still applied — it carries
    /// nothing — but it is never written to the cache, so the next run tries
    /// the file again instead of serving the failure as a result (R29).
    fn is_failure(&self) -> bool {
        false
    }
}

/// Swap every extractor id for the run's own, refusing one this run does not
/// know.
///
/// Both halves matter and neither is optional.
///
/// **Refusing** is the honesty guard the hand-written `Deserialize` in
/// `extract::cache` used to provide. It made an unknown id a decode error,
/// which `CacheStore::get` turns into a miss, so the file was re-parsed. With a
/// derived `Deserialize` an unknown id decodes cleanly and the record would be
/// served — a corpus built by a build that no longer exists, attributed to an
/// extractor this one does not have.
///
/// **Swapping** is what keeps a warm run's memory flat. Serde's `rc` impls do
/// not preserve sharing: decoding N provenances that all name `treesitter`
/// yields N separate allocations. Replacing each with the registry's own `Arc`
/// drops them back to one, which is what `&'static str` gave for free before
/// the id had to exist at run time.
fn canonicalize_ids<Out: WalkOutput>(out: &mut Out, known: &crate::graph::Ranks) -> bool {
    for provenance in out.provenances_mut() {
        match known.canonical(&provenance.extractor) {
            Some(id) => provenance.extractor = id,
            None => return false,
        }
    }
    true
}

impl WalkOutput for GraphOps {
    type Wire = WireOps;

    fn to_wire(&self, vocabulary: &Vocabulary) -> Option<WireOps> {
        GraphOps::to_wire(self, vocabulary)
    }

    fn from_wire(wire: WireOps, vocabulary: &Vocabulary) -> Option<Self> {
        wire.into_ops(vocabulary)
    }

    fn provenances_mut(&mut self) -> Vec<&mut Provenance> {
        let mut out = Vec::new();
        // Every field is bound by name and none is discarded with `..`. That is
        // the point: a field added to any variant later must not compile until
        // someone decides whether it carries provenance. Missing one is
        // undetectable by any output check this project has — an
        // un-canonicalized id is CONTENT-EQUAL to the canonical one, so it
        // renders and hashes identically and every snapshot stays green. The
        // same discipline `compute_fingerprint` applies to `ExtractOptions`
        // (`cache.rs`), and for the same reason.
        for op in &mut self.0 {
            match op {
                GraphOp::Fact { key: _, fact } => out.extend(fact.provenances_mut()),
                GraphOp::Symbol {
                    key: _,
                    facts,
                    edges,
                } => {
                    for fact in facts {
                        out.extend(fact.provenances_mut());
                    }
                    for (_, _, edge) in edges {
                        out.extend(edge.provenances_mut());
                    }
                }
                GraphOp::External {
                    key: _,
                    identity,
                    uses,
                } => {
                    for fact in identity {
                        out.extend(fact.provenances_mut());
                    }
                    if let Some((_, edge)) = uses {
                        out.extend(edge.provenances_mut());
                    }
                }
                GraphOp::Edge {
                    from: _,
                    to: _,
                    edge,
                } => out.extend(edge.provenances_mut()),
            }
        }
        out
    }
}

/// One file's buffered changes, in emission order. See [`GraphOp`] for why it
/// is not serializable and [`WireOps`] for what goes to disk instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct GraphOps(pub(in crate::extract) Vec<GraphOp>);

impl GraphOps {
    pub(crate) fn fact(&mut self, key: NodeKey, fact: Fact) {
        self.0.push(GraphOp::Fact { key, fact });
    }

    pub(crate) fn edge(&mut self, from: NodeKey, to: NodeKey, edge: Edge) {
        self.0.push(GraphOp::Edge { from, to, edge });
    }

    pub(crate) fn external(
        &mut self,
        key: NodeKey,
        identity: Vec<Fact>,
        uses: Option<(NodeKey, Edge)>,
    ) {
        self.0.push(GraphOp::External {
            key,
            identity,
            uses,
        });
    }

    /// Open a symbol group and hand back a handle for what belongs to it.
    pub(crate) fn symbol(&mut self, key: NodeKey) -> &mut GraphOp {
        self.0.push(GraphOp::Symbol {
            key,
            facts: Vec::new(),
            edges: Vec::new(),
        });
        self.0.last_mut().expect("just pushed")
    }

    /// Apply this file's changes, in emission order.
    ///
    /// Reads as one long match, and that is the point: this is the only place
    /// the graph is touched by any of the three walks.
    ///
    /// Every decision the walk used to make against a live graph is made here
    /// instead, by the same `Graph` methods, in the same order — so the graph
    /// sees the identical sequence of calls it saw when the walk mutated it
    /// directly. That is what makes byte identity a consequence of the design
    /// rather than a hope about it.
    pub(crate) fn apply(self, graph: &mut Graph) {
        self.apply_with_kinds(graph, &HashMap::new());
    }

    /// [`GraphOps::apply`], creating a symbol whose key `declared` names as a
    /// node of that kind rather than as a [`NodeKind::Symbol`] — a symbol of a
    /// node kind from a plugin's own vocabulary (contract 0.3). Nothing else
    /// about the group changes: the same guard, the same facts and edges, in
    /// the same order. An empty map is exactly `apply`.
    pub(crate) fn apply_with_kinds(self, graph: &mut Graph, declared: &HashMap<NodeKey, NodeKind>) {
        for op in self.0 {
            match op {
                GraphOp::Fact { key, fact } => {
                    graph.add_fact(&key, fact);
                }
                GraphOp::Symbol { key, facts, edges } => {
                    let kind = declared.get(&key).copied().unwrap_or(NodeKind::Symbol);
                    // The `sym:` prefix keeps this disjoint from file/module
                    // keys, so a collision is unreachable in practice; if the
                    // invariant ever broke, skip this symbol rather than attach
                    // its facts to a wrong-kind node (E4 idiom).
                    if let Err(e) = graph.ensure_node(key.clone(), kind) {
                        eprintln!("treesitter: {e}");
                        continue;
                    }
                    for fact in facts {
                        graph.add_fact(&key, fact);
                    }
                    for (from, to, edge) in edges {
                        graph.add_edge(&from, &to, edge);
                    }
                }
                GraphOp::External {
                    key,
                    identity,
                    uses,
                } => {
                    // Read before the create, or every observation looks new.
                    let is_new = graph.node(&key).is_none();
                    // The `ext:`/`res:` prefixes make a collision unreachable in
                    // practice; if the invariant ever broke, skip rather than
                    // corrupt a wrong-kind node (E4).
                    if let Err(e) = graph.ensure_node(key.clone(), NodeKind::External) {
                        eprintln!("external: {e}");
                        continue;
                    }
                    if is_new {
                        for fact in identity {
                            graph.add_fact(&key, fact);
                        }
                    }
                    if let Some((owner, edge)) = uses {
                        graph.add_edge(&owner, &key, edge);
                    }
                }
                GraphOp::Edge { from, to, edge } => {
                    graph.add_edge(&from, &to, edge);
                }
            }
        }
    }
}

/// How many files are read, parsed and applied before the batch is dropped.
///
/// A count, not a byte budget: this bounds how many blobs and outputs are held
/// at once, and a batch of unusually large files is correspondingly larger. It
/// is a ceiling on the shape of the buffer rather than on its size, which is
/// enough here — peak memory over the R17 target rose 4% (2.63 GB to 2.74 GB)
/// against a budget of double.
///
/// The value is not a speed choice: parse time over that target is flat from
/// 128 to 14,396 files per batch, so batch boundaries are not where this walk
/// loses time. The numbers were measured when the file walks went
/// parallel.
const BATCH: usize = 512;

/// Run one per-file walk: read a batch's blobs, parse them in parallel, apply
/// the results in file order.
///
/// `wants` decides which files this walk has any use for; the rest are counted
/// and skipped. `parse` receives a batch of `(graph key, bytes)` — MISSES only,
/// once caching is in play, see below — and must return one output per input,
/// **in input order**. `apply` receives one output per wanted, non-symlink,
/// readable file, in file order, regardless of whether that output came from
/// `parse` or from the cache, and is the only thing that touches the graph.
///
/// `phase_id` names this pass for the cache (`"languages-decl"`,
/// `"languages-usages"`, `"external"`, …) so its records never collide with
/// another pass's for the same blob. `cache` is `None` for a plain walk — in
/// which case every file is a miss and behavior is byte-for-byte what it was
/// before caching existed — or `Some(&store)` when the caller (U5) has already
/// checked the run's [`super::cache::RunFingerprint`] against the one on disk
/// and found them equal (KTD1); this function does not itself check validity,
/// it trusts `Some` to mean "valid."
///
/// A file already recorded under `(phase_id, its blob id, its own graph key)`
/// is never handed to `parse` at all — its cached value is read directly into
/// the position `apply` will see. Every output `parse` DOES produce is written
/// back to the store immediately, before the next batch starts (KTD5): a crash
/// between batches leaves every already-processed file's record on disk, not
/// just the records of a run that reached the end.
///
/// **Blobs are read here, serially, on the handle the caller already holds.**
/// `git2::Repository` is not `Sync`, so a worker could not share it — but
/// measured over the R17 target, reading every one of the 14,396 blobs costs
/// 0:01 against the declaration walk's 0:33. Roughly 3%, which buys nothing
/// worth a second repository handle per worker, a thread-local cache, or a new
/// class of worker-initialization failure (KTD6). The figures were measured
/// when the file walks went parallel.
// `phase_id` and `cache` (U3) join an already-established `repo`/`tracked`/
// `progress`/`wants`/`parse`/`apply` shape (see `treesitter.rs`'s existing
// `#[allow(clippy::too_many_arguments)]` call sites for the same idiom) —
// splitting into a struct would hide the "these two are optional, the rest
// are not" distinction this driver relies on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn walk_files<Out>(
    repo: &Repo,
    tracked: &[TrackedFile],
    progress: &dyn crate::progress::Progress,
    phase_id: &str,
    cache: Option<&CacheStore>,
    wants: impl Fn(&str) -> bool,
    parse: impl Fn(&[(String, Vec<u8>)]) -> Vec<Out> + Sync,
    mut apply: impl FnMut(Out),
) -> Result<(), ExtractError>
where
    Out: WalkOutput,
{
    let total = tracked.len();
    let mut seen = 0usize;
    for chunk in tracked.chunks(BATCH) {
        // Every wanted, readable, non-symlink file in this chunk gets a slot
        // here, in file order — a cache hit fills its slot immediately, a miss
        // is left `None` and backfilled once `parse` returns. Order therefore
        // survives the hit/miss split exactly as it always survived pure
        // parsing: by index, never by arrival.
        let mut results: Vec<Option<Out>> = Vec::with_capacity(chunk.len());
        let mut miss_batch: Vec<(String, Vec<u8>)> = Vec::with_capacity(chunk.len());
        // `miss_blob_ids[i]` and `miss_positions[i]` describe `miss_batch[i]`:
        // which blob a freshly-parsed output belongs to, and which slot in
        // `results` it fills — both needed only after `parse` returns.
        let mut miss_blob_ids: Vec<git2::Oid> = Vec::with_capacity(chunk.len());
        let mut miss_positions: Vec<usize> = Vec::with_capacity(chunk.len());

        for tf in chunk {
            seen += 1;
            // Progress counts every tracked file, including the ones about to be
            // skipped: the operator is watching a walk, and one that counted only
            // what it could parse would appear to stall and then leap. The serial
            // read owns the counter, so no worker ever touches progress and the
            // cadence stays exactly what `should_tick` says (KTD8).
            if crate::progress::should_tick(seen, total) {
                progress.tick(seen);
            }
            if !wants(&tf.key) {
                continue;
            }
            if tf.is_symlink {
                continue;
            }
            // Cache checked BEFORE the blob is read: `tf.blob_id` is already on
            // `TrackedFile`, so a hit needs no git object lookup at all. Reading
            // the blob unconditionally would pay a real, measured cost (see
            // `walk_files`'s own doc comment: reading every blob costs ~3% of an
            // uncached run) on every file of every warm run, for bytes a hit
            // throws away unused — exactly the work caching exists to skip.
            if let Some(store) = cache {
                if let Some(wire) = store.get::<Out::Wire>(phase_id, tf.blob_id, &tf.key) {
                    // Two refusals, one shape. A record naming a KIND this corpus
                    // cannot name (U7 — a renamed declaration) and a record naming
                    // an EXTRACTOR this run does not have both fall through to a
                    // re-parse, exactly as the second did when the check lived in
                    // `Provenance`'s hand-written decoder — degrading to work,
                    // never to a wrong answer.
                    if let Some(mut out) = Out::from_wire(wire, store.vocabulary()) {
                        if canonicalize_ids(&mut out, store.known_ids()) {
                            results.push(Some(out));
                            continue;
                        }
                    }
                }
            }
            // Content as the source defines it, checked against `tf.blob_id` by
            // `Repo::blob_bytes`. Under git that is the INDEX BLOB and not the
            // working tree (KTD10 determinism: same commit → same bytes → same
            // symbols, regardless of a dirty tree or autocrlf); under a directory
            // it is the file, and the id check is what keeps a file rewritten
            // mid-run from being parsed and then cached under the id it no longer
            // has. A symlink's bytes are its target path text rather than source,
            // so skip it. Every failure here — unreadable, missing, or changed
            // underneath the run — degrades to a skipped file, not a fatal
            // error (E4). The blob id below is therefore always the enumerated
            // one, and always the one the parsed bytes hash to.
            let Ok(src) = repo.blob_bytes(tf) else {
                continue;
            };
            miss_positions.push(results.len());
            miss_blob_ids.push(tf.blob_id);
            results.push(None);
            miss_batch.push((tf.key.clone(), src));
        }
        if results.is_empty() {
            continue;
        }
        let outputs = if miss_batch.is_empty() {
            // Every wanted file in this chunk was already cached — a real
            // batch with nothing to parse, not the "no wanted files" case
            // above. `parse` is a caller-supplied closure with no contract
            // for an empty slice beyond "return as many outputs as inputs",
            // so it is simply not called.
            Vec::new()
        } else {
            parse(&miss_batch)
        };
        // The contract above is a contract, so it is checked. A `parse` that
        // dropped or reordered an output would shift every later file's result
        // onto the wrong input, and the corpus would still look plausible.
        assert_eq!(
            outputs.len(),
            miss_batch.len(),
            "a walk returned {} outputs for {} inputs; one per input, in input order",
            outputs.len(),
            miss_batch.len()
        );
        // Written to the store BEFORE they land in `results` — a batch's
        // records are on disk as soon as this batch is done, not only once the
        // whole walk returns (KTD5). Tagged with the file's own key, not just
        // its blob id: two files sharing a blob (identical content) must never
        // let one's cached output answer for the other (see `CacheStore`'s doc
        // comment).
        if let Some(store) = cache {
            write_records(store, phase_id, &miss_batch, &miss_blob_ids, &outputs);
        }
        for (i, out) in outputs.into_iter().enumerate() {
            results[miss_positions[i]] = Some(out);
        }
        for out in results {
            apply(
                out.expect("every slot is filled by either a cache hit or a freshly parsed output"),
            );
        }
    }
    Ok(())
}

/// Write one batch's fresh outputs to the store, in parallel.
///
/// Serializing a record and creating its file is per-file work with nothing
/// shared, and done serially it was the largest part of a language walk over a
/// large repository: 16,000 small files a pass, each a JSON encode and a file
/// create, while every worker waited for the next batch.
///
/// Two things the serial loop gave for free are kept on purpose:
///
/// - **The last file with a blob wins.** Two files with identical content share
///   one record path. Serially the later one overwrote the earlier; here only
///   the last file per blob in the batch writes at all, so the same record
///   survives and no two threads race for one path.
/// - **Messages come out in file order.** A record this store cannot name is
///   not written. That case means the walk declared a kind the store's
///   vocabulary snapshot does not hold — an A2 violation — and the honest answer
///   is a cache miss next run, not a record on disk naming `undeclared:<hex>`
///   that no run will ever read back (RG2). Each such file is reported, in the
///   order the files were read.
fn write_records<Out: WalkOutput>(
    store: &CacheStore,
    phase_id: &str,
    batch: &[(String, Vec<u8>)],
    blob_ids: &[git2::Oid],
    outputs: &[Out],
) {
    use rayon::prelude::*;
    let mut last: std::collections::HashMap<git2::Oid, usize> =
        std::collections::HashMap::with_capacity(blob_ids.len());
    for (i, id) in blob_ids.iter().enumerate() {
        last.insert(*id, i);
    }
    let unnamed: Vec<Option<&str>> = outputs
        .par_iter()
        .enumerate()
        .map(|(i, out)| {
            if out.is_failure() || last.get(&blob_ids[i]) != Some(&i) {
                return None;
            }
            match out.to_wire(store.vocabulary()) {
                Some(wire) => {
                    store.put(phase_id, blob_ids[i], &batch[i].0, &wire);
                    None
                }
                None => Some(batch[i].0.as_str()),
            }
        })
        .collect();
    for key in unnamed.into_iter().flatten() {
        eprintln!("cache: not writing {key} — it names a kind this run's vocabulary does not hold");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeKind, FactKind, FactValue, Provenance, Tier};

    fn prov() -> Provenance {
        Provenance::new("src/lib.rs", Some(1), "filetree".into(), Tier::Measured)
    }

    /// U1: `GraphOps` buffers exactly what a per-file walk hands the reducer,
    /// so the on-disk cache a later unit adds must be able to read one back
    /// unchanged — including the nested `Vec<Fact>` / `Vec<(NodeKey, NodeKey,
    /// Edge)>` the `Symbol` and `External` variants carry.
    #[test]
    fn graph_ops_round_trips_through_json() {
        let mut ops = GraphOps::default();

        let sym_key = NodeKey::new("sym:rust:crate::a#T");
        let handle = ops.symbol(sym_key.clone());
        handle.push_fact(Fact::new(
            FactKind::SymbolName,
            FactValue::Text("T".into()),
            prov(),
        ));
        let other = NodeKey::new("sym:rust:crate::a#U");
        handle.push_edge(sym_key.clone(), other, Edge::new(EdgeKind::Calls, prov()));

        ops.external(
            NodeKey::new("ext:rust:std::fmt::Debug"),
            vec![Fact::new(
                FactKind::ExternalName,
                FactValue::Text("std::fmt::Debug".into()),
                prov(),
            )],
            Some((sym_key, Edge::new(EdgeKind::Uses, prov()))),
        );

        // Through the wire twin, which is the only way these ops ever reach a
        // disk (U7). The round trip is therefore the real one: kind → word →
        // JSON → word → kind.
        let vocabulary = crate::graph::Vocabulary::default();
        let json = serde_json::to_string(
            &ops.to_wire(&vocabulary)
                .expect("every kind in this fixture is one the vocabulary can name"),
        )
        .unwrap();
        let wire: WireOps = serde_json::from_str(&json).unwrap();
        let round_tripped = wire
            .into_ops(&vocabulary)
            .expect("every kind here is a core kind, which any vocabulary can name");
        assert_eq!(ops, round_tripped);
    }

    // ---- walk_files × CacheStore (U3) ----

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A minimal `Out` for these tests: which file it came from, and whether
    /// it was read from the cache or freshly parsed — enough to assert both
    /// "did `parse` run" and "is file order preserved" without pulling in a
    /// real extractor's output type.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Marker {
        key: String,
        from_cache: bool,
    }

    /// A parse result no cached fixture in this file equals, so "was it served
    /// or re-parsed?" is answerable from the applied value alone.
    fn freshly_parsed() -> GraphOps {
        let mut ops = GraphOps::default();
        ops.fact(
            NodeKey::new("freshly-parsed"),
            Fact::new(
                crate::graph::FactKind::Loc,
                crate::graph::FactValue::Int(-1),
                Provenance::new(
                    "a.rs",
                    None,
                    crate::extract::ids::filetree(),
                    crate::graph::Tier::Measured,
                ),
            ),
        );
        ops
    }

    /// Carries no provenance, so canonicalization has nothing to do and a hit
    /// is always accepted — which is what keeps these tests about the driver's
    /// hit/miss bookkeeping rather than about id validation.
    impl WalkOutput for Marker {
        /// Its own twin: a marker carries no kind, so there is nothing for a
        /// vocabulary to translate and the wire form is the value itself. The
        /// trait allows that on purpose — only an output that actually holds
        /// kinds pays for a mirrored type.
        type Wire = Marker;

        fn to_wire(&self, _vocabulary: &crate::graph::Vocabulary) -> Option<Marker> {
            Some(self.clone())
        }

        fn from_wire(wire: Marker, _vocabulary: &crate::graph::Vocabulary) -> Option<Marker> {
            Some(wire)
        }

        fn provenances_mut(&mut self) -> Vec<&mut Provenance> {
            Vec::new()
        }
    }

    /// Mirrors the fixture pattern in `cache.rs`'s own tests (and
    /// `tests/common::init_repo_with_files`): a real `Repo` over a real git
    /// commit, not hand-built `TrackedFile`s, so `blob_id` is the actual git
    /// blob id a `CacheStore` record must be keyed on.
    fn repo_with_files(dir: &std::path::Path, files: &[(&str, &str)]) -> Repo {
        for (rel, contents) in files {
            let full = dir.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, contents).unwrap();
        }
        let repository = git2::Repository::init(dir).unwrap();
        let mut index = repository.index().unwrap();
        for (rel, _) in files {
            index.add_path(std::path::Path::new(rel)).unwrap();
        }
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::new(
            "Ann",
            "ann@example.test",
            &git2::Time::new(1_600_000_000, 0),
        )
        .unwrap();
        repository
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        Repo::open(dir).unwrap()
    }

    fn counting_parse(calls: &AtomicUsize) -> impl Fn(&[(String, Vec<u8>)]) -> Vec<Marker> + '_ {
        move |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            batch
                .iter()
                .map(|(key, _)| Marker {
                    key: key.clone(),
                    from_cache: false,
                })
                .collect()
        }
    }

    #[test]
    fn a_cache_hit_is_never_handed_to_parse() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let blob_id = tracked[0].blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        cache.put(
            "test-phase",
            blob_id,
            "a.rs",
            &Marker {
                key: "a.rs".to_string(),
                from_cache: true,
            },
        );

        let parse_calls = AtomicUsize::new(0);
        let mut applied = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            counting_parse(&parse_calls),
            |out: Marker| applied.push(out),
        )
        .unwrap();

        assert_eq!(
            parse_calls.load(Ordering::SeqCst),
            0,
            "a file already in the cache must never reach parse"
        );
        assert_eq!(
            applied,
            vec![Marker {
                key: "a.rs".to_string(),
                from_cache: true,
            }]
        );
    }

    #[test]
    fn a_freshly_parsed_output_is_readable_from_the_cache_once_walk_files_returns() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let blob_id = tracked[0].blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        let parse_calls = AtomicUsize::new(0);

        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            counting_parse(&parse_calls),
            |_out: Marker| {},
        )
        .unwrap();

        let cached: Option<Marker> = cache.get("test-phase", blob_id, "a.rs");
        assert_eq!(
            cached,
            Some(Marker {
                key: "a.rs".to_string(),
                from_cache: false,
            }),
            "a fresh parse must be written back to the store, keyed on the file's blob id"
        );
    }

    #[test]
    fn a_corrupted_cache_record_degrades_to_a_miss_not_a_panic() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let blob_id = tracked[0].blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        // Seed a valid record so the file/directory exist, then corrupt it in
        // place the way a torn write would.
        cache.put(
            "test-phase",
            blob_id,
            "a.rs",
            &Marker {
                key: "a.rs".to_string(),
                from_cache: true,
            },
        );
        let path = cache_dir
            .path()
            .join("test-phase")
            .join(format!("{blob_id}.json"));
        std::fs::write(&path, b"{ not valid json").unwrap();

        let parse_calls = AtomicUsize::new(0);
        let mut applied = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            counting_parse(&parse_calls),
            |out: Marker| applied.push(out),
        )
        .unwrap();

        assert_eq!(
            parse_calls.load(Ordering::SeqCst),
            1,
            "a corrupted record must be treated as a miss, so parse still runs"
        );
        assert_eq!(
            applied,
            vec![Marker {
                key: "a.rs".to_string(),
                from_cache: false,
            }]
        );
    }

    /// The honesty guard the deleted manual `Deserialize` used to provide, at
    /// its new home.
    ///
    /// Through U2 an unknown extractor id made `Provenance` fail to decode,
    /// `CacheStore::get` swallowed that into a miss, and the file was
    /// re-parsed. `Provenance` derives `Deserialize` now, so such a record
    /// decodes cleanly and would be SERVED unless something else refuses it —
    /// a corpus attributed to an extractor this build does not have. This is
    /// the test that it is still refused, and still by re-parsing rather than
    /// by failing the run.
    #[test]
    fn a_record_naming_an_unknown_extractor_is_not_served() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let blob_id = tracked[0].blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );

        // A record that is perfectly well-formed and names an extractor this
        // build has never heard of — a stale corpus, not a torn write.
        let mut stale = GraphOps::default();
        stale.fact(
            NodeKey::new("a.rs"),
            Fact::new(
                crate::graph::FactKind::Loc,
                crate::graph::FactValue::Int(1),
                Provenance::new(
                    "a.rs",
                    Some(1),
                    "not-a-real-extractor".into(),
                    crate::graph::Tier::Measured,
                ),
            ),
        );
        cache.put(
            "test-phase",
            blob_id,
            "a.rs",
            &stale
                .to_wire(&crate::graph::Vocabulary::default())
                .expect("every kind in this fixture is one the vocabulary can name"),
        );

        // The parse result is deliberately DISTINGUISHABLE from the cached one.
        // An earlier draft had the parse stub return `GraphOps::default()` and
        // asserted the applied value equalled it — which a served stale record
        // could also satisfy by coincidence, so the test could not tell refusal
        // from luck.
        let parse_calls = AtomicUsize::new(0);
        let mut applied: Vec<GraphOps> = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            |batch| {
                parse_calls.fetch_add(batch.len(), Ordering::SeqCst);
                batch.iter().map(|_| freshly_parsed()).collect()
            },
            |out: GraphOps| applied.push(out),
        )
        .unwrap();

        assert_eq!(
            parse_calls.load(Ordering::SeqCst),
            1,
            "the stale record must be refused, which means the file is re-parsed"
        );
        assert_eq!(
            applied,
            vec![freshly_parsed()],
            "and the fresh parse is what reaches the graph, not the stale record"
        );
    }

    /// A `GraphOps` carrying a provenance in EVERY shape `provenances_mut` has
    /// to reach, each one built from `extractor` so a caller can make any of
    /// them the unknown one.
    ///
    /// `dissent` is included even though extraction never produces it (merge
    /// does, afterwards): `Fact` and `Edge` both carry the field, so a
    /// `provenances_mut` that skipped it would be wrong the day that changes,
    /// and wrong invisibly.
    fn ops_with_every_provenance_shape(extractor: &str) -> GraphOps {
        let prov = |line: u32| {
            Provenance::new(
                "a.rs",
                Some(line),
                extractor.into(),
                crate::graph::Tier::Measured,
            )
        };
        let fact = |line: u32| {
            Fact::new(
                crate::graph::FactKind::Loc,
                crate::graph::FactValue::Int(line as i64),
                prov(line),
            )
        };
        let edge = |line: u32| Edge::new(crate::graph::EdgeKind::Contains, prov(line));

        let mut ops = GraphOps::default();

        // Fact, with a dissenting claim of its own.
        let mut with_dissent = fact(1);
        with_dissent.dissent.push(crate::graph::Claim {
            value: crate::graph::FactValue::Int(99),
            provenance: prov(2),
        });
        ops.fact(NodeKey::new("a.rs"), with_dissent);

        // Symbol: facts AND edges hang off it.
        let symbol = ops.symbol(NodeKey::new("sym:a"));
        symbol.push_fact(fact(3));
        symbol.push_edge(NodeKey::new("a.rs"), NodeKey::new("sym:a"), edge(4));

        // External: identity facts AND an optional uses edge.
        ops.external(
            NodeKey::new("ext:x"),
            vec![fact(5)],
            Some((NodeKey::new("sym:a"), edge(6))),
        );

        // A bare edge, also with dissent.
        let mut edge_with_dissent = edge(7);
        edge_with_dissent.dissent.push(crate::graph::Claim {
            value: crate::graph::FactValue::Bool(true),
            provenance: prov(8),
        });
        ops.edge(
            NodeKey::new("a.rs"),
            NodeKey::new("sym:a"),
            edge_with_dissent,
        );

        ops
    }

    /// Canonicalization reaches EVERY provenance, in every shape.
    ///
    /// This is the test the byte-identity proof structurally cannot be. An
    /// un-canonicalized id is CONTENT-EQUAL to the canonical one, and
    /// `ExtractorId` orders by content — so a missed field renders, hashes and
    /// snapshots identically, and every output check in this project stays
    /// green. Only pointer identity can see it.
    #[test]
    fn canonicalization_reaches_every_provenance_shape() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let blob_id = tracked[0].blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        // Written with `"metrics".into()` — a fresh allocation, NOT the run's
        // own id. Serde will hand back another fresh one on decode; only
        // canonicalization can make the served value point at the run's.
        cache.put(
            "test-phase",
            blob_id,
            "a.rs",
            &ops_with_every_provenance_shape("metrics")
                .to_wire(&crate::graph::Vocabulary::default())
                .expect("every kind in this fixture is one the vocabulary can name"),
        );

        let mut applied: Vec<GraphOps> = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            |batch| batch.iter().map(|_| freshly_parsed()).collect(),
            |out: GraphOps| applied.push(out),
        )
        .unwrap();

        let mut served = applied.into_iter().next().expect("one file was walked");
        assert_ne!(
            served,
            freshly_parsed(),
            "the record must have been served, or this proves nothing"
        );

        let run_id = crate::extract::ids::metrics();
        let provenances = served.provenances_mut();
        assert_eq!(
            provenances.len(),
            8,
            "every provenance placed by the fixture must be reachable: 2 on the \
             dissenting fact, 2 on the symbol, 2 on the external, 2 on the \
             dissenting edge"
        );
        for provenance in provenances {
            assert!(
                std::sync::Arc::ptr_eq(&provenance.extractor, &run_id),
                "a provenance at {:?} kept serde's allocation — provenances_mut \
                 does not reach it, and no output check can see that",
                provenance.span
            );
        }
    }

    /// An unknown id is refused wherever it sits, not only at the top level.
    #[test]
    fn an_unknown_id_is_refused_in_every_provenance_shape() {
        for shape in ["metrics", "not-a-real-extractor"] {
            let repo_dir = tempfile::TempDir::new().unwrap();
            let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
            let tracked = repo.tracked_files_with_paths().unwrap();
            let blob_id = tracked[0].blob_id;

            let cache_dir = tempfile::TempDir::new().unwrap();
            let cache = CacheStore::new(
                cache_dir.path().to_path_buf(),
                "test-digest".to_string(),
                crate::extract::trust_ranks(),
                crate::graph::Vocabulary::default(),
            );
            cache.put(
                "test-phase",
                blob_id,
                "a.rs",
                &ops_with_every_provenance_shape(shape)
                    .to_wire(&crate::graph::Vocabulary::default())
                    .expect("every kind in this fixture is one the vocabulary can name"),
            );

            let mut applied: Vec<GraphOps> = Vec::new();
            walk_files(
                &repo,
                &tracked,
                &crate::progress::Silent,
                "test-phase",
                Some(&cache),
                |_key| true,
                |batch| batch.iter().map(|_| freshly_parsed()).collect(),
                |out: GraphOps| applied.push(out),
            )
            .unwrap();

            let served = applied.into_iter().next().unwrap();
            if shape == "metrics" {
                assert_ne!(served, freshly_parsed(), "a known id must be served");
            } else {
                assert_eq!(
                    served,
                    freshly_parsed(),
                    "an unknown id anywhere in the record must refuse the whole record"
                );
            }
        }
    }

    /// A served record carries the RUN's id, not the one serde just allocated.
    ///
    /// This is the invariant the deleted
    /// `provenance_deserialize_recovers_the_same_static_extractor_id` protected,
    /// restated for an owned id. It matters more than it did: serde's `rc`
    /// impls do not preserve sharing, so without canonicalization a warm run
    /// over the R17 target would hold roughly 1.6 million separate allocations
    /// of seven distinct strings.
    #[test]
    fn a_served_record_carries_the_runs_own_id_allocation() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let blob_id = tracked[0].blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );

        let mut cached = GraphOps::default();
        cached.fact(
            NodeKey::new("a.rs"),
            Fact::new(
                crate::graph::FactKind::Loc,
                crate::graph::FactValue::Int(1),
                Provenance::new(
                    "a.rs",
                    Some(1),
                    crate::extract::ids::filetree(),
                    crate::graph::Tier::Measured,
                ),
            ),
        );
        cache.put(
            "test-phase",
            blob_id,
            "a.rs",
            &cached
                .to_wire(&crate::graph::Vocabulary::default())
                .expect("every kind in this fixture is one the vocabulary can name"),
        );

        let mut applied: Vec<GraphOps> = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            |batch| batch.iter().map(|_| GraphOps::default()).collect(),
            |out: GraphOps| applied.push(out),
        )
        .unwrap();

        let mut served = applied.into_iter().next().expect("one file was walked");
        let provenances = served.provenances_mut();
        assert_eq!(
            provenances.len(),
            1,
            "the record must have been served, not re-parsed"
        );
        let served_id = provenances[0].extractor.clone();
        let run_id = crate::extract::ids::filetree();
        assert!(
            std::sync::Arc::ptr_eq(&served_id, &run_id),
            "a served id must be the run's own allocation, not the one serde built"
        );
    }

    #[test]
    fn apply_sees_hits_and_misses_in_file_order() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(
            repo_dir.path(),
            &[
                ("a.rs", "fn a() {}\n"),
                ("b.rs", "fn b() {}\n"),
                ("c.rs", "fn c() {}\n"),
            ],
        );
        let tracked = repo.tracked_files_with_paths().unwrap();
        let b_blob = tracked.iter().find(|tf| tf.key == "b.rs").unwrap().blob_id;

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );
        cache.put(
            "test-phase",
            b_blob,
            "b.rs",
            &Marker {
                key: "b.rs".to_string(),
                from_cache: true,
            },
        );

        let parse_calls = AtomicUsize::new(0);
        let mut applied = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            counting_parse(&parse_calls),
            |out: Marker| applied.push(out),
        )
        .unwrap();

        assert_eq!(
            applied,
            vec![
                Marker {
                    key: "a.rs".to_string(),
                    from_cache: false,
                },
                Marker {
                    key: "b.rs".to_string(),
                    from_cache: true,
                },
                Marker {
                    key: "c.rs".to_string(),
                    from_cache: false,
                },
            ],
            "apply must see files in input order regardless of which were cache hits"
        );
    }

    #[test]
    fn no_cache_means_every_file_is_a_miss() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(
            repo_dir.path(),
            &[("a.rs", "fn a() {}\n"), ("b.rs", "fn b() {}\n")],
        );
        let tracked = repo.tracked_files_with_paths().unwrap();

        let parse_calls = AtomicUsize::new(0);
        let mut applied = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            None,
            |_key| true,
            counting_parse(&parse_calls),
            |out: Marker| applied.push(out),
        )
        .unwrap();

        assert_eq!(
            parse_calls.load(Ordering::SeqCst),
            1,
            "with no cache, one batch of misses reaches parse exactly once"
        );
        assert_eq!(
            applied,
            vec![
                Marker {
                    key: "a.rs".to_string(),
                    from_cache: false,
                },
                Marker {
                    key: "b.rs".to_string(),
                    from_cache: false,
                },
            ]
        );
    }

    /// The collision `CacheStore`'s key-in-record guard exists for: two
    /// distinct tracked files with BYTE-IDENTICAL content share one blob id.
    /// Each must still get its own output — never one file's cached record
    /// answering for the other — across two runs against the same store, the
    /// second of which finds a store partially populated by the first.
    #[test]
    fn files_sharing_a_blob_never_swap_outputs_across_two_runs() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let identical = "fn shared() {}\n";
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", identical), ("b.rs", identical)]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        assert_eq!(
            tracked[0].blob_id, tracked[1].blob_id,
            "precondition: identical content must hash to one blob"
        );

        let cache_dir = tempfile::TempDir::new().unwrap();
        let cache = CacheStore::new(
            cache_dir.path().to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            crate::graph::Vocabulary::default(),
        );

        let expected = vec![
            Marker {
                key: "a.rs".to_string(),
                from_cache: false,
            },
            Marker {
                key: "b.rs".to_string(),
                from_cache: false,
            },
        ];

        for run in 1..=2 {
            let parse_calls = AtomicUsize::new(0);
            let mut applied = Vec::new();
            walk_files(
                &repo,
                &tracked,
                &crate::progress::Silent,
                "test-phase",
                Some(&cache),
                |_key| true,
                counting_parse(&parse_calls),
                |out: Marker| applied.push(out),
            )
            .unwrap();

            assert_eq!(
                applied, expected,
                "run {run}: each file must carry its own key, never the other's, \
                 regardless of what the shared blob's record on disk holds"
            );
            if run == 1 {
                // Both files missed in one batch and share one record path.
                // Written in parallel, the later file must still be the one
                // left on disk, exactly as when the writes were serial.
                let blob = tracked[0].blob_id;
                assert!(cache.get::<Marker>("test-phase", blob, "b.rs").is_some());
                assert!(
                    cache.get::<Marker>("test-phase", blob, "a.rs").is_none(),
                    "the earlier file with the shared blob must not win the record"
                );
            }
        }
    }
    // ---- Declared kinds through the cache (U7, R6) ----

    /// A corpus that declared one fact kind and one edge kind of its own, with
    /// the ids so a test can build ops that use them.
    ///
    /// Built through `Graph`, not by hand: the declaration API is the only door
    /// an id comes through, and a test that minted one another way would be
    /// testing a shape production cannot produce.
    fn declared_corpus() -> (Graph, crate::graph::FactKindId, crate::graph::EdgeKindId) {
        let mut graph = Graph::new();
        let fact = graph
            .declare_fact_kind("open", "uroven", crate::graph::Cardinality::Scalar)
            .unwrap();
        let edge = graph
            .declare_edge_kind("open", "drzi", crate::graph::LinkRole::Opaque)
            .unwrap();
        (graph, fact, edge)
    }

    /// One symbol carrying a declared fact, and a declared edge leaving it.
    /// Both categories in one output, because the two travel through different
    /// halves of the wire twin and a test covering one would say nothing about
    /// the other.
    fn declared_ops(fact: crate::graph::FactKindId, edge: crate::graph::EdgeKindId) -> GraphOps {
        let mut ops = GraphOps::default();
        let key = NodeKey::new("sym:open:Faktura");
        let other = NodeKey::new("sym:open:Radek");
        let handle = ops.symbol(key.clone());
        handle.push_fact(Fact::new(
            FactKind::Declared(fact),
            FactValue::Text("3".into()),
            prov(),
        ));
        ops.symbol(other.clone());
        ops.edge(key, other, Edge::new(EdgeKind::Declared(edge), prov()));
        ops
    }

    fn store_for(dir: &std::path::Path, vocabulary: crate::graph::Vocabulary) -> CacheStore {
        CacheStore::new(
            dir.to_path_buf(),
            "test-digest".to_string(),
            crate::extract::trust_ranks(),
            vocabulary,
        )
    }

    /// The one record `walk_files` wrote, as text.
    fn only_record(cache_dir: &std::path::Path) -> String {
        let mut entries: Vec<_> = std::fs::read_dir(cache_dir.join("test-phase"))
            .expect("the walk must have created the phase directory")
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        assert_eq!(entries.len(), 1, "one tracked file, one record");
        std::fs::read_to_string(&entries[0]).unwrap()
    }

    #[test]
    fn a_declared_fact_and_a_declared_edge_survive_the_cache_cycle() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let cache_dir = tempfile::TempDir::new().unwrap();

        let (graph, fact, edge) = declared_corpus();
        let expected = declared_ops(fact, edge);

        let mut runs = Vec::new();
        for _ in 0..2 {
            // A fresh store each time, as a second process would open: the
            // vocabulary is rebuilt from the declarations, never carried over in
            // memory from the run that wrote the record.
            let cache = store_for(cache_dir.path(), graph.vocabulary().clone());
            let parse_calls = AtomicUsize::new(0);
            let mut applied: Vec<GraphOps> = Vec::new();
            walk_files(
                &repo,
                &tracked,
                &crate::progress::Silent,
                "test-phase",
                Some(&cache),
                |_key| true,
                |batch| {
                    parse_calls.fetch_add(1, Ordering::SeqCst);
                    batch.iter().map(|_| declared_ops(fact, edge)).collect()
                },
                |out: GraphOps| applied.push(out),
            )
            .unwrap();
            runs.push((parse_calls.load(Ordering::SeqCst), applied));
        }

        assert_eq!(
            runs[0].0, 1,
            "the first run must actually parse, or the second proves nothing"
        );
        assert_eq!(
            runs[1].0, 0,
            "the second run must be served from the cache — a declared kind that \
             could not be read back would look exactly like a cold cache, and \
             nothing else in the suite would notice"
        );
        assert_eq!(runs[0].1, vec![expected.clone()]);
        assert_eq!(
            runs[1].1,
            vec![expected],
            "the served ops must carry the same declared kinds the cold run built"
        );
    }

    #[test]
    fn a_cache_record_names_a_declared_kind_and_never_identifies_it() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let cache_dir = tempfile::TempDir::new().unwrap();

        let (graph, fact, edge) = declared_corpus();
        let cache = store_for(cache_dir.path(), graph.vocabulary().clone());
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&cache),
            |_key| true,
            |batch| batch.iter().map(|_| declared_ops(fact, edge)).collect(),
            |_out: GraphOps| {},
        )
        .unwrap();

        let record = only_record(cache_dir.path());
        assert!(
            record.contains("open:uroven") && record.contains("open:drzi"),
            "a record must spell a declared kind by name: {record}"
        );
        // KTD7, and the reason the twin exists at all. A digest on disk would
        // make the record readable only by a run that could reproduce the hash,
        // and unreadable by anything else — including a person.
        for name in ["open:uroven", "open:drzi"] {
            let digest = crate::graph::fnv1a64_hex(name);
            assert!(
                !record.contains(&digest),
                "a record must carry no identity for {name}, found {digest}: {record}"
            );
        }
    }

    #[test]
    fn a_record_naming_a_kind_this_corpus_cannot_name_is_refused() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let cache_dir = tempfile::TempDir::new().unwrap();

        // Written under the old vocabulary.
        let (before, fact, edge) = declared_corpus();
        let writer = store_for(cache_dir.path(), before.vocabulary().clone());
        writer.put(
            "test-phase",
            tracked[0].blob_id,
            "a.rs",
            &declared_ops(fact, edge)
                .to_wire(before.vocabulary())
                .expect("every kind in this fixture is one the vocabulary can name"),
        );

        // Read under a vocabulary that renamed the fact kind. The record still
        // decodes as JSON — it is the NAME the corpus can no longer resolve,
        // which is the only staleness a wire form of names can have.
        let mut after = Graph::new();
        after
            .declare_fact_kind("open", "layer", crate::graph::Cardinality::Scalar)
            .unwrap();
        after
            .declare_edge_kind("open", "drzi", crate::graph::LinkRole::Opaque)
            .unwrap();
        let reader = store_for(cache_dir.path(), after.vocabulary().clone());

        let parse_calls = AtomicUsize::new(0);
        let mut applied: Vec<GraphOps> = Vec::new();
        walk_files(
            &repo,
            &tracked,
            &crate::progress::Silent,
            "test-phase",
            Some(&reader),
            |_key| true,
            |batch| {
                parse_calls.fetch_add(1, Ordering::SeqCst);
                batch.iter().map(|_| freshly_parsed()).collect()
            },
            |out: GraphOps| applied.push(out),
        )
        .unwrap();

        assert_eq!(
            parse_calls.load(Ordering::SeqCst),
            1,
            "a record naming a kind this corpus cannot name must degrade to a \
             re-parse, exactly as a record naming an unknown extractor does"
        );
        assert_eq!(
            applied,
            vec![freshly_parsed()],
            "the refused record must not be served in any part"
        );
    }

    /// A2, at the level the cache actually lives: a warm run over a corpus with
    /// declared kinds must produce the same artifact as a cold one.
    ///
    /// The comparison is the manifest's bytes rather than the graph, because the
    /// manifest is what a consumer reads and it is where a declared kind's name
    /// finally lands. Two graphs that differed only in a kind's identity would
    /// still serialize identically here — but they cannot, because the only way
    /// to get a declared kind back out of a record is through the vocabulary.
    #[test]
    fn a_warm_run_over_a_declared_corpus_writes_the_same_manifest_as_a_cold_one() {
        let repo_dir = tempfile::TempDir::new().unwrap();
        let repo = repo_with_files(repo_dir.path(), &[("a.rs", "fn a() {}\n")]);
        let tracked = repo.tracked_files_with_paths().unwrap();
        let cache_dir = tempfile::TempDir::new().unwrap();

        let mut manifests = Vec::new();
        let mut parsed = Vec::new();
        for _ in 0..2 {
            let (mut graph, fact, edge) = declared_corpus();
            let cache = store_for(cache_dir.path(), graph.vocabulary().clone());
            let mut ops: Vec<GraphOps> = Vec::new();
            let parse_calls = AtomicUsize::new(0);
            walk_files(
                &repo,
                &tracked,
                &crate::progress::Silent,
                "test-phase",
                Some(&cache),
                |_key| true,
                |batch| {
                    parse_calls.fetch_add(1, Ordering::SeqCst);
                    batch.iter().map(|_| declared_ops(fact, edge)).collect()
                },
                |out: GraphOps| ops.push(out),
            )
            .unwrap();
            // Without this the loop would compare two COLD runs and pass even if
            // the cache never served anything — the exact way a warm-run test
            // becomes decoration.
            parsed.push(parse_calls.load(Ordering::SeqCst));
            for op in ops {
                op.apply(&mut graph);
            }
            let manifest = crate::manifest::build_manifest(&graph, None).unwrap();
            manifests.push(serde_json::to_string(&manifest).unwrap());
        }

        assert_eq!(parsed, vec![1, 0], "run 1 parses, run 2 is served warm");
        assert_eq!(
            manifests[0], manifests[1],
            "a warm run must write the same bytes as the cold one that filled the cache"
        );
        for name in ["open:uroven", "open:drzi"] {
            assert!(
                manifests[0].contains(name),
                "the fixture must actually exercise {name}, or this proves nothing about it"
            );
        }
    }
}
