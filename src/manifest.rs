//! The manifest emitter (U5): the machine projection of the graph (KTD4).
//!
//! A RAG consumer indexes this — the tool itself generates neither embeddings
//! nor an index (R11). The manifest carries stable leaf ids, chunk boundaries,
//! guiding questions, the canonical edges (R10 — they live here, not in the leaf
//! bodies), and the provenance of every fact and edge (R12). The schema is
//! versioned from the start.
//!
//! The wire model is intentionally self-contained (owned fields, both
//! `Serialize` and `Deserialize`) so it can round-trip independently of the
//! graph IR — the graph types are serialize-only.

use crate::graph::{
    CardPolicy, Cardinality, Claim, Confidence, EdgeKind, Fact, FactValue, Graph, LinkRole,
    NodeKind, Provenance, Resolution, StructuralRole, Tier, Vocabulary,
};
use crate::lens;
use crate::taxonomy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// Current manifest schema version.
///
/// v5 (this branch): repeated long strings are interned. A top-level `strings`
/// table holds each one once, and every reference to it is an index in a field
/// named with an `_id` suffix — `nodes[].key_id`, `edges[].from_id` / `to_id`,
/// and `provenance.file_id`. Measured over the 14,396-file R17 target, node
/// keys were 27.1% of a 725 MB artifact and provenance paths another 23.4%.
///
/// The fields are RENAMED rather than retyped in place. A `from` that silently
/// turned from a string into a number is the worst kind of break: a consumer
/// reads `12` where it expected a key and may not notice until its index is
/// wrong. A missing `from` fails immediately, which is what a breaking change
/// should do.
///
/// `extractor`, `tier` and `resolution` stay literal strings. They are short,
/// they come from a closed vocabulary, and `"tier":"measured"` is worth the 12%
/// it costs over `"tier":11`.
///
/// v6: provenance may carry `ambiguous_candidates: usize` (omitted when
/// absent — never `null`) when a heuristic name match found more than one
/// repo-wide candidate for that specific edge, matching the equally-additive
/// `end_line` precedent from v3 below: a v5 consumer would read every
/// relation as if it were an unambiguous, singular match, missing the new
/// axis entirely rather than merely defaulting it, so the vocabulary growth
/// is signalled here too rather than shipped silently under v5. Unlike
/// `end_line`, no wire-compatibility break forces this bump on its own — a
/// missing `Option<T>` field decodes to `None` either way — but manifest.json
/// is this project's one artifact meant for downstream, out-of-process
/// consumers (unlike the extraction cache, which the tool only ever reads
/// back itself and which `CACHE_VERSION` bumps for the same field for a
/// different, correctness-driven reason — see its own doc comment), so the
/// discoverability bar `resolution`/`end_line` already set for this struct
/// applies here too.
///
/// v4: `end_line` is now written exactly when a fact spans MORE
/// THAN ONE LINE. Under v3 a declaration that fit on a single line wrote
/// `end_line` equal to `line`, so the field distinguished "a one-line
/// declaration" from "a point anchored at that line" — a distinction no
/// artifact ever rendered (both anchor as `file:7`) and no consumer could act
/// on. The graph now models a place as one value rather than a start beside an
/// independent end (`Span`), which makes the states that meant nothing
/// unrepresentable and collapses those two spellings into one. A v3 consumer
/// reading `end_line` would see it disappear from single-line declarations, so
/// the change is signalled rather than shipped silently.
///
/// v3: every provenance gained a mandatory `resolution` — the
/// precision axis, `"heuristic"` or `"resolved"` (R4/KD2) — and an optional
/// `end_line` for facts anchored to a line range rather than a point. Both are
/// additive, but a v2 consumer would read every relation as if its precision were
/// unknown and could not tell a range-anchored fact from a point-anchored one, so
/// the vocabulary growth is signalled by the bump instead of shipped silently
/// under v2. `resolution` is always present; `end_line` is omitted when absent.
///
/// v8: `documents` lists every card and every architecture page beside the
/// views, each with a `kind` (`card:type`, `card:file`, `card:module`,
/// `card:<declared>`, `page`) and one chunk per `##` section. A view document
/// carries no `kind`, so a v6 consumer that filters on it keeps reading exactly
/// what it read before — but the document list itself grew by tens of
/// thousands of entries, and a consumer indexing "every document" would index
/// them, so the change is signalled.
///
/// v9: the provenance of a name-resolved relation carries its confidence rung
/// and the rule that admitted it (plan A, U1: R1, R2) — `confidence`, one of
/// `resolved_by_index`, `rules_backed`, `ambiguous`, `name_only`, and
/// `rule_id`, an index into `strings` of a host-owned rule id such as
/// `name-match`. Both are omitted when absent, never `null`, and both are absent
/// from every fact and every structural edge (`contains`, `declares`,
/// `has_member`, `imports`), whose bytes therefore do not move.
/// `resolved_by_index` is derived from `resolution: "resolved"` and comes
/// without a `rule_id`. Signalled for the reason v6 was: a v8 consumer would
/// read every call as equally trustworthy and miss the axis that says which
/// ones are a guess. The rule id is interned on first use, so in a corpus that
/// has name-resolved edges the string table gains one entry and any string
/// first seen after it moves up by one index. The same version also gives such
/// an edge `sites` (plan A, U3): the sorted list of every usage site the
/// relation was read at — file as a string index, line, and a `column` key
/// once a plugin reports columns — so an edge is one relation summarizing its
/// sites, and the alternatives of one ambiguous site share that site's entry.
/// Omitted, like the rung, on every edge no usage site produced.
///
/// v10 (plan B, one bump for the whole area; later units on that branch extend
/// this paragraph instead of bumping again): a node's `facts` may carry the
/// core kind `db_access` (U2) — one per database access a SQL literal makes,
/// on the declaration around it, its text `<access> <object>` with the access
/// one of `reads`, `writes`, `calls`, `mentions` and the occurrence's line in
/// the provenance. No field changed shape, but a v9 consumer that reads the
/// core fact kinds as a closed set meets a word it does not know, and one that
/// counts facts sees more of them, so the growth is signalled as v6's was.
/// The `uses` edge to the object is unchanged beside it.
/// The anchor kinds of plan B, U3 (`anchor`, `anchor_skipped`,
/// `anchors_dropped`) are core kinds of the graph from the same version on,
/// but a v10 manifest never carries one: their only projection is the detail
/// (KTD1), so a v10 consumer's closed set of fact words stays exact. Also from
/// U3 on, a SQL statement in a C# raw literal (`"""…"""`) or in the text of an
/// interpolated one (`$"…"`, holes left out) is read like any other, which
/// adds `uses` edges and `db_access` facts the lexer used to miss.
/// The same holds for the kinds of plan B, U5 (`enum_decision`,
/// `enum_decisions_unmatched`): core kinds of the graph, never in a v10
/// manifest — the decisions' projection is the enum's detail and the count's
/// is coverage.
/// From plan B, U11 the `documents` describe the three levels of the card
/// corpus (KTD9, R4, R16), so each channel picks what it indexes. Two new
/// document kinds: `map`, one per module map, id `map:<module key>`, and
/// `card:detail`, one per detail, id `detail:<card identity>` — never
/// `<identity>#detail`, which is the shape of a chunk id. Both ids are as
/// stable across runs as the keys they are built from, so an importer can
/// use one as its `source_ref`. Four optional fields, omitted when absent,
/// never `null`, and absent from every view and page document, whose bytes
/// therefore do not move: `level` (`map`, `card` or `detail` — the one field
/// to filter the levels on, since `card:detail` shares the `card:` prefix
/// with the cards); `parent`, the id of the document one level up — a
/// detail's card, a map's module card, a type card's map when its module has
/// one, and nothing on any other card; `embed: false` on every detail and
/// on nothing else — the owner's decision that details are not imported
/// into a vector index (an absent `embed` means a document may be embedded);
/// and `header`, the full context header of the entity the document is
/// about: `path` (the source file, or a module's directory), `module` (the
/// module it lives in — the project of R4, as this corpus groups code),
/// `namespace` (the qualifier its language's plugin keys a symbol under — a
/// C# namespace, a Java package, a Rust module path, a JS/TS file), `kind`
/// (as the signature states it: `public static class`), `name`, and
/// `purpose` (the sentence a module's map gives it, in the run's language).
/// Signalled within v10: a v9 consumer that reads every `card:` kind as a
/// card would index the details as cards.
///
/// Since plan A, U7 the rule ids come from the shared resolver's catalog —
/// `role-mismatch`, `declared-receiver`, `static-receiver`, `implicit-this`,
/// `arity`, `namespace-visibility`, `nested-type-visibility`, `base-spelling`
/// beside `name-match` — and
/// a rung can be `rules_backed`. The id names the rule that disputed the edge,
/// or failing that the one that backed it, or `name-match` when none decided.
/// No field changed shape, so the version did not move; a v9 consumer that
/// treats `rule_id` as an open string reads the new ids as it read the first.
///
/// What the manifest does NOT carry is the usage site itself: which
/// candidates its name matched, and what every rule said about each (KTD7).
/// Those records go to [`SITES_FILENAME`] beside the manifest only when the
/// operator asks (`--sites`), one JSON object per line in stable-id order
/// (file, owner, line, column, name, kind): `file`, `owner` (a node key),
/// `line`, `column` when the plugin reports one, `name`, `kind` (`calls`,
/// `instantiates`, `references`), `owner_mapped: false` when the line alone
/// could not tell the enclosing declaration, `candidate_count`, and
/// `candidates` — each with `target`, `trace` (`[{rule, verdict}]`, verdict
/// one of `back`, `dispute`, `exclude`, `abstain`) and `outcome`: `rated`
/// with `rung` and `rule`, `deleted` with `rule`, or `over_cap`. A site over
/// the ceiling that no rule could thin lists no candidates. A run given a SCIP
/// index adds `index` to every site in a file the index has a document for
/// (plan A, U9): `{"pairing": …}`, one of `resolved` with the `target` node
/// key, `external`, `unverified_document` or `unmapped`. The candidates'
/// outcomes stay what the resolver decided; the pairing is the index's verdict
/// beside them. Keys are literal
/// strings, not `strings` indices: the file is a debugging and measuring aid,
/// read on its own. Without `--sites` the corpus is byte-for-byte what it is
/// with it, minus this one file.
///
/// v2: file->symbol edges serialize as `"declares"`, not `"contains"` —
/// `EdgeKind` gained a `Declares` variant so module-level containment
/// (`"contains"`) and symbol declaration are distinct on the wire. A v1 consumer
/// that treated `"contains"` as the only containment edge would read a repo with
/// symbols differently, so the vocabulary change is signalled by the bump rather
/// than shipped silently under v1.
pub const SCHEMA_VERSION: u32 = 10;

