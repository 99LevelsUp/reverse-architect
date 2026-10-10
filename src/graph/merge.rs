//! The merge layer (U9) — unify facts from every extractor without silently
//! discarding any (E5, E6, E7, KTD-B5/B7/B8).
//!
//! Merge runs once, after `extract::run_all` and before both projections
//! (KTD-B5). It branches on each [`FactKind`]'s [`Cardinality`] (KTD-B7):
//!
//! - **Additive** kinds are unioned and deduped — a node legitimately carries
//!   many (a file has many authors, a dep has many scopes). Merge NEVER picks a
//!   winner here; collapsing a genuinely multi-valued fact is the corrupting bug.
//! - **Scalar** kinds carry one resolved value. On a conflict the highest
//!   trust-rank wins (E6); every other claim survives in the fact's `dissent`
//!   (E7). No claim is ever deleted.
//!
//! Edges merge on the same principle one axis over (KTD6, U4): the relation
//! identity is `(from, to, EdgeKind)` — NOT the provenance — so a relation two
//! sources assert becomes one edge, ranked by [`super::Resolution`] so an
//! index-resolved relation always outranks a name heuristic, with the losing
//! assertion preserved as dissent on the edge itself.
//!
//! Determinism (KTD7): facts are grouped through a [`BTreeMap`] and every claim
//! set through a [`BTreeSet`], equal-rank scalar ties break on a total order over
//! `(FactValue, file, line)`, and the output is sorted — so a second merge over
//! an already-merged graph is byte-identical to the first (idempotent).

use super::{Claim, ExtractorId, Fact, FactKind, Graph, Vocabulary};
use std::collections::{BTreeMap, BTreeSet};

/// The rank an id the registry does not know is merged at.
///
/// Zero, so such a claim loses every scalar conflict it takes part in and can
/// never overwrite a fact from an extractor this run actually loaded.
pub const UNKNOWN_RANK: u8 = 0;

/// The id-and-rank view `graph` gets of the run's extractor registry (KTD17).
///
/// `graph` must never depend on `extract`, so it cannot hold the registry
/// itself — it holds this projection, built once per run by
/// [`crate::extract::trust_ranks`] and threaded in. That is the same shape the
/// bare `BTreeMap` had; making it a type gives the unknown-id decision one
/// place to live instead of an `unwrap_or` at the point of use.
///
/// **On an unknown id.** [`Self::rank`] returns [`UNKNOWN_RANK`], and that is a
/// decision rather than a fallback: a claim from an extractor this view does not
/// rank loses every scalar conflict it enters, so it can never overwrite a fact
/// from one that *is* ranked. Losing is the safe direction, and the loser is
/// still kept as dissent, so nothing is discarded.
///
/// This is deliberately **not** an assertion. An earlier draft of U3 called an
/// unknown id a programming error and put a `debug_assert` here; the suite
/// disproved it within one run. Merging against a partial or empty rank map is
/// a legitimate and used shape — `graph`'s own unit tests merge with
/// [`Ranks::default`] to exercise resolution without ranks at all, and
/// `tests/merge.rs` ranks only the extractors a given case cares about.
/// Asserting an invariant the code does not actually hold would have made
/// debug builds fail on correct usage.
///
/// Bad *data* — a stale cache record naming an extractor this build no longer
/// has — is a different problem and is caught earlier, at the cache boundary,
/// where it can still degrade to a re-parse instead of quietly losing a
/// conflict here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ranks(BTreeMap<ExtractorId, u8>);

impl Ranks {
    pub fn new(ranks: BTreeMap<ExtractorId, u8>) -> Self {
        Ranks(ranks)
    }

    /// This extractor's precedence in a scalar conflict; see the type doc for
    /// what an unknown id means and why it is not an error here.
    pub fn rank(&self, id: &ExtractorId) -> u8 {
        self.0.get(id).copied().unwrap_or(UNKNOWN_RANK)
    }

