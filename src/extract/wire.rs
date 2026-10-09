//! The on-disk twin of a walk's output (U7, R6).
//!
//! Split out of `walk.rs` in RG2, on a review finding: that file was already
//! over a thousand lines before U7 and grew by half again, entirely from the
//! machinery below. The two have no coupling — this module knows nothing about
//! batching, blob reads or the cache, and `walk.rs` reaches it only through the
//! [`super::walk::WalkOutput`] associated type — so the split is mechanical and
//! leaves the driver readable as a driver.

use super::walk::{GraphOp, GraphOps};
#[allow(unused_imports)]
use crate::graph::Graph;
use crate::graph::{Claim, Edge, Fact, FactValue, NodeKey, Provenance, SiteId, Vocabulary};
use serde::{Deserialize, Serialize};

// ---- The on-disk twin (U7, R6) ----

/// [`GraphOps`] as the extraction cache writes it: every kind spelled out as a
/// **name**, never as an identity.
///
/// ## Why a twin rather than a serde attribute
///
/// A declared kind is a `u64` digest in memory and has to be `namespace:name` on
/// disk, and only the corpus's [`Vocabulary`] knows which is which. Serde cannot
/// be handed that vocabulary: `Serialize` and `Deserialize` take no context. So
/// the conversion has to happen at a seam the caller controls, and a twin type
/// IS that seam — the same answer `manifest` already gives, where the wire model
/// is deliberately self-contained and the graph types are serialize-only. This
/// is the second instance of an established shape, not a new idea.
///
/// The alternatives were weighed and both fail on the same point. A record-local
/// digest→name side table leaves the ops themselves carrying numbers, so the
/// artifact still refers to a kind by an identity nothing outside the run can
/// resolve. Leaking a `&'static str` onto the id keeps serde context-free but
/// needs a process-global interner, and — worse — an unknown name would intern
/// happily into a valid-looking id, which is the `undeclared:` leak arriving
/// through the back door.
///
/// ## What this does NOT change
///
/// The wire form is byte-identical to the one the pre-U7 binary wrote. A core
/// kind's `#[serde(rename_all = "snake_case")]` spelling and its
/// [`crate::graph::fact_word`] spelling are the same string, by construction and
/// by test. So every warm cache survives the upgrade, and the pre-U3 fixture
/// still decodes — a guard that discarded every existing record would show up
/// only as an unexplained slow run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct WireOps(Vec<WireOp>);

/// One buffered change, named rather than identified. Mirrors [`GraphOp`]
/// variant for variant; `NodeKey` carries no kind, so only facts and edges need
/// a twin of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WireOp {
    Fact {
        key: NodeKey,
        fact: WireFact,
    },
    Symbol {
        key: NodeKey,
        facts: Vec<WireFact>,
        edges: Vec<(NodeKey, NodeKey, WireEdge)>,
    },
    External {
        key: NodeKey,
        identity: Vec<WireFact>,
        uses: Option<(NodeKey, WireEdge)>,
    },
    Edge {
        from: NodeKey,
        to: NodeKey,
        edge: WireEdge,
    },
}

/// [`Fact`] with its kind as a word. Field names and order match `Fact`'s
/// exactly — that identity is what makes the format unchanged rather than
/// merely compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WireFact {
    kind: String,
    value: FactValue,
    provenance: Provenance,
    dissent: Vec<Claim>,
}

/// [`Edge`] with its kind as a word. See [`WireFact`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WireEdge {
    kind: String,
    provenance: Provenance,
    dissent: Vec<Claim>,
    /// The edge's usage sites (plan A, KTD7). Omitted when empty, so a
    /// structural edge's cached bytes are what they were. A record written
    /// before sites existed still decodes, with none — and is never served,
    /// because `CACHE_VERSION` moved to 6 for exactly that (see its history).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sites: Vec<SiteId>,
}

impl WireFact {
    /// `None` when this vocabulary cannot name the fact's kind.
    ///
    /// The write path has to be able to refuse, not just the read path. Found in
    /// RG2: `fact_word` falls back to `undeclared:<hex>` for an id it cannot
    /// resolve, so without this the cache would MANUFACTURE exactly the identity
    /// leak `into_ops` exists to refuse — and the record would then be
    /// permanently unreadable, re-parsed on every future run with nothing said.
    /// Refusing here turns that into an ordinary cache miss.
    fn of(fact: &Fact, vocabulary: &Vocabulary) -> Option<Self> {
        let kind = crate::graph::fact_word(fact.kind, vocabulary).into_owned();
        crate::graph::fact_kind_from_word(&kind, vocabulary)?;
        Some(WireFact {
            kind,
            value: fact.value.clone(),
            provenance: fact.provenance.clone(),
            dissent: fact.dissent.clone(),
        })
    }

    fn into_fact(self, vocabulary: &Vocabulary) -> Option<Fact> {
        Some(Fact {
            kind: crate::graph::fact_kind_from_word(&self.kind, vocabulary)?,
            value: self.value,
            provenance: self.provenance,
            dissent: self.dissent,
        })
    }
}

