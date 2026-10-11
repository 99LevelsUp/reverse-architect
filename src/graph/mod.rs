//! The fact graph — the single intermediate representation (IR) of the whole
//! tool (U2, KTD3).
//!
//! Every extractor writes into this graph; both projections (the human corpus
//! and the machine manifest) read from it. Nodes are entities (files, modules,
//! symbols, dependencies); edges are relations (contains, imports, depends-on).
//! Every fact and edge carries [`Provenance`] and a dissent list of [`Claim`]s,
//! so no data is ever silently discarded and every assertion is traceable back
//! to its source (E8, R12, R16).
//!
//! Determinism (KTD7) is structural: node keys are repo-root-relative and
//! `/`-normalized (never `fs::canonicalize`), the lookup index is a
//! [`BTreeMap`], and traversal accessors return entries in a stable sorted
//! order regardless of insertion order.

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod merge;
pub use merge::{merge, Cardinality, Ranks, UNKNOWN_RANK};

pub mod sites;
pub use sites::{
    CandidateOutcome, CandidateRecord, IndexPairing, IndexSummary, RuleVerdict, SiteCounts,
    SiteKind, SiteLedger, SiteRecord, Verdict,
};

mod vocabulary;
pub use vocabulary::{
    edge_kind_from_word, edge_role, edge_word, fact_kind_from_word, fact_word, node_role,
    node_word, undeclared_name, CardDeclaration, CardKindId, CardPolicy, EdgeDeclaration,
    EdgeKindId, FactDeclaration, FactKindId, LinkRole, NodeDeclaration, NodeKindId, StructuralRole,
    Vocabulary, VocabularyError, DIGEST_NAME,
};

/// An IR-integrity error: a shape the graph refuses to hold rather than
/// record wrongly. The first variant is a node-key collision
/// across kinds — a real error in every build, not a debug-only assert, so a
/// Symbol/Dependency key colliding with a file/module key can never silently
/// attach facts to the wrong node in release (E5, KTD-B1).
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("node key {key:?} reused with a different kind (have {existing:?}, got {got:?})")]
    KindCollision {
        key: String,
        existing: NodeKind,
        got: NodeKind,
    },
    #[error("{category} kind used at {key:?} was never declared on this graph")]
    UndeclaredKind { category: &'static str, key: String },
    #[error(
        "node key {key:?} is in the Container role but is not `/`-hierarchical \
         (no leading or trailing separator, no empty segment, no backslash)"
    )]
    NonHierarchicalContainer { key: String },
}

/// Whether a key has the shape the module tree walk requires: `/`-separated,
/// no leading or trailing separator, no empty segment, no backslash.
///
/// This is the *shape* half of the `NodeKey` precondition. The other half —
/// every intermediate level exists as its own node — is a whole-graph property
/// that cannot be judged at insertion time, because nothing fixes the order in
/// which an extractor emits. `taxonomy::views::parent_closed` judges that one,
/// and since U6 it asks for the `Container` ROLE rather than for
/// `NodeKind::Module` — so a declared container is parent-checked on exactly the
/// same terms as a core module. That is the whole of the closure half: a
/// `debug_assert`, silent in release, which is the treatment core modules have
/// always had and not something U6 invented for declared kinds.
///
/// The backslash clause is deliberately kept though it cannot fire through
/// [`NodeKey::new`], which normalizes `\` to `/` and is the only constructor —
/// `is_hierarchical` states the whole precondition rather than only the part
/// today's callers can violate.
fn is_hierarchical(key: &str) -> bool {
    key == "."
        || (!key.is_empty()
            && !key.contains('\\')
            && !key.starts_with('/')
            && !key.ends_with('/')
            && !key.contains("//"))
}

/// A stable, portable node key: a repo-root-relative, `/`-normalized path or a
/// logical identifier. Never an absolute or canonicalized path (KTD7).
///
/// The separator is load-bearing for anyone assembling a graph outside this
/// crate: the taxonomy derives a module's parent by cutting the key at its last
/// `/`, so a `Module` node nested under another must spell the whole path
/// (`a/b/c`, never `c`), and every intermediate directory must exist as its own
/// `Module` node. A synthetic root node is *not* required — top-level modules
/// hang under `.` implicitly.
///
/// Shared, not owned: a key is cloned into every edge, every caller and every
/// row of the second layer that names it, and a clone is a count, not a copy
/// of the text. Compared, ordered, hashed and serialized by its text, exactly
/// as a `String` is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct NodeKey(std::sync::Arc<str>);

impl NodeKey {
    /// Build a key, normalizing OS path separators to `/` for portability.
    pub fn new(value: impl Into<String>) -> Self {
        NodeKey(value.into().replace('\\', "/").into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Manual, not derived: deserializing straight into the tuple field would skip
/// [`NodeKey::new`]'s backslash-to-forward-slash normalization, so a key
/// serialized on Windows and read back would carry `\` where every other key
/// in the graph carries `/` (KTD7). Routing through the constructor keeps a
/// round-tripped key identical to one built fresh from the same input.
impl<'de> Deserialize<'de> for NodeKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(NodeKey::new)
    }
}

/// Stable identifier of the extractor that produced a fact or edge.
///
/// An owned `Arc<str>` rather than the `&'static str` this was through U2: a
/// plugin's id is not known until its manifest is read at run time (R12), so
/// the type can no longer require a literal compiled into this binary.
///
/// **Owned and content-ordered, never interned to a number.** [`Provenance`]
/// derives [`Ord`] with `extractor` as its third field, and `merge_facts`
/// emits from an ordered `BTreeSet<Claim>` — so provenance ordering *is*
/// emission order, and every corpus byte depends on it. `Arc<str>` compares
/// bytewise exactly as `&'static str` did; a numeric intern handle would order
/// by allocation sequence instead, silently reordering additive facts and
/// dissent lists in every artifact (U3, KTD17).
///
/// `Arc` and not `Rc` because the extraction walks are rayon `par_iter` and a
/// [`Provenance`] crosses threads inside a `GraphOp`.
///
/// One allocation per id, not per fact: the bundled ids live in
/// [`crate::extract::ids`] and every emission site clones from there, which is
/// a refcount bump. A `&str` reaching this type through `.into()` allocates —
/// acceptable in a test, never on an emission path.
pub type ExtractorId = std::sync::Arc<str>;

/// Which deterministic tier a fact belongs to (Tier 0 only in v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Directly measured from a source (e.g. a git author, a line count).
    Measured,
    /// Derived by deterministic computation over measured facts.
    Derived,
    /// Written by an LLM around verified facts (Tier 1). Ranks below the
    /// deterministic tiers: a narrative can never override a measured fact
    /// (T1-KTD9).
    Narrated,
}

/// How precisely a fact or relation was determined — the *precision* axis, which
/// is deliberately separate from [`Tier`] (KTD1). `Tier` is the trust axis and
/// drives merge ranking; `Resolution` says nothing about trust, only about how the
/// relation was pinned down. Folding the two together would break conflict
/// resolution: a `Resolved` narration must still lose to a `Measured` heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Determined by a universal heuristic — name matching, textual import paths,
    /// convention. Always available, so it is the default for every extractor
    /// (KD2). It can confirm what it finds but never prove an absence.
    Heuristic,
    /// Determined against an external semantic index (SCIP/LSIF or equivalent),
    /// so the symbol identity is authoritative within the indexed scope. The ONLY
    /// scope in which the corpus may claim "this symbol is used nowhere"; outside
    /// it the honest answer is "not found" (R20). Never a default — an extractor
    /// must opt in, and only where such an index actually covered the source.
    Resolved,
}

/// How far a relation established by NAME RESOLUTION can be trusted — the rung
/// a reader sees on a call, a reference or an inheritance edge (R1, R2, KTD1).
///
/// A field of [`Provenance`], not a new [`EdgeKind`]: "maybe also" is a
/// statement about how well a relation is supported, the precision axis, not a
/// different relation. A separate kind would split the edge's identity in
/// merge and force every consumer of `Calls` to opt into it or silently lose
/// edges.
///
/// **Variant order is deliberate: strongest first.** The derived [`Ord`] is
/// what `merge_edges` consults when two claims on one relation tie on
/// [`Resolution`], so the smaller variant is the stronger one and must win
/// (see `graph::merge`, which states that comparison explicitly rather than
/// leaving it to whatever field order `Provenance` happens to have).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Pinned by an external semantic index. **Never stored**: it is derived
    /// from [`Resolution::Resolved`] by [`Provenance::rung`], so the two can
    /// never disagree about the same claim.
    ResolvedByIndex,
    /// Every candidate but this one was excluded by rules, and a rule backs
    /// this one.
    RulesBacked,
    /// The one candidate this repository declares under the name, which no
    /// rule backed or disputed. Never presented as backed: the real target may
    /// lie outside the repository.
    ///
    /// Stronger than [`Confidence::Ambiguous`] on purpose: an undisputed claim
    /// keeps a relation in the card's main lists (plan A, Key Decision on
    /// routing), so a relation one site names undisputed and another site
    /// leaves ambiguous must not sink into "maybe also" on a merge tie.
    NameOnly,
    /// Several candidates survived, or a rule disputed this one without the
    /// right to remove it.
    Ambiguous,
}

/// The rule that admitted a name-resolved relation (R1, KTD1) — a closed
/// vocabulary of short ids the HOST owns.
///
/// An enum rather than an open string: a plugin supplies facts about a usage
/// site, the host decides which rule applied, so no id can arrive that this
/// build does not define. A stale cache record naming a rule this build
/// retired fails to decode, which `CacheStore::get` treats as a miss and
/// answers by re-parsing — the safe direction. Being an enum also means the
/// cache hit path has nothing to canonicalize, unlike an [`ExtractorId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rule {
    /// The relation was matched by its simple name against the repo-wide
    /// symbol table and nothing more: no sifting rule backed or disputed it.
    NameMatch,
    /// A construction or a call names a declaration of the wrong role — a
    /// C# call that is not `new` naming a constructor (plan A, KTD6).
    RoleMismatch,
    /// The receiver's declared type: members of that type and its ancestors
    /// are backed, members elsewhere disputed (KTD6).
    DeclaredReceiver,
    /// `Type.Member()` on a repository type: its members are backed, members
    /// elsewhere disputed (KTD6).
    StaticReceiver,
    /// A bare call the plugin said has no receiver: members of the caller's
    /// own type and its ancestors are backed. Never disputes (KTD6).
    ImplicitThis,
    /// The argument count lies outside the candidate's parameter range.
    /// Only ever disputes (KTD6).
    Arity,
    /// The candidate type's namespace is not visible in the file — through
    /// its own usings or its project's (global usings, `<Using>` items,
    /// implicit usings). Only ever disputes: a using a source generator emits
    /// can make it visible all the same (KTD6).
    NamespaceVisibility,
    /// A type named by its simple name where that name cannot reach it: the
    /// candidate is nested in another type, and the site is neither inside
    /// that type (or a type nested in it, or one deriving from it) nor under a
    /// `using static` of it or an alias of the name (KTD6). Its negative claim
    /// is C#'s own name lookup, so it may earn a right to delete — after
    /// measurement, like the receiver rules.
    NestedTypeVisibility,
    /// A type relation whose base was found by its spelling — the declaring
    /// type's own module or the qualified path written — rather than as the
    /// one type of that name anywhere in the repository. Only ever backs
    /// (KTD6).
    BaseSpelling,
    /// Not a sifting rule: the external index paired the site and pointed at
    /// another declaration, so this candidate is only a guess there (plan A,
    /// U9, KTD8). Set by the index pairing on the relation it moves to "maybe
    /// also", never by the resolver, so it appears in no site's trace.
    ///
    /// Host-owned and measured by nothing: it is not in the resolver's
    /// catalog, so it has no revision, no right to delete can name it (a right
    /// needs a catalog revision), and the per-rule precision report (U11) must
    /// skip it — it is the yardstick the rules are measured against, not one
    /// of them.
    IndexElsewhere,
    /// Not a sifting rule: the external index paired the site with a symbol
    /// from outside the repository, so every candidate there is only a guess
    /// (plan A, U9, KTD8). Host-owned and unmeasured exactly as
    /// [`Rule::IndexElsewhere`].
    IndexOutside,
    /// Not a sifting rule: a reference by name (contract 0.3, `name-ref`)
    /// found its declaration in the lookup block it is written in, or in a
    /// block around it, of the same file (plan 2c, U5: KTD5). Host-owned and
    /// outside the resolver's catalog, like [`Rule::IndexElsewhere`]: it names
    /// the scope a lookup ended in, and no right to delete can name it.
    ScopeBlock,
    /// A reference by name found its declaration elsewhere in its own file
    /// (plan 2c, KTD5). Host-owned, as [`Rule::ScopeBlock`].
    ScopeFile,
    /// A reference by name found its declaration in a file its file imports,
    /// directly or transitively — a merged resource dictionary (plan 2c,
    /// KTD5). Host-owned, as [`Rule::ScopeBlock`].
    ScopeImports,
    /// A reference by name found its declaration in another file of the same
    /// project — the same assembly — that its file does not import (plan 2c,
    /// KTD5): still backed, but by a scope of its own, so a reader can tell it
    /// from a lookup the file itself states. Host-owned, as
    /// [`Rule::ScopeBlock`].
    ScopeProject,
}