/// The version a corpus gets when it carries a [`ManifestVocabulary`] — that
/// is, when its extractor declared kinds of its own (U4, R12).
///
/// **The version rises only for the corpora the change is about.** A corpus
/// with no declarations keeps [`SCHEMA_VERSION`] and stays byte-identical to the
/// one this tool wrote before the vocabulary existed (R4). The alternative —
/// bumping everything to 7 — would have forced every existing consumer to
/// upgrade in order to read a field it would never see, and would have made the
/// byte-identity gate unsatisfiable without exempting the version field from it.
/// Exempting a field from the gate that exists to catch drift is how a gate
/// stops being one.
///
/// What a v7 reader must handle that a v6 reader need not: a top-level
/// `vocabulary` object, and `kind` strings on nodes, edges and facts that are
/// not from the core's closed set — they are `namespace:name`, and the
/// `vocabulary` section is what explains them. A v6 consumer meeting a v7
/// corpus should stop rather than guess: an unknown `kind` string is exactly the
/// case where guessing produces a confident wrong answer.
///
/// It moves in step with [`SCHEMA_VERSION`], one above it, because a declaring
/// corpus carries every change the plain one does: 9 with v8's documents, 10
/// with v9's confidence rung and rule, 11 with v10's database accesses and
/// its map and detail documents.
pub const SCHEMA_VERSION_WITH_VOCABULARY: u32 = 11;

