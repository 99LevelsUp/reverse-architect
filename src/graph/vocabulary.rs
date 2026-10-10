//! The per-corpus kind vocabulary (U2).
//!
//! The four kind enums are closed, and that closure is why a corpus can be
//! trusted: a consumer knows what may arrive. This module does not remove the
//! closure — it **moves it down a level**, from "one fixed set for every corpus"
//! to "one fixed set per corpus, written into its manifest". The core still
//! enumerates its *own* vocabulary without reading any plugin (R2).
//!
//! # What a declaration may and may not say
//!
//! A declaration carries only what the core needs in order to *act*, never what
//! the kind *means*:
//!
//! - a fact kind declares its [`Cardinality`], because `merge` cannot otherwise
//!   choose between a union and a single winner (R7);
//! - a node kind declares its card policy (R8) and its [`StructuralRole`] (R9);
//! - an edge kind declares its [`LinkRole`] (R9).
//!
//! Nothing here lets the core ask what a declared kind is *about*. That is
//! domain knowledge and it stays outside.
//!
//! # Why four id types instead of one
//!
//! A single `KindId` checked only for existence would let a fact-kind
//! declaration be passed where a node kind is expected — the ids are just
//! integers, so nothing would catch it (R13). With four types the compiler does,
//! and the `declare_*` functions get their return types for free.
//!
//! # Why the digest is named and pinned
//!
//! Ids are FNV-1a 64 over `namespace:name`, implemented here rather than taken
//! from `DefaultHasher`, whose output is documented as unspecified across Rust
//! releases. Since all four enums derive `Ord`, anything that sorts by kind
//! sorts by id — so a hasher that changed between toolchains would silently
//! reorder a corpus, and neither the byte-identity gate nor the ordering gate
//! would see it coming. The digest's name is part of the corpus format and is
//! written into the manifest (R14).

use std::collections::BTreeMap;

use super::merge::Cardinality;

/// FNV-1a 64-bit. Pinned by [`self::tests`] against literal values: this is a
/// wire format, not an implementation detail (R14).
///
/// Named in the manifest under this exact string so a reader of a corpus can
/// tell which function produced its ids.
pub const DIGEST_NAME: &str = "fnv1a64";

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a, 64-bit. Also the digest behind [`super::fnv1a64_hex`], which card
/// paths and the run fingerprint use — one implementation for both, so a change
/// here cannot make two parts of the corpus disagree about the same string.
pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The full name of a declared kind, as it appears on disk and in the manifest.
fn qualified(namespace: &str, name: &str) -> String {
    format!("{namespace}:{name}")
}

macro_rules! kind_id {
    ($id:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// Opaque by design: the only way to obtain one is a declaration on the
        /// graph that will carry it, so an id can never outlive the vocabulary
        /// that explains it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $id(u64);

        impl $id {
            fn of(namespace: &str, name: &str) -> Self {
                $id(fnv1a64(&qualified(namespace, name)))
            }

            /// The raw digest. For serialization of the *manifest*, where the id
            /// appears beside its name; never for the corpus body, which carries
            /// names only (KTD7).
            #[must_use]
            pub fn get(self) -> u64 {
                self.0
            }
        }
    };
}

kind_id!(NodeKindId, "Identity of a declared node kind.");
kind_id!(EdgeKindId, "Identity of a declared edge kind.");
kind_id!(FactKindId, "Identity of a declared fact kind.");
kind_id!(CardKindId, "Identity of a declared card kind.");

/// The part a node plays in the backbone views — the only thing those views are
/// allowed to branch on (R9, KTD6).
///
/// The set is deliberately no wider than what the views actually distinguish
/// today. It is not a second copy of `NodeKind`: `HasMember`, `Extends` and
/// friends have no role because they drive card and smell policy, not the shape
/// of the tree. Widening this enum to cover them would re-derive the kind enum
/// under a new name and buy nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StructuralRole {
    /// Forms the tree and takes part in the parent-closure check. Inherits the
    /// `NodeKey` precondition: keys are `/`-hierarchical and every intermediate
    /// level has its own node (R16).
    Container,
    /// Hangs beneath a container and is counted in the overview.
    Leaf,
    /// A declaration inside a leaf.
    Definition,
    /// A unit of distribution in the dependency view.
    Package,
    /// Referenced but not contained.
    External,
    /// Takes no part in the backbone views. The default for a declared kind:
    /// a plugin that wants its nodes in the tree has to say so.
    Opaque,
}