impl Rule {
    /// The rule's stable id, as the manifest interns it and as the cache
    /// spells it.
    pub fn id(self) -> &'static str {
        match self {
            Rule::NameMatch => "name-match",
            Rule::RoleMismatch => "role-mismatch",
            Rule::DeclaredReceiver => "declared-receiver",
            Rule::StaticReceiver => "static-receiver",
            Rule::ImplicitThis => "implicit-this",
            Rule::Arity => "arity",
            Rule::NamespaceVisibility => "namespace-visibility",
            Rule::NestedTypeVisibility => "nested-type-visibility",
            Rule::BaseSpelling => "base-spelling",
            Rule::IndexElsewhere => "index-elsewhere",
            Rule::IndexOutside => "index-outside",
            Rule::ScopeBlock => "scope-block",
            Rule::ScopeFile => "scope-file",
            Rule::ScopeImports => "scope-imports",
            Rule::ScopeProject => "scope-project",
        }
    }
}

/// The place in a source file an assertion is answerable to.
///
/// One value rather than two independent options. The pair it replaces —
/// `line: Option<u32>` beside `end_line: Option<u32>` — could be written down in
/// states that mean nothing: an end with no start, an end BEFORE its start, a
/// line zero in a 1-based scheme. None of them were reachable through the
/// constructors, and every one of them was silently degraded at render time
/// rather than refused. Here they cannot be written down at all.
///
/// A range that ends where it starts is a [`Span::Line`], not a one-line
/// `Range`: two spellings of one place would make two provenances compare
/// unequal that a reader could not tell apart, and provenance ordering decides
/// merge ties (KTD7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Span {
    /// The whole file. The assertion is about the file, not about a place in it —
    /// a commit count, a language, a coverage verdict.
    Whole,
    /// One 1-based line.
    Line(u32),
    /// An inclusive 1-based range spanning more than one line.
    Range { start: u32, end: u32 },
}

impl Span {
    /// A point anchor, or [`Span::Whole`] when there is no line to point at.
    ///
    /// Line zero is not a line: a 1-based scheme has no such row, so it can only
    /// arrive from a bug, and the honest reading of a nonsense anchor is that
    /// there is no anchor. Degrading beats panicking on a value that reaches
    /// here from parsed source (E4).
    pub fn at(line: Option<u32>) -> Span {
        match line {
            Some(line) if line > 0 => Span::Line(line),
            _ => Span::Whole,
        }
    }

    /// An inclusive range between two 1-based lines.
    ///
    /// Order-insensitive: a caller that hands the ends over backwards gets the
    /// range it meant rather than a silently discarded end. Nothing in the tree
    /// produces a backwards range, so this is a guard, not a behaviour anyone
    /// should rely on.
    pub fn range(start: u32, end: u32) -> Span {
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        match (lo, hi) {
            (0, 0) => Span::Whole,
            (0, hi) => Span::Line(hi),
            (lo, hi) if lo == hi => Span::Line(lo),
            (lo, hi) => Span::Range { start: lo, end: hi },
        }
    }

    /// The first line, when the span names one.
    pub fn start(self) -> Option<u32> {
        match self {
            Span::Whole => None,
            Span::Line(line) => Some(line),
            Span::Range { start, .. } => Some(start),
        }
    }

    /// The last line INCLUSIVE, when the span names one. A `Line` ends where it
    /// starts — the caller that asks "where does this end" wants that answer, not
    /// `None`.
    pub fn end(self) -> Option<u32> {
        match self {
            Span::Whole => None,
            Span::Line(line) => Some(line),
            Span::Range { end, .. } => Some(end),
        }
    }

    /// The end of a MULTI-LINE span only — the wire form's `end_line`, which is
    /// written exactly when a span covers more than one line and omitted
    /// otherwise, so a consumer never reads a range where a point was meant.
    pub fn range_end(self) -> Option<u32> {
        match self {
            Span::Range { end, .. } => Some(end),
            _ => None,
        }
    }
}

/// Where a fact or edge came from — the provenance trail back to the source.
///
/// [`Deserialize`] is derived as of U3. It used to be hand-written over in
/// `extract::cache`, for one reason: an `ExtractorId` was a `&'static str`,
/// serde cannot manufacture a `'static` borrow from a runtime buffer, and the
/// only way back was to look the wire string up in `trust_ranks()` — which
/// meant `graph` could not own its own decoder without depending on
/// `extract`. An owned id removes the lookup and with it the inversion.
///
/// What that hand-written impl also did, and a derive cannot, is refuse an
/// extractor id this build does not know. That check did not disappear: it
/// moved to where a stale record actually enters, the cache-hit path in
/// `extract::walk`, which can still answer it by re-parsing the file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Provenance {
    /// Repo-root-relative, `/`-normalized source path.
    pub file: String,
    /// Where in the file — the whole of it, one line, or an inclusive range.
    ///
    /// One value rather than a start beside an independent end: a point-anchored
    /// fact stays exactly that without an extractor having to invent an end, and
    /// the states that meant nothing are no longer expressible (see [`Span`]).
    pub span: Span,
    /// The extractor that asserted this.
    pub extractor: ExtractorId,
    /// The tier of the assertion.
    pub tier: Tier,
    /// How precisely the assertion was determined (KTD1) — an axis of its own,
    /// beside `tier`, never derived from it.
    pub resolution: Resolution,
    /// How many repo-wide candidates a heuristic name match found for this
    /// specific edge, when more than one — `None` for every unambiguous or
    /// non-heuristic assertion. A third axis beside `tier`/`resolution`: it
    /// says nothing about precision or provenance kind, only that this
    /// particular claim was one of several equally-named candidates the
    /// extractor could not tell apart (R23).
    pub ambiguous_candidates: Option<usize>,
    /// The confidence rung a name-resolved relation was given (R2, KTD1), set
    /// together with `rule` by [`Provenance::rated`]. `None` for every
    /// assertion that did not come from resolving a name — a fact, a
    /// `Contains`/`Declares`/`Imports` edge — and for an index-resolved one,
    /// whose rung [`Provenance::rung`] derives instead of storing it twice.
    ///
    /// Skipped when `None`, so the cache and manifest bytes of every such
    /// assertion are the bytes the pre-rung binary wrote. Placed before
    /// `anchor` so that field stays last; while every value is `None` the
    /// position cannot reorder anything, and merge does not rely on it to pick
    /// the stronger rung (it compares the rung explicitly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// The rule that admitted this relation, when `confidence` is set — never
    /// one without the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<Rule>,
    /// Where inside a **composite** source this came from, when the source has
    /// an inside (R34).
    ///
    /// `file` above answers "which path in the tree". For a source whose unit is
    /// not a file in a tree — an archive, an image layer, a document store — that
    /// is not the whole address, and squeezing the rest into `file` as a
    /// synthesized path would produce a string that looks openable and is not.
    ///
    /// **Nothing produces this yet.** The type is defined here, before U21 stands
    /// a component in the source seam, because a corpus-visible field added
    /// *after* U11 would make R17 unsatisfiable — so it must be added while the
    /// artifact can still absorb it. It is `skip_serializing_if` precisely so
    /// that adding it changes not one byte of a git-source corpus, and it is the
    /// LAST field so the derived `Ord` — which is emission order, and therefore
    /// every corpus byte's order — compares it last and is unaffected while every
    /// value is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<SourceAnchor>,
}

/// An address inside a composite source (R34): which container, which entry in
/// it, and optionally where in that entry.
///
/// Generic on purpose. A ZIP member is `(archive path, member path)`; a
/// container layer is `(layer digest, path in the layer)`; a document store is
/// `(document id, section)`. Naming the three parts once means a consumer reads
/// one shape instead of one per source kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceAnchor {
    /// The container's own key — the archive, the layer, the document.
    pub container: String,
    /// The key inside the container.
    pub entry: String,
    /// A line within the entry, when the entry has lines and the fact names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl Provenance {
    /// Construct point-anchored provenance, normalizing the source path
    /// separators to `/`.
    ///
    /// Defaults to the honest [`Resolution::Heuristic`] and no line range:
    /// `Resolved` is a claim about external-index coverage that an extractor must
    /// opt into explicitly via [`Provenance::resolved`] (KD2/R20), because a
    /// silently defaulted `Resolved` would license "used nowhere" conclusions the
    /// data cannot support.
    pub fn new(
        file: impl Into<String>,
        line: Option<u32>,
        extractor: ExtractorId,
        tier: Tier,
    ) -> Self {
        Provenance {
            file: file.into().replace('\\', "/"),
            span: Span::at(line),
            extractor,
            tier,
            resolution: Resolution::Heuristic,
            ambiguous_candidates: None,
            confidence: None,
            rule: None,
            anchor: None,
        }
    }

    /// Anchor this provenance inside a composite source (R34).
    ///
    /// Unused by every bundled extractor: a file in a tree has no inside, so
    /// `file` is already its whole address. This exists for the source
    /// implementations U21 admits.
    #[must_use]
    pub fn in_source(mut self, anchor: SourceAnchor) -> Self {
        self.anchor = Some(anchor);
        self
    }

    /// Anchor this provenance to the inclusive line range `line..=end_line`.
    /// `line` stays the start, so a range never loses the point a consumer that
    /// only understands single lines already reads.
    #[must_use]
    pub fn with_range(mut self, line: u32, end_line: u32) -> Self {
        self.span = Span::range(line, end_line);
        self
    }

    /// Mark this assertion as [`Resolution::Resolved`] — determined against an
    /// external semantic index. Only call this where such an index actually
    /// covered the source (R20).
    #[must_use]
    pub fn resolved(mut self) -> Self {
        self.resolution = Resolution::Resolved;
        self
    }

    /// Mark this assertion as one of `n` repo-wide candidates a heuristic name
    /// match could not tell apart (R23). Call only for `n > 1` — an
    /// unambiguous match stays `None`, never `Some(1)`.
    #[must_use]
    pub fn ambiguous(mut self, n: usize) -> Self {
        self.ambiguous_candidates = Some(n);
        self
    }

    /// Record the rung a name-resolved relation earned and the rule that
    /// assigned it (R1). One call sets both, so a rung never arrives without
    /// its rule. Never pass [`Confidence::ResolvedByIndex`]: that rung is
    /// derived from [`Resolution::Resolved`], see [`Provenance::rung`].
    #[must_use]
    pub fn rated(mut self, confidence: Confidence, rule: Rule) -> Self {
        self.confidence = Some(confidence);
        self.rule = Some(rule);
        self
    }

    /// The rung this assertion stands on: [`Confidence::ResolvedByIndex`] when
    /// an external index resolved it, otherwise the stored rung, if any.
    ///
    /// The one place the index rung comes from, so a `Resolved` claim and a
    /// stored rung can never contradict each other — there is nothing stored
    /// to contradict.
    pub fn rung(&self) -> Option<Confidence> {
        match self.resolution {
            Resolution::Resolved => Some(Confidence::ResolvedByIndex),
            Resolution::Heuristic => self.confidence,
        }
    }
}

/// A scalar fact value. Kept small and typed so scalar conflicts can be
/// compared during the merge layer (U9).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactValue {
    Text(String),
    Int(i64),
    Bool(bool),
}