/// The full manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// The kinds this corpus declared for itself, absent when it declared none
    /// (R3). Written before the bulk so a consumer streaming the file learns the
    /// vocabulary before it meets a kind that needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<ManifestVocabulary>,
    /// The language plugins the corpus was built with, in precedence order
    /// (U13): which extractor wrote a fact is in its provenance, and this is
    /// what that id was — a version, a digest, bundled or declared.
    ///
    /// Additive and without a version bump: a consumer that does not know the
    /// key misreads nothing by skipping it, unlike the `kind` strings the
    /// vocabulary bump guards. Omitted when empty, which only a hand-built
    /// manifest is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<ManifestPlugin>,
    /// Every repeated long string in the artifact, written once.
    ///
    /// Node keys and source paths are the two things this export says over and
    /// over: measured over the 14,396-file R17 target, `key`/`from`/`to` were
    /// 27.1% of a 725 MB file and provenance paths another 23.4% — half the
    /// artifact was the same few hundred thousand strings, repeated across
    /// 218k nodes, 950k edges and 2.4M facts.
    ///
    /// Indices into this table are named with an `_id` suffix wherever they
    /// appear. `extractor`, `tier` and `resolution` are deliberately NOT
    /// interned: they are short, they come from a closed vocabulary of a
    /// handful of words, and reading `"tier":"measured"` instead of
    /// `"tier":11` is worth the 12% they cost.
    ///
    /// Order is first use, which is deterministic because every collection the
    /// build walks is already sorted (R18).
    pub strings: Vec<String>,
    pub documents: Vec<ManifestDoc>,
    pub nodes: Vec<ManifestNode>,
    pub edges: Vec<ManifestEdge>,
}

/// One language plugin a corpus was built with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPlugin {
    /// The id every fact it wrote carries as its extractor.
    pub id: String,
    pub version: String,
    /// SHA-256 of the component, hex.
    pub sha256: String,
    /// `bundled` or `declared`. Never the path: that is the operator's
    /// machine, not the corpus.
    pub origin: String,
    /// The languages it claims, by name.
    pub languages: Vec<String>,
}

/// The corpus's own account of the kinds it declared (R3).
///
/// This is what makes a corpus self-describing rather than self-referential: a
/// consumer that has never heard of the extractor can still enumerate every kind
/// the corpus uses and read how the core treated each one. What it deliberately
/// does NOT carry is what a kind *means* — that is domain knowledge, it lives
/// with the extractor, and a field for it here would be an invitation to put
/// domain knowledge in the core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestVocabulary {
    /// The digest that turns a qualified name into a kind's identity, by name
    /// (R14).
    ///
    /// Nothing below is keyed by that identity — the corpus refers to a kind by
    /// name everywhere (KTD7). It is named here because the digest is still
    /// observable in the artifact: all four kind enums derive `Ord`, so declared
    /// kinds sort among themselves by digest, and a reader who wants to
    /// reproduce that ordering needs to know which function produced it.
    pub digest: String,
    pub node_kinds: Vec<ManifestNodeKind>,
    pub edge_kinds: Vec<ManifestEdgeKind>,
    pub fact_kinds: Vec<ManifestFactKind>,
    pub card_kinds: Vec<ManifestCardKind>,
}

/// Fields every declared kind carries, whatever its category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestKindName {
    /// The qualified name, `namespace:name` — the same spelling that appears as
    /// a `kind` on nodes, edges and facts, so the two join directly.
    pub name: String,
    /// The namespace half, carried rather than left to be re-split out of
    /// `name`: grouping by extractor is the obvious thing a consumer wants, and
    /// making it re-parse a string is the stringly-typed shape this design
    /// rejected elsewhere.
    pub namespace: String,
}

// No `id` field, deliberately. An earlier draft of U4 published the digest of
// `name` beside it, and the R2 behavioural gate turned red on the spot: two
// corpora built from vocabularies differing only in their names now differed in
// something that was not a name. The gate was right, and for a second reason it
// could not see — the four categories share one digest, so a node kind
// `open:table` and a card kind `open:table` would have carried the SAME id, and
// an artifact that spells two different things identically is worse than one
// that omits a derivable value. `digest` above names the function; a consumer
// that wants an id can compute it.

/// A declared node kind and the two decisions the core took from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestNodeKind {
    #[serde(flatten)]
    pub kind: ManifestKindName,
    /// The part it plays in the backbone views (R9): `container`, `leaf`,
    /// `definition`, `package`, `external`, or `opaque` for a kind that takes no
    /// part in them.
    pub role: String,
    /// The declared card kind its nodes render under, or absent when the kind is
    /// not card-bearing (R8). A qualified name, joining to `card_kinds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
}

/// A declared edge kind and its part in the backbone views (R9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEdgeKind {
    #[serde(flatten)]
    pub kind: ManifestKindName,
    /// `containment`, `declaration`, `dependency`, or `opaque`.
    pub role: String,
}

/// A declared fact kind and the property `merge` resolved it by (R7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestFactKind {
    #[serde(flatten)]
    pub kind: ManifestKindName,
    /// `additive` (a node legitimately holds several) or `scalar` (one resolved
    /// value, conflicts settled by trust rank). This is the one property a
    /// consumer genuinely needs in order to read the facts correctly, and it can
    /// be read without knowing what the kind means.
    pub cardinality: String,
}

/// A declared card kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestCardKind {
    #[serde(flatten)]
    pub kind: ManifestKindName,
}