/// The part an edge plays in the backbone views (R9, KTD6). Same narrowness
/// rule as [`StructuralRole`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkRole {
    /// Parent to child in the module tree.
    Containment,
    /// A leaf to a definition it declares.
    Declaration,
    /// A dependency on a package.
    Dependency,
    /// Takes no part in the backbone views. The default.
    Opaque,
}

/// Whether a node of a declared kind earns a card, and under which card kind.
///
/// Deliberately independent of [`StructuralRole`]: "this is a definition" and
/// "this gets a card" are different questions, and coupling them would make one
/// of the two unsayable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPolicy {
    /// No card. The default.
    None,
    /// A card rendered under the given declared card kind.
    Card(CardKindId),
}

/// What a declaration says about a node kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeDeclaration {
    pub namespace: String,
    pub name: String,
    pub role: StructuralRole,
    pub card: CardPolicy,
}

/// What a declaration says about an edge kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeDeclaration {
    pub namespace: String,
    pub name: String,
    pub role: LinkRole,
}

/// What a declaration says about a fact kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactDeclaration {
    pub namespace: String,
    pub name: String,
    pub cardinality: Cardinality,
}

/// What a declaration says about a card kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDeclaration {
    pub namespace: String,
    pub name: String,
}

/// Why a declaration was refused. Every case is a named error rather than a
/// silent overwrite (R11): a vocabulary that quietly redefined a kind would make
/// the manifest disagree with the corpus it describes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VocabularyError {
    #[error("{category} kind {name:?} redeclared with different properties")]
    Conflict {
        category: &'static str,
        name: String,
    },
    #[error("{category} kind {name:?} collides with already-declared {existing:?} under {digest}")]
    DigestCollision {
        category: &'static str,
        name: String,
        existing: String,
        digest: &'static str,
    },
    /// A name that would not survive being written down.
    ///
    /// Two conditions, one error. `:` is the separator the wire name is built
    /// from, so a half carrying one could not be split back. A control character
    /// or a backtick is refused for a different reason: the name goes verbatim
    /// into a Markdown heading, into a card's lead sentence and into a directory
    /// name, and one of those sinks would come out meaning something the
    /// declaration never said (RG3). See `forbidden`.
    #[error(
        "namespace and name must both be non-empty, free of leading and trailing space, and free of ':', control characters, format characters and '`' (got {namespace:?}:{name:?})"
    )]
    MalformedName { namespace: String, name: String },
    /// A node declaration named a card kind this vocabulary does not hold.
    ///
    /// The ids are `Copy` integers, so one minted by a different `Graph` is
    /// accepted by the type system; without this check it would reach the card
    /// projection and write `undeclared:<hex>` into a card path and into the
    /// card's own opening sentence. Found by review: the three gates on
    /// `ensure_node`/`add_fact`/`add_edge` covered every category except this
    /// one, which is reachable only through a declaration.
    #[error("node kind {name:?} names card kind {card:?}, which this vocabulary does not hold")]
    UndeclaredCardPolicy { name: String, card: u64 },
    /// A declaration claimed a namespace the core reserves for itself: the
    /// unresolvable-id marker ([`RESERVED_NAMESPACE`]) or the card-path fallback
    /// tree ([`RESERVED_HASHED_NAMESPACE`]).
    #[error(
        "namespace {namespace:?} is reserved for the core's unresolvable-id marker (got {namespace:?}:{name:?})"
    )]
    ReservedNamespace { namespace: String, name: String },
}

/// Every declared kind of one corpus, and the properties the core reads.
///
/// Held inside the `Graph` (KTD4), so anything holding a `&Graph` can resolve a
/// declared kind without a signature change. The two places that cannot —
/// `merge_facts`, which runs inside a mutable node iteration, and the card
/// renderer, which never receives a graph — take it explicitly instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    nodes: BTreeMap<NodeKindId, NodeDeclaration>,
    edges: BTreeMap<EdgeKindId, EdgeDeclaration>,
    facts: BTreeMap<FactKindId, FactDeclaration>,
    cards: BTreeMap<CardKindId, CardDeclaration>,
}