/// The kind of fact carried on a node. Grows additively as extractors land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    /// The source name of a symbol (tree-sitter). Carried on a Symbol node.
    SymbolName,
    /// The kind of a symbol — "function", "struct", "enum", … (tree-sitter).
    SymbolKind,
    /// The enclosing type/impl/namespace of a symbol, when it has one
    /// (tree-sitter). Absent for free functions and top-level types.
    SymbolContainer,
    /// The declaration header of a symbol as the source writes it —
    /// `public int Run(int a)`, `pub fn run(&self) -> bool`, `class Cfg`
    /// (tree-sitter). Everything up to the body, whitespace-collapsed so a
    /// reformat cannot churn the fact; never the implementation. Carried on a
    /// Symbol node. Distinct from the identity's normalized parameter list
    /// (KTD3), which lives in the node key, not in a fact.
    SymbolSignature,
    /// The access modifier the source actually STATES — Rust `pub`/`pub(crate)`,
    /// C# `public`/`internal`/…, a TS `private` or the `export` that wraps a
    /// module-scope declaration (tree-sitter). Carried on a Symbol node, and
    /// ABSENT when the source states nothing: Go and Python express visibility by
    /// naming convention, and reading a convention would be interpretation, not a
    /// measured fact (E9). There is no "default visibility" fact.
    SymbolVisibility,
    /// A doc comment quoted from the source verbatim — Rust `///`, C# `///`,
    /// TS/JS `/** … */`, a Python docstring (tree-sitter). Only the comment
    /// DELIMITERS are removed; the inner text, its language, its diacritics and
    /// its line breaks are never touched, reflowed or translated (R10). Carried
    /// on a Symbol node, or on a File node for Rust's file-level `//!` (which
    /// documents the module, not any item). Absent when there is no doc comment —
    /// never an empty fact. Its provenance range spans the COMMENT's own lines,
    /// not the declaration's.
    DocComment,
    /// Marks a symbol as a language-level entry point — `fn main`,
    /// `if __name__ == "__main__"`, … (tree-sitter).
    EntryPoint,
    /// Lines of code, total physical lines (code + comments + blanks) as tokei
    /// counts them (metrics). Carried on a File node (`Measured`) and, as a
    /// clearly-derived sum over its descendant files, on a Module node
    /// (`Derived`).
    Loc,
    /// Whether a source file has a paired test *by naming/tree convention*
    /// (test-pairing). Deliberately phrased as "has a test by convention" /
    /// "test by convention not found" — the heuristic can confirm a
    /// convention-matching test but can never prove one absent, so it never
    /// says "untested" (E9/R18). Carried on a source File node.
    TestByConvention,
    /// Per-language extraction coverage: how many source files of a grammar-
    /// supported language carry tree-sitter structural facts vs. only metrics
    /// and the file tree (test-pairing). The honest extraction-level number E9
    /// requires, stated as a displayed fact. Carried on the root module node.
    Coverage,
    /// The declared name of an ecosystem dependency (manifest-deps). Carried on
    /// a Dependency node — the human-readable name, mirrored from the node key's
    /// opaque `dep:<ecosystem>:<name>` identity so projections never parse the
    /// key back apart (same discipline as `SymbolName`).
    DependencyName,
    /// A dependency's scope within a manifest — `"normal"` or `"dev"`
    /// (manifest-deps). Carried on a Dependency node. A dependency declared in
    /// both a normal and a dev table carries both facts (additive, truthful);
    /// the `DependsOn` edge itself is deduped per (manifest, dependency).
    DependencyScope,
    /// A manifest-declared entry point — a Cargo `[[bin]]`/`[lib]`, a
    /// `package.json` `main`/`bin`, or a `.csproj` `OutputType` (manifest-deps).
    /// Carried on the manifest's own File node, with the declared target/kind as
    /// its text value. Distinct from the language-level `EntryPoint` (a `bool`
    /// on a Symbol node): TS/JS and library projects have no language-level
    /// entry, so the manifest is the only honest source (plan U6 note, E3).
    EntryPointDecl,
    /// A heuristically resolved identifier matched MORE THAN ONE candidate, and
    /// the count was within the configured ambiguity ceiling, so one named edge
    /// per candidate WAS emitted alongside this fact (KTD6, R23). Carried on the
    /// referring node (the innermost declaration around the site, or the file),
    /// with the identifier and the count as its text: `"Get: 3 candidates"`.
    /// Absent when the resolution was unambiguous — a single-candidate match needs
    /// no caveat, and emitting one everywhere would bury the ambiguous cases.
    CandidateCount,
    /// A heuristically resolved identifier matched MORE candidates than the
    /// configured ceiling, so NO named edge was emitted at all — this fact is the
    /// only thing the graph says about that identifier (R23). Same node, same
    /// `"Execute: 412 candidates"` text as [`FactKind::CandidateCount`]; the KIND
    /// is what tells a consumer that the missing edges are ambiguity and not
    /// absence (R20). Without the split, a common name like `Get` or `Execute`
    /// would either drown the index in edges or vanish from it silently.
    AmbiguousReference,
    /// The human-readable name of an external symbol or resource (external) —
    /// `Newtonsoft.Json.JsonConvert`, `dbo.Orders`, `api.example.com/v1/orders`,
    /// `Orbit:ConnectionName`. Carried on an External node, mirrored from the
    /// opaque `ext:`/`res:` node key exactly as [`FactKind::DependencyName`]
    /// mirrors `dep:` — so no projection ever parses a key back apart (KTD-B1).
    ExternalName,
    /// What SORT of external thing an External node is — `"symbol"`, `"db_object"`,
    /// `"url"` or `"config_key"` (external). Carried on an External node beside
    /// [`FactKind::ExternalName`]. It exists precisely so a consumer can tell a
    /// foreign type from a database object WITHOUT reading the key's `ext:`/`res:db:`
    /// prefix, which the opaque-key discipline forbids.
    ExternalKind,
    /// A literal whose SHAPE matched a secret pattern — a password or token
    /// assignment, a connection string, a PEM private-key header, a URL carrying
    /// userinfo (external). R22: the VALUE is never carried, here or anywhere
    /// downstream; the fact's text is the NAME of the pattern that fired
    /// (`"password"`, `"connection string"`, …) and its provenance is the place in
    /// the code. Carried on the innermost declaration around the literal, or on the
    /// file when no declaration encloses it. Deliberately a pattern-match claim and
    /// not a proof: a redacted reference costs nothing when the shape was
    /// coincidental, whereas a leaked value cannot be taken back.
    RedactedLiteral,
    /// The symbol's identity in an external semantic index — its SCIP moniker
    /// (scip). Carried on a Symbol node with `Resolution::Resolved`, beside the
    /// heuristic fallback identity that IS the node key (KTD3). Only emitted for a
    /// symbol whose definition the index and the working tree agree on, so the
    /// moniker is never a claim about code the index no longer describes.
    SymbolMoniker,
    /// A source file an external semantic index covered AND whose declarations
    /// still line up with the index (scip). Carried on a File node; its text names
    /// the indexer (`"scip-dotnet 0.2.14"`), because which tool produced the
    /// coverage is part of how far it may be trusted.
    ///
    /// This fact is what licenses an absence claim inside that file per R20. A file
    /// the index covered but whose declarations have MOVED gets no such fact — see
    /// the drift gate in [`crate::extract::scip`].
    ResolvedByIndex,
    /// The run-level outcome of consuming an external index (scip): how many of its
    /// documents verified, how many drifted, how many named no tracked file.
    /// Carried on the root module node as displayed text. Deliberately reports the
    /// rejected counts too — a run where nothing matched must diagnose itself
    /// rather than quietly degrade to heuristic (E9).
    ResolvedCoverage,
    /// A language plugin failed on this file (R29): it trapped, ran out of
    /// fuel, or could not be started. Carried on the File node, with the plugin
    /// as its extractor and the pass and the reason as text — redacted like any
    /// other text a plugin produced, since a trap message can quote source.
    ///
    /// What makes coverage say "the plugin failed" instead of the false "it
    /// declares no symbol" about a file nothing ever finished reading.
    ExtractionFailed,
    /// One database access at one place in the code (plan B, U2: R11, KTD2):
    /// what a SQL statement in a literal does with a qualified object. Carried
    /// on the innermost declaration around the occurrence — the method — or on
    /// the file when none encloses it, with the occurrence's line as its
    /// provenance. The text is `<access> <object>`: `reads dba.t`,
    /// `writes dba.t`, `calls dba.proc`, or `mentions dba.t` where the
    /// statement's target or an alias could not be told apart, so a reader is
    /// told no more than that the code names the object.
    ///
    /// Read per statement, not from the word right before the object:
    /// `UPDATE t SET … FROM dba.t t` writes `dba.t`. Additive — one per
    /// occurrence. Beside the `Uses` edge, never instead of it: that edge
    /// still says once that the declaration uses the object at all.
    DbAccess,
    /// A text a reader might search for — a string literal or a line of an
    /// ordinary comment — leading to the declaration it sits in and its line
    /// (plan B, U3: R9, R13, KTD1). Read by the host lexer for every language
    /// it lexes, so it needs no plugin. Carried on the innermost declaration
    /// around the text, or on the file when none encloses it (a C file has no
    /// plugin, so all of its anchors are the file's); the provenance line is
    /// the FIRST line the text occurs on in that owner. The value is an
    /// [`AnchorText`] in its encoded form — read it with [`AnchorText::decode`].
    ///
    /// Doc comments are not anchors: [`FactKind::DocComment`] carries them
    /// already. Nothing the secret filter withholds ever becomes one — that
    /// leaves an [`FactKind::AnchorSkipped`] or a
    /// [`FactKind::RedactedLiteral`] instead.
    ///
    /// Detail-only: its one projection is a type's or a file's detail, never
    /// the manifest ([`FactKind::is_detail_only`]), which would otherwise grow
    /// by an order of magnitude. Additive — one per distinct text per owner
    /// per file, at most [`ANCHOR_CAP`] of them.
    Anchor,
    /// A place a text was found and withheld (plan B, U3: R10, KTD11): a
    /// comment line the secret filter replaced, or a literal that is one long
    /// token shaped like a key or a hash. The text is the REASON — the
    /// pattern's name (`"password"`, `"private key"`, `"secret value"`) or
    /// `"long token"` — never what was withheld; the provenance line is the
    /// place. A literal the filter withholds whole says so as a
    /// [`FactKind::RedactedLiteral`], as it did before anchors existed, and
    /// leaves no second fact here. Detail-only, additive.
    AnchorSkipped,
    /// How many anchors an owner had beyond [`ANCHOR_CAP`] and lost to it
    /// (plan B, U3: KTD1) — an integer, on the owner, once per file it has
    /// text in. A partial type whose parts sit in two files carries one per
    /// part, its provenance naming the file. Detail-only, additive.
    AnchorsDropped,
    /// One place a C# method decides by the value of an enum the repository
    /// declares (plan B, U5: R12, R13, KTD3): a `case E.V` label, a switch
    /// expression arm `E.V =>`, or a comparison `== E.V`, `!= E.V`, `is E.V`.
    /// Found by the host lexer in the code of a C# file and checked against
    /// the graph: the enum is a node of kind `enum`, and `V` one of its
    /// `variant` members. Carried on the innermost declaration around the
    /// place — the method — with the place's line as its provenance. The
    /// value is an [`EnumDecision`] in its encoded form — read it with
    /// [`EnumDecision::decode`].
    ///
    /// A label or an arm is `verified` only when `E` is, by the visibility
    /// rules the host can read without a syntax tree, the one type of that
    /// name the place can see; a comparison is never verified, because a
    /// local or a lambda parameter of that name could stand in for the type.
    /// A place no single enum could be named for leaves no fact; it is
    /// counted in [`FactKind::EnumDecisionsUnmatched`] instead.
    ///
    /// Detail-only ([`FactKind::is_detail_only`]): its one projection is the
    /// enum's detail (U8). Additive — one per place.
    EnumDecision,
    /// How many places in a file looked like a decision on an enum the
    /// repository declares and could be tied to no single one of them (plan
    /// B, U5: KTD3) — the qualifier names enums of which none, or more than
    /// one, has the member and visibility does not settle it. An integer, on
    /// the File node, absent when zero. Its one projection is coverage; left
    /// out of the manifest like the decisions it counts. Additive.
    EnumDecisionsUnmatched,
    /// The `bodies` pass of the second layer failed on this file (plan 2a,
    /// U4: KTD8): the plugin trapped, ran out of fuel or memory, or refused
    /// the file with a reason of its own. Carried on the File node, with the
    /// plugin as its extractor and `bodies: <reason>` as text, redacted like
    /// every text a plugin returns.
    ///
    /// The file loses only its second layer: its declarations and usages are
    /// the first layer's, and an [`FactKind::ExtractionFailed`] is never
    /// written for it. Written only by a run with `--detail-level`, so a run
    /// without one carries none. Detail-only ([`FactKind::is_detail_only`]):
    /// its projections are coverage and, from a later unit, the detail of a
    /// type that lost its second layer. Additive.
    BodyExtractionFailed,
    /// A reference by name (contract 0.3, `name-ref`: a markup file's
    /// `{StaticResource K}`) that no declaration of its target kind answers
    /// anywhere in the repository (plan 2c, U5: R11). Carried on the File node
    /// the reference is written in, whatever node it came from, with the
    /// plugin as its extractor, the reference's line as its provenance and
    /// `<name>: no <target kind> declares this name` as text — admitted, not
    /// dropped, so a missing edge is never indistinguishable from no reference
    /// at all (R20). One per name and target kind per file, at its first site.
    ///
    /// Written only for a plugin that reports references by name, so no run
    /// without one carries any. Detail-only ([`FactKind::is_detail_only`]):
    /// a v10 manifest keeps its closed set of fact words; its projection is
    /// the file's documents, from a later unit (plan 2c, U8). Additive.
    UnresolvedReference,
    /// A kind this corpus declared for itself (R1). Its cardinality comes from
    /// the declaration, which is why `merge` can resolve it (R7).
    #[serde(skip)]
    Declared(crate::graph::FactKindId),
}