    /// Every id this view knows, in the map's (content) order.
    pub fn ids(&self) -> impl Iterator<Item = &ExtractorId> {
        self.0.keys()
    }

    /// The registry's own [`ExtractorId`] for `id`, or `None` when this run
    /// does not know it.
    ///
    /// Two callers want this. A decoder needs to swap the fresh `Arc` serde
    /// built for the shared one, so a warm run holds one allocation per id
    /// rather than one per fact. And a validator needs to reject an id no
    /// extractor in this run could have produced.
    pub fn canonical(&self, id: &str) -> Option<ExtractorId> {
        // `Arc<str>: Borrow<str>`, so the map answers this directly — the same
        // lookup `rank` above does. An earlier draft scanned the keys, which
        // gave the identical answer for a linear cost per provenance on every
        // cache hit.
        self.0.get_key_value(id).map(|(known, _)| known.clone())
    }
}

/// How many values of one [`FactKind`] a single node may legitimately hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// Multi-valued: union + dedup, never a winner (a file has many authors).
    Additive,
    /// Single-valued: one resolved value; conflicts resolve by trust-rank.
    Scalar,
}

impl FactKind {
    /// Whether this kind is multi-valued (additive) or single-valued (scalar)
    /// on one node — the property merge branches on (KTD-B7).
    ///
    /// When in doubt, prefer `Additive`: collapsing a genuinely multi-valued
    /// fact silently corrupts data, whereas an extra fact is harmless.
    /// A declared kind answers from its declaration, which is the whole reason
    /// a declaration must carry one (R7). An id this corpus never declared
    /// answers `Additive`, matching the "when in doubt" rule above: keeping two
    /// values a reader can compare beats silently discarding one.
    pub fn cardinality(&self, vocabulary: &Vocabulary) -> Cardinality {
        if let FactKind::Declared(id) = self {
            return vocabulary
                .fact(*id)
                .map_or(Cardinality::Additive, |d| d.cardinality);
        }
        match self {
            // Additive — a node legitimately carries many of these.
            FactKind::DependencyScope   // a dep can be both normal AND dev — THE case
            | FactKind::Coverage        // one per language on the root module
            | FactKind::EntryPointDecl  // a manifest declares several targets
            // One per ambiguous identifier a declaration mentions — a method that
            // calls three same-named things carries three of them, and collapsing
            // them would erase two real ambiguities (R23).
            | FactKind::CandidateCount
            | FactKind::AmbiguousReference
            // One per secret-shaped literal a declaration contains — a method with
            // three of them has three places to look at, and collapsing them would
            // hide two of them (R22).
            | FactKind::RedactedLiteral
            // One per index that covered the file, and one summary per index — a
            // repo indexed per project has several, and collapsing them would drop
            // all but one indexer's coverage (R6).
            | FactKind::ResolvedByIndex
            | FactKind::ResolvedCoverage
            // Each pass that failed says so; one must not hide the other.
            | FactKind::ExtractionFailed
            | FactKind::BodyExtractionFailed
            // One per database access the code makes — a method that reads a
            // table on two lines makes two, and collapsing them would hide one
            // of the places (plan B, R11).
            | FactKind::DbAccess
            // One per distinct text an owner holds, one per withheld place,
            // and one dropped-count per file a partial type has text in
            // (plan B, U3).
            | FactKind::Anchor
            | FactKind::AnchorSkipped
            | FactKind::AnchorsDropped
            // One per place a method decides by an enum's value, and one
            // unmatched-count per file (plan B, U5).
            | FactKind::EnumDecision
            | FactKind::EnumDecisionsUnmatched
            // One per name a file refers to that nothing declares (plan 2c,
            // U5): two missing names are two admissions, not one.
            | FactKind::UnresolvedReference
            | FactKind::DependencyName => Cardinality::Additive,

            // Scalar — one resolved value per node.
            FactKind::Loc                // one line count per file/module
            | FactKind::TestByConvention // one convention verdict per source file
            // Symbol identity: one per Symbol node (the node IS the symbol).
            | FactKind::SymbolName
            | FactKind::SymbolKind
            | FactKind::SymbolContainer
            // One declaration site per symbol node, so one signature, one stated
            // visibility and one doc comment.
            | FactKind::SymbolSignature
            | FactKind::SymbolVisibility
            | FactKind::DocComment
            | FactKind::EntryPoint
            // External identity: one per External node (the node IS the external
            // symbol or resource), exactly like a Symbol node's own identity facts.
            | FactKind::ExternalName
            | FactKind::ExternalKind
            // One moniker per symbol per index; the same symbol re-indexed under
            // the same moniker is one fact, and a genuinely different moniker for
            // one node would be a conflict worth resolving, not a union.
            | FactKind::SymbolMoniker => Cardinality::Scalar,

            // Answered above from the declaration; a declared kind has no
            // entry in this table by construction.
            FactKind::Declared(_) => unreachable!(),
        }
    }
}