/// Generates the four declare/lookup pairs, which differ only in their id type,
/// their declaration type and the map they live in.
macro_rules! category {
    ($declare:ident, $lookup:ident, $iter:ident, $map:ident, $id:ident, $decl:ident, $label:literal) => {
        impl Vocabulary {
            /// Declare a kind, or return its existing id when the declaration is
            /// identical. A second declaration under the same name with
            /// different properties is an error, never an overwrite (R11).
            pub fn $declare(&mut self, decl: $decl) -> Result<$id, VocabularyError> {
                check_name(&decl.namespace, &decl.name)?;
                let id = $id::of(&decl.namespace, &decl.name);
                match self.$map.get(&id) {
                    Some(existing) if *existing == decl => Ok(id),
                    Some(existing)
                        if existing.namespace == decl.namespace && existing.name == decl.name =>
                    {
                        Err(VocabularyError::Conflict {
                            category: $label,
                            name: qualified(&decl.namespace, &decl.name),
                        })
                    }
                    // Same digest, different name: two distinct kinds would share
                    // an identity. Astronomically unlikely and fatal if ignored,
                    // so it is named rather than assumed away.
                    Some(existing) => Err(VocabularyError::DigestCollision {
                        category: $label,
                        name: qualified(&decl.namespace, &decl.name),
                        existing: qualified(&existing.namespace, &existing.name),
                        digest: DIGEST_NAME,
                    }),
                    None => {
                        self.$map.insert(id, decl);
                        Ok(id)
                    }
                }
            }

            /// The declaration behind an id, or `None` when this corpus never
            /// declared it.
            #[must_use]
            pub fn $lookup(&self, id: $id) -> Option<&$decl> {
                self.$map.get(&id)
            }

            /// Every declaration of this category, ordered by id.
            ///
            /// Ordered by id rather than by name because that is the order the
            /// enums themselves sort in; anything writing for a human sorts by
            /// name at the point of writing (U4).
            pub fn $iter(&self) -> impl Iterator<Item = (&$id, &$decl)> {
                self.$map.iter()
            }
        }
    };
}

category!(
    declare_node,
    node,
    node_declarations,
    nodes,
    NodeKindId,
    NodeDeclaration,
    "node"
);
category!(
    declare_edge,
    edge,
    edge_declarations,
    edges,
    EdgeKindId,
    EdgeDeclaration,
    "edge"
);
category!(
    declare_fact,
    fact,
    fact_declarations,
    facts,
    FactKindId,
    FactDeclaration,
    "fact"
);
category!(
    declare_card,
    card,
    card_declarations,
    cards,
    CardKindId,
    CardDeclaration,
    "card"
);

impl Vocabulary {
    /// The wire name of a declared node kind: `namespace:name` (KTD7).
    #[must_use]
    pub fn node_name(&self, id: NodeKindId) -> Option<String> {
        self.node(id).map(|d| qualified(&d.namespace, &d.name))
    }

    /// The wire name of a declared edge kind.
    #[must_use]
    pub fn edge_name(&self, id: EdgeKindId) -> Option<String> {
        self.edge(id).map(|d| qualified(&d.namespace, &d.name))
    }

    /// The wire name of a declared fact kind.
    #[must_use]
    pub fn fact_name(&self, id: FactKindId) -> Option<String> {
        self.fact(id).map(|d| qualified(&d.namespace, &d.name))
    }

    /// The wire name of a declared card kind.
    #[must_use]
    pub fn card_name(&self, id: CardKindId) -> Option<String> {
        self.card(id).map(|d| qualified(&d.namespace, &d.name))
    }

    /// Whether this corpus declared anything at all.
    ///
    /// The manifest's schema version rises only for a corpus that carries a
    /// vocabulary section (R12), so this is the question it asks — and it is why
    /// a corpus with no declarations stays byte-identical to today's (R4).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.edges.is_empty()
            && self.facts.is_empty()
            && self.cards.is_empty()
    }

    /// How many kinds this corpus declared, across all four categories.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len() + self.edges.len() + self.facts.len() + self.cards.len()
    }
}