/// A single assertion by one extractor. Losing claims survive here as dissent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Claim {
    pub value: FactValue,
    pub provenance: Provenance,
}

/// A fact attached to a node: a resolved value plus any dissenting claims.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Fact {
    pub kind: FactKind,
    pub value: FactValue,
    pub provenance: Provenance,
    /// Claims from other extractors that lost a scalar conflict (E7). Empty
    /// until the merge layer (U9) populates it.
    pub dissent: Vec<Claim>,
}

impl Fact {
    /// A fact with no dissent yet (the common case at extraction time).
    pub fn new(kind: FactKind, value: FactValue, provenance: Provenance) -> Self {
        Fact {
            kind,
            value,
            provenance,
            dissent: Vec::new(),
        }
    }

    /// Every provenance this fact carries — its own and each dissenting
    /// claim's. Dissent is included deliberately: a losing claim names an
    /// extractor too, and a canonicalization that skipped it would leave one
    /// freshly-allocated id per dissenting claim behind (U3).
    pub fn provenances_mut(&mut self) -> impl Iterator<Item = &mut Provenance> {
        std::iter::once(&mut self.provenance)
            .chain(self.dissent.iter_mut().map(|c| &mut c.provenance))
    }
}

/// The kind of entity a node represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Module,
    Symbol,
    Dependency,
    /// Something the repository REFERENCES but does not contain (R3, U5): a type
    /// from a foreign library, a database object, a network endpoint, a
    /// configuration key. Distinct from [`NodeKind::Dependency`], which is an
    /// ecosystem package a manifest DECLARES — a dependency is a unit of
    /// distribution, an external is a unit of use, and one package supplies many
    /// externals.
    ///
    /// Keys are prefixed by what the target is, keeping them disjoint from every
    /// other key-space (KTD-B1): `ext:<lang>:<fqname>` for a foreign symbol,
    /// `res:db:<object>`, `res:url:<host/path>`, `res:cfg:<key>` for a resource.
    /// The key stays opaque — [`FactKind::ExternalName`] and
    /// [`FactKind::ExternalKind`] carry the readable name and the sort, so nothing
    /// parses a key back apart.
    External,
    /// A kind this corpus declared for itself (R1).
    ///
    /// One variant, not a hole: the core still enumerates its own kinds via
    /// [`NodeKind::core_kinds`] without reading any plugin (R2). What the core
    /// may do with a declared kind is fixed by its declaration — a structural
    /// role (R9) and a card policy (R8) — never by its identity.
    #[serde(skip)]
    Declared(crate::graph::NodeKindId),
}

/// The kind of relation an edge represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A module/directory contains a file or sub-module.
    Contains,
    /// A file declares a symbol (function, type, …). Kept distinct from
    /// `Contains` so module-level consumers filtering on containment never pull
    /// in file→symbol edges (which would otherwise need a node-kind workaround).
    Declares,
    /// A type owns a member — a method, property, field, enum variant or nested
    /// type (KTD5). Members are their own Symbol nodes rather than facts on the
    /// type, because a relation *leaving* a member (a call, a reference)
    /// originates from the member and could not be attached to a fact. Kept
    /// distinct from `Declares` (which is file → symbol, whatever its nesting), so
    /// a consumer asking "what does this type contain" never has to filter a
    /// file's whole symbol list.
    HasMember,
    /// A type inherits from another type the repo also declares — a C#
    /// `class D : B`, a TS `class D extends B` / `interface I extends J`, a Python
    /// `class D(B)`, a Rust supertrait `trait U: T` (R2). Always derived → base,
    /// mirroring `Imports`' importer → imported direction.
    ///
    /// Emitted ONLY when the base type resolves to a Symbol node that already
    /// exists: a base outside the repo gets NO edge rather than a fabricated
    /// target node (U5 introduces external nodes). That loses nothing a reader
    /// needs — the declaration's own spelling of the base (`: B`, `extends B`,
    /// `(B)`, `: T`) is carried verbatim by the deriving type's `SymbolSignature`,
    /// so a missing edge is never indistinguishable from "no base type" (R20).
    Extends,
    /// A type implements an interface or a trait the repo also declares — a C#
    /// `class D : IFoo`, a TS `class D implements IFoo`, a Rust
    /// `impl Trait for Type` (R2). Same derived → base direction and the same
    /// no-phantom discipline as `Extends`.
    ///
    /// No Go source ever produces one: Go declares no implements relation at all,
    /// and deciding which interfaces a type satisfies needs type information,
    /// i.e. `Resolution::Resolved` territory, not a name heuristic.
    Implements,
    /// A callable calls another callable this repo also declares (R2/R4, U4).
    /// Caller → callee, the same direction `Extends`/`Imports` use. The caller is
    /// the innermost declaration the call site sits inside — normally a member,
    /// which is exactly why members are their own Symbol nodes (KTD5) — or the
    /// FILE when the call sits in module-level code (a Python script body, a C#
    /// top-level statement) that no declaration encloses.
    ///
    /// Heuristic by construction (KD2): the callee is matched by NAME against the
    /// repo-wide symbol table, so a name several types declare yields one edge per
    /// candidate plus a [`FactKind::CandidateCount`] fact, and a name with more
    /// candidates than the configured ceiling yields NO named edges and a single
    /// [`FactKind::AmbiguousReference`] fact instead (R23). A name this repo does
    /// not declare at all yields no edge and no fact — never an invented target.
    Calls,
    /// A declaration names a type this repo also declares in a TYPE position — a
    /// parameter type, a field/property type, a return type, a local declaration's
    /// type (R2/R4, U4). Referrer → referenced type, with the same innermost-
    /// declaration origin, the same name heuristic, the same candidate/ceiling
    /// discipline and the same no-phantom rule as `Calls`.
    References,
    /// A declaration constructs a type this repo also declares — a C#
    /// `new T(...)`, a TS `new T(...)`, a Rust `T { … }` struct literal, a Go
    /// `T{…}` composite literal, a Python `T()` call on a declared class. Kept
    /// distinct from `Calls` because construction names a TYPE, not a callable:
    /// a consumer asking "who builds this type" must not have to filter a
    /// constructor's name out of the call set.
    Instantiates,
    /// A file imports another (source-level).
    Imports,
    /// A depends-on relation from a manifest.
    DependsOn,
    /// Code reaches OUTSIDE the repository: a member (or the file, when no
    /// declaration encloses the site) → an [`NodeKind::External`] node (R3, U5).
    /// User → used, the same direction every other relation here runs in.
    ///
    /// Always `Resolution::Heuristic` (KTD8/KD2): the target was recognized by an
    /// unambiguous textual pattern — an import matched against a manifest-declared
    /// dependency, a qualified object name inside a SQL statement, an absolute URL,
    /// a configuration read through a named API — never by an external index. A
    /// pattern that only PLAUSIBLY matches emits nothing at all: a false external
    /// would discredit every true one.
    Uses,
    /// A kind this corpus declared for itself (R1). See [`NodeKind::Declared`].
    #[serde(skip)]
    Declared(crate::graph::EdgeKindId),
}

/// One place in the source a name-resolved relation was read at — the
/// stable id of a usage site as the corpus shows it (plan A, KTD7): the file,
/// the 1-based line, and the column where the plugin reports one.
///
/// Output is at the level of RELATIONS (plan A, Key Decision on relations):
/// an [`Edge`] is the summary of its sites and carries their ids, never one
/// edge per site. Alternatives of one ambiguous site are separate edges that
/// share the site's id, which is what lets a consumer group them back into the
/// one claim they are.
///
/// `column` comes from plugin contract 0.2 (plan A, U5/U6), which the bundled
/// C# plugin reports: the column of the used name, counted in characters, and
/// `line` is then that name's line too. It is skipped on the wire while
/// absent — a 0.1 plugin, or a language whose plugin reads no context — and
/// there two calls of one name on one line (`sb.Append(a).Append(b)`) are ONE
/// site: such a plugin cannot tell them apart, and inventing a distinction it
/// did not report would be worse.
///
/// Field order is the derived [`Ord`] — file, then line, then column — which is
/// source order, and the order every site list is kept in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SiteId {
    /// Repo-root-relative, `/`-normalized source path, as in [`Provenance`].
    pub file: String,
    /// The 1-based line of the usage.
    pub line: u32,
    /// The 1-based column of the used name, in characters, when the plugin
    /// reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

impl SiteId {
    /// The site a point-anchored provenance names, or `None` when it names no
    /// line — a whole-file assertion has no site to offer.
    pub fn of(provenance: &Provenance) -> Option<SiteId> {
        provenance.span.start().map(|line| SiteId {
            file: provenance.file.clone(),
            line,
            column: None,
        })
    }
}

/// An entity in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    pub key: NodeKey,
    pub kind: NodeKind,
    pub facts: Vec<Fact>,
}

/// A relation between two entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub kind: EdgeKind,
    pub provenance: Provenance,
    /// Claims about this same relation that lost the merge ranking (E7, KTD6).
    /// Two sources asserting one relation — a name heuristic and an external index
    /// — become ONE edge at the resolved precision, with the heuristic assertion
    /// preserved here rather than deleted: the edge stays traceable to every source
    /// that ever asserted it.
    ///
    /// A claim carries a value because [`Claim`] is shared with facts; for an edge
    /// that value is always `FactValue::Bool(true)` — "this relation is asserted".
    /// The relation's kind is already the edge's own `kind` (and part of the merge
    /// group key), so restating it in the claim would only fork the wire vocabulary
    /// `manifest` owns. What matters here is the [`Provenance`].
    pub dissent: Vec<Claim>,
    /// Every usage site this relation was read at, sorted and deduplicated
    /// (plan A, KTD7) — empty for a relation no usage site produced: a
    /// `Contains`, a `Declares`, an import.
    ///
    /// On the edge, not on [`Provenance`]: a provenance is ONE claim, and merge
    /// pools claims by value, so a site list inside it would make two claims
    /// of one relation differ only by where they were read — the second site
    /// would turn into dissent and the winner would keep one site of several.
    /// Here `merge_edges` unions the lists of every claim on the relation,
    /// which is what "an edge is the summary of its sites" means. The
    /// provenance stays the site whose rung is the edge's rung.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<SiteId>,
}

impl Edge {
    pub fn new(kind: EdgeKind, provenance: Provenance) -> Self {
        Edge {
            kind,
            provenance,
            dissent: Vec::new(),
            sites: Vec::new(),
        }
    }

    /// This edge with `sites` added to its site list, which stays sorted and
    /// free of duplicates however the sites arrive.
    #[must_use]
    pub fn at_sites(mut self, sites: impl IntoIterator<Item = SiteId>) -> Self {
        self.sites.extend(sites);
        self.sites.sort();
        self.sites.dedup();
        self
    }

    /// Every provenance this edge carries — see [`Fact::provenances_mut`].
    pub fn provenances_mut(&mut self) -> impl Iterator<Item = &mut Provenance> {
        std::iter::once(&mut self.provenance)
            .chain(self.dissent.iter_mut().map(|c| &mut c.provenance))
    }
}

/// A borrowed, resolved view of an edge with its endpoint keys — the shape the
/// projections consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeView<'a> {
    pub from: &'a NodeKey,
    pub to: &'a NodeKey,
    pub edge: &'a Edge,
}