/// Merge every node's facts and every parallel edge. Idempotent, deterministic.
///
/// `ranks` maps an extractor id to its trust-rank; `generate` builds it from
/// `default_extractors()` and passes it in, so `graph` never depends on
/// `extract` (KTD-B5, no module cycle). An unknown id merges at
/// [`UNKNOWN_RANK`] and never panics; see [`Ranks`] for why that is a
/// decision rather than a fallback, and why it is deliberately not an
/// assertion.
pub fn merge(graph: &mut Graph, ranks: &Ranks) {
    // Split borrow: `merge_facts` runs inside a mutable node iteration, so it
    // cannot hold the graph and reach the vocabulary through it. Handing the
    // vocabulary in beside `ranks` is the same shape the ranks already use, and
    // avoids cloning the declarations once per run (KTD4).
    let Graph {
        inner, vocabulary, ..
    } = graph;
    for node in inner.node_weights_mut() {
        merge_facts(&mut node.facts, ranks, vocabulary);
    }
    merge_edges(graph);
}

/// Resolve one node's facts in place: group by kind, then union (additive) or
/// resolve-by-rank (scalar). The result is sorted for byte-determinism.
fn merge_facts(facts: &mut Vec<Fact>, ranks: &Ranks, vocabulary: &Vocabulary) {
    // Group by kind through a BTreeMap so iteration is kind-ordered (KTD7).
    let mut groups: BTreeMap<FactKind, Vec<Fact>> = BTreeMap::new();
    for fact in facts.drain(..) {
        groups.entry(fact.kind).or_default().push(fact);
    }

    let mut out: Vec<Fact> = Vec::new();
    for (kind, group) in groups {
        // Pool every claim carried by the group, INCLUDING claims already in
        // each fact's dissent. This is what makes merge idempotent: a second
        // run reconstitutes the identical claim pool from a resolved fact and
        // re-resolves to the same result — losers are never silently dropped.
        let mut pool: BTreeSet<Claim> = BTreeSet::new();
        for fact in group {
            pool.insert(Claim {
                value: fact.value,
                provenance: fact.provenance,
            });
            pool.extend(fact.dissent);
        }

        match kind.cardinality(vocabulary) {
            Cardinality::Additive => {
                // Union + dedup: every distinct claim survives as its own fact,
                // never demoted to dissent. Two authors both stay.
                for claim in pool {
                    out.push(Fact {
                        kind,
                        value: claim.value,
                        provenance: claim.provenance,
                        dissent: Vec::new(),
                    });
                }
            }
            Cardinality::Scalar => {
                // One resolved value: highest trust-rank wins; equal rank breaks
                // on the (value, file, line) total order (KTD-B7). Every other
                // claim survives as dissent (E7).
                let claims: Vec<Claim> = pool.into_iter().collect();
                let rank = |c: &Claim| ranks.rank(&c.provenance.extractor);
                let winner = claims
                    .iter()
                    .cloned()
                    .reduce(|best, c| {
                        let (rb, rc) = (rank(&best), rank(&c));
                        match rc.cmp(&rb) {
                            std::cmp::Ordering::Greater => c,
                            std::cmp::Ordering::Less => best,
                            // Equal rank: the smaller (value, file, line) wins —
                            // deterministic, never arbitrary (KTD7).
                            std::cmp::Ordering::Equal => {
                                if tie_key(&c) < tie_key(&best) {
                                    c
                                } else {
                                    best
                                }
                            }
                        }
                    })
                    .expect("a fact group is never empty");

                let dissent: Vec<Claim> = claims.into_iter().filter(|c| *c != winner).collect();
                out.push(Fact {
                    kind,
                    value: winner.value,
                    provenance: winner.provenance,
                    dissent, // already sorted: derived from a BTreeSet, winner removed
                });
            }
        }
    }

    // Final sort for a stable, insertion-order-independent fact order (KTD7).
    out.sort();
    *facts = out;
}