/// One retrievable document (leaf): stable id, framing, and chunk boundaries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestDoc {
    pub id: String,
    /// `card:type`, `card:file`, `card:module`, `card:<declared kind>`,
    /// `card:detail`, `map` or `page`; absent on a view (v8, v10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Which level of the card corpus the document is (plan B, U11): `map`,
    /// `card` or `detail`. Absent on a view and a page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The id of the document one level up: a detail's card, a map's module
    /// card, a type card's map. See [`SCHEMA_VERSION`], v10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// `false` on a document a vector index should leave out — every detail
    /// (plan B, KTD9) and nothing else; absent means it may be embedded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed: Option<bool>,
    /// The context header of the entity a card, a detail or a map is about
    /// (plan B, R4). Absent on a view and a page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<ManifestHeader>,
    pub title: String,
    pub guiding_question: String,
    pub path: String,
    pub chunks: Vec<ManifestChunk>,
}

/// The full context header of a document's entity (plan B, R4, KTD6): what a
/// chunk retrieved on its own needs to say where it is from. Nested rather
/// than flattened into [`ManifestDoc`], whose `path` is the document's own
/// path in the corpus — this `path` is the entity's source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestHeader {
    /// The source file the entity is declared in (its first part's, for a
    /// type in several files), or a module's directory key.
    pub path: String,
    /// The module the entity lives in — a module's own key on its card and
    /// its map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// The qualifier a language plugin keys a symbol under: a C# namespace,
    /// a Java package, a Rust module path, a JS/TS file. Absent on a file, a
    /// module and a map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// The kind as the card's own header states it — a type's from its
    /// signature (`public static class`), else `file`, `module` or the
    /// declared kind's word.
    pub kind: String,
    pub name: String,
    /// What the entity is for, in a sentence: the one its module's map gives
    /// it, in the run's language. Absent where the card knows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// A document for a rendered card or page: its chunks are its `##` sections,
/// in the order they appear, each id `<doc id>#<slug>`.
pub fn rendered_document(
    id: &str,
    kind: &str,
    title: &str,
    guiding_question: &str,
    path: &str,
    contents: &str,
) -> ManifestDoc {
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let chunks = contents
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(|heading| {
            let base: String = heading
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .split('-')
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("-");
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let slug = if *n == 1 { base } else { format!("{base}-{n}") };
            ManifestChunk {
                id: format!("{id}#{slug}"),
                heading: heading.to_string(),
            }
        })
        .collect();
    ManifestDoc {
        id: id.to_string(),
        kind: Some(kind.to_string()),
        level: None,
        parent: None,
        embed: None,
        header: None,
        title: title.to_string(),
        guiding_question: guiding_question.to_string(),
        path: path.to_string(),
        chunks,
    }
}

/// A logical chunk boundary within a document (a section).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestChunk {
    pub id: String,
    pub heading: String,
}

/// A node with its facts and their provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestNode {
    /// Index into [`Manifest::strings`] of this node's key.
    pub key_id: usize,
    pub kind: String,
    pub facts: Vec<ManifestFact>,
}

/// A canonical edge with provenance (R10, R12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEdge {
    /// Indices into [`Manifest::strings`] of the endpoint keys.
    pub from_id: usize,
    pub to_id: usize,
    pub kind: String,
    pub provenance: ManifestProvenance,
    pub dissent: Vec<ManifestClaim>,
    /// Every usage site the relation was read at, in source order (v9, plan A
    /// KTD7). Omitted for a relation no usage site produced — `contains`,
    /// `declares`, `has_member`, `imports` — whose bytes therefore do not move.
    /// The alternatives of one ambiguous site are separate edges that share
    /// its site, which is how a consumer groups them back into one claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<ManifestSite>,
}

/// One usage site on the wire (v9): the source path as a string-table index,
/// the 1-based line, and the column once a plugin reports one. The column key
/// is omitted while absent — every site a contract-0.1 plugin produces — so
/// adding it later fills a field this format already defines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestSite {
    /// Index into [`Manifest::strings`] of the source path.
    pub file_id: usize,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestFact {
    pub kind: String,
    pub value: Value,
    pub provenance: ManifestProvenance,
    pub dissent: Vec<ManifestClaim>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestClaim {
    pub value: Value,
    pub provenance: ManifestProvenance,
}

/// The wire form of [`Provenance`]. `tier` and `resolution` are the two
/// independent axes (KTD1) and both are always written; `end_line` is written only
/// when the fact is anchored to a range, and a missing `end_line` deserializes
/// back to `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestProvenance {
    /// Index into [`Manifest::strings`] of the source path.
    pub file_id: usize,
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub extractor: String,
    pub tier: String,
    pub resolution: String,
    /// How many repo-wide candidates a heuristic name match found for this
    /// specific edge, when more than one. Additive: a missing key deserializes
    /// to `None` (serde's built-in behavior for an `Option<T>` field), so a
    /// manifest written before this field existed still round-trips without a
    /// `SCHEMA_VERSION` bump.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguous_candidates: Option<usize>,
    /// The confidence rung of a name-resolved relation (v9, R2): one of
    /// `resolved_by_index`, `rules_backed`, `ambiguous`, `name_only`. Written
    /// on edges and their dissent only, and omitted for every relation that
    /// did not come from resolving a name — a `contains`, `declares` or
    /// `imports` edge carries no such key, and neither does any fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Index into [`Manifest::strings`] of the id of the rule that admitted
    /// the relation (v9, R1) — `name-match` and the rules later units add.
    /// Interned like every other repeated string, and present exactly when a
    /// rung was stored; an index-resolved edge, whose rung is derived, has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<usize>,
}