impl NodeKind {
    /// The kinds the core owns, independent of any corpus (R2).
    ///
    /// Kept in step with the enum by `core_kind_lists_are_exhaustive`, which
    /// fails to compile — not merely fails — when a variant is added.
    ///
    /// **What that does and does not buy** (RG2 sharpened this): adding a
    /// variant breaks the exhaustive match in that test, so a decision is
    /// forced. Nothing forces the decision to reach THIS list — a variant added
    /// to the enum and to the match, but not here, compiles and passes. Rust
    /// cannot enumerate an enum's variants without a macro, and generating these
    /// enums would cost the per-variant documentation that is most of their
    /// value. Since U7 the completeness of this list is what keeps
    /// [`crate::graph::fact_kind_from_word`] and its siblings total, so the gap
    /// is named rather than left to be discovered.
    #[must_use]
    pub const fn core_kinds() -> &'static [NodeKind] {
        &[
            NodeKind::File,
            NodeKind::Module,
            NodeKind::Symbol,
            NodeKind::Dependency,
            NodeKind::External,
        ]
    }
}

impl EdgeKind {
    /// The kinds the core owns, independent of any corpus (R2).
    #[must_use]
    pub const fn core_kinds() -> &'static [EdgeKind] {
        &[
            EdgeKind::Contains,
            EdgeKind::Declares,
            EdgeKind::HasMember,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::Calls,
            EdgeKind::References,
            EdgeKind::Instantiates,
            EdgeKind::Imports,
            EdgeKind::DependsOn,
            EdgeKind::Uses,
        ]
    }
}

impl FactKind {
    /// The kinds the core owns, independent of any corpus (R2).
    #[must_use]
    pub const fn core_kinds() -> &'static [FactKind] {
        &[
            FactKind::SymbolName,
            FactKind::SymbolKind,
            FactKind::SymbolContainer,
            FactKind::SymbolSignature,
            FactKind::SymbolVisibility,
            FactKind::DocComment,
            FactKind::EntryPoint,
            FactKind::Loc,
            FactKind::TestByConvention,
            FactKind::Coverage,
            FactKind::DependencyName,
            FactKind::DependencyScope,
            FactKind::EntryPointDecl,
            FactKind::CandidateCount,
            FactKind::AmbiguousReference,
            FactKind::ExternalName,
            FactKind::ExternalKind,
            FactKind::RedactedLiteral,
            FactKind::SymbolMoniker,
            FactKind::ResolvedByIndex,
            FactKind::ResolvedCoverage,
            FactKind::ExtractionFailed,
            FactKind::DbAccess,
            FactKind::Anchor,
            FactKind::AnchorSkipped,
            FactKind::AnchorsDropped,
            FactKind::EnumDecision,
            FactKind::EnumDecisionsUnmatched,
            FactKind::BodyExtractionFailed,
            FactKind::UnresolvedReference,
        ]
    }

    /// Whether this kind is projected into a detail only and never into the
    /// manifest (plan B, KTD1). The graph and the cache keep it like any other
    /// fact; the manifest leaves it out, because the anchors of a large
    /// repository outnumber every other fact together, and a channel that wants
    /// them reads the detail documents.
    ///
    /// The enum decisions of U5 join them for a reason of their own: the
    /// enum's detail is where the plan shows them (R12), and half of them are
    /// comparisons the host cannot verify — claims a channel reading the
    /// manifest as the graph's facts would take at face value, while the
    /// detail says what each one is. Their unmatched count goes with them; its
    /// projection is coverage.
    #[must_use]
    pub const fn is_detail_only(&self) -> bool {
        matches!(
            self,
            FactKind::Anchor
                | FactKind::AnchorSkipped
                | FactKind::AnchorsDropped
                | FactKind::EnumDecision
                | FactKind::EnumDecisionsUnmatched
                | FactKind::BodyExtractionFailed
                | FactKind::UnresolvedReference
        )
    }
}

/// How many anchors one owner keeps per file (plan B, U3: KTD1). The rest are
/// counted in an [`FactKind::AnchorsDropped`] fact. Generous on purpose: a
/// method rarely holds more texts than this, and what the cap protects is the
/// size of a detail, not the graph's.
pub const ANCHOR_CAP: usize = 48;

/// How many characters of one anchor's text are kept (plan B, U3: KTD1). A
/// longer text is cut at a character boundary, and the anchor states how long
/// it was.
pub const ANCHOR_TEXT_CAP: usize = 160;

/// Where an anchor's text was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnchorSource {
    /// A string literal — what a message, a query or a key is written as.
    Literal,
    /// A line of an ordinary (non-doc) comment.
    Comment,
}

impl AnchorSource {
    fn word(self) -> &'static str {
        match self {
            AnchorSource::Literal => "literal",
            AnchorSource::Comment => "comment",
        }
    }
}

/// The value of one [`FactKind::Anchor`] fact (plan B, U3).
///
/// A fact value is a single text, so the anchor's four parts are written into
/// it as `<source> <occurrences> <length> <text>` — the text last, because it
/// is the only part that may hold a space, and whitespace inside it is already
/// collapsed to single spaces, so the split is unambiguous. Kept in one place
/// with its reader so the detail (U8) never re-derives the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorText {
    pub source: AnchorSource,
    /// How many times this exact (normalized) text occurs in the owner, in
    /// this file. The fact's line is the first of them.
    pub occurrences: u32,
    /// The length of the normalized text in characters BEFORE it was cut to
    /// [`ANCHOR_TEXT_CAP`] — equal to the length of `text` when it was not.
    pub length: u32,
    /// The text, whitespace collapsed, at most [`ANCHOR_TEXT_CAP`] characters.
    pub text: String,
}

impl AnchorText {
    /// The fact value this anchor is stored as.
    #[must_use]
    pub fn encode(&self) -> String {
        format!(
            "{} {} {} {}",
            self.source.word(),
            self.occurrences,
            self.length,
            self.text
        )
    }

    /// Read an [`FactKind::Anchor`] value back, or `None` when it is not one.
    #[must_use]
    pub fn decode(value: &str) -> Option<AnchorText> {
        let mut parts = value.splitn(4, ' ');
        let source = match parts.next()? {
            "literal" => AnchorSource::Literal,
            "comment" => AnchorSource::Comment,
            _ => return None,
        };
        Some(AnchorText {
            source,
            occurrences: parts.next()?.parse().ok()?,
            length: parts.next()?.parse().ok()?,
            text: parts.next()?.to_string(),
        })
    }
}

/// What a SQL statement does with one object it names (plan B, U2: KTD2) —
/// the first word of a [`FactKind::DbAccess`] value, which is
/// `<access> <object>`. The extractor writes it and the cards read it back
/// through the same type, so the vocabulary has one home.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DbAccessKind {
    Reads,
    Writes,
    /// `EXEC dba.proc` — in the reference repository the commonest statement of all, and
    /// neither a read nor a write the code itself performs.
    Calls,
    /// Inside a recognized statement whose target, or the alias standing for
    /// it, could not be told: the code names the object, and that is all.
    Mentions,
}

impl DbAccessKind {
    /// The [`FactKind::DbAccess`] word — the whole of that vocabulary.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            DbAccessKind::Reads => "reads",
            DbAccessKind::Writes => "writes",
            DbAccessKind::Calls => "calls",
            DbAccessKind::Mentions => "mentions",
        }
    }

    /// Read a [`FactKind::DbAccess`] value back as its access and object, or
    /// `None` when it is not one. The object holds no space — it is a
    /// qualified SQL name — so the split at the first space is unambiguous.
    #[must_use]
    pub fn decode(value: &str) -> Option<(DbAccessKind, &str)> {
        let (word, object) = value.split_once(' ')?;
        let access = match word {
            "reads" => DbAccessKind::Reads,
            "writes" => DbAccessKind::Writes,
            "calls" => DbAccessKind::Calls,
            "mentions" => DbAccessKind::Mentions,
            _ => return None,
        };
        (!object.is_empty()).then_some((access, object))
    }
}

/// The form of a place that decides by an enum's value (plan B, U5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DecisionShape {
    /// A `case E.V:` label of a switch statement, `when` clause or not.
    Label,
    /// An arm `E.V =>` of a switch expression.
    Arm,
    /// A comparison `== E.V`, `!= E.V`, `is E.V` or `is not E.V`, on either
    /// side of the operator. Never verified (see [`FactKind::EnumDecision`]).
    Comparison,
}

impl DecisionShape {
    fn word(self) -> &'static str {
        match self {
            DecisionShape::Label => "label",
            DecisionShape::Arm => "arm",
            DecisionShape::Comparison => "comparison",
        }
    }
}

/// The value of one [`FactKind::EnumDecision`] fact (plan B, U5).
///
/// Written as `<verified|unverified> <shape> <enum key> <member>`. The first
/// two are fixed words and a member is an identifier, so the split is
/// unambiguous even though an enum's node key may hold a space: a
/// namespace-less C# file is keyed by its path stem, and a directory may be
/// named `Shared Code`. The enum is named by
/// its node key, not its spelling, so the detail (U8) groups the places under
/// the enum without resolving anything again; the member names the `variant`
/// node under it the place was checked against. Kept in one place with its
/// reader, as [`AnchorText`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecision {
    /// Whether the host could tie the qualifier to this enum alone (KTD3).
    pub verified: bool,
    pub shape: DecisionShape,
    /// The enum's node.
    pub enum_key: NodeKey,
    /// The value decided on — the name of a `variant` member of the enum.
    pub member: String,
}

impl EnumDecision {
    /// The fact value this decision is stored as.
    #[must_use]
    pub fn encode(&self) -> String {
        format!(
            "{} {} {} {}",
            if self.verified {
                "verified"
            } else {
                "unverified"
            },
            self.shape.word(),
            self.enum_key.as_str(),
            self.member
        )
    }

    /// Read an [`FactKind::EnumDecision`] value back, or `None` when it is not
    /// one.
    #[must_use]
    pub fn decode(value: &str) -> Option<EnumDecision> {
        let mut parts = value.splitn(3, ' ');
        let verified = match parts.next()? {
            "verified" => true,
            "unverified" => false,
            _ => return None,
        };
        let shape = match parts.next()? {
            "label" => DecisionShape::Label,
            "arm" => DecisionShape::Arm,
            "comparison" => DecisionShape::Comparison,
            _ => return None,
        };
        let (enum_key, member) = parts.next()?.rsplit_once(' ')?;
        if enum_key.is_empty() || member.is_empty() {
            return None;
        }
        let (enum_key, member) = (NodeKey::new(enum_key), member.to_string());
        Some(EnumDecision {
            verified,
            shape,
            enum_key,
            member,
        })
    }
}