/// The name a projection writes when a declared id has no declaration in this
/// corpus.
///
/// Four gates keep a well-formed graph away from it: `ensure_node`, `add_fact`
/// and `add_edge` refuse an id this graph never declared, and `declare_node_kind`
/// refuses a [`CardPolicy::Card`] naming an undeclared card kind. Review found
/// the fourth gate missing — the first three covered nodes, facts and edges, and
/// a card id reaches the projection only through a declaration, so it had no
/// door to be refused at.
///
/// It still exists, and is still not a panic: `card_path` is public and takes a
/// `CardKind` a caller can build from any id, so the projection has to be total.
/// Panicking there would turn a data problem into a crash in the one place a
/// reader most needs output, and a silent empty string would be
/// indistinguishable from a real name. The digest is carried so a broken corpus
/// can still be traced back.
#[must_use]
pub fn undeclared_name(id: u64) -> String {
    format!("{RESERVED_NAMESPACE}:{id:016x}")
}

/// The one wire spelling of an edge kind, for every consumer.
///
/// Stable, English, snake_case. Lives here rather than in `card` or `manifest`
/// because both of them must produce the same word — a card that says
/// `has_member` while the export says `hasMember` describes a relation nobody
/// can join. Review found those two modules holding independently written
/// copies of this table; one canonical function is what stops them drifting.
#[must_use]
pub fn edge_word(kind: super::EdgeKind, vocabulary: &Vocabulary) -> std::borrow::Cow<'static, str> {
    use super::EdgeKind as E;
    match kind {
        E::Contains => "contains".into(),
        E::Declares => "declares".into(),
        E::HasMember => "has_member".into(),
        E::Extends => "extends".into(),
        E::Implements => "implements".into(),
        E::Calls => "calls".into(),
        E::References => "references".into(),
        E::Instantiates => "instantiates".into(),
        E::Imports => "imports".into(),
        E::DependsOn => "depends_on".into(),
        E::Uses => "uses".into(),
        E::Declared(id) => std::borrow::Cow::Owned(
            vocabulary
                .edge_name(id)
                .unwrap_or_else(|| undeclared_name(id.get())),
        ),
    }
}

/// The one wire spelling of a node kind. See [`edge_word`].
#[must_use]
pub fn node_word(kind: super::NodeKind, vocabulary: &Vocabulary) -> std::borrow::Cow<'static, str> {
    use super::NodeKind as N;
    match kind {
        N::File => "file".into(),
        N::Module => "module".into(),
        N::Symbol => "symbol".into(),
        N::Dependency => "dependency".into(),
        N::External => "external".into(),
        N::Declared(id) => std::borrow::Cow::Owned(
            vocabulary
                .node_name(id)
                .unwrap_or_else(|| undeclared_name(id.get())),
        ),
    }
}

/// The one wire spelling of a fact kind. See [`edge_word`].
///
/// Lived in `manifest` until U7 needed the same table on the cache path. Two
/// copies of a wire vocabulary is the drift `edge_word` already exists to
/// prevent, so the table moved here and `manifest` calls it.
#[must_use]
pub fn fact_word(kind: super::FactKind, vocabulary: &Vocabulary) -> std::borrow::Cow<'static, str> {
    use super::FactKind as F;
    match kind {
        F::SymbolName => "symbol_name".into(),
        F::SymbolKind => "symbol_kind".into(),
        F::SymbolContainer => "symbol_container".into(),
        F::SymbolSignature => "symbol_signature".into(),
        F::SymbolVisibility => "symbol_visibility".into(),
        F::DocComment => "doc_comment".into(),
        F::EntryPoint => "entry_point".into(),
        F::Loc => "loc".into(),
        F::TestByConvention => "test_by_convention".into(),
        F::Coverage => "coverage".into(),
        F::DependencyName => "dependency_name".into(),
        F::DependencyScope => "dependency_scope".into(),
        F::EntryPointDecl => "entry_point_decl".into(),
        F::CandidateCount => "candidate_count".into(),
        F::AmbiguousReference => "ambiguous_reference".into(),
        F::ExternalName => "external_name".into(),
        F::ExternalKind => "external_kind".into(),
        F::RedactedLiteral => "redacted_literal".into(),
        F::SymbolMoniker => "symbol_moniker".into(),
        F::ResolvedByIndex => "resolved_by_index".into(),
        F::ResolvedCoverage => "resolved_coverage".into(),
        F::ExtractionFailed => "extraction_failed".into(),
        F::DbAccess => "db_access".into(),
        F::Anchor => "anchor".into(),
        F::AnchorSkipped => "anchor_skipped".into(),
        F::AnchorsDropped => "anchors_dropped".into(),
        F::EnumDecision => "enum_decision".into(),
        F::EnumDecisionsUnmatched => "enum_decisions_unmatched".into(),
        F::BodyExtractionFailed => "body_extraction_failed".into(),
        F::Declared(id) => std::borrow::Cow::Owned(
            vocabulary
                .fact_name(id)
                .unwrap_or_else(|| undeclared_name(id.get())),
        ),
    }
}