/// Build the manifest from the graph. Documents come from the same catalog the
/// corpus renders, so leaf ids and guiding questions match exactly.
///
/// `profile` MUST be the same value passed to
/// [`crate::render::build_corpus`]: both projections select views through
/// `taxonomy::select_views`, so passing the profile to only one would make the
/// RAG-indexed manifest list a different view set than the corpus (KTD-C5/F1).
pub fn build_manifest(
    graph: &Graph,
    profile: Option<&str>,
) -> Result<Manifest, crate::taxonomy::profile::UnknownProfile> {
    let documents = taxonomy::select_views(graph, profile)?
        .iter()
        .map(|view| {
            let composed = lens::compose(view, graph);
            let chunks = chunks_for(&composed);
            ManifestDoc {
                id: composed.view_id.clone(),
                kind: None,
                level: None,
                parent: None,
                embed: None,
                header: None,
                title: composed.title,
                guiding_question: composed.guiding_question,
                path: format!("{}.md", composed.view_id),
                chunks,
            }
        })
        .collect();

    let mut strings = Interner::default();

    let nodes = graph
        .nodes_sorted()
        .iter()
        .map(|node| ManifestNode {
            key_id: strings.intern(node.key.as_str()),
            kind: node_kind(node.kind, graph.vocabulary()).into_owned(),
            // Anchors are left out before anything is interned, so their
            // files and texts never reach `strings` either (plan B, KTD1).
            facts: node
                .facts
                .iter()
                .filter(|f| !f.kind.is_detail_only())
                .map(|f| fact(f, &mut strings, graph.vocabulary()))
                .collect(),
        })
        .collect();

    let edges = graph
        .edges_sorted()
        .iter()
        .map(|e| ManifestEdge {
            from_id: strings.intern(e.from.as_str()),
            to_id: strings.intern(e.to.as_str()),
            kind: edge_kind(e.edge.kind, graph.vocabulary()).into_owned(),
            provenance: edge_provenance(&e.edge.provenance, &mut strings),
            dissent: e
                .edge
                .dissent
                .iter()
                .map(|c| edge_claim(c, &mut strings))
                .collect(),
            sites: e
                .edge
                .sites
                .iter()
                .map(|site| ManifestSite {
                    file_id: strings.intern(&site.file),
                    line: site.line,
                    column: site.column,
                })
                .collect(),
        })
        .collect();

    // R12: the version follows the presence of the section, not the release.
    let vocabulary = manifest_vocabulary(graph.vocabulary());
    let schema_version = if vocabulary.is_some() {
        SCHEMA_VERSION_WITH_VOCABULARY
    } else {
        SCHEMA_VERSION
    };

    Ok(Manifest {
        schema_version,
        vocabulary,
        plugins: Vec::new(),
        strings: strings.into_table(),
        documents,
        nodes,
        edges,
    })
}

/// Assigns each distinct string one index, in first-use order.
///
/// First use rather than sorted: the walk that feeds it is over
/// `nodes_sorted()` and `edges_sorted()`, so the order is already deterministic
/// (R18), and interning as we go costs one pass instead of two over a graph
/// with 218k nodes and 950k edges.
#[derive(Default)]
struct Interner {
    seen: BTreeMap<String, usize>,
    table: Vec<String>,
}

impl Interner {
    fn intern(&mut self, s: &str) -> usize {
        if let Some(&id) = self.seen.get(s) {
            return id;
        }
        let id = self.table.len();
        self.seen.insert(s.to_string(), id);
        self.table.push(s.to_string());
        id
    }

    fn into_table(self) -> Vec<String> {
        self.table
    }
}

impl Manifest {
    /// The string an `_id` field points at, or `""` for an index this manifest
    /// does not contain.
    ///
    /// Empty rather than a panic: a manifest is data, and a consumer holding a
    /// stale or hand-edited one deserves a readable nothing rather than a
    /// crash. Every id this build emits is in range by construction.
    pub fn string(&self, id: usize) -> &str {
        self.strings.get(id).map(String::as_str).unwrap_or_default()
    }
}

/// Serialize the manifest deterministically (stable field and element order).
///
/// Compact, not pretty-printed. Measured over the 14,396-file R17 target, where
/// this artifact came to 1002 MB: **29.9% of those bytes were leading
/// indentation and a further 3.0% were newlines**. A third of the largest thing
/// RA writes was whitespace.
///
/// The JSON VALUE is unchanged — only insignificant whitespace goes — so no
/// consumer breaks and the schema version does not move. And this artifact is
/// the machine-facing one: a RAG indexer parses it, a person reads the views and
/// `coverage.json`, which stay pretty for exactly that reason.
///
/// Determinism is unaffected: serde writes fields in declaration order and every
/// collection was sorted before it got here (R18).
pub fn to_json(manifest: &Manifest) -> String {
    serde_json::to_string(manifest).expect("manifest serializes")
}

/// The same manifest, indented, for a person reading a diff.
///
/// Not what gets written — see [`to_json`] for why. It exists so the golden
/// snapshot stays reviewable: a snapshot whose diff is one line ten million
/// characters wide is a snapshot nobody can approve.
pub fn to_json_pretty(manifest: &Manifest) -> String {
    serde_json::to_string_pretty(manifest).expect("manifest serializes")
}

/// U10/KTD4: default cutover threshold (total nodes + edges) above which
/// [`should_write_line_delimited`] recommends the line-delimited export
/// instead of one [`to_json`] blob.
///
/// Deliberately a rough estimate, not a calibrated value: the only real
/// measurement available (the reference C# repository target: 14,396 files -> 218k
/// nodes + 950k edges) implies a density of roughly 81 nodes+edges per
/// file, so this threshold would already trip on a repository of about
/// 620 files — well short of what "typical small/medium" suggests.
/// Density varies by language and coding style, so one data point cannot
/// fix a reliable general threshold. Named and easy to tune; real
/// calibration against at least one mid-size repository is a separate,
/// later step (see the U10 plan's Scope Boundaries).
pub const LINE_DELIMITED_THRESHOLD: usize = 50_000;

/// U10/KTD3: whether `total_nodes_and_edges` crosses [`LINE_DELIMITED_THRESHOLD`].
///
/// The count is `graph.node_count()` plus `graph.edge_count()`
/// ([`crate::graph::Graph`]), or equivalently the length of an already-built
/// [`Manifest`]'s `nodes` plus its `edges` — both read the same O(1) counts
/// over the same graph.
///
/// Takes a bare count, not a `&Graph` or `&Manifest`, so the boundary is
/// testable on its own without building either.
pub fn should_write_line_delimited(total_nodes_and_edges: usize) -> bool {
    total_nodes_and_edges > LINE_DELIMITED_THRESHOLD
}