/// The fact graph. Wraps a `petgraph` digraph with a deterministic key index.
///
/// Carries its own [`Vocabulary`] (KTD4). Anything already holding a `&Graph` —
/// `merge`, `build_manifest`, `build_cards`, `build_coverage`, every view — can
/// therefore resolve a declared kind without a signature change, and a declared
/// id can never be read against a vocabulary that did not issue it.
#[derive(Debug, Default)]
pub struct Graph {
    inner: DiGraph<Node, Edge>,
    index: BTreeMap<NodeKey, NodeIndex>,
    vocabulary: Vocabulary,
    /// What the shared resolver decided at every usage site (plan A, KTD7):
    /// its counters always, its full records when the run asked for them.
    /// Carried beside the nodes and edges, never inside them — no projection
    /// of the corpus reads it, so it cannot change a corpus byte.
    sites: SiteLedger,
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            inner: DiGraph::new(),
            index: BTreeMap::new(),
            vocabulary: Vocabulary::default(),
            sites: SiteLedger::default(),
        }
    }

    /// This corpus's declared vocabulary. Empty for every corpus the bundled
    /// extractors produce.
    #[must_use]
    pub fn vocabulary(&self) -> &Vocabulary {
        &self.vocabulary
    }

    /// The resolver's usage-site ledger (plan A, KTD7).
    #[must_use]
    pub fn sites(&self) -> &SiteLedger {
        &self.sites
    }

    /// The ledger, for the extractor that resolves usage sites to bank into.
    pub fn sites_mut(&mut self) -> &mut SiteLedger {
        &mut self.sites
    }

    /// Declare a node kind for this corpus (R1).
    ///
    /// `role` decides whether the kind takes part in the backbone views and how
    /// (R9); `card` decides whether its nodes earn a card (R8). The two are
    /// independent — a definition need not be card-worthy, and a card-worthy
    /// node need not appear in the tree.
    pub fn declare_node_kind(
        &mut self,
        namespace: impl Into<String>,
        name: impl Into<String>,
        role: StructuralRole,
        card: CardPolicy,
    ) -> Result<NodeKindId, VocabularyError> {
        let namespace = namespace.into();
        let name = name.into();
        if let CardPolicy::Card(id) = card {
            if self.vocabulary.card(id).is_none() {
                return Err(VocabularyError::UndeclaredCardPolicy {
                    name: format!("{namespace}:{name}"),
                    card: id.get(),
                });
            }
        }
        self.vocabulary.declare_node(NodeDeclaration {
            namespace,
            name,
            role,
            card,
        })
    }

    /// Declare an edge kind for this corpus (R1). `role` decides its part in the
    /// backbone views (R9).
    pub fn declare_edge_kind(
        &mut self,
        namespace: impl Into<String>,
        name: impl Into<String>,
        role: LinkRole,
    ) -> Result<EdgeKindId, VocabularyError> {
        self.vocabulary.declare_edge(EdgeDeclaration {
            namespace: namespace.into(),
            name: name.into(),
            role,
        })
    }

    /// Declare a fact kind for this corpus (R1).
    ///
    /// The cardinality is mandatory because `merge` has no other way to choose
    /// between a union and a single winner (R7). When in doubt prefer
    /// `Additive`, for the same reason the core kinds do: collapsing a genuinely
    /// multi-valued fact loses data, an extra fact does not.
    pub fn declare_fact_kind(
        &mut self,
        namespace: impl Into<String>,
        name: impl Into<String>,
        cardinality: Cardinality,
    ) -> Result<FactKindId, VocabularyError> {
        self.vocabulary.declare_fact(FactDeclaration {
            namespace: namespace.into(),
            name: name.into(),
            cardinality,
        })
    }

    /// Declare a card kind for this corpus (R1).
    pub fn declare_card_kind(
        &mut self,
        namespace: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<CardKindId, VocabularyError> {
        self.vocabulary.declare_card(CardDeclaration {
            namespace: namespace.into(),
            name: name.into(),
        })
    }

    /// The part a node kind plays in the backbone views (R9, KTD6).
    ///
    /// The single question the views are allowed to ask. Core kinds answer from
    /// the table in the plan; a declared kind answers from its declaration, and
    /// an id this corpus never declared answers `Opaque` — a view must render
    /// something rather than panic on data it cannot place.
    #[must_use]
    pub fn node_role(&self, kind: NodeKind) -> StructuralRole {
        vocabulary::node_role(kind, &self.vocabulary)
    }

    /// The part an edge kind plays in the backbone views (R9, KTD6).
    ///
    /// Only the three the views branch on carry a role. Every other core edge
    /// kind is card and smell policy rather than the shape of the tree, so it is
    /// `Opaque` here and keeps its concrete-kind match where it is read. The
    /// arm below lists them instead of using a wildcard, so a new core edge kind
    /// has to decide rather than default.
    #[must_use]
    pub fn edge_role(&self, kind: EdgeKind) -> LinkRole {
        vocabulary::edge_role(kind, &self.vocabulary)
    }

    /// Whether every declared kind this node, fact or edge names was declared on
    /// *this* graph (A2).
    ///
    /// An id from another graph is meaningless here — the digest would resolve
    /// to nothing, and the projection would write `undeclared:…` into a corpus
    /// that claims to describe itself. Refusing at the door keeps that
    /// unreachable rather than merely unlikely.
    fn knows_node_kind(&self, kind: NodeKind) -> bool {
        match kind {
            NodeKind::Declared(id) => self.vocabulary.node(id).is_some(),
            _ => true,
        }
    }

    /// Insert a node, or return the existing one if the key is already present.
    /// Deduplication is by canonical key, so two extractors describing the same
    /// entity converge onto one node (E5).
    ///
    /// A key must map to exactly one kind. Extractors converge onto an existing
    /// node only when they mean the same entity (E5); a same-key/different-kind
    /// reuse — e.g. a Symbol or Dependency key colliding with a file/module path
    /// — is a key-space bug (KTD-B1), not a merge, and would silently attach
    /// facts to the wrong node. It is a hard [`GraphError`] in every build, so
    /// the caller must decide to skip the emission rather than corrupt the IR.
    /// The prefixed `sym:`/`dep:` key-spaces make this unreachable in practice;
    /// this is the integrity guard behind that invariant, never a resolver.
    ///
    /// A declared kind is refused unless *this* graph declared it (A2), and a
    /// declared kind in the `Container` role is refused unless its key has the
    /// hierarchical shape the module tree requires (R16). Both are named errors:
    /// the alternative is a view that renders empty for a reason no message
    /// explains — the failure mode `parent_closed` already guards for core
    /// kinds.
    pub fn ensure_node(&mut self, key: NodeKey, kind: NodeKind) -> Result<NodeIndex, GraphError> {
        if !self.knows_node_kind(kind) {
            return Err(GraphError::UndeclaredKind {
                category: "node",
                key: key.as_str().to_string(),
            });
        }
        if self.node_role(kind) == StructuralRole::Container && !is_hierarchical(key.as_str()) {
            return Err(GraphError::NonHierarchicalContainer {
                key: key.as_str().to_string(),
            });
        }
        if let Some(&idx) = self.index.get(&key) {
            let existing = self.inner[idx].kind;
            if existing != kind {
                return Err(GraphError::KindCollision {
                    key: key.as_str().to_string(),
                    existing,
                    got: kind,
                });
            }
            return Ok(idx);
        }
        let idx = self.inner.add_node(Node {
            key: key.clone(),
            kind,
            facts: Vec::new(),
        });
        self.index.insert(key, idx);
        Ok(idx)
    }

    /// Attach a fact to an existing node. Returns `false` if the node is absent,
    /// or if the fact names a kind this graph never declared (A2).
    ///
    /// The undeclared case is reported on stderr in every build and the fact is
    /// not recorded. A `Result` here would ripple through every extractor for a
    /// case a correct caller cannot reach, and a debug-only assertion would let
    /// a release build lose the fact in silence — which is the exact failure
    /// this plan exists to remove.
    pub fn add_fact(&mut self, key: &NodeKey, fact: Fact) -> bool {
        if let FactKind::Declared(id) = fact.kind {
            if self.vocabulary.fact(id).is_none() {
                // Diagnosed unconditionally, not behind `debug_assert`. Every
                // production caller ignores the bool, so a debug-only signal
                // would let a release build produce a smaller corpus that is
                // internally consistent and therefore indistinguishable from a
                // correct one. An earlier draft asserted here and made the
                // documented "returns false" contract impossible to exercise —
                // the refusal panicked instead of returning.
                eprintln!(
                    "graph: dropped a fact on {} — kind {id:?} was never declared on this graph",
                    key.as_str()
                );
                return false;
            }
        }
        match self.index.get(key) {
            Some(&idx) => {
                self.inner[idx].facts.push(fact);
                true
            }
            None => false,
        }
    }

    /// Add an edge between two existing nodes. Returns `false` if either
    /// endpoint is absent (edges never implicitly create endpoints), or if the
    /// edge names a kind this graph never declared (A2, see [`Graph::add_fact`]).
    pub fn add_edge(&mut self, from: &NodeKey, to: &NodeKey, edge: Edge) -> bool {
        if let EdgeKind::Declared(id) = edge.kind {
            if self.vocabulary.edge(id).is_none() {
                // See `add_fact`: a silent drop in release is the failure mode
                // this whole plan exists to remove.
                eprintln!(
                    "graph: dropped an edge {} -> {} — kind {id:?} was never declared here",
                    from.as_str(),
                    to.as_str()
                );
                return false;
            }
        }
        match (self.index.get(from), self.index.get(to)) {
            (Some(&a), Some(&b)) => {
                self.inner.add_edge(a, b, edge);
                true
            }
            _ => false,
        }
    }

    /// Look up a node by key.
    pub fn node(&self, key: &NodeKey) -> Option<&Node> {
        self.index.get(key).map(|&idx| &self.inner[idx])
    }

    pub fn node_count(&self) -> usize {
        self.inner.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.inner.edge_count()
    }

    /// All nodes in a stable order (by key), independent of insertion order.
    pub fn nodes_sorted(&self) -> Vec<&Node> {
        let mut nodes: Vec<&Node> = self.inner.node_weights().collect();
        nodes.sort_by(|a, b| a.key.cmp(&b.key));
        nodes
    }

    /// Every node, in no particular order — for a count or an index whose
    /// result does not depend on one, which so must not pay for
    /// [`Graph::nodes_sorted`]'s sort. Nothing that reaches output may
    /// iterate this in order.
    pub(crate) fn nodes_unordered(&self) -> impl Iterator<Item = &Node> {
        self.inner.node_weights()
    }

    /// Every edge, in no particular order — for a pass that only looks for the
    /// edges it will change and so must not pay for [`Graph::edges_sorted`]'s
    /// sort. Nothing that reaches output may iterate this.
    pub(crate) fn edges_unordered(&self) -> impl Iterator<Item = EdgeView<'_>> {
        self.inner.edge_references().map(|e| EdgeView {
            from: &self.inner[e.source()].key,
            to: &self.inner[e.target()].key,
            edge: e.weight(),
        })
    }

    /// Replace edges in place: `rewrite` sees every edge with its endpoints and
    /// returns its replacement, or `None` to leave it alone. Endpoints and
    /// kinds are the caller's to keep — this changes what an edge says, never
    /// which relation it is, so the merge grouping it feeds stays the same.
    ///
    /// Two phases, because a node's key and an edge's weight both live in the
    /// petgraph, and the replacement is decided while the keys are borrowed.
    pub(crate) fn rewrite_edges(
        &mut self,
        mut rewrite: impl FnMut(&NodeKey, &NodeKey, &Edge) -> Option<Edge>,
    ) {
        let changes: Vec<(petgraph::graph::EdgeIndex, Edge)> = self
            .inner
            .edge_references()
            .filter_map(|e| {
                rewrite(
                    &self.inner[e.source()].key,
                    &self.inner[e.target()].key,
                    e.weight(),
                )
                .map(|edge| (e.id(), edge))
            })
            .collect();
        for (index, edge) in changes {
            self.inner[index] = edge;
        }
    }

    /// All edges in a total, insertion-order-independent order: by `from`, then
    /// `to`, then kind, then provenance, then dissent, then sites. The
    /// provenance/dissent/sites tail is what makes the order *total* — two edges sharing (from, to, kind)
    /// but differing in provenance would otherwise fall back to petgraph's
    /// insertion order, the one thing the module contract says can never reach
    /// output. This is the ONLY order that reaches output. `dedup_edges`
    /// (`merge.rs`) sorts by `NodeIndex` instead — a different key, but that is
    /// harmless: dedup only needs identical edges adjacent, never a stable
    /// cross-run order, which is what this method alone provides.
    pub fn edges_sorted(&self) -> Vec<EdgeView<'_>> {
        let mut edges: Vec<EdgeView<'_>> = self
            .inner
            .edge_references()
            .map(|e| EdgeView {
                from: &self.inner[e.source()].key,
                to: &self.inner[e.target()].key,
                edge: e.weight(),
            })
            .collect();
        edges.sort_by(|a, b| {
            (
                a.from,
                a.to,
                &a.edge.kind,
                &a.edge.provenance,
                &a.edge.dissent,
                &a.edge.sites,
            )
                .cmp(&(
                    b.from,
                    b.to,
                    &b.edge.kind,
                    &b.edge.provenance,
                    &b.edge.dissent,
                    &b.edge.sites,
                ))
        });
        edges
    }
}