/// Read a fact kind back from the word [`fact_word`] wrote (U7, R6).
///
/// `None` means "this corpus cannot name that kind" — not a panic and not a
/// silent fallback, because the one caller is the cache's wire decoder and a
/// record it cannot read degrades to a re-parse, exactly as a record naming an
/// unknown extractor already does.
///
/// The two halves are told apart by the separator alone, and they cannot
/// collide: every core word is a bare snake_case identifier, and
/// [`Vocabulary::declare_fact`] refuses a namespace or name that is empty or
/// contains `:`. So a word with a colon is a declared name and a word without
/// one is a core kind, with no third case.
///
/// A declared name is hashed and then **looked up**. Hashing alone would accept
/// any well-formed string and mint an id this corpus never declared — the
/// `undeclared:<hex>` leak, arriving through the back door.
#[must_use]
pub fn fact_kind_from_word(word: &str, vocabulary: &Vocabulary) -> Option<super::FactKind> {
    use super::FactKind as F;
    if word.contains(':') {
        let id = FactKindId(fnv1a64(word));
        // Existence is not enough — the NAME has to match back. Without this a
        // record written under a name that happens to collide with a different
        // currently-declared name would decode to the wrong kind, consistently
        // and silently: the one "degrades to a wrong answer" this design forbids.
        // `RunFingerprint` already refuses hash-only equality for the same
        // reason; this is the same repository answering the same question twice.
        // The odds are 2^-64 and the check is one string comparison.
        return vocabulary
            .fact(id)
            .filter(|d| qualified(&d.namespace, &d.name) == word)
            .map(|_| F::Declared(id));
    }
    Some(match word {
        "symbol_name" => F::SymbolName,
        "symbol_kind" => F::SymbolKind,
        "symbol_container" => F::SymbolContainer,
        "symbol_signature" => F::SymbolSignature,
        "symbol_visibility" => F::SymbolVisibility,
        "doc_comment" => F::DocComment,
        "entry_point" => F::EntryPoint,
        "loc" => F::Loc,
        "test_by_convention" => F::TestByConvention,
        "coverage" => F::Coverage,
        "dependency_name" => F::DependencyName,
        "dependency_scope" => F::DependencyScope,
        "entry_point_decl" => F::EntryPointDecl,
        "candidate_count" => F::CandidateCount,
        "ambiguous_reference" => F::AmbiguousReference,
        "external_name" => F::ExternalName,
        "external_kind" => F::ExternalKind,
        "redacted_literal" => F::RedactedLiteral,
        "symbol_moniker" => F::SymbolMoniker,
        "resolved_by_index" => F::ResolvedByIndex,
        "resolved_coverage" => F::ResolvedCoverage,
        "extraction_failed" => F::ExtractionFailed,
        "db_access" => F::DbAccess,
        "anchor" => F::Anchor,
        "anchor_skipped" => F::AnchorSkipped,
        "anchors_dropped" => F::AnchorsDropped,
        "enum_decision" => F::EnumDecision,
        "enum_decisions_unmatched" => F::EnumDecisionsUnmatched,
        "body_extraction_failed" => F::BodyExtractionFailed,
        _ => return None,
    })
}