/// The equal-rank scalar tie-break key: a total order over the claim's value and
/// source location. Deterministic, so idempotency and cross-run stability hold.
fn tie_key(c: &Claim) -> (&super::FactValue, &str, super::Span) {
    (&c.value, c.provenance.file.as_str(), c.provenance.span)
}

/// Merge parallel edges by relation identity, ranking by [`Resolution`] and
/// preserving every losing assertion as dissent ON THE EDGE (KTD6).
///
/// Two edges are the SAME relation when they share endpoints and [`EdgeKind`] —
/// provenance is deliberately NOT part of that key. That is the U4 change: until
/// now edges collapsed only on full identity (provenance included), so a name
/// heuristic and an external index asserting one relation stayed two edges and a
/// consumer had to reconcile them. Now they become one edge at the higher
/// precision, with the loser retrievable in `dissent` — nothing is deleted (E7),
/// and a `resolved` relation can never be shadowed by a heuristic guess.
///
/// Ranking is by `Resolution` first, not by trust-rank: `Resolution` is the
/// precision axis KTD6 names for edges, and no extractor emits a `Narrated` edge
/// for a tier comparison to matter. Equal precision breaks on the confidence rung
/// (plan A, KTD1: the stronger rung wins, see [`rung_strength`]), and only then on
/// the total order over the whole [`Provenance`] — deterministic, never
/// arbitrary (KTD7). The merged edge carries the sorted union of every grouped
/// edge's sites (plan A, KTD7): a relation is the summary of all the places it
/// was read at, whichever claim won.
///
/// When it has work to do the edge set is rebuilt from a [`BTreeMap`], never
/// mutated in place: `remove_edge` swap-removes and invalidates indices, which
/// would corrupt the set. Rebuilding from a sorted map also makes the result
/// independent of the order extractors ran in, and idempotent — a second merge
/// re-pools each winner with its own dissent and re-resolves to the identical edge.
///
/// Scale (R17): U4 makes edges the graph's bulk — a 13,270-file repository carries
/// millions once calls and references land — so the rebuild is guarded by a cheap
/// pre-check that clones nothing. Almost every real graph reaches merge with unique
/// relations already (extraction dedups within a file, and different extractors
/// assert different kinds), and skipping the rebuild there avoids a `Provenance`
/// clone per edge for no gain.
fn merge_edges(graph: &mut Graph) {
    use petgraph::graph::NodeIndex;
    use petgraph::visit::EdgeRef;

    if !edges_need_merging(graph) {
        return;
    }

    // Group every assertion by relation identity: (from, to, kind). The pool
    // includes each edge's existing dissent, which is what makes merge idempotent.
    // Beside it, the union of every grouped edge's sites (plan A, KTD7): one
    // relation read at three places by two claims is one edge with all three,
    // and a set keeps the union sorted and the second merge a no-op.
    type Group = (BTreeSet<Claim>, BTreeSet<super::SiteId>);
    let mut groups: BTreeMap<(usize, usize, super::EdgeKind), Group> = BTreeMap::new();
    for e in graph.inner.edge_references() {
        let edge = e.weight();
        let (pool, sites) = groups
            .entry((e.source().index(), e.target().index(), edge.kind))
            .or_default();
        pool.insert(Claim {
            // An edge has no scalar value; the assertion IS the relation, and its
            // kind is already the group key (see `Edge::dissent`).
            value: super::FactValue::Bool(true),
            provenance: edge.provenance.clone(),
        });
        pool.extend(edge.dissent.iter().cloned());
        sites.extend(edge.sites.iter().cloned());
    }

    graph.inner.clear_edges();
    for ((from, to, kind), (pool, sites)) in groups {
        let claims: Vec<Claim> = pool.into_iter().collect();
        let winner = claims
            .iter()
            .cloned()
            .reduce(|best, c| {
                match c
                    .provenance
                    .resolution
                    .cmp(&best.provenance.resolution)
                    .then_with(|| rung_strength(&c.provenance, &best.provenance))
                {
                    std::cmp::Ordering::Greater => c,
                    std::cmp::Ordering::Less => best,
                    // Equal precision: the smaller provenance wins — the same
                    // "deterministic, never arbitrary" discipline as `tie_key`.
                    std::cmp::Ordering::Equal => {
                        if c.provenance < best.provenance {
                            c
                        } else {
                            best
                        }
                    }
                }
            })
            .expect("an edge group is never empty");
        let dissent: Vec<Claim> = claims.into_iter().filter(|c| *c != winner).collect();
        graph.inner.add_edge(
            NodeIndex::new(from),
            NodeIndex::new(to),
            super::Edge {
                kind,
                provenance: winner.provenance,
                dissent, // already sorted: derived from a BTreeSet, winner removed
                sites: sites.into_iter().collect(),
            },
        );
    }
}