impl WireEdge {
    /// `None` when this vocabulary cannot name the edge's kind — see
    /// [`WireFact::of`].
    fn of(edge: &Edge, vocabulary: &Vocabulary) -> Option<Self> {
        let kind = crate::graph::edge_word(edge.kind, vocabulary).into_owned();
        crate::graph::edge_kind_from_word(&kind, vocabulary)?;
        Some(WireEdge {
            kind,
            provenance: edge.provenance.clone(),
            dissent: edge.dissent.clone(),
            sites: edge.sites.clone(),
        })
    }

    fn into_edge(self, vocabulary: &Vocabulary) -> Option<Edge> {
        Some(Edge {
            kind: crate::graph::edge_kind_from_word(&self.kind, vocabulary)?,
            provenance: self.provenance,
            dissent: self.dissent,
            sites: self.sites,
        })
    }
}

impl GraphOps {
    /// This file's changes in wire form, or `None` when this vocabulary cannot
    /// name one of the kinds they carry.
    ///
    /// Clones. It runs once per cache **miss** — on a file this run is parsing
    /// anyway, where the copy is a rounding error beside the parse — and never
    /// on a hit, so a warm run, the case the cache exists for, pays nothing.
    pub(crate) fn to_wire(&self, vocabulary: &Vocabulary) -> Option<WireOps> {
        fn facts(rows: &[Fact], vocabulary: &Vocabulary) -> Option<Vec<WireFact>> {
            rows.iter().map(|f| WireFact::of(f, vocabulary)).collect()
        }

        let ops = self
            .0
            .iter()
            .map(|op| {
                Some(match op {
                    GraphOp::Fact { key, fact } => WireOp::Fact {
                        key: key.clone(),
                        fact: WireFact::of(fact, vocabulary)?,
                    },
                    GraphOp::Symbol {
                        key,
                        facts: f,
                        edges,
                    } => WireOp::Symbol {
                        key: key.clone(),
                        facts: facts(f, vocabulary)?,
                        edges: edges
                            .iter()
                            .map(|(from, to, edge)| {
                                Some((from.clone(), to.clone(), WireEdge::of(edge, vocabulary)?))
                            })
                            .collect::<Option<Vec<_>>>()?,
                    },
                    GraphOp::External {
                        key,
                        identity,
                        uses,
                    } => WireOp::External {
                        key: key.clone(),
                        identity: facts(identity, vocabulary)?,
                        uses: match uses {
                            None => None,
                            Some((key, edge)) => {
                                Some((key.clone(), WireEdge::of(edge, vocabulary)?))
                            }
                        },
                    },
                    GraphOp::Edge { from, to, edge } => WireOp::Edge {
                        from: from.clone(),
                        to: to.clone(),
                        edge: WireEdge::of(edge, vocabulary)?,
                    },
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(WireOps(ops))
    }
}

impl WireOps {
    /// Read the ops back, or refuse the whole record.
    ///
    /// `None` when any kind in it is one this corpus cannot name — a plugin that
    /// renamed `open:uroven` to `open:layer` leaves records naming the old word,
    /// and they are refused rather than half-read. The caller's answer is a
    /// re-parse, the same degrade-to-work an unknown extractor id already gets.
    /// Whole record, not per-op: a file's ops guard each other (see
    /// [`GraphOp::Symbol`]), so serving the readable half would apply facts the
    /// op that guards them never created.
    ///
    /// This is also why nothing needs to invalidate the cache when a
    /// declaration's ROLE or CARDINALITY changes while its name stays: those
    /// properties are read from the current vocabulary at merge and projection
    /// time, never baked into a cached fact. Only the name is on disk, so only
    /// the name can go stale — and a stale name is exactly what this refuses.
    pub(crate) fn into_ops(self, vocabulary: &Vocabulary) -> Option<GraphOps> {
        fn facts(rows: Vec<WireFact>, vocabulary: &Vocabulary) -> Option<Vec<Fact>> {
            rows.into_iter().map(|f| f.into_fact(vocabulary)).collect()
        }

        let ops = self
            .0
            .into_iter()
            .map(|op| {
                Some(match op {
                    WireOp::Fact { key, fact } => GraphOp::Fact {
                        key,
                        fact: fact.into_fact(vocabulary)?,
                    },
                    WireOp::Symbol {
                        key,
                        facts: f,
                        edges,
                    } => GraphOp::Symbol {
                        key,
                        facts: facts(f, vocabulary)?,
                        edges: edges
                            .into_iter()
                            .map(|(from, to, edge)| Some((from, to, edge.into_edge(vocabulary)?)))
                            .collect::<Option<Vec<_>>>()?,
                    },
                    WireOp::External {
                        key,
                        identity,
                        uses,
                    } => GraphOp::External {
                        key,
                        identity: facts(identity, vocabulary)?,
                        uses: match uses {
                            None => None,
                            Some((key, edge)) => Some((key, edge.into_edge(vocabulary)?)),
                        },
                    },
                    WireOp::Edge { from, to, edge } => GraphOp::Edge {
                        from,
                        to,
                        edge: edge.into_edge(vocabulary)?,
                    },
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(GraphOps(ops))
    }
}