/// Read an edge kind back from the word [`edge_word`] wrote. See
/// [`fact_kind_from_word`] for why a colon decides which half of the vocabulary
/// answers and why a declared name is looked up rather than merely hashed.
#[must_use]
pub fn edge_kind_from_word(word: &str, vocabulary: &Vocabulary) -> Option<super::EdgeKind> {
    use super::EdgeKind as E;
    if word.contains(':') {
        let id = EdgeKindId(fnv1a64(word));
        // The name has to match back — see [`fact_kind_from_word`].
        return vocabulary
            .edge(id)
            .filter(|d| qualified(&d.namespace, &d.name) == word)
            .map(|_| E::Declared(id));
    }
    Some(match word {
        "contains" => E::Contains,
        "declares" => E::Declares,
        "has_member" => E::HasMember,
        "extends" => E::Extends,
        "implements" => E::Implements,
        "calls" => E::Calls,
        "references" => E::References,
        "instantiates" => E::Instantiates,
        "imports" => E::Imports,
        "depends_on" => E::DependsOn,
        "uses" => E::Uses,
        _ => return None,
    })
}

/// The part a node kind plays in the backbone views, without a `Graph` (R9).
///
/// [`super::Graph::node_role`] delegates here. The free function exists for the
/// same reason [`node_word`] does: the card projection never receives a graph,
/// only the vocabulary, and a second copy of this table in `card` is the drift
/// one canonical function prevents.
#[must_use]
pub fn node_role(kind: super::NodeKind, vocabulary: &Vocabulary) -> StructuralRole {
    use super::NodeKind as N;
    match kind {
        N::Module => StructuralRole::Container,
        N::File => StructuralRole::Leaf,
        N::Symbol => StructuralRole::Definition,
        N::Dependency => StructuralRole::Package,
        N::External => StructuralRole::External,
        N::Declared(id) => vocabulary
            .node(id)
            .map_or(StructuralRole::Opaque, |d| d.role),
    }
}

/// The part an edge kind plays in the backbone views, without a `Graph` (R9).
/// See [`node_role`].
///
/// Only the three the views branch on carry a role. Every other core edge kind
/// is card and smell policy rather than the shape of the tree, so it is
/// `Opaque` here and keeps its concrete-kind match where it is read. The arm
/// below lists them instead of using a wildcard, so a new core edge kind has to
/// decide rather than default.
#[must_use]
pub fn edge_role(kind: super::EdgeKind, vocabulary: &Vocabulary) -> LinkRole {
    use super::EdgeKind as E;
    match kind {
        E::Contains => LinkRole::Containment,
        E::Declares => LinkRole::Declaration,
        E::DependsOn => LinkRole::Dependency,
        E::Declared(id) => vocabulary.edge(id).map_or(LinkRole::Opaque, |d| d.role),
        E::HasMember
        | E::Extends
        | E::Implements
        | E::Calls
        | E::References
        | E::Instantiates
        | E::Imports
        | E::Uses => LinkRole::Opaque,
    }
}

/// The namespace [`undeclared_name`] writes when the core cannot resolve an id.
/// Reserved, so a plugin's kinds can never be mistaken in an artifact for the
/// core's own "I could not resolve this" marker — found in RG2, and cheap to
/// close precisely because nothing legitimate wants that word.
const RESERVED_NAMESPACE: &str = "undeclared";

/// The namespace segment [`crate::card::card_path`] writes when a card's own
/// name or identity cannot be spelled as a path.
///
/// Reserved for the same reason as [`RESERVED_NAMESPACE`] and found the same way
/// — by asking what a plugin could take that the core already uses. A kind
/// declared in this namespace would compose `declared/_hashed/<name>` and share
/// a tree with the core's own fallback, where two unrelated cards could then
/// contend for one path.
const RESERVED_HASHED_NAMESPACE: &str = "_hashed";