/// How `a`'s confidence rung compares to `b`'s, `Greater` meaning `a` is the
/// stronger claim (plan A, KTD1).
///
/// Consulted only after [`super::Resolution`] tied, so in practice it decides between
/// two heuristic claims. It is stated here rather than left to the derived
/// `Ord` over the whole [`super::Provenance`]: that order compares file, line and
/// extractor long before it reaches the rung, so a weaker claim at an earlier
/// line would win. [`super::Confidence`] lists its rungs strongest first, so
/// the SMALLER rung is the stronger one. A claim that carries a rung outranks
/// one that carries none — a relation whose support was stated beats one that
/// says nothing about it — and two claims without one tie, which leaves every
/// merge that existed before the rung exactly as it was.
fn rung_strength(a: &super::Provenance, b: &super::Provenance) -> std::cmp::Ordering {
    match (a.rung(), b.rung()) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Whether [`merge_edges`] has anything to do: are two edges the same relation, or
/// does any edge already carry dissent?
///
/// Compares only `(from, to, kind)` triples — 12 bytes each, no allocation per edge
/// beyond one vector — so the guard costs a sort where the merge itself would cost a
/// clone of every provenance. An edge that already carries dissent takes the full
/// path even when its relation is unique: a hand-built edge whose dissent outranks
/// its own provenance must still be re-resolved, so the fast path can never be the
/// reason a `resolved` claim stays buried in dissent.
fn edges_need_merging(graph: &Graph) -> bool {
    use petgraph::visit::EdgeRef;

    let mut keys: Vec<(usize, usize, super::EdgeKind)> =
        Vec::with_capacity(graph.inner.edge_count());
    for e in graph.inner.edge_references() {
        if !e.weight().dissent.is_empty() {
            return true;
        }
        keys.push((e.source().index(), e.target().index(), e.weight().kind));
    }
    let before = keys.len();
    keys.sort_unstable();
    keys.dedup();
    keys.len() != before
}