/// Top-level filenames the line-delimited export writes, above the
/// threshold, instead of `manifest.json`.
pub const STRINGS_FILENAME: &str = "manifest-strings.json";
pub const NODES_FILENAME: &str = "manifest-nodes.jsonl";
pub const EDGES_FILENAME: &str = "manifest-edges.jsonl";
pub const DOCUMENTS_FILENAME: &str = "manifest-documents.jsonl";

/// The usage-site records `--sites` writes beside the manifest (plan A,
/// KTD7) — see the note on [`SCHEMA_VERSION`].
pub const SITES_FILENAME: &str = "sites.jsonl";

/// [`SITES_FILENAME`]'s content: every site record the run kept, one per line
/// in stable-id order. Empty when the run kept none.
pub fn to_sites_jsonl(graph: &Graph) -> String {
    to_jsonl(&graph.sites().records_sorted())
}

/// The four filenames above, as the one array both [`owned_filenames`] and
/// the write path draw from — so a caller cleaning up the line-delimited
/// form (or checking what it owns) can never drift out of sync with the
/// four names actually written.
pub const LINE_DELIMITED_FILENAMES: [&str; 4] = [
    STRINGS_FILENAME,
    NODES_FILENAME,
    EDGES_FILENAME,
    DOCUMENTS_FILENAME,
];

/// The top-level manifest filename(s) a run owns for the given mode: the
/// four line-delimited files, or the single `manifest.json` monolith.
/// Exactly one manifest form is ever owned at once (U10/KTD5) — a caller
/// writing or cleaning up a manifest calls this instead of re-deriving the
/// branch itself, so the two call sites (`write_corpus`, `run_enrich`)
/// can never disagree about which names belong to which mode.
pub fn owned_filenames(line_delimited: bool) -> Vec<&'static str> {
    if line_delimited {
        LINE_DELIMITED_FILENAMES.to_vec()
    } else {
        vec!["manifest.json"]
    }
}

/// The wire form of [`STRINGS_FILENAME`]: the same `strings` table
/// [`Manifest`] carries, as its own small, bounded document — a consumer
/// loads this once, then streams the (potentially huge) node/edge/document
/// files, resolving every `_id` field against it exactly as it would
/// against a [`to_json`] blob's `strings` array (KTD6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestStrings {
    pub schema_version: u32,
    /// The corpus's own declared vocabulary, when it has one (R3).
    ///
    /// It lives here and not in a fifth file because this is the header document:
    /// small, bounded, and the one a consumer loads before it streams anything
    /// else. The external review round found it missing from this form entirely —
    /// and `schema_version` right above it was already announcing
    /// [`SCHEMA_VERSION_WITH_VOCABULARY`], so a large declaring corpus promised a
    /// section it then carried nowhere. Which form gets written is decided by
    /// SIZE ([`should_write_line_delimited`]), so the corpora that lost their
    /// vocabulary were exactly the big ones.
    ///
    /// `Option` + `skip_serializing_if`, like the whole-blob form: a corpus that
    /// declares nothing writes the same bytes here it always did (R4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<ManifestVocabulary>,
    /// The plugins the corpus was built with — see [`Manifest::plugins`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<ManifestPlugin>,
    pub strings: Vec<String>,
}

/// Borrowed sibling of [`ManifestStrings`] used only for serializing —
/// same field order, so the JSON it produces is byte-identical, but it
/// borrows `manifest.strings` instead of cloning the whole table just to
/// hand it to `serde_json`.
#[derive(Serialize)]
struct ManifestStringsRef<'a> {
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    vocabulary: Option<&'a ManifestVocabulary>,
    #[serde(skip_serializing_if = "<[ManifestPlugin]>::is_empty")]
    plugins: &'a [ManifestPlugin],
    strings: &'a [String],
}

/// [`STRINGS_FILENAME`]'s content: the same `strings` table as [`to_json`],
/// as its own compact document.
pub fn to_strings_json(manifest: &Manifest) -> String {
    let wire = ManifestStringsRef {
        schema_version: manifest.schema_version,
        vocabulary: manifest.vocabulary.as_ref(),
        plugins: &manifest.plugins,
        strings: &manifest.strings,
    };
    serde_json::to_string(&wire).expect("manifest strings serialize")
}

/// One [`ManifestNode`] JSON object per line — [`NODES_FILENAME`]'s content.
/// `key_id` resolves against [`to_strings_json`]'s table, unchanged (KTD6).
pub fn to_nodes_jsonl(manifest: &Manifest) -> String {
    to_jsonl(&manifest.nodes)
}

/// One [`ManifestEdge`] JSON object per line — [`EDGES_FILENAME`]'s content.
/// `from_id`/`to_id` resolve against [`to_strings_json`]'s table, unchanged.
pub fn to_edges_jsonl(manifest: &Manifest) -> String {
    to_jsonl(&manifest.edges)
}

/// One [`ManifestDoc`] JSON object per line, chunks nested inside it —
/// [`DOCUMENTS_FILENAME`]'s content. Documents are few (one per view) even
/// on the largest repository, so a document's own chunks stay inline rather
/// than a fifth file: KTD2 asks for one consistent mechanism, not for
/// splitting something that is never large.
pub fn to_documents_jsonl(manifest: &Manifest) -> String {
    to_jsonl(&manifest.documents)
}

/// Writes each record straight into the buffer via `serde_json::to_writer`
/// rather than through `serde_json::to_string` — at the hundreds-of-thousands-
/// of-edges scale this unit targets, skipping one owned `String` allocation
/// per record (immediately copied into `out` and dropped) is a real saving,
/// not a micro-optimization.
fn to_jsonl<T: Serialize>(records: &[T]) -> String {
    // 64 bytes/record is a rough floor for a `ManifestNode`/`ManifestEdge` —
    // avoids repeated reallocation during the loop below without needing an
    // exact size, which `serde_json::to_writer` doesn't report in advance.
    let mut out = Vec::with_capacity(records.len().saturating_mul(64));
    for record in records {
        serde_json::to_writer(&mut out, record).expect("record serializes");
        out.push(b'\n');
    }
    String::from_utf8(out).expect("serde_json emits valid UTF-8")
}