/// Characters a declared name may not contain, beyond the separator.
///
/// A declared name is not only a map key: it is written verbatim into a Markdown
/// heading in `index.md`, into a backtick span in a card's lead sentence, and it
/// names the card's directory. RG3 found that gap by declaring a kind whose name
/// carried a newline and getting a heading into `index.md` that no corpus had
/// declared — while the path side folded the same characters away, so the two
/// sinks disagreed about what the kind was called.
///
/// Refused here rather than escaped at each sink, and that is the whole point of
/// the choice. The manifest publishes `namespace:name` as the join key a consumer
/// uses to tie a card, a coverage exclusion and a vocabulary entry together; a
/// name silently rewritten on the way to one of them would break that join
/// quietly. A refused declaration breaks it loudly, at the moment the plugin can
/// still fix it, and one gate covers every sink instead of three that have to be
/// kept in step.
///
/// Control characters cover the newline, the carriage return and NUL. The
/// backtick is listed on its own because it is not a control character and it is
/// what closes the span the lead sentence opens.
///
/// The third group is the one the first version of this gate missed, and three
/// reviewers found it independently: **a Unicode format character is not a
/// control character.** `char::is_control` answers for category `Cc` alone, so
/// U+202E RIGHT-TO-LEFT OVERRIDE and U+200B ZERO WIDTH SPACE passed it and
/// reached both the heading and the directory name — measured, not argued. They
/// do not break the markdown the way a newline does; they do something subtler
/// and worse for an artifact whose whole purpose is to be read: two kinds that
/// are distinct join keys render indistinguishably, or a heading displays in an
/// order the bytes do not have.
///
/// Listed as ranges rather than answered by a predicate because the standard
/// library exposes no `is_format`, and a crate dependency for one character
/// class is not worth it in a tool that promises to be offline and small.
///
/// What is deliberately NOT here: everything that merely needs folding on the
/// way to a path. `#`, `|`, `[`, `(` and a space are all legal in a name and are
/// folded by `sanitize_segment` (R15). The wire name is the join key a consumer
/// reads out of the manifest, and the core does not get to rewrite it — it only
/// gets to refuse a name that would stop being a name.
fn forbidden(c: char) -> bool {
    c.is_control()
        || c == '`'
        || matches!(c,
            '\u{00ad}'                 // SOFT HYPHEN
            | '\u{200b}'..='\u{200f}'  // zero-width, ZWJ/ZWNJ, LRM/RLM
            | '\u{202a}'..='\u{202e}'  // bidi embedding and override
            | '\u{2060}'..='\u{2064}'  // word joiner, invisible operators
            | '\u{2066}'..='\u{206f}'  // bidi isolates, deprecated formatting
            | '\u{feff}'               // zero-width no-break space / BOM
            | '\u{fff9}'..='\u{fffb}'  // interlinear annotation
        )
}

/// Whether one half of a declared name cannot be written down as itself.
///
/// `trim() != half` refuses a leading or trailing space, which also refuses a
/// half that is nothing but spaces. Neither is caught by `is_empty`, and both
/// produce a name a reader cannot tell from its neighbour: on the page the
/// space is invisible, and on disk `sanitize_segment` folds it away, so
/// `open:table` and `open:table ` would render the same and share a directory
/// while staying two distinct join keys.
fn ill_formed(half: &str) -> bool {
    half.is_empty() || half.trim() != half || half.contains(':') || half.contains(forbidden)
}