/// FNV-1a, 64-bit, as 16 lowercase hex digits.
///
/// A hash here only has to be deterministic across runs and platforms and to
/// spread short identity strings; it is never a security boundary, so a
/// dependency-free 12-line function is the honest choice over pulling in a
/// cryptographic crate. Lives here, in the dependency-free leaf both `card`
/// and `extract` already import, rather than in either of them — `card`
/// uses it for hashing card paths, `extract` for
/// [`crate::extract::cache::RunFingerprint::digest`], and neither module may
/// depend on the other (KTD2's "graph never depends on extract" holds the
/// other direction too: putting this in `card` would have made `extract`
/// depend on `card`).
/// Since U2 this is the hex spelling of [`vocabulary::fnv1a64`], which the kind
/// ids also use. One implementation, not two: two copies of the same digest are
/// the kind of thing that agrees for a year and then quietly stops.
pub(crate) fn fnv1a64_hex(input: &str) -> String {
    format!("{:016x}", vocabulary::fnv1a64(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A namespace-less C# file's types are keyed by its path stem, which
    /// keeps a directory's space — and an enum decision written with such a
    /// key reads back whole.
    #[test]
    fn an_enum_decision_with_a_space_in_its_key_round_trips() {
        for verified in [true, false] {
            for shape in [
                DecisionShape::Label,
                DecisionShape::Arm,
                DecisionShape::Comparison,
            ] {
                let decision = EnumDecision {
                    verified,
                    shape,
                    enum_key: NodeKey::new("sym:csharp:Shared Code/My Enums#Status"),
                    member: "Open".to_string(),
                };
                assert_eq!(EnumDecision::decode(&decision.encode()), Some(decision));
            }
        }
        assert_eq!(EnumDecision::decode("verified label sym:csharp:N#E"), None);
        assert_eq!(
            EnumDecision::decode("maybe label sym:csharp:N#E Open"),
            None
        );
    }

    fn prov() -> Provenance {
        Provenance::new("src/lib.rs", Some(1), "filetree".into(), Tier::Measured)
    }

    /// Holds `core_kinds` to its enum. The matches below are exhaustive, so a
    /// new variant makes this **fail to compile** rather than pass while
    /// quietly omitting itself from the corpus's own account of the core
    /// vocabulary. Adding the variant to the list is what fixes it.
    #[test]
    fn core_kind_lists_are_exhaustive() {
        for k in NodeKind::core_kinds() {
            match k {
                NodeKind::File
                | NodeKind::Module
                | NodeKind::Symbol
                | NodeKind::Dependency
                | NodeKind::External
                | NodeKind::Declared(_) => {}
            }
        }
        for k in EdgeKind::core_kinds() {
            match k {
                EdgeKind::Contains
                | EdgeKind::Declares
                | EdgeKind::HasMember
                | EdgeKind::Extends
                | EdgeKind::Implements
                | EdgeKind::Calls
                | EdgeKind::References
                | EdgeKind::Instantiates
                | EdgeKind::Imports
                | EdgeKind::DependsOn
                | EdgeKind::Uses
                | EdgeKind::Declared(_) => {}
            }
        }
        for k in FactKind::core_kinds() {
            match k {
                FactKind::SymbolName
                | FactKind::SymbolKind
                | FactKind::SymbolContainer
                | FactKind::SymbolSignature
                | FactKind::SymbolVisibility
                | FactKind::DocComment
                | FactKind::EntryPoint
                | FactKind::Loc
                | FactKind::TestByConvention
                | FactKind::Coverage
                | FactKind::DependencyName
                | FactKind::DependencyScope
                | FactKind::EntryPointDecl
                | FactKind::CandidateCount
                | FactKind::AmbiguousReference
                | FactKind::ExternalName
                | FactKind::ExternalKind
                | FactKind::RedactedLiteral
                | FactKind::SymbolMoniker
                | FactKind::ResolvedByIndex
                | FactKind::ResolvedCoverage
                | FactKind::ExtractionFailed
                | FactKind::DbAccess
                | FactKind::Anchor
                | FactKind::AnchorSkipped
                | FactKind::AnchorsDropped
                | FactKind::EnumDecision
                | FactKind::EnumDecisionsUnmatched
                | FactKind::BodyExtractionFailed
                | FactKind::UnresolvedReference
                | FactKind::Declared(_) => {}
            }
        }

        // **Duplicates only.** An earlier version of this comment claimed "none
        // missing", which this cannot see: comparing a list to its own set
        // catches a repeat, never an omission. The pinned lengths below are the
        // half that notices a variant being REMOVED from a list; a variant added
        // to the enum but never added to a list is caught by neither, and is
        // recorded as a known limit on `core_kinds` itself.
        assert_eq!(
            NodeKind::core_kinds().iter().collect::<BTreeSet<_>>().len(),
            NodeKind::core_kinds().len()
        );
        assert_eq!(
            EdgeKind::core_kinds().iter().collect::<BTreeSet<_>>().len(),
            EdgeKind::core_kinds().len()
        );
        // Pinned, so a variant dropped from a list is a failure rather than a
        // quietly smaller core vocabulary.
        assert_eq!(NodeKind::core_kinds().len(), 5);
        assert_eq!(EdgeKind::core_kinds().len(), 11);
        assert_eq!(FactKind::core_kinds().len(), 30);
        assert_eq!(
            FactKind::core_kinds().iter().collect::<BTreeSet<_>>().len(),
            FactKind::core_kinds().len()
        );
    }

    /// Every core node kind maps to a distinct structural role, and every core
    /// edge kind the backbone views read maps to a distinct link role. This is
    /// the table U6 rewrites the views against (KTD6); if two core kinds
    /// collapsed onto one role, the rewrite would silently merge two branches.
    #[test]
    fn core_kinds_map_one_to_one_onto_their_roles() {
        let g = Graph::new();
        let roles: BTreeSet<_> = NodeKind::core_kinds()
            .iter()
            .map(|k| g.node_role(*k))
            .collect();
        assert_eq!(roles.len(), NodeKind::core_kinds().len());
        assert!(!roles.contains(&StructuralRole::Opaque));

        assert_eq!(g.edge_role(EdgeKind::Contains), LinkRole::Containment);
        assert_eq!(g.edge_role(EdgeKind::Declares), LinkRole::Declaration);
        assert_eq!(g.edge_role(EdgeKind::DependsOn), LinkRole::Dependency);
        for k in [
            EdgeKind::HasMember,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::Calls,
            EdgeKind::Imports,
        ] {
            assert_eq!(
                g.edge_role(k),
                LinkRole::Opaque,
                "{k:?} carries card or smell policy, not the shape of the tree"
            );
        }
    }

    /// R2, structurally: the core's own account of itself does not change when
    /// a corpus declares kinds. The behavioural half of the gate lives in
    /// `tests/vocabulary_declared.rs`.
    #[test]
    fn declaring_kinds_does_not_change_the_core_vocabulary() {
        let before = NodeKind::core_kinds().to_vec();
        let mut g = Graph::new();
        g.declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::None)
            .unwrap();
        g.declare_fact_kind("open", "uroven", Cardinality::Scalar)
            .unwrap();
        assert_eq!(NodeKind::core_kinds(), before.as_slice());
        assert_eq!(g.vocabulary().len(), 2);
    }

    #[test]
    fn narrated_tier_ranks_below_measured_and_derived() {
        // Ord is derived; declaration order defines rank. Narrated must sort last so
        // it can never win a trust comparison against a measured/derived fact (T1-KTD9).
        assert!(Tier::Narrated > Tier::Derived);
        assert!(Tier::Narrated > Tier::Measured);
    }

    #[test]
    fn narrated_tier_serializes_snake_case() {
        let json = serde_json::to_string(&Tier::Narrated).unwrap();
        assert_eq!(json, "\"narrated\"");
    }

    /// The precision axis is a sibling of `Tier`, not a refinement of it: the
    /// wire vocabulary must be snake_case like every other graph enum.
    #[test]
    fn resolution_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&Resolution::Heuristic).unwrap(),
            "\"heuristic\""
        );
        assert_eq!(
            serde_json::to_string(&Resolution::Resolved).unwrap(),
            "\"resolved\""
        );
    }

    /// KD2/R20: the default is the claim the data can always support. A silently
    /// `Resolved` default would license "used nowhere" conclusions outside any
    /// indexed scope.
    #[test]
    fn provenance_defaults_to_heuristic_and_no_range() {
        let p = Provenance::new("src/lib.rs", Some(7), "filetree".into(), Tier::Measured);
        assert_eq!(p.resolution, Resolution::Heuristic);
        assert_eq!(p.span, Span::Line(7));
        assert_eq!(p.span.range_end(), None);
        assert_eq!(p.resolved().resolution, Resolution::Resolved);
    }

    /// A range keeps its start reachable, so a consumer that only understands a
    /// single anchor line still reads the right one.
    #[test]
    fn a_line_range_carries_both_ends_and_omits_an_absent_end() {
        let ranged = Provenance::new("src/lib.rs", None, "treesitter".into(), Tier::Measured)
            .with_range(10, 42);
        assert_eq!(ranged.span, Span::Range { start: 10, end: 42 });
        assert_eq!(
            (ranged.span.start(), ranged.span.end()),
            (Some(10), Some(42))
        );

        // A point has no range end to report. What the WIRE does with that —
        // omit `end_line` rather than write `null` — is asserted against
        // `ManifestProvenance` in tests/manifest.rs, which is the form a
        // consumer actually reads.
        let point = Provenance::new("src/lib.rs", Some(10), "treesitter".into(), Tier::Measured);
        assert_eq!(point.span, Span::Line(10));
        assert_eq!(point.span.range_end(), None);
        assert_eq!(point.span.end(), Some(10), "a line ends where it starts");
    }

    #[test]
    fn a_span_cannot_be_written_down_in_a_state_that_means_nothing() {
        // Each of these was representable in the `Option` pair this replaced,
        // and each was silently degraded when it came time to render.
        assert_eq!(Span::at(None), Span::Whole);
        assert_eq!(Span::at(Some(0)), Span::Whole, "there is no line zero");
        assert_eq!(
            Span::range(42, 10),
            Span::Range { start: 10, end: 42 },
            "a backwards range is the range it meant, not a discarded end"
        );
        assert_eq!(
            Span::range(7, 7),
            Span::Line(7),
            "one place has one spelling, or two provenances the reader cannot \
             tell apart would compare unequal and break a merge tie (KTD7)"
        );
        assert_eq!(Span::range(0, 0), Span::Whole);
    }

    #[test]
    fn spans_order_from_least_specific_to_most() {
        // Provenance ordering breaks equal-rank merge ties, so the order has to
        // be total and stable, not merely present.
        let mut spans = vec![
            Span::Range { start: 1, end: 9 },
            Span::Line(4),
            Span::Whole,
            Span::Line(2),
            Span::Range { start: 1, end: 3 },
        ];
        spans.sort();
        assert_eq!(
            spans,
            vec![
                Span::Whole,
                Span::Line(2),
                Span::Line(4),
                Span::Range { start: 1, end: 3 },
                Span::Range { start: 1, end: 9 },
            ]
        );
    }

    /// KTD1 crux: precision is its own axis, so two assertions that agree on
    /// value, tier and location but disagree on *how they were determined* are two
    /// distinct claims. Merge must keep both — collapsing them would let a
    /// heuristic guess masquerade as index-resolved (or erase the resolved one).
    #[test]
    fn facts_differing_only_in_resolution_survive_merge_as_two() {
        let mut g = Graph::new();
        let key = NodeKey::new("src/lib.rs");
        g.ensure_node(key.clone(), NodeKind::File).unwrap();
        // Author is Additive: both claims must stay whole facts, not dissent.
        let value = FactValue::Text("Alice".into());
        g.add_fact(
            &key,
            Fact::new(FactKind::DependencyScope, value.clone(), prov().resolved()),
        );
        g.add_fact(&key, Fact::new(FactKind::DependencyScope, value, prov()));

        merge(&mut g, &Ranks::default());

        let resolutions: Vec<Resolution> = g
            .node(&key)
            .unwrap()
            .facts
            .iter()
            .map(|f| f.provenance.resolution)
            .collect();
        assert_eq!(
            resolutions,
            [Resolution::Heuristic, Resolution::Resolved],
            "the precision axis must not collapse two claims into one"
        );
    }

    /// The edge side, as KTD6 (U4) redefines it. Until U4 an edge's identity
    /// INCLUDED its provenance, so a name heuristic and an index-resolved source
    /// asserting one relation stayed two edges and every consumer had to reconcile
    /// them. Now the relation identity is `(from, to, kind)`: one edge, at the
    /// resolved precision, with the heuristic assertion preserved as dissent on that
    /// same edge — nothing deleted (E7), nothing shadowing the resolved claim.
    #[test]
    fn a_relation_asserted_twice_becomes_one_edge_at_the_higher_precision() {
        let mut g = Graph::new();
        let a = NodeKey::new("src/a.rs");
        let b = NodeKey::new("src/b.rs");
        g.ensure_node(a.clone(), NodeKind::File).unwrap();
        g.ensure_node(b.clone(), NodeKind::File).unwrap();
        g.add_edge(&a, &b, Edge::new(EdgeKind::Imports, prov().resolved()));
        g.add_edge(&a, &b, Edge::new(EdgeKind::Imports, prov()));
        // A genuine duplicate of the heuristic edge, to prove dedup still runs.
        g.add_edge(&a, &b, Edge::new(EdgeKind::Imports, prov()));

        merge(&mut g, &Ranks::default());

        let edges = g.edges_sorted();
        assert_eq!(edges.len(), 1, "one relation, one edge");
        assert_eq!(edges[0].edge.provenance.resolution, Resolution::Resolved);
        // The losing heuristic assertion is retrievable, not discarded — and the
        // identical duplicate of it collapsed rather than dissenting twice.
        assert_eq!(edges[0].edge.dissent.len(), 1);
        assert_eq!(
            edges[0].edge.dissent[0].provenance.resolution,
            Resolution::Heuristic
        );
    }

    /// Different relations between one pair are different edges: `Declares` and
    /// `References` from a file to one symbol are two statements, not a conflict.
    #[test]
    fn different_edge_kinds_between_one_pair_stay_separate() {
        let mut g = Graph::new();
        let a = NodeKey::new("src/a.rs");
        let b = NodeKey::new("sym:rust:crate::a#T");
        g.ensure_node(a.clone(), NodeKind::File).unwrap();
        g.ensure_node(b.clone(), NodeKind::Symbol).unwrap();
        g.add_edge(&a, &b, Edge::new(EdgeKind::Declares, prov()));
        g.add_edge(&a, &b, Edge::new(EdgeKind::References, prov()));

        merge(&mut g, &Ranks::default());

        let kinds: Vec<EdgeKind> = g.edges_sorted().iter().map(|e| e.edge.kind).collect();
        assert_eq!(kinds, [EdgeKind::Declares, EdgeKind::References]);
    }

    /// Determinism with the new axis in play: the two resolution-differing facts
    /// must land in the same order regardless of which was inserted first. The
    /// existing insertion-order test covers nodes and edges but not fact order.
    #[test]
    fn fact_order_is_stable_across_insertion_orders() {
        let build = |resolved_first: bool| {
            let mut g = Graph::new();
            let key = NodeKey::new("src/lib.rs");
            g.ensure_node(key.clone(), NodeKind::File).unwrap();
            let value = FactValue::Text("Alice".into());
            let mut provs = vec![prov().resolved(), prov()];
            if !resolved_first {
                provs.reverse();
            }
            for p in provs {
                g.add_fact(&key, Fact::new(FactKind::DependencyScope, value.clone(), p));
            }
            merge(&mut g, &Ranks::default());
            serde_json::to_string(&g.node(&key).unwrap().facts).unwrap()
        };
        assert_eq!(build(true), build(false));
    }

    #[test]
    fn node_edge_and_fact_round_trip() {
        let mut g = Graph::new();
        let a = NodeKey::new("src");
        let b = NodeKey::new("src/lib.rs");
        g.ensure_node(a.clone(), NodeKind::Module).unwrap();
        g.ensure_node(b.clone(), NodeKind::File).unwrap();
        assert!(g.add_fact(&b, Fact::new(FactKind::Loc, FactValue::Int(3), prov())));
        assert!(g.add_edge(&a, &b, Edge::new(EdgeKind::Contains, prov())));

        let node = g.node(&b).unwrap();
        assert_eq!(node.kind, NodeKind::File);
        assert_eq!(node.facts.len(), 1);
        assert_eq!(node.facts[0].value, FactValue::Int(3));

        let edges = g.edges_sorted();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, &a);
        assert_eq!(edges[0].to, &b);
        assert_eq!(edges[0].edge.kind, EdgeKind::Contains);
        assert_eq!(edges[0].edge.provenance.extractor, "filetree".into());
    }

    #[test]
    fn same_key_is_deduplicated_into_one_node() {
        let mut g = Graph::new();
        let key = NodeKey::new("src/lib.rs");
        let first = g.ensure_node(key.clone(), NodeKind::File).unwrap();
        let second = g.ensure_node(key.clone(), NodeKind::File).unwrap();
        assert_eq!(first, second);
        assert_eq!(g.node_count(), 1);
    }

    /// The Phase-B guard: reusing a key with a different kind is a key-space
    /// bug and is a hard error in *every* build (not debug-only), so it can
    /// never silently attach facts to the wrong-kind node in release.
    #[test]
    fn same_key_different_kind_is_rejected() {
        let mut g = Graph::new();
        let key = NodeKey::new("src");
        g.ensure_node(key.clone(), NodeKind::Module).unwrap();
        let err = g.ensure_node(key, NodeKind::File).unwrap_err();
        assert!(matches!(
            err,
            GraphError::KindCollision {
                existing: NodeKind::Module,
                got: NodeKind::File,
                ..
            }
        ));
    }

    #[test]
    fn backslash_paths_normalize_to_forward_slash() {
        let key = NodeKey::new("src\\extract\\git.rs");
        assert_eq!(key.as_str(), "src/extract/git.rs");
    }

    #[test]
    fn edge_and_fact_require_existing_nodes() {
        let mut g = Graph::new();
        let missing = NodeKey::new("nope");
        let present = NodeKey::new("here");
        g.ensure_node(present.clone(), NodeKind::File).unwrap();

        assert!(!g.add_fact(
            &missing,
            Fact::new(
                FactKind::DependencyScope,
                FactValue::Text("x".into()),
                prov()
            )
        ));
        assert!(!g.add_edge(&missing, &present, Edge::new(EdgeKind::Contains, prov())));
        assert!(!g.add_edge(&present, &missing, Edge::new(EdgeKind::Contains, prov())));
        assert_eq!(g.edge_count(), 0);
    }

    /// Determinism: building the same graph with different *insertion* orders
    /// yields byte-identical serialized output (KTD7).
    #[test]
    fn serialization_order_is_stable_across_insertion_orders() {
        let keys = ["c", "a", "b"];
        let build = |order: &[&str]| {
            let mut g = Graph::new();
            for k in order {
                g.ensure_node(NodeKey::new(*k), NodeKind::File).unwrap();
            }
            // add edges in yet another order
            g.add_edge(
                &NodeKey::new("a"),
                &NodeKey::new("b"),
                Edge::new(EdgeKind::Imports, prov()),
            );
            g.add_edge(
                &NodeKey::new("c"),
                &NodeKey::new("a"),
                Edge::new(EdgeKind::Imports, prov()),
            );
            g
        };
        let _ = keys;
        let g1 = build(&["c", "a", "b"]);
        let g2 = build(&["b", "c", "a"]);

        let nodes1 = serde_json::to_string(&g1.nodes_sorted()).unwrap();
        let nodes2 = serde_json::to_string(&g2.nodes_sorted()).unwrap();
        assert_eq!(nodes1, nodes2);

        let edges1: Vec<_> = g1
            .edges_sorted()
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.edge.kind))
            .collect();
        let edges2: Vec<_> = g2
            .edges_sorted()
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.edge.kind))
            .collect();
        assert_eq!(edges1, edges2);
        assert_eq!(edges1[0].0.as_str(), "a"); // sorted: (a,b) before (c,a)
    }

    /// U1: the graph IR was serialize-only (see `manifest.rs`'s module doc);
    /// this is the round trip that removes that limitation for the tier/
    /// resolution/span axes and a multi-line `Span::Range`.
    #[test]
    fn fact_round_trips_through_json_with_a_range_span() {
        let provenance = Provenance::new("src/lib.rs", None, "treesitter".into(), Tier::Measured)
            .with_range(10, 42);
        assert_eq!(provenance.resolution, Resolution::Heuristic);
        assert_eq!(provenance.span, Span::Range { start: 10, end: 42 });
        let fact = Fact::new(FactKind::Loc, FactValue::Int(7), provenance);

        let json = serde_json::to_string(&fact).unwrap();
        let round_tripped: Fact = serde_json::from_str(&json).unwrap();
        assert_eq!(fact, round_tripped);
    }

    /// The `Resolution::Resolved` sibling of the above: precision is its own
    /// axis (KTD1), so a resolved fact must round-trip exactly as faithfully
    /// as the heuristic default.
    #[test]
    fn fact_built_via_provenance_resolved_round_trips_through_json() {
        let fact = Fact::new(
            FactKind::SymbolMoniker,
            FactValue::Text("scip-rust go to::Cfg#".into()),
            prov().resolved(),
        );
        assert_eq!(fact.provenance.resolution, Resolution::Resolved);

        let json = serde_json::to_string(&fact).unwrap();
        let round_tripped: Fact = serde_json::from_str(&json).unwrap();
        assert_eq!(fact, round_tripped);
        assert_eq!(round_tripped.provenance.resolution, Resolution::Resolved);
    }

    // ---- the R34 source anchor (U15) ----

    fn anchor_case(file: &str, extractor: &str) -> Provenance {
        Provenance::new(file, None, extractor.into(), Tier::Measured)
    }

    fn anchor(entry: &str) -> SourceAnchor {
        SourceAnchor {
            container: "vendor/lib.zip".to_string(),
            entry: entry.to_string(),
            line: None,
        }
    }

    /// `Provenance`'s `Ord` IS emission order — `merge_facts` emits from an
    /// ordered set, so every corpus byte's position depends on it. `anchor` is
    /// the last field precisely so it can only ever break a tie the existing
    /// fields already failed to break. If it were placed earlier, a populated
    /// anchor could reorder facts that today sort by extractor, and the corpus
    /// would move for a reason no reader could see.
    #[test]
    fn an_anchor_never_outranks_a_field_declared_before_it() {
        // `a` carries an anchor that sorts high; `b` carries none. They differ
        // in `extractor`, which is declared earlier, so `extractor` must decide.
        let mut a = anchor_case("src/lib.rs", "aaa");
        a.anchor = Some(anchor("zzz"));
        let b = anchor_case("src/lib.rs", "bbb");

        assert!(a < b, "extractor must decide, not the anchor");
    }

    /// It does still participate — otherwise two facts identical but for where
    /// inside a container they came from would collapse into one in the ordered
    /// set `merge_facts` emits from.
    #[test]
    fn two_provenances_alike_but_for_their_anchor_are_ordered_by_it() {
        let mut first = anchor_case("vendor/lib.zip", "filetree");
        first.anchor = Some(anchor("a.rs"));
        let mut second = anchor_case("vendor/lib.zip", "filetree");
        second.anchor = Some(anchor("b.rs"));

        assert!(first < second);
        assert_ne!(first, second);
    }

    /// An unanchored provenance sorts before an anchored one with otherwise
    /// equal fields (`None < Some`), which is the stable answer: today every
    /// value is `None`, so nothing an existing corpus contains can be displaced
    /// by a value that does not exist yet.
    #[test]
    fn an_unanchored_provenance_sorts_before_an_anchored_one() {
        let plain = anchor_case("vendor/lib.zip", "filetree");
        let mut anchored = anchor_case("vendor/lib.zip", "filetree");
        anchored.anchor = Some(anchor("a.rs"));

        assert!(plain < anchored);
    }

    /// Plan A, U3 (KTD7): two calls of one name on one line —
    /// `sb.Append(a).Append(b)` — are two sites once a plugin reports their
    /// columns, each with its own id, kept in source order. Without columns
    /// (plugin contract 0.1) the same two calls are one site: the limitation
    /// `SiteId` documents, not a merge of two places that were told apart.
    #[test]
    fn two_calls_on_one_line_stay_two_sites_once_columns_tell_them_apart() {
        let at = |column: Option<u32>| SiteId {
            file: "src/Report.cs".into(),
            line: 12,
            column,
        };
        let edge = Edge::new(EdgeKind::Calls, prov()).at_sites([at(Some(20)), at(Some(8))]);
        assert_eq!(edge.sites, [at(Some(8)), at(Some(20))]);

        let edge = Edge::new(EdgeKind::Calls, prov()).at_sites([at(None), at(None)]);
        assert_eq!(edge.sites, [at(None)]);
    }

    /// A site id without a column writes no `column` key, so the day a plugin
    /// reports one it fills a field the format already had.
    #[test]
    fn a_site_without_a_column_writes_no_column_key() {
        let site = SiteId {
            file: "a.cs".into(),
            line: 3,
            column: None,
        };
        assert_eq!(
            serde_json::to_string(&site).unwrap(),
            r#"{"file":"a.cs","line":3}"#
        );
    }
}