/// The logical sections a leaf carries, as chunk boundaries. Derived from the
/// composed lens so the boundaries track the `##`-level headings `render_leaf`
/// emits — the "Failure modes" section exists only when the lens has failure
/// modes.
///
/// Scope note: `##` level only. A wide module-structure leaf is cut into `###`
/// sub-sections (U4) that this list deliberately does not enumerate, so a
/// consumer chunking on these boundaries still gets "In this repository" whole.
/// Exposing those sub-sections as retrievable chunks is a deferred
/// follow-up.
fn chunks_for(lens: &lens::Lens) -> Vec<ManifestChunk> {
    let mut sections = vec![("problem", "Problem"), ("mechanism", "Mechanism")];
    // The diagram section (U11) exists only when the lens carries a diagram, so a
    // RAG consumer can target or skip a distinct `diagram` chunk instead of the
    // mermaid folding into the Mechanism chunk (KTD-C2/B1). Slotted after
    // Mechanism and before the instances, matching `render_leaf`.
    if lens.diagram.is_some() {
        sections.push(("diagram", "Diagram"));
    }
    sections.push(("in-this-repository", "In this repository"));
    if !lens.failure_modes.is_empty() {
        sections.push(("failure-modes", "Failure modes"));
    }
    sections
        .iter()
        .map(|(slug, heading)| ManifestChunk {
            id: format!("{}#{slug}", lens.view_id),
            heading: heading.to_string(),
        })
        .collect()
}

/// The corpus's vocabulary in wire form, or `None` when nothing was declared.
///
/// Every list is sorted **by name**. The declarations themselves are stored by
/// digest, which is deterministic but arbitrary to a reader — a plugin's kinds
/// would be scattered through the list in an order nothing explains. Sorting
/// here costs one pass over a few hundred entries (A1) and makes the section
/// readable, which is most of what it is for.
fn manifest_vocabulary(vocabulary: &Vocabulary) -> Option<ManifestVocabulary> {
    if vocabulary.is_empty() {
        return None;
    }

    fn named(namespace: &str, name: &str) -> ManifestKindName {
        ManifestKindName {
            name: format!("{namespace}:{name}"),
            namespace: namespace.to_string(),
        }
    }

    fn by_name<T>(mut rows: Vec<(String, T)>) -> Vec<T> {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows.into_iter().map(|(_, row)| row).collect()
    }

    let node_kinds = by_name(
        vocabulary
            .node_declarations()
            .map(|(_, d)| {
                let kind = named(&d.namespace, &d.name);
                (
                    kind.name.clone(),
                    ManifestNodeKind {
                        kind,
                        role: structural_role(d.role).to_string(),
                        // Resolved to the card kind's NAME, not its id: the id
                        // means nothing to a consumer, and a name joins to
                        // `card_kinds` directly.
                        card: match d.card {
                            CardPolicy::None => None,
                            CardPolicy::Card(id) => vocabulary.card_name(id),
                        },
                    },
                )
            })
            .collect(),
    );

    let edge_kinds = by_name(
        vocabulary
            .edge_declarations()
            .map(|(_, d)| {
                let kind = named(&d.namespace, &d.name);
                (
                    kind.name.clone(),
                    ManifestEdgeKind {
                        kind,
                        role: link_role(d.role).to_string(),
                    },
                )
            })
            .collect(),
    );

    let fact_kinds = by_name(
        vocabulary
            .fact_declarations()
            .map(|(_, d)| {
                let kind = named(&d.namespace, &d.name);
                (
                    kind.name.clone(),
                    ManifestFactKind {
                        kind,
                        cardinality: cardinality(d.cardinality).to_string(),
                    },
                )
            })
            .collect(),
    );

    let card_kinds = by_name(
        vocabulary
            .card_declarations()
            .map(|(_, d)| {
                let kind = named(&d.namespace, &d.name);
                (kind.name.clone(), ManifestCardKind { kind })
            })
            .collect(),
    );

    Some(ManifestVocabulary {
        digest: crate::graph::DIGEST_NAME.to_string(),
        node_kinds,
        edge_kinds,
        fact_kinds,
        card_kinds,
    })
}

/// Wire spelling of a structural role. Closed, core-owned, snake_case like every
/// other enumerated string in this artifact.
fn structural_role(role: StructuralRole) -> &'static str {
    match role {
        StructuralRole::Container => "container",
        StructuralRole::Leaf => "leaf",
        StructuralRole::Definition => "definition",
        StructuralRole::Package => "package",
        StructuralRole::External => "external",
        StructuralRole::Opaque => "opaque",
    }
}

/// Wire spelling of a link role. See [`structural_role`].
fn link_role(role: LinkRole) -> &'static str {
    match role {
        LinkRole::Containment => "containment",
        LinkRole::Declaration => "declaration",
        LinkRole::Dependency => "dependency",
        LinkRole::Opaque => "opaque",
    }
}

/// Wire spelling of a cardinality — the one declared property a consumer needs
/// in order to read a fact list correctly (R7).
fn cardinality(c: Cardinality) -> &'static str {
    match c {
        Cardinality::Additive => "additive",
        Cardinality::Scalar => "scalar",
    }
}

fn fact(f: &Fact, strings: &mut Interner, vocabulary: &Vocabulary) -> ManifestFact {
    ManifestFact {
        kind: fact_kind(f, vocabulary),
        value: fact_value(&f.value),
        provenance: provenance(&f.provenance, strings),
        dissent: f.dissent.iter().map(|c| claim(c, strings)).collect(),
    }
}

fn claim(c: &Claim, strings: &mut Interner) -> ManifestClaim {
    ManifestClaim {
        value: fact_value(&c.value),
        provenance: provenance(&c.provenance, strings),
    }
}