fn check_name(namespace: &str, name: &str) -> Result<(), VocabularyError> {
    if namespace == RESERVED_NAMESPACE || namespace == RESERVED_HASHED_NAMESPACE {
        return Err(VocabularyError::ReservedNamespace {
            namespace: namespace.to_string(),
            name: name.to_string(),
        });
    }
    // A name has to survive a round trip through `namespace:name`, so neither
    // half may be empty or carry the separator — and neither may carry anything
    // that changes the meaning of a reader-facing artifact (see `ill_formed`).
    if ill_formed(namespace) || ill_formed(name) {
        return Err(VocabularyError::MalformedName {
            namespace: namespace.to_string(),
            name: name.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim the whole U7 wire twin rests on: a core kind's serde spelling
    /// and its `*_word` spelling are the SAME string.
    ///
    /// It was true by inspection when the twin was written, and inspection is
    /// exactly what does not survive someone renaming a variant. If the two ever
    /// diverge, every existing warm cache goes cold on upgrade — as a silent
    /// re-parse, which looks like nothing at all. This is the only thing that
    /// would say so.
    ///
    /// The inverse is checked in the same loop because the `*_from_word` tables
    /// are hand-written a second time, and nothing else proves the two agree.
    #[test]
    fn a_core_kinds_serde_spelling_is_its_wire_word_and_survives_the_inverse() {
        let vocabulary = Vocabulary::default();

        for kind in super::super::FactKind::core_kinds() {
            let word = fact_word(*kind, &vocabulary);
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(word.to_string()),
                "fact kind {kind:?} serializes to something other than its wire word {word:?}"
            );
            assert_eq!(
                fact_kind_from_word(&word, &vocabulary),
                Some(*kind),
                "fact word {word:?} does not read back as {kind:?}"
            );
        }

        for kind in super::super::EdgeKind::core_kinds() {
            let word = edge_word(*kind, &vocabulary);
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(word.to_string()),
                "edge kind {kind:?} serializes to something other than its wire word {word:?}"
            );
            assert_eq!(
                edge_kind_from_word(&word, &vocabulary),
                Some(*kind),
                "edge word {word:?} does not read back as {kind:?}"
            );
        }
    }

    /// A word this corpus cannot name is refused rather than minted (KTD7).
    #[test]
    fn an_undeclared_qualified_name_reads_back_as_nothing() {
        let vocabulary = Vocabulary::default();
        assert_eq!(fact_kind_from_word("open:uroven", &vocabulary), None);
        assert_eq!(edge_kind_from_word("open:drzi", &vocabulary), None);
        // The projection's own fallback spelling must not be a back door either:
        // it carries a colon, so it takes the declared branch and finds nothing.
        assert_eq!(
            fact_kind_from_word(&undeclared_name(1234), &vocabulary),
            None
        );
    }

    fn node(namespace: &str, name: &str, role: StructuralRole) -> NodeDeclaration {
        NodeDeclaration {
            namespace: namespace.into(),
            name: name.into(),
            role,
            card: CardPolicy::None,
        }
    }

    /// The digest is a wire format (R14). If this test is ever "fixed" by
    /// re-recording the numbers, every corpus written before the change silently
    /// stops matching the one written after it.
    #[test]
    fn the_digest_is_pinned_to_literal_values() {
        assert_eq!(fnv1a64(""), FNV_OFFSET);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64("foobar"), 0x85944171f73967e8);
        assert_eq!(NodeKindId::of("open", "table").get(), fnv1a64("open:table"));
    }

    #[test]
    fn the_same_name_yields_the_same_id_in_every_category() {
        assert_eq!(NodeKindId::of("open", "t").get(), fnv1a64("open:t"));
        assert_eq!(EdgeKindId::of("open", "t").get(), fnv1a64("open:t"));
        assert_eq!(FactKindId::of("open", "t").get(), fnv1a64("open:t"));
        assert_eq!(CardKindId::of("open", "t").get(), fnv1a64("open:t"));
    }

    #[test]
    fn redeclaring_the_same_thing_returns_the_same_id() {
        let mut v = Vocabulary::default();
        let a = v
            .declare_node(node("open", "t", StructuralRole::Leaf))
            .unwrap();
        let b = v
            .declare_node(node("open", "t", StructuralRole::Leaf))
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn redeclaring_with_different_properties_is_a_named_error() {
        let mut v = Vocabulary::default();
        v.declare_node(node("open", "t", StructuralRole::Leaf))
            .unwrap();
        let err = v
            .declare_node(node("open", "t", StructuralRole::Container))
            .unwrap_err();
        assert_eq!(
            err,
            VocabularyError::Conflict {
                category: "node",
                name: "open:t".into()
            }
        );
    }

    #[test]
    fn a_malformed_name_is_refused() {
        let mut v = Vocabulary::default();
        assert!(matches!(
            v.declare_node(node("", "t", StructuralRole::Leaf)),
            Err(VocabularyError::MalformedName { .. })
        ));
        assert!(matches!(
            v.declare_node(node("open", "a:b", StructuralRole::Leaf)),
            Err(VocabularyError::MalformedName { .. })
        ));
    }

    /// The four categories are separate id spaces (R13): declaring a fact kind
    /// must not make a node kind appear.
    #[test]
    fn categories_do_not_leak_into_each_other() {
        let mut v = Vocabulary::default();
        let f = v
            .declare_fact(FactDeclaration {
                namespace: "open".into(),
                name: "t".into(),
                cardinality: Cardinality::Scalar,
            })
            .unwrap();
        assert!(v.fact(f).is_some());
        assert!(v.node(NodeKindId::of("open", "t")).is_none());
    }

    #[test]
    fn an_empty_vocabulary_reports_itself_empty() {
        let mut v = Vocabulary::default();
        assert!(v.is_empty());
        v.declare_card(CardDeclaration {
            namespace: "open".into(),
            name: "t".into(),
        })
        .unwrap();
        assert!(!v.is_empty());
    }
}