/// A claim on an EDGE — the losing assertions an edge keeps as dissent. Its
/// provenance reads the derived rung exactly as the edge's own does.
fn edge_claim(c: &Claim, strings: &mut Interner) -> ManifestClaim {
    ManifestClaim {
        value: fact_value(&c.value),
        provenance: edge_provenance(&c.provenance, strings),
    }
}

/// The provenance of a fact: the rung as stored, which no fact carries, so a
/// fact's bytes are what they were before the rung existed — including a fact
/// an external index resolved, whose `resolution` already says so.
fn provenance(p: &Provenance, strings: &mut Interner) -> ManifestProvenance {
    provenance_with(p, p.confidence, strings)
}

/// The provenance of a relation: the rung [`Provenance::rung`] derives, so an
/// index-resolved edge reads `resolved_by_index` without that rung ever being
/// stored beside its `resolution` (KTD1).
fn edge_provenance(p: &Provenance, strings: &mut Interner) -> ManifestProvenance {
    provenance_with(p, p.rung(), strings)
}

fn provenance_with(
    p: &Provenance,
    rung: Option<Confidence>,
    strings: &mut Interner,
) -> ManifestProvenance {
    ManifestProvenance {
        file_id: strings.intern(&p.file),
        line: p.span.start(),
        end_line: p.span.range_end(),
        extractor: p.extractor.to_string(),
        tier: tier(p.tier).to_string(),
        resolution: resolution(p.resolution).to_string(),
        ambiguous_candidates: p.ambiguous_candidates,
        confidence: rung.map(|r| confidence(r).to_string()),
        rule_id: p.rule.map(|r| strings.intern(r.id())),
    }
}

fn fact_value(v: &FactValue) -> Value {
    match v {
        FactValue::Text(s) => Value::String(s.clone()),
        FactValue::Int(n) => Value::Number((*n).into()),
        FactValue::Bool(b) => Value::Bool(*b),
    }
}

/// The wire spelling of a fact kind — the shared one. The table used to live
/// here; U7 needed the same words on the cache path and moved it to
/// [`crate::graph::fact_word`] rather than write a second copy.
fn fact_kind(f: &Fact, vocabulary: &Vocabulary) -> String {
    crate::graph::fact_word(f.kind, vocabulary).into_owned()
}

/// The wire spelling of a node kind — the shared one, so the manifest and a
/// card can never name the same thing two ways (see [`crate::graph::node_word`]).
fn node_kind(k: NodeKind, vocabulary: &Vocabulary) -> Cow<'static, str> {
    crate::graph::node_word(k, vocabulary)
}

/// The wire spelling of an edge kind. See [`node_kind`].
fn edge_kind(k: EdgeKind, vocabulary: &Vocabulary) -> Cow<'static, str> {
    crate::graph::edge_word(k, vocabulary)
}

fn tier(t: Tier) -> &'static str {
    match t {
        Tier::Measured => "measured",
        Tier::Derived => "derived",
        Tier::Narrated => "narrated",
    }
}

/// The precision axis on the wire — a sibling of [`tier`], never derived from it
/// (KTD1). Spelled out here rather than via `serde` on [`Resolution`] so the wire
/// vocabulary is owned by the manifest module, exactly like `tier`/`node_kind`.
fn resolution(r: Resolution) -> &'static str {
    match r {
        Resolution::Heuristic => "heuristic",
        Resolution::Resolved => "resolved",
    }
}

/// The confidence rung on the wire (v9, R2) — a closed vocabulary owned here,
/// like [`tier`] and [`resolution`], and in the same snake_case as every other
/// word the manifest writes. The rule beside it is an id, not a word, so it
/// keeps the spelling its owner gave it ([`crate::graph::Rule::id`]).
pub(crate) fn confidence(c: Confidence) -> &'static str {
    match c {
        Confidence::ResolvedByIndex => "resolved_by_index",
        Confidence::RulesBacked => "rules_backed",
        Confidence::Ambiguous => "ambiguous",
        Confidence::NameOnly => "name_only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lens::Lens;

    fn lens_with_failure_modes(failure_modes: Vec<String>) -> Lens {
        Lens {
            view_id: "overview".to_string(),
            title: "Overview".to_string(),
            guiding_question: "?".to_string(),
            problem: "p".to_string(),
            mechanism: "m".to_string(),
            sections: vec![crate::lens::Section::undivided(Vec::new())],
            diagram: None,
            failure_modes,
        }
    }

    #[test]
    fn chunks_track_the_conditional_failure_modes_section() {
        let without = chunks_for(&lens_with_failure_modes(Vec::new()));
        assert_eq!(
            without
                .iter()
                .map(|c| c.heading.as_str())
                .collect::<Vec<_>>(),
            ["Problem", "Mechanism", "In this repository"]
        );

        let with = chunks_for(&lens_with_failure_modes(vec!["smell".to_string()]));
        assert_eq!(
            with.iter().map(|c| c.heading.as_str()).collect::<Vec<_>>(),
            [
                "Problem",
                "Mechanism",
                "In this repository",
                "Failure modes"
            ]
        );
        assert_eq!(with[3].id, "overview#failure-modes");
    }

    /// The wire vocabulary is owned here, and U3's two relation kinds join it
    /// additively — exactly as `has_member` did, so no consumer of an older
    /// manifest is invalidated by THEM.
    #[test]
    fn type_relations_have_snake_case_wire_names() {
        let v = Vocabulary::default();
        assert_eq!(edge_kind(EdgeKind::Extends, &v), "extends");
        assert_eq!(edge_kind(EdgeKind::Implements, &v), "implements");
        // The version is at 10 for unrelated reasons documented on
        // SCHEMA_VERSION itself (interning at v5, ambiguous_candidates at v6,
        // cards and pages as documents at v8, the confidence rung at v9, the
        // database accesses at v10) — this assertion just pins that no one
        // moved the constant back down.
        assert_eq!(SCHEMA_VERSION, 10);
    }
}
