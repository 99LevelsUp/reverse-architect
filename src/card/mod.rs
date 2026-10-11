//! Entity-centric cards (U8, KTD4).
//!
//! The existing pipeline is view-centric: one document per view over the whole
//! graph ([`crate::lens::compose`] + [`crate::render`]). A card is the other
//! projection — one document per *entity* — and it is built here, beside that
//! pipeline rather than inside it, so the views and their snapshots are
//! untouched by construction (KTD4).
//!
//! Three card kinds, per R8 and R13:
//!
//! - a **type card** for every Symbol node whose `SymbolKind` names a type,
//! - a **file card** for every File node that declares no type,
//! - a **module card** for every Module node.
//!
//! A member (a method, a property, a field) gets no card of its own: it is a row
//! in its owner's card (R9). A nested type is both — a member row in the owner
//! and a card in its own right — because it is a type.
//!
//! ## Reading the graph, not the keys
//!
//! Everything a card *displays* is read from facts: [`FactKind::SymbolName`] for
//! the title, [`FactKind::SymbolKind`] for the kind, and so on. No projection
//! parses a node key apart to recover readable content (KTD-B1).
//!
//! The card's *path* is the one exception the product contract asks for: R14 and
//! KTD3 require the location to derive from the symbol's identity, so that moving
//! a type between files leaves its card — and every link to it — where it was.
//! [`card_path`] therefore takes the identity as its input, but treats it as a
//! string: the transformation is total and textual (separators become path
//! separators, illegal characters are folded) and never interprets a segment.

pub mod detail;
pub mod documents;
pub mod explain;
pub mod header;
pub mod index;
pub mod insight;
pub mod keywords;
pub mod layer2;
/// The second layer on a type's card (plan 2a, U6) — the file lives beside
/// the analyses it renders.
#[path = "layer2/render.rs"]
pub mod layer2_render;
pub mod links;
pub mod map;
pub mod pages;
pub mod purpose;
pub mod render;
pub mod story;
/// The registry of triggers, `cards/triggers.md` (plan 2b, U7), and its line
/// of coverage.
pub mod triggers;

use crate::coverage::CardRows;
use crate::extract::treesitter::{symbol_role, Role};
use crate::graph::Vocabulary;
use crate::graph::{
    CardPolicy, Confidence, EdgeKind, FactKind, FactValue, Graph, Node, NodeKind, Provenance,
    Resolution, Rule, SiteId, Span, StructuralRole, Tier,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// What entity a card documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Type,
    File,
    Module,
    /// A card kind this corpus declared for itself (R1).
    Declared(crate::graph::CardKindId),
}

impl CardKind {
    /// The card tree this kind lives under. Three disjoint namespaces, so a
    /// module `src` and a file `src` can never contend for one path.
    fn dir(self, vocabulary: &Vocabulary) -> Cow<'static, str> {
        match self {
            CardKind::Type => "type".into(),
            CardKind::File => "file".into(),
            CardKind::Module => "module".into(),
            // `declared/<namespace>/<name>`, each half sanitized the same way a
            // path segment is. Keeping the namespace is what stops two plugins'
            // `table` cards from contending for one path — the same reason the
            // three core kinds have separate trees (R15). `:` is not a legal
            // Windows path character, so the qualified name cannot be used raw.
            CardKind::Declared(id) => {
                let name = vocabulary
                    .card_name(id)
                    .unwrap_or_else(|| crate::graph::undeclared_name(id.get()));
                let (namespace, local) = name.split_once(':').unwrap_or(("declared", &name));
                let (namespace, local) = (sanitize_segment(namespace), sanitize_segment(local));
                // A name the vocabulary accepts can still sanitize to nothing:
                // `.` and `..` survive `check_name` and are stripped here by the
                // trailing-dot rule. (A run of spaces cannot arrive any more —
                // `check_name` refuses an untrimmed half since RG3 — and it would
                // fold to underscores rather than vanish in any case; the earlier
                // wording here said "trimmed away", which was never what the
                // code did.) An empty half would put an empty segment in the
                // path, and `links::relative` counts path components — so every
                // relative link on that card would climb one directory too far.
                // Fall back to the digest, the same answer `card_path` already
                // gives an identity that sanitizes to nothing.
                let dir = format!("declared/{namespace}/{local}");
                // Two ways to overflow the budget and one answer to both: a half
                // that sanitized to nothing, and a pair that together are simply
                // too long (RG3). The digest is bounded by construction, so the
                // fallback cannot itself overflow.
                if namespace.is_empty() || local.is_empty() || dir.len() > MAX_DECLARED_DIR {
                    Cow::Owned(format!(
                        "declared/_hashed/{}",
                        crate::graph::fnv1a64_hex(&name)
                    ))
                } else {
                    Cow::Owned(dir)
                }
            }
        }
    }
}

/// A place in the source: the anchor a card statement is answerable to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub file: String,
    /// Where in the file, as one value — see [`crate::graph::Span`]. A start
    /// beside an independent end could name places that do not exist, and
    /// `anchor` used to swallow them rather than refuse them.
    pub span: Span,
}

impl SourceRef {
    fn from(prov: &Provenance) -> Self {
        SourceRef {
            file: prov.file.clone(),
            span: prov.span,
        }
    }

    /// `path/to/File.cs:12-88`, `path:12`, or the bare path — whatever the
    /// provenance actually knows, never an invented end.
    ///
    /// Three spans, three spellings, no degradation branch: the shapes this used
    /// to fall back from cannot reach it any more.
    pub fn anchor(&self) -> String {
        match self.span {
            Span::Whole => self.file.clone(),
            Span::Line(line) => format!("{}:{line}", self.file),
            Span::Range { start, end } => format!("{}:{start}-{end}", self.file),
        }
    }
}

/// One member row inside a type card (R9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub name: String,
    pub member_kind: Option<String>,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub source: Option<SourceRef>,
    /// The member's own identity — U9 links member rows against it.
    pub identity: String,
}

/// One relation leaving or entering the carded entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: EdgeKind,
    /// The reader-facing word for `kind`, resolved while the vocabulary is
    /// still in reach (KTD4). The renderer receives a `Card` and nothing else,
    /// so a declared kind could not be named there.
    pub kind_word: Cow<'static, str>,
    /// Readable label, read from the target's facts (never parsed out of its key).
    pub label: String,
    /// The target's identity — the link target U9 resolves.
    pub identity: String,
    pub resolution: Resolution,
    pub source: Option<SourceRef>,
    /// The member this relation actually leaves from or arrives at, when it is not
    /// the carded entity itself.
    ///
    /// A call leaves a METHOD, not the type around it (KTD5), and a method owns no
    /// card. Without lifting those relations onto the owning type's card, a type
    /// card would say nothing about what the type uses and nothing about who uses
    /// it — R9 and R11 would be satisfied only for types whose relations happen to
    /// sit on the type declaration itself, which is almost none of them.
    ///
    /// The member's name WITH its parameter list (`Solve(int)`), read from its
    /// identity (plan A, R18): two overloads are two members, and a bare name
    /// made a call to each read as the same row twice.
    pub via: Option<String>,
    /// The identity of the member `via` names — what tells two members apart
    /// when their readable names do not, and so part of the relation's
    /// identity on the card (plan A, KTD13).
    pub via_identity: Option<String>,
    /// How many repo-wide candidates the heuristic name match found for this
    /// relation, when more than one — `None` for an unambiguous or non-heuristic
    /// relation. Lets the render layer mark the relation as ambiguous right at
    /// the claim (R1), instead of only in a separate `## Notes` caveat.
    ///
    /// The count only: whether the relation IS ambiguous is [`Relation::rung`]'s
    /// to say (plan A, KTD13).
    pub ambiguous_candidates: Option<usize>,
    /// The rung the relation stands on ([`Provenance::rung`]): what decides
    /// whether it is certain, a name match nothing backed, or ambiguous (plan A,
    /// R1). `None` for a relation no name resolution produced — a structural
    /// edge, an import — which reads as it did before rungs existed.
    pub rung: Option<Confidence>,
    /// Every usage site the relation was read at, sorted (plan A, KTD7). A row
    /// is a relation, not a site: a caller that calls one member from three
    /// places is one row carrying three sites. Empty for a relation no usage
    /// site produced, which keeps its `source` anchor alone.
    pub sites: Vec<SiteId>,
}

/// One end of a "maybe also" claim, as a reader sees it and as a link resolves
/// it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mention {
    pub label: String,
    pub identity: String,
}

/// A relation the source MAY hold, kept out of the main lists (plan A, R7,
/// R19, KTD2): one usage site whose name matched several declarations that no
/// rule told apart, or whose candidate a rule disputed without the right to
/// remove it (R8), shown as ONE claim with its alternatives rather than as one
/// row per candidate.
///
/// Stated from the site's point of view — the source is the member the usage
/// sits in, the alternatives are what it may mean — so it reads caller to
/// callee whichever side of it this card is on (R18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaybeAlso {
    pub kind: EdgeKind,
    /// The reader-facing word for `kind`. See [`Relation::kind_word`].
    pub kind_word: Cow<'static, str>,
    /// Whether the claim points at this card (another entity's usage) rather
    /// than away from it (one of this card's own usages).
    pub incoming: bool,
    /// The certain end: the member, or the entity itself, the usage sits in.
    pub source: Mention,
    /// The declarations the name could mean and no rule settled, sorted. A
    /// candidate another site supports is a main-list relation, not one of
    /// these (plan A, U3).
    pub alternatives: Vec<Mention>,
    /// How many repo-wide candidates the name matched at the site — what "1 of
    /// N" counts. At least the number of alternatives.
    pub candidates: usize,
    /// Why the claim is only a guess (R7: "with its reason and the count").
    pub reason: MaybeReason,
    pub resolution: Resolution,
    /// The sites the claim was read at, sorted: one, or several when the same
    /// source makes the same unsettled claim in several places.
    pub sites: Vec<SiteId>,
    /// What `verify`'s model said about the claim's sites, when generation was
    /// asked to read its layer (plan A, U12). With [`ModelMark::Promoted`] the
    /// candidate the model named is the first alternative. `None` for a claim
    /// the layer says nothing about, and for every claim of a run without it.
    pub model: Option<ModelMark>,
}

/// What `verify`'s model said about one "maybe also" claim (plan A, U12,
/// KTD12). A mark on the claim and an order of its alternatives — never a
/// removal, and never a change of the relation's rung (R14, R16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelMark {
    /// The model named one of the claim's alternatives, now listed first.
    Promoted,
    /// The model named none of the claim's alternatives: it answered "none",
    /// or named a candidate of the site this claim does not list.
    Demoted,
}

/// The model's verdicts by usage site, as generation hands them to the card
/// layer (plan A, U12). Filled by [`crate::verify::apply_layer`] from the
/// layer records that still match the graph; empty for a run that did not
/// ask for the layer, which then builds exactly the cards it always did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelVerdicts {
    /// `(source, kind, site)` → each verdict given there. Several when two
    /// usages of different names share one site id — a line without columns.
    by_site: BTreeMap<(String, EdgeKind, SiteId), Vec<SiteVerdict>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SiteVerdict {
    /// The node keys the model chose among.
    candidates: BTreeSet<String>,
    /// The one it named, or `None` for "none".
    promoted: Option<String>,
}

/// A claim's mark and, for a promotion, the alternative to list first.
type Verified = (ModelMark, Option<String>);

impl ModelVerdicts {
    /// Record the model's verdict at one usage site: the usage sits in
    /// `source`, has edge kind `kind`, and chose among `candidates`.
    pub fn insert(
        &mut self,
        source: &str,
        kind: EdgeKind,
        site: SiteId,
        candidates: BTreeSet<String>,
        promoted: Option<String>,
    ) {
        self.by_site
            .entry((source.to_string(), kind, site))
            .or_default()
            .push(SiteVerdict {
                candidates,
                promoted,
            });
    }

    /// The verdict for one claim: the one given at its site whose candidates
    /// include every alternative the claim lists — the usage the claim is
    /// about, when usages of other names share the site id.
    fn mark(
        &self,
        source: &str,
        kind: EdgeKind,
        site: &SiteId,
        alternatives: &[Mention],
    ) -> Option<Verified> {
        // A run without the layer asks this of every claim; the lookup key
        // alone would cost it two allocations each time.
        if self.by_site.is_empty() {
            return None;
        }
        let verdicts = self
            .by_site
            .get(&(source.to_string(), kind, site.clone()))?;
        let verdict = verdicts.iter().find(|v| {
            alternatives
                .iter()
                .all(|a| v.candidates.contains(&a.identity))
        })?;
        Some(match &verdict.promoted {
            Some(target) if alternatives.iter().any(|a| &a.identity == target) => {
                (ModelMark::Promoted, Some(target.clone()))
            }
            _ => (ModelMark::Demoted, None),
        })
    }
}

/// Why a "maybe also" claim is a guess rather than a relation (plan A, R7),
/// read from the rule its edges name ([`Provenance::rule`]) and worded by the
/// render layer.
///
/// The rule on an ambiguous edge is the rule that disputed it, or
/// `name-match` when none did. Several candidates backed alike —
/// `solver.Solve(1)` on a declared `GeoSolver` with two `Solve` overloads — are
/// a tie, and the resolver names their edges `name-match`, not the backing
/// rule (`resolve::resolve_site`): a claim keeps only the alternatives no
/// other site settled, so a lone survivor of a tie would otherwise read as
/// disputed by the very rule that backed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaybeReason {
    /// A rule disputed the candidates without a right to remove them (R8):
    /// `arity`, `namespace-visibility`, `nested-type-visibility`,
    /// `role-mismatch`, or a receiver rule.
    Disputed(Rule),
    /// The external index paired the site and pointed at another declaration
    /// (plan A, U9) — `index-elsewhere` on the edge.
    IndexElsewhere,
    /// The external index paired the site with a symbol from outside the
    /// repository (plan A, U9) — `index-outside` on the edge.
    IndexOutside,
    /// Several declarations answer to the name and no rule picked one.
    SharedName,
}

impl MaybeReason {
    /// The reason of one claim, from the rules its alternatives' edges name.
    ///
    /// A dispute is named when every alternative names the same rule, and
    /// that rule disputed them:
    ///
    /// - `arity`, `namespace-visibility`, `nested-type-visibility`,
    ///   `role-mismatch`, `declared-receiver` and `static-receiver` name an
    ///   ambiguous edge only where they disputed it (a tie they backed names
    ///   `name-match`), so at any count they are the reason — the AE2 shape
    ///   among them;
    /// - `name-match` and the rules that only back (`implicit-this`,
    ///   `base-spelling`) name an ambiguous edge only when several candidates
    ///   remained;
    /// - `index-elsewhere` and `index-outside` are set only by the index
    ///   pairing, on a candidate the index refuted, so they are the index's
    ///   reason at any count.
    ///
    /// Anything else — alternatives naming different rules — is the shared
    /// name, which is true of every claim with more than one candidate.
    ///
    /// The edge is a relation, not a site: one that several sites name carries
    /// the rule of the site that ranked it (`graph::merge`), so for such a
    /// relation the reason is that site's.
    fn of(rules: &[Option<Rule>]) -> MaybeReason {
        let Some((first, rest)) = rules.split_first() else {
            return MaybeReason::SharedName;
        };
        if rest.iter().any(|r| r != first) {
            return MaybeReason::SharedName;
        }
        match first {
            Some(
                rule @ (Rule::Arity
                | Rule::NamespaceVisibility
                | Rule::NestedTypeVisibility
                | Rule::RoleMismatch
                | Rule::DeclaredReceiver
                | Rule::StaticReceiver),
            ) => MaybeReason::Disputed(*rule),
            Some(Rule::IndexElsewhere) => MaybeReason::IndexElsewhere,
            Some(Rule::IndexOutside) => MaybeReason::IndexOutside,
            _ => MaybeReason::SharedName,
        }
    }
}

/// The paired test a card names, and the tier that pairing sits on.
///
/// The one thing a card shows that is not a direct reading of the source. The
/// pairing is CONCLUDED from how files are named, never seen at a place in the
/// source, which is why it carries `Tier::Derived` and has no anchor to offer.
/// Everything else on a card is `Measured`, so this is the single spot where the
/// trust axis says something a reader could not otherwise guess — and the card
/// has to say it, or a conclusion reads as an observation.
///
/// The tier is READ from the fact rather than spelled here, so the card stays
/// honest if a future extractor ever measures the pairing instead of inferring
/// it. A cross-model review settled why the
/// card carries the tier only here and not yet on every displayed fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPairing {
    /// Verbatim from [`FactKind::TestByConvention`].
    pub statement: String,
    pub tier: Tier,
    /// The file the pairing was measured on, when that is NOT the carded entity.
    ///
    /// The heuristic pairs FILES. A type card reads the statement through its
    /// declaring file, and a file declaring three types would otherwise hand the
    /// same sentence to all three as if it were about each of them — a claim
    /// about a file, presented as a claim about a type. Naming the file is the
    /// same move `Relation::via` makes for a lifted relation.
    pub from_file: Option<String>,
}

/// A doc comment quoted from the source (R10), after the R22 redaction filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocQuote {
    /// The comment text as the source wrote it — language, diacritics and line
    /// breaks untouched — except for the lines the R22 filter replaced wholesale
    /// (see [`redact_doc`]). That is any line whose own SHAPE matched a secret
    /// pattern, AND the lines a matched line pulls with it: the body of a key
    /// block, and the value on the line below a bare `password:` label. Those
    /// carry the secret while matching nothing themselves.
    pub text: String,
    pub source: SourceRef,
    /// True when at least one line was replaced, so the card can say so instead
    /// of quietly presenting a hole as the original text.
    pub redacted: bool,
}

/// One honest caveat on a card: what the core withheld or could not resolve
/// (R22, R23), and which kind of admission it is.
///
/// The kind is kept beside the sentence because it decides where the caveat
/// is shown (plan B, card trim after U9): a caveat sits beside what it
/// qualifies. On a type's short card that is only the card itself — a plugin
/// that failed on the file — and, as one count, the names past the ambiguity
/// ceiling, which qualify the card's role sentences; everything else qualifies
/// a quoted comment, a relation or a text, which since the short card are the
/// detail's, so the caveat is the detail's too. Every other card kind keeps
/// its lists and so all its caveats.
///
/// Ordered by `text` first, so a sorted list reads in the order the plain
/// sentences always did.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Caveat {
    pub text: String,
    pub kind: CaveatKind,
}

/// What a [`Caveat`] admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaveatKind {
    /// A doc-comment line the secret filter replaced (R22).
    RedactedDoc,
    /// A literal whose value the secret filter withheld (R22).
    RedactedLiteral,
    /// A name several declarations answer, below the ambiguity ceiling (R23):
    /// its relations are "maybe also" claims.
    SharedName,
    /// A name past the ambiguity ceiling (R23): no relation is shown for it.
    OverCeiling,
    /// The plugin failed on the file, so the card may lack what it declares.
    ExtractionFailed,
}

impl CaveatKind {
    /// Whether a type's short card shows this caveat itself rather than leave
    /// it to the detail. Only a failed plugin: it says the card as a whole
    /// may be missing what the file declares. A name past the ceiling is
    /// counted on the card instead of listed ([`render::render_card`]).
    pub fn on_short_card(self) -> bool {
        self == CaveatKind::ExtractionFailed
    }
}

/// One card: an entity, its verified facts, and where each came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// The entity's node key, verbatim.
    pub identity: String,
    /// Corpus-relative output path, derived from `identity` per R14.
    pub path: String,
    pub kind: CardKind,
    /// The reader-facing word for `kind`. See [`Relation::kind_word`].
    pub kind_word: Cow<'static, str>,
    pub title: String,
    pub entity_kind: Option<String>,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    /// A type's header read from the signatures of all its parts (plan B, U1:
    /// R4, KTD6) — `None` on every other card, and on a type whose signature
    /// the closed reading in [`header`] does not accept, where the header
    /// falls back to `visibility` and `entity_kind`.
    pub header: Option<header::TypeHeader>,
    pub source: Option<SourceRef>,
    pub doc: Option<DocQuote>,
    pub members: Vec<MemberRow>,
    pub outgoing: Vec<Relation>,
    pub incoming: Vec<Relation>,
    /// `Uses` edges into [`NodeKind::External`] — kept apart from `outgoing`
    /// because R9 asks what the entity reaches OUTSIDE the repository as its own
    /// question.
    pub externals: Vec<Relation>,
    /// The ambiguous relations in either direction, one claim per unsettled
    /// site (plan A, R7). Neither `outgoing` nor `incoming` holds an ambiguous
    /// row; a name match nothing backed or disputed stays in those lists.
    pub maybe_also: Vec<MaybeAlso>,
    /// The paired test, and the axis the pairing was reached on.
    pub test: Option<TestPairing>,
    /// Honest caveats: ambiguity admissions (R23) and redaction notes (R22),
    /// each with the kind that decides where it is shown ([`Caveat`]).
    pub caveats: Vec<Caveat>,
    /// What the evidence above adds up to — summary, collaborators, offered
    /// surface, tests (see [`story`]).
    pub story: story::Story,
    /// The place-level facts only the detail shows (plan B, U8): anchors,
    /// database accesses per member, and — on an enum — where its values
    /// decide. Empty on a module card and a declared one.
    pub navigators: detail::Navigators,
    /// Where this card's detail lives, when it has one (plan B, U8: R3,
    /// KTD7) — set by `assign_paths`, which alone decides that a detail
    /// exists ([`detail::wants_detail`]) and checks its path for collisions
    /// together with every card's.
    pub detail: Option<String>,
    /// Where this module's map lives (plan B, U10: R1, KTD7) — set by
    /// `assign_paths` on a module card whose module holds at least one type
    /// card, and on no other card.
    pub map: Option<String>,
    /// Whether every part of this type sits in a generated file (plan B,
    /// U10): one its name marks as designer or generator output
    /// ([`pages::is_generated_path`]), or one opening with an
    /// `<auto-generated>` comment. Its module's map counts such a type
    /// instead of giving it a row. Always `false` on any other card.
    pub generated: bool,
}

impl Card {
    /// The kind as the card's header states it, in the fact vocabulary: a
    /// type's as its signature reads ([`header`], AE3: `public static
    /// class`), else its stated visibility and kind; `file`, `module`, or the
    /// declared kind's word. One answer for the card's strip and the
    /// manifest's header (plan B, R4), so the two cannot disagree.
    pub fn kind_phrase(&self) -> String {
        match self.kind {
            CardKind::Type => match &self.header {
                Some(header) => header.text(),
                None => {
                    let kind = self.entity_kind.clone().unwrap_or_else(|| "type".into());
                    match &self.visibility {
                        Some(v) => format!("{v} {kind}"),
                        None => kind,
                    }
                }
            },
            CardKind::File => "file".into(),
            CardKind::Module => "module".into(),
            CardKind::Declared(_) => self.kind_word.to_string(),
        }
    }
}

/// Longest corpus-relative card path we emit before falling back to a hashed
/// name.
///
/// Windows resolves a path against a 260-character limit that includes the output
/// directory, which this layer does not know. 120 leaves a workable prefix budget
/// (`C:\Users\<user>\...\<repo>.reverse-architect\` is comfortably under 140)
/// without shortening the ordinary case: a card path is normally well under half
/// of it.
const MAX_REL_PATH: usize = 120;

/// Longest, **in bytes**, a single sanitized segment may be inside a hashed
/// fallback name.
///
/// Bytes, because every other number in this budget is bytes: `path.len()` is
/// what `card_path` compares against [`MAX_REL_PATH`], and [`MAX_DECLARED_DIR`]
/// is subtracted from the same total. Truncating by CHARACTERS here — which is
/// what this did until RG3 — let a multi-byte identity spend up to four bytes per
/// counted character and put the fallback back over the budget it exists to
/// enforce.
const TRUNCATED_SEGMENT: usize = 40;

/// Longest the directory of a declared card kind may be before it, too, falls
/// back to a digest.
///
/// [`MAX_REL_PATH`] is enforced by `card_path` falling back to `hashed_path` —
/// but `hashed_path` composes `cards/<dir>/_hashed/<stub>-<digest>.md`, and for a
/// declared kind `<dir>` is `declared/<namespace>/<name>`, whose length the
/// plugin chooses. RG3 measured the consequence: a hundred-character namespace
/// produced a 157-character path THROUGH the fallback, so the bound was not a
/// bound at all. The three core kinds cannot reach this — their directories are
/// four to six characters of compile-time constant.
///
/// 45 is what is left of the 120-character budget after everything `hashed_path`
/// adds: `cards/` (6) + `/_hashed/` (9) + a stub of at most
/// [`TRUNCATED_SEGMENT`] (40) + `-` (1) + sixteen hex digits + `.md` (3) = 75.
/// Truncating the two halves instead would not work: 40 + 40 plus `declared/`
/// and two separators is still over.
const MAX_DECLARED_DIR: usize = 45;

/// One edge, on its way from the graph to a [`Relation`]: the neighbor key,
/// the edge kind, and the independent axes its [`Provenance`] carries
/// through this pass — precision (`resolution`), how many repo-wide
/// candidates a heuristic name match found (`ambiguous_candidates`, when
/// more than one), the rung it stands on (`rung`), and where in the source it
/// was asserted (`anchor`). A
/// named struct rather than a growing positional tuple: field access can't
/// silently pick up the wrong slot the way a wildcard-count mismatch in a
/// `(_, _, _, _, _)` pattern could.
struct EdgeRow<'a> {
    neighbor: &'a str,
    kind: EdgeKind,
    resolution: Resolution,
    ambiguous_candidates: Option<usize>,
    rung: Option<Confidence>,
    /// The rule that gave the edge its rung ([`Provenance::rule`]) — what a
    /// "maybe also" claim reads its reason from.
    rule: Option<Rule>,
    anchor: SourceRef,
    sites: &'a [SiteId],
}

/// Every ambiguous row of the graph, read once for the "maybe also" claims
/// ([`maybe_also`]), and where each one stands: by `(source, kind, site)` —
/// `None` for a row with no site — in the order the source's own rows list
/// them.
///
/// Built once for the whole card set, because the same rows are read over and
/// over. Each claim used to scan its source's entire outgoing list, test every
/// row's site list for the claim's site, and build each matching row's whole
/// relation — its every site copied — to read its label. A member with
/// hundreds of unsettled calls of one name made that quadratic per member, and
/// it was repeated on every card the member's claims appear on: over the
/// reference repository, most of the card phase.
struct AmbiguousRows<'a> {
    rows: Vec<AmbiguousRow<'a>>,
    at: HashMap<(&'a str, EdgeKind, Option<&'a SiteId>), Vec<usize>>,
}

/// One ambiguous row as a claim lists it.
struct AmbiguousRow<'a> {
    row: &'a EdgeRow<'a>,
    /// The name the alternative answers to: its `SymbolName`, or its key.
    name: &'a str,
    /// What the alternative is listed as ([`neighbor_label`]).
    label: String,
}

impl<'a> AmbiguousRows<'a> {
    fn build(
        out_edges: &'a BTreeMap<&'a str, Vec<EdgeRow<'a>>>,
        nodes: &BTreeMap<&'a str, &'a Node>,
    ) -> AmbiguousRows<'a> {
        let mut index = AmbiguousRows {
            rows: Vec::new(),
            at: HashMap::new(),
        };
        for (source, rows) in out_edges {
            for row in rows {
                if row.rung != Some(Confidence::Ambiguous) {
                    continue;
                }
                let target = nodes.get(row.neighbor).copied();
                let i = index.rows.len();
                index.rows.push(AmbiguousRow {
                    row,
                    name: target
                        .and_then(|t| fact_text(t, FactKind::SymbolName))
                        .unwrap_or(row.neighbor),
                    label: neighbor_label(row.neighbor, target),
                });
                if row.sites.is_empty() {
                    index
                        .at
                        .entry((source, row.kind, None))
                        .or_default()
                        .push(i);
                }
                for site in row.sites {
                    let at = index.at.entry((source, row.kind, Some(site))).or_default();
                    // A row lists a site once as the edge keeps it; should it
                    // ever repeat one, the row is still one alternative there,
                    // as the scan this replaces found it once.
                    if at.last() != Some(&i) {
                        at.push(i);
                    }
                }
            }
        }
        index
    }

    /// The ambiguous rows `source` has of `kind` at `site`.
    fn at<'s>(
        &'s self,
        source: &'s str,
        kind: EdgeKind,
        site: Option<&'s SiteId>,
    ) -> impl Iterator<Item = &'s AmbiguousRow<'a>> + 's {
        self.at
            .get(&(source, kind, site))
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&i| &self.rows[i])
    }
}

/// Build every card the graph supports, in deterministic identity order.
///
/// One linear pass builds the indexes, one pass builds the cards, one pass
/// resolves path collisions — so the result depends on the graph's content, never
/// on insertion order (R18).
pub fn build_cards(graph: &Graph) -> Vec<Card> {
    build_cards_with_insight(graph).0
}

/// [`build_cards`], also handing back the aggregation the cards were built
/// from, so the architecture pages read the same numbers the cards show, and
/// the card noise counted while building them (plan A, U4: R21, KTD14), for
/// `coverage.json` — see [`CardRows`] for what is counted.
pub fn build_cards_with_insight(graph: &Graph) -> (Vec<Card>, insight::Insight, CardRows) {
    build_cards_with_verdicts(graph, &ModelVerdicts::default())
}

/// [`build_cards_with_insight`], with the model's verdicts applied to the
/// "maybe also" claims they speak about (plan A, U12). Every other part of a
/// card is built exactly as without them.
pub fn build_cards_with_verdicts(
    graph: &Graph,
    verdicts: &ModelVerdicts,
) -> (Vec<Card>, insight::Insight, CardRows) {
    let vocabulary = graph.vocabulary();
    let nodes: BTreeMap<&str, &Node> = graph
        .nodes_sorted()
        .into_iter()
        .map(|n| (n.key.as_str(), n))
        .collect();

    let mut out_edges: BTreeMap<&str, Vec<EdgeRow>> = BTreeMap::new();
    let mut in_edges: BTreeMap<&str, Vec<EdgeRow>> = BTreeMap::new();
    // Every `HasMember` target, graph-wide (KTD3, U1): a symbol claimed by a
    // type, regardless of which file declares the type or the member.
    // Folded into this same pass over `graph.edges_sorted()` rather than a
    // second traversal of `out_edges` — the `HasMember` edges are already in
    // hand here. A plain lookup set, never iterated in an order-sensitive
    // way (output order comes from iterating `nodes`, a `BTreeMap`, below),
    // so `HashSet` over `BTreeSet` costs nothing in determinism.
    let mut claimed: HashSet<&str> = HashSet::new();
    for e in graph.edges_sorted() {
        if e.edge.kind == EdgeKind::HasMember {
            claimed.insert(e.to.as_str());
        }
        let anchor = SourceRef::from(&e.edge.provenance);
        out_edges.entry(e.from.as_str()).or_default().push(EdgeRow {
            neighbor: e.to.as_str(),
            kind: e.edge.kind,
            resolution: e.edge.provenance.resolution,
            ambiguous_candidates: e.edge.provenance.ambiguous_candidates,
            rung: e.edge.provenance.rung(),
            rule: e.edge.provenance.rule,
            anchor: anchor.clone(),
            sites: &e.edge.sites,
        });
        in_edges.entry(e.to.as_str()).or_default().push(EdgeRow {
            neighbor: e.from.as_str(),
            kind: e.edge.kind,
            resolution: e.edge.provenance.resolution,
            ambiguous_candidates: e.edge.provenance.ambiguous_candidates,
            rung: e.edge.provenance.rung(),
            rule: e.edge.provenance.rule,
            anchor,
            sites: &e.edge.sites,
        });
    }

    let ambiguous = AmbiguousRows::build(&out_edges, &nodes);

    // Every file gets a card. Until the explaining layer, a file that declared
    // only types had none, because its types' cards already held the facts; but
    // a mention of such a file then rendered as a dead-end code span, and a
    // module's contents could not be walked. A file card is now a page of its
    // own — what the file declares, how large it is, what it imports — and it
    // still never re-lists a type's members, which stay on the type's card.
    let insight = insight::Insight::build(graph);
    let mut file_types: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for unit in insight.type_units() {
        if let Some(file) = insight.file_of(unit) {
            file_types
                .entry(file.to_string())
                .or_default()
                .insert(unit.to_string());
        }
    }
    // What each member reaches, at unit grain and on undisputed relations only:
    // the API list says "uses X" per method, and a guess must not read as that.
    // A name match nothing backed still stands (plan A, KTD2) — only the
    // ambiguous rung is a guess.
    let mut member_reach: std::collections::HashMap<&str, std::collections::BTreeSet<&str>> =
        std::collections::HashMap::new();
    for e in graph.edges_sorted() {
        if !matches!(
            e.edge.kind,
            EdgeKind::Calls | EdgeKind::References | EdgeKind::Instantiates
        ) || e.edge.provenance.rung() == Some(Confidence::Ambiguous)
        {
            continue;
        }
        if let Some(unit) = insight.unit_of(e.to.as_str()) {
            member_reach
                .entry(e.from.as_str())
                .or_default()
                .insert(unit);
        }
    }
    let cx = story::Context {
        insight: &insight,
        nodes: &nodes,
        member_reach: &member_reach,
        file_types: &file_types,
    };

    // The place-level facts of every unit, and every enum's decisions, read in
    // one pass over the nodes (plan B, U8). Each card takes its own share, so
    // no card scans the graph again.
    let mut navigators = detail::NavigatorIndex::build(&nodes, &insight);
    // The files whose own comments say a tool wrote them (plan B, U10), read
    // once for the whole set.
    let marked = map::marked_generated(&nodes);

    let mut cards: Vec<Card> = Vec::new();
    let mut card_rows = CardRows::default();
    for node in nodes.values() {
        let kind = match node.kind {
            // Both kinds whose claim to a card is a property of the node itself
            // rather than of the tree around it: a symbol that is a type, and a
            // declared kind whose declaration says `CardPolicy::Card` (R8).
            // One function answers for both, so this arm and the file rule above
            // can never drift into disagreeing about who owns the source.
            NodeKind::Symbol | NodeKind::Declared(_) => match own_card(Some(node), vocabulary) {
                Some(kind) => kind,
                // A symbol that is not a type, or a declared kind whose
                // declaration asked for no card: real, and deliberately
                // undocumented on a card of its own (R4, KTD1, R8).
                None => continue,
            },
            NodeKind::File => CardKind::File,
            NodeKind::Module => CardKind::Module,
            // A dependency and an external are named on the cards that use them.
            NodeKind::Dependency | NodeKind::External => continue,
        };
        let mut card = build_card(
            kind,
            node,
            vocabulary,
            &nodes,
            &out_edges,
            &in_edges,
            &ambiguous,
            &claimed,
            &cx,
            verdicts,
            &mut card_rows,
        );
        if matches!(kind, CardKind::Type | CardKind::File) {
            card.navigators = navigators.take(kind, &card.identity);
        }
        if kind == CardKind::Type {
            card.generated = map::generated_type(node, &marked);
        }
        cards.push(card);
    }

    assign_paths(&mut cards, graph.vocabulary());
    drop(member_reach);
    (cards, insight, card_rows)
}

/// The card a node earns in its own right, if any — the single answer to "does
/// this entity own its documentation, or does someone else's card hold it?".
///
/// Two ways to earn one, and no third. A core `Symbol` earns a `Type` card when
/// its emitted `SymbolKind` reads as a type through the same table the extractor
/// used, so the two can never disagree. A `Declared` kind earns one when its
/// declaration says so (R8) — and under the card kind the declaration named, not
/// under a core kind it would have to pretend to be.
///
/// A `File` and a `Module` earn cards too, but on a different question: theirs
/// depends on the tree around them (which declarations they hold), not on the
/// node itself. That is why `build_cards` asks this one first and those two
/// separately, and why this function must not try to answer for them.
fn own_card(node: Option<&Node>, vocabulary: &Vocabulary) -> Option<CardKind> {
    let node = node?;
    match node.kind {
        NodeKind::Symbol => (fact_text(node, FactKind::SymbolKind).and_then(symbol_role)
            == Some(Role::Type))
        .then_some(CardKind::Type),
        NodeKind::Declared(id) => match vocabulary.node(id)?.card {
            CardPolicy::Card(card) => Some(CardKind::Declared(card)),
            CardPolicy::None => None,
        },
        NodeKind::File | NodeKind::Module | NodeKind::Dependency | NodeKind::External => None,
    }
}

// The seven indexes are built once in uild_cards and borrowed here; bundling
// them into a struct would only rename the same borrows.
#[allow(clippy::too_many_arguments)]
fn build_card(
    kind: CardKind,
    node: &Node,
    vocabulary: &Vocabulary,
    nodes: &BTreeMap<&str, &Node>,
    out_edges: &BTreeMap<&str, Vec<EdgeRow>>,
    in_edges: &BTreeMap<&str, Vec<EdgeRow>>,
    ambiguous: &AmbiguousRows,
    claimed: &HashSet<&str>,
    cx: &story::Context,
    verdicts: &ModelVerdicts,
    card_rows: &mut CardRows,
) -> Card {
    let key = node.key.as_str();
    let mut caveats: Vec<Caveat> = Vec::new();

    let title = match kind {
        // A File/Module key IS its repo-relative path, which is what a reader
        // wants to see; a Symbol's readable name is a fact.
        CardKind::File | CardKind::Module => key.to_string(),
        CardKind::Type => fact_text(node, FactKind::SymbolName)
            .unwrap_or(key)
            .to_string(),
        // A declared card kind has no core notion of a readable name, and
        // guessing one from a fact would be the core reading meaning into a
        // domain it does not know. The key is what the plugin chose to call it.
        CardKind::Declared(_) => key.to_string(),
    };

    let source = fact_prov(node, FactKind::SymbolName)
        .or_else(|| fact_prov(node, FactKind::SymbolKind))
        .map(SourceRef::from)
        .or_else(|| match kind {
            CardKind::File => Some(SourceRef {
                file: key.to_string(),
                span: Span::Whole,
            }),
            // A file's key IS a path, so it can stand in for a source anchor. No
            // other card kind's key is one: a type without a `SymbolName` fact
            // has no location to point at, a module is a directory rather than a
            // file, and a declared kind's key means whatever the plugin chose —
            // the core must not read it as a path it could then open.
            CardKind::Type | CardKind::Module | CardKind::Declared(_) => None,
        });

    let doc = fact_text(node, FactKind::DocComment).and_then(|text| {
        let prov = fact_prov(node, FactKind::DocComment)?;
        let (text, redacted) = redact_doc(text);
        if redacted {
            caveats.push(Caveat {
                text: "a doc-comment line matched a secret pattern and was replaced".into(),
                kind: CaveatKind::RedactedDoc,
            });
        }
        Some(DocQuote {
            text,
            source: SourceRef::from(prov),
            redacted,
        })
    });

    let empty: Vec<EdgeRow> = Vec::new();
    let outs = out_edges.get(key).unwrap_or(&empty);
    let ins = in_edges.get(key).unwrap_or(&empty);

    let mut members: Vec<MemberRow> = Vec::new();
    let mut outgoing: Vec<Relation> = Vec::new();
    let mut externals: Vec<Relation> = Vec::new();
    for row in outs {
        let target = nodes.get(row.neighbor).copied();
        match row.kind {
            EdgeKind::HasMember => {
                if let Some(t) = target {
                    members.push(member_row(t));
                }
            }
            // A file card's own `Declares` edges (KTD3/KTD4, U1): a target
            // that is neither a type nor a claimed member is an unclaimed
            // declaration — a free function, or any other top-level item no
            // type owns — and becomes a member row exactly like a type's own
            // `HasMember` children. `Declares` only ever originates from a
            // file, so this arm is inert for Type and Module cards.
            EdgeKind::Declares
                if own_card(target, vocabulary).is_none() && !claimed.contains(row.neighbor) =>
            {
                if let Some(t) = target {
                    members.push(member_row(t));
                }
            }
            // Something that already has its own card (its own, or reached via
            // the owning type's `HasMember` list), or a claimed member of one.
            // Skipped entirely, never even as a generic relation, so a file card
            // never re-lists what the other card already documents (KTD4/R5).
            EdgeKind::Declares => {}
            // `Uses` stays a concrete edge kind (it carries no role by
            // design, KTD6), but WHAT it points at is asked by role: a declared
            // kind whose declaration says `External` is referenced-but-not-
            // contained in exactly the sense this section is about, and the core
            // learns that from the declaration without learning what it means.
            EdgeKind::Uses
                if target.is_some_and(|t| {
                    crate::graph::node_role(t.kind, vocabulary) == StructuralRole::External
                }) =>
            {
                externals.push(relation(row, target, vocabulary));
            }
            // Everything else becomes a generic outgoing relation, and that
            // INCLUDES a declared edge kind — deliberately. A card that dropped
            // a relation it could not classify would be the silent omission R10
            // forbids; a card that lists it under the plugin's own word for it
            // is honest and costs the core no domain knowledge. A type card's
            // own `Declares` edge comes FROM its file, never from the type; a
            // module's `Contains` list is the module card's content.
            _ => outgoing.push(relation(row, target, vocabulary)),
        }
    }

    // Incoming relations are the module card's "depended on by" side (R13). For
    // type and file cards the backlink SECTION is U9's; the data is carried here
    // so U9 adds a renderer, not a second traversal.
    let mut incoming: Vec<Relation> = ins
        .iter()
        .map(|row| relation(row, nodes.get(row.neighbor).copied(), vocabulary))
        .collect();

    push_caveats(node, None, &mut caveats);

    // A member owns no card, so a secret withheld inside it, or an ambiguity the
    // extractor admitted there, would be recorded in the graph and confessed
    // nowhere a reader looks (R22, R23). That is owed by EVERY card that holds
    // members, which is why this loop is not inside the `Type | File` gate below.
    //
    // It was, until RG3. The confession and the relation lifting had been written
    // as one block, so the argument for fencing the lifting silently fenced the
    // confession too — and a declared card, which can hold members through the
    // core `HasMember` edge, swallowed its members' redactions. Two questions,
    // two gates.
    for member in &members {
        if let Some(member_node) = nodes.get(member.identity.as_str()) {
            push_caveats(member_node, Some(&member.name), &mut caveats);
        }
    }

    // Lift each member's own relations onto this card, tagged with the member they
    // belong to (see `Relation::via`). Without this a type card is silent about
    // everything its methods touch — and, since U1, a file card is equally silent
    // about what its own unclaimed declarations (free functions and the like) call
    // and are called by, unless this loop also runs for `CardKind::File`.
    //
    // Type and file only, and U5 decided it stays that way. A module card's
    // content is its `Contains` list; a declared card kind has no core notion of
    // membership at all — `HasMember` is a core edge a plugin MAY emit but
    // nothing obliges it to, so lifting would silently do nothing for every
    // plugin that does not happen to use that one core edge. A declared card
    // lists what it points at under the plugin's own words for those edges,
    // which is the honest answer; inventing a members section out of one core
    // edge would be the core guessing at a structure it was never told.
    if matches!(kind, CardKind::Type | CardKind::File) {
        for member in &members {
            let member_out = out_edges.get(member.identity.as_str()).unwrap_or(&empty);
            let member_in = in_edges.get(member.identity.as_str()).unwrap_or(&empty);
            for row in member_out {
                // `HasMember` is the type→member edge itself, and a relation back
                // to the owning type is noise on the owner's own card.
                if row.kind == EdgeKind::HasMember || row.neighbor == key {
                    continue;
                }
                let target = nodes.get(row.neighbor).copied();
                let mut lifted = relation(row, target, vocabulary);
                lifted.via = Some(member_with_params(member));
                lifted.via_identity = Some(member.identity.clone());
                // By role, for the same reason as the `Uses` arm above.
                if target.is_some_and(|t| {
                    crate::graph::node_role(t.kind, vocabulary) == StructuralRole::External
                }) {
                    externals.push(lifted);
                } else {
                    outgoing.push(lifted);
                }
            }
            for row in member_in {
                // `Declares` is the file→member edge. For a type card it repeats
                // the file the card already names once; for a file card it is
                // simply this card's own declaring edge, caught below anyway by
                // `row.neighbor == key`. Either way it says nothing new and would bury
                // the backlinks that do.
                if matches!(row.kind, EdgeKind::HasMember | EdgeKind::Declares)
                    || row.neighbor == key
                {
                    continue;
                }
                let mut lifted = relation(row, nodes.get(row.neighbor).copied(), vocabulary);
                lifted.via = Some(member_with_params(member));
                lifted.via_identity = Some(member.identity.clone());
                incoming.push(lifted);
            }
        }
    }

    // R9 asks which test corresponds to the entity. The pairing heuristic pairs
    // FILES, not types, so a type card reads the fact through the `Declares` edge
    // that brought it into being rather than staying silent about it.
    let mut test = test_pairing(node);
    if test.is_none() && kind == CardKind::Type {
        test = ins
            .iter()
            .filter(|row| row.kind == EdgeKind::Declares)
            .filter_map(|row| nodes.get(row.neighbor).copied())
            .find_map(|declaring_file| {
                let mut inherited = test_pairing(declaring_file)?;
                inherited.from_file = Some(declaring_file.key.as_str().to_string());
                Some(inherited)
            });
    }

    // Plan A, U3 (R17–R19, KTD13): the lists the story, the renderer and the
    // backlinks all read are cleaned HERE, once, so none of them can disagree.
    // A relation between two members of this card's own unit is the unit's
    // inside, not a collaboration — and on a type card each such call showed
    // twice, once each way. Then one row per relation, and the ambiguous rows
    // move out into one claim per unsettled site.
    //
    // The noise is counted on the lists as assembled and again on the lists the
    // card keeps, with the same two predicates (plan A, U4): the first count
    // is what the graph handed the card, the second must be 0.
    let own_unit = |r: &Relation| cx.insight.unit_of(&r.identity) == Some(key);
    card_rows.duplicate_rows.before_filter += duplicate_rows(&outgoing) + duplicate_rows(&incoming);
    card_rows.reversed_rows.before_filter +=
        count_rows(&outgoing, &own_unit) + count_rows(&incoming, &own_unit);
    outgoing.retain(|r| !own_unit(r));
    incoming.retain(|r| !own_unit(r));
    let (mut outgoing, outgoing_unsettled) = split_unsettled(dedup_relations(outgoing));
    let (mut incoming, incoming_unsettled) = split_unsettled(dedup_relations(incoming));
    card_rows.duplicate_rows.after_filter += duplicate_rows(&outgoing) + duplicate_rows(&incoming);
    card_rows.reversed_rows.after_filter +=
        count_rows(&outgoing, &own_unit) + count_rows(&incoming, &own_unit);
    let maybe_also = maybe_also(
        (key, &title),
        &outgoing_unsettled,
        &incoming_unsettled,
        ambiguous,
        vocabulary,
        verdicts,
    );

    // Canonical order, so the rendered bytes depend on the graph's content and
    // never on which edge happened to be visited first (R18).
    members.sort_by_cached_key(|m| (m.name.clone(), m.identity.clone()));
    sort_relations(&mut outgoing);
    sort_relations(&mut incoming);
    sort_relations(&mut externals);
    caveats.sort();
    caveats.dedup();

    let header = (kind == CardKind::Type)
        .then(|| type_header(node))
        .flatten();
    let story = story::build(kind, key, &members, &outgoing, header.as_ref(), cx);

    Card {
        identity: key.to_string(),
        kind_word: card_word(kind, vocabulary),
        // Filled by `assign_paths`, which needs the whole set to break
        // case-insensitive collisions deterministically.
        path: String::new(),
        kind,
        title,
        entity_kind: fact_text(node, FactKind::SymbolKind).map(str::to_string),
        visibility: fact_text(node, FactKind::SymbolVisibility).map(str::to_string),
        signature: fact_text(node, FactKind::SymbolSignature).map(str::to_string),
        header,
        source,
        doc,
        members,
        outgoing,
        incoming,
        externals,
        maybe_also,
        test,
        caveats,
        story,
        // Handed out by the caller from the one index it builds for the
        // whole set; set by `assign_paths` once the set is complete, and the
        // generated flag by the caller, which reads the marked files once.
        navigators: detail::Navigators::default(),
        detail: None,
        map: None,
        generated: false,
    }
}

/// A member as `via` names it: its name and, for a callable, its parameter
/// list from the identity (`Solve(int)`) — see [`Relation::via`].
fn member_with_params(member: &MemberRow) -> String {
    match params_of(&member.identity) {
        Some(params) => format!("{}{params}", member.name),
        None => member.name.clone(),
    }
}

/// One row per relation (plan A, R18): rows that name the same kind, the same
/// neighbour and the same member of this card are one relation, carrying the
/// sorted union of their sites and the strongest rung among them.
///
/// A merged graph already holds one edge per relation, so on it this is a
/// guarantee rather than a change; it is what keeps the card honest over a
/// graph that has not been merged, and over any later source of rows.
fn dedup_relations(relations: Vec<Relation>) -> Vec<Relation> {
    use std::collections::btree_map::Entry;
    let mut by_relation: BTreeMap<(EdgeKind, String, Option<String>), Relation> = BTreeMap::new();
    for relation in relations {
        let (kind, identity, via) = relation_key(&relation);
        let key = (kind, identity.to_string(), via.map(str::to_string));
        match by_relation.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(relation);
            }
            Entry::Occupied(mut slot) => {
                let held = slot.get_mut();
                let mut sites = std::mem::take(&mut held.sites);
                sites.extend(relation.sites.iter().cloned());
                // `Confidence` lists its rungs strongest first; a rung beats
                // none, as in `graph::merge`.
                let stronger = match (relation.rung, held.rung) {
                    (Some(new), Some(old)) => new < old,
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                if stronger {
                    *held = relation;
                }
                sites.sort();
                sites.dedup();
                held.sites = sites;
            }
        }
    }
    by_relation.into_values().collect()
}

/// What makes two rows of one card list one relation: the edge kind, the
/// neighbour, and the member of this card the row is lifted through, by
/// identity. One key for [`dedup_relations`] and for [`duplicate_rows`], so the
/// counter measures exactly what the dedup removes.
fn relation_key(r: &Relation) -> (EdgeKind, &str, Option<&str>) {
    (r.kind, &r.identity, r.via_identity.as_deref())
}

/// How many rows of one list repeat a relation an earlier row already states
/// (see [`CardRows::duplicate_rows`]).
fn duplicate_rows(relations: &[Relation]) -> u64 {
    let distinct: std::collections::BTreeSet<_> = relations.iter().map(relation_key).collect();
    (relations.len() - distinct.len()) as u64
}

/// How many rows of one list match `predicate`.
fn count_rows(relations: &[Relation], predicate: &impl Fn(&Relation) -> bool) -> u64 {
    relations.iter().filter(|r| predicate(r)).count() as u64
}

/// Split off the rows that only a "maybe also" claim may carry (plan A, KTD2):
/// the ambiguous rung. Every other rung — a name match nothing backed or
/// disputed included — stays in the main lists.
fn split_unsettled(relations: Vec<Relation>) -> (Vec<Relation>, Vec<Relation>) {
    relations
        .into_iter()
        .partition(|r| r.rung != Some(Confidence::Ambiguous))
}

/// Fold the ambiguous rows into one claim per unsettled site (plan A, R19,
/// KTD5): the alternatives of one site are separate edges that share its
/// [`SiteId`], so they are grouped by (direction, kind, source, site) and become
/// one line with their alternatives, never N rows.
///
/// The alternatives are read from the SOURCE's own outgoing edges, not from
/// this card's rows: an incoming claim would otherwise list only the candidates
/// that happen to live on this card and hide that the name also fits elsewhere.
/// A site's own-unit candidates are therefore listed too — but a claim needs at
/// least one row that survived the own-unit filter to exist at all, so a guess
/// that never leaves this card is not shown as one. Claims identical but for
/// their site are then one claim with all their sites.
///
/// Within one site the alternatives are split by the NAME they answer to (their
/// `SymbolName`). Every candidate of one usage bears the usage's name, so the
/// split never cuts a claim in two — but without columns (plugin contract 0.1)
/// two different calls on one line share a [`SiteId`], and `m.IsValid()` and
/// `log.Write(1)` on one line would otherwise read as one call with four
/// candidates. Two calls of the SAME name on one line stay one claim until
/// columns tell them apart (see `SiteId`).
///
/// `card` is this card's key and title: the source of a usage the entity
/// itself makes, rather than one of its members.
fn maybe_also(
    card: (&str, &str),
    outgoing: &[Relation],
    incoming: &[Relation],
    ambiguous: &AmbiguousRows,
    vocabulary: &Vocabulary,
    verdicts: &ModelVerdicts,
) -> Vec<MaybeAlso> {
    let (key, title) = card;
    // (incoming, kind, source identity, site) -> (source label, resolution). A
    // row with no site — nothing produces one today — is its own site.
    type SiteKey = (bool, EdgeKind, String, Option<SiteId>);
    let mut at_site: BTreeMap<SiteKey, (String, Resolution)> = BTreeMap::new();
    for (incoming, relations) in [(false, outgoing), (true, incoming)] {
        for r in relations {
            let (label, identity) = if incoming {
                (r.label.clone(), r.identity.clone())
            } else {
                (
                    r.via.clone().unwrap_or_else(|| title.to_string()),
                    r.via_identity.clone().unwrap_or_else(|| key.to_string()),
                )
            };
            let sites: Vec<Option<SiteId>> = if r.sites.is_empty() {
                vec![None]
            } else {
                r.sites.iter().cloned().map(Some).collect()
            };
            for site in sites {
                at_site
                    .entry((incoming, r.kind, identity.clone(), site))
                    .or_insert_with(|| (label.clone(), r.resolution));
            }
        }
    }

    type ClaimKey = (
        bool,
        EdgeKind,
        Mention,
        Vec<Mention>,
        usize,
        MaybeReason,
        Resolution,
        // The model's verdict at the claim's sites: a site it spoke about and
        // one it did not are two claims, never one with a mark that holds for
        // only some of its sites.
        Option<Verified>,
    );
    let mut claims: BTreeMap<ClaimKey, Vec<SiteId>> = BTreeMap::new();
    for ((incoming, kind, source, site), (label, resolution)) in at_site {
        // name -> (alternatives with the rule each edge names, the largest
        // candidate count among them)
        type Alternatives = Vec<(Mention, Option<Rule>)>;
        let mut by_name: BTreeMap<&str, (Alternatives, usize)> = BTreeMap::new();
        // The source's ambiguous rows of this kind at this site.
        for AmbiguousRow { row, name, label } in ambiguous.at(&source, kind, site.as_ref()) {
            let (alternatives, candidates) = by_name.entry(name).or_default();
            *candidates = (*candidates).max(row.ambiguous_candidates.unwrap_or(0));
            alternatives.push((
                Mention {
                    label: label.clone(),
                    identity: row.neighbor.to_string(),
                },
                row.rule,
            ));
        }
        for (mut alternatives, candidates) in by_name.into_values() {
            alternatives.sort();
            alternatives.dedup();
            let rules: Vec<Option<Rule>> = alternatives.iter().map(|(_, r)| *r).collect();
            let reason = MaybeReason::of(&rules);
            let mut alternatives: Vec<Mention> = alternatives.into_iter().map(|(m, _)| m).collect();
            alternatives.dedup();
            let candidates = candidates.max(alternatives.len());
            let verified = site
                .as_ref()
                .and_then(|site| verdicts.mark(&source, kind, site, &alternatives));
            let source = Mention {
                label: label.clone(),
                identity: source.clone(),
            };
            claims
                .entry((
                    incoming,
                    kind,
                    source,
                    alternatives,
                    candidates,
                    reason,
                    resolution,
                    verified,
                ))
                .or_default()
                .extend(site.clone());
        }
    }

    // (claim, the alternative the model named, to list first)
    let mut out: Vec<(MaybeAlso, Option<String>)> = claims
        .into_iter()
        .map(
            |(
                (incoming, kind, source, alternatives, candidates, reason, resolution, verified),
                mut sites,
            )| {
                sites.sort();
                sites.dedup();
                let (model, promoted) = match verified {
                    Some((mark, promoted)) => (Some(mark), promoted),
                    None => (None, None),
                };
                let claim = MaybeAlso {
                    kind,
                    kind_word: edge_word(kind, vocabulary),
                    incoming,
                    source,
                    alternatives,
                    candidates,
                    reason,
                    resolution,
                    sites,
                    model,
                };
                (claim, promoted)
            },
        )
        .collect();
    // This card's own usages first, then those about it; within each, by the
    // kind's word (R2: never a declared kind's id), the source and the sites.
    // Compared in place: a cached key would copy every claim's alternatives
    // and sites. Both sorts are stable, so the order is the same.
    out.sort_by(|(a, _), (b, _)| {
        (
            a.incoming,
            &*a.kind_word,
            &a.source,
            &a.alternatives,
            &a.sites,
        )
            .cmp(&(
                b.incoming,
                &*b.kind_word,
                &b.source,
                &b.alternatives,
                &b.sites,
            ))
    });
    // The promoted alternative moves first only now, so the claims keep the
    // order they have without the layer.
    out.into_iter()
        .map(|(mut claim, promoted)| {
            if let Some(at) =
                promoted.and_then(|t| claim.alternatives.iter().position(|a| a.identity == t))
            {
                let first = claim.alternatives.remove(at);
                claim.alternatives.insert(0, first);
            }
            claim
        })
        .collect()
}

fn member_row(node: &Node) -> MemberRow {
    MemberRow {
        name: fact_text(node, FactKind::SymbolName)
            .unwrap_or(node.key.as_str())
            .to_string(),
        member_kind: fact_text(node, FactKind::SymbolKind).map(str::to_string),
        visibility: fact_text(node, FactKind::SymbolVisibility).map(str::to_string),
        signature: fact_text(node, FactKind::SymbolSignature).map(str::to_string),
        source: fact_prov(node, FactKind::SymbolName).map(SourceRef::from),
        identity: node.key.as_str().to_string(),
    }
}

/// How a Symbol target reads in a relation row: `Money.IsValid`, or `Invoice`
/// for a type that encloses nothing.
///
/// Qualified by its container, and the reason is measured. Over the 14,396-file
/// R17 target, **32% of the relation rows on type cards were byte-identical to
/// another row on the same card**, and 45% of cards carried at least one. They
/// were never duplicates: an ambiguous call resolves to up to `ambiguity_ceiling`
/// distinct candidates (R23), each a real edge to a real node — but a member owns
/// no card (KTD5), so every one of them rendered as the bare method name at the
/// same call site. Eight different targets, eight identical lines.
///
/// The container turns that noise back into the information it always was. The
/// card's caveats already said "IsValid: 8 candidates"; now the rows say WHICH.
fn symbol_label(target: &Node) -> Option<String> {
    let name = fact_text(target, FactKind::SymbolName)?;
    let qualified = match fact_text(target, FactKind::SymbolContainer) {
        // A free function and a top-level type have no container, and inventing
        // a separator for them would read as a member of nothing.
        None => name.to_string(),
        Some(container) => format!("{container}.{name}"),
    };
    Some(match params_of(target.key.as_str()) {
        Some(params) => format!("{qualified}{params}"),
        None => qualified,
    })
}

/// The parameter list an identity carries, `(string,int)`, or `None` for a
/// symbol that takes none — a type, a field, a namespace.
///
/// Qualifying by container alone left the overloads: measured over the R17
/// target, `OrbitActionContext.MessageBox` appeared four times on one card at
/// one call site, because four overloads are four distinct nodes that a
/// container and a name cannot tell apart. The parameter list is the only thing
/// that does, and KTD3 already put it in the identity — every callable's key
/// ends in the list, appended last after the type path (which is built from
/// identifiers and so can never itself contain a parenthesis).
fn params_of(identity: &str) -> Option<&str> {
    let after_hash = identity.split_once('#')?.1;
    let open = after_hash.find('(')?;
    // A residual duplicate carries an ordinal suffix after the parameters
    // (`…(string)~2`); it disambiguates the KEY, not the declaration a reader
    // is looking at, so it stays out of the label.
    let params = &after_hash[open..];
    Some(match params.rsplit_once('~') {
        Some((before, ordinal)) if ordinal.chars().all(|c| c.is_ascii_digit()) => before,
        _ => params,
    })
}

fn relation(row: &EdgeRow, target: Option<&Node>, vocabulary: &Vocabulary) -> Relation {
    Relation {
        kind: row.kind,
        kind_word: edge_word(row.kind, vocabulary),
        label: neighbor_label(row.neighbor, target),
        identity: row.neighbor.to_string(),
        resolution: row.resolution,
        source: Some(row.anchor.clone()),
        via: None,
        via_identity: None,
        ambiguous_candidates: row.ambiguous_candidates,
        rung: row.rung,
        sites: row.sites.to_vec(),
    }
}

/// What a relation row names its neighbour by: the readable name its facts
/// carry, or the key itself where the key is what a reader would read.
fn neighbor_label(neighbor: &str, target: Option<&Node>) -> String {
    target
        .and_then(|t| match t.kind {
            // Readable names live in facts for every opaque key-space.
            NodeKind::Symbol => symbol_label(t),
            NodeKind::External => fact_text(t, FactKind::ExternalName).map(str::to_string),
            NodeKind::Dependency => fact_text(t, FactKind::DependencyName).map(str::to_string),
            // A file/module key is already the readable path.
            NodeKind::File | NodeKind::Module => None,
            // Same reasoning: fall back to the key, below. A declared kind may well carry a readable name in a
            // declared fact, but which one that is, only the plugin knows.
            NodeKind::Declared(_) => None,
        })
        .unwrap_or_else(|| neighbor.to_string())
}

/// Append everything `node` admits about itself: an ambiguity the extractor could
/// not resolve (R23), a literal whose value was withheld (R22).
///
/// `via` names the member the admission came from, when the node is not the
/// carded entity itself. A reader who is told a secret was withheld from "this
/// type" and cannot tell which method it was in has been told half of it.
fn push_caveats(node: &Node, via: Option<&str>, caveats: &mut Vec<Caveat>) {
    let whose = |what: String, kind: CaveatKind| Caveat {
        text: match via {
            Some(member) => format!("in `{member}`: {what}"),
            None => what,
        },
        kind,
    };
    for fact in &node.facts {
        match fact.kind {
            // Same underlying ambiguity (R23), two different reader-facing
            // states — and, until this text diverged, one identical sentence
            // for both. CandidateCount means the edges above ARE there, just
            // marked; AmbiguousReference means no edge is shown at all
            // because the ceiling was exceeded. Conflating them let a reader
            // believe an unlisted relation was merely uncertain when it was
            // never emitted (R2).
            FactKind::CandidateCount => {
                if let FactValue::Text(v) = &fact.value {
                    // Since plan A, U3 the edges are not "above": an
                    // ambiguous relation is a Maybe-also claim, and one that
                    // stays inside this card's own unit is not shown at all.
                    // Not "listed under": since plan B, U9 a type's claims
                    // are in its detail, a file's on its card.
                    caveats.push(whose(
                        format!(
                            "{v} — ambiguous: a Maybe also claim where it leaves this card's own type or file"
                        ),
                        CaveatKind::SharedName,
                    ));
                }
            }
            FactKind::AmbiguousReference => {
                if let FactValue::Text(v) = &fact.value {
                    caveats.push(whose(
                        format!(
                            "ambiguous reference: {v} — no relation shown, the ambiguity ceiling was exceeded"
                        ),
                        CaveatKind::OverCeiling,
                    ));
                }
            }
            FactKind::RedactedLiteral => {
                if let FactValue::Text(v) = &fact.value {
                    caveats.push(whose(
                        format!(
                            "a literal matched the `{v}` secret pattern at {} — the value is not carried",
                            SourceRef::from(&fact.provenance).anchor()
                        ),
                        CaveatKind::RedactedLiteral,
                    ));
                }
            }
            // The plugin never finished this file, so the card is missing
            // whatever it would have declared — the reader is owed that
            // before they read an empty card as an empty file.
            FactKind::ExtractionFailed => {
                if let FactValue::Text(v) = &fact.value {
                    caveats.push(whose(
                        format!(
                            "the `{}` plugin failed on this file ({v}) — what it declares is not shown",
                            fact.provenance.extractor
                        ),
                        CaveatKind::ExtractionFailed,
                    ));
                }
            }
            // Every other fact kind, declared ones included. A caveat is the
            // core confessing something IT withheld or could not resolve; a
            // declared fact carries no such admission, because the core never
            // touched its value. Nothing is dropped here that a reader is owed.
            _ => {}
        }
    }
}

/// Total order over relations: kind, then label, then identity, then anchor. All
/// four are needed — one caller can call one callee from several lines, and those
/// rows differ only in the anchor.
///
/// **A declared kind is ordered by its NAME, never by its id.** `EdgeKind` derives
/// `Ord`, so `Declared(EdgeKindId)` would compare by the FNV-1a digest — and the
/// order of rows on a card is a decision, which R2 says the core may not take
/// from a declared kind's identity. Renaming a declared edge kind would have
/// silently reshuffled every card that carries two of them. Found by the external
/// review round; the internal gate could not see it, because every fixture
/// declared exactly one kind per category and one kind has no sibling to
/// reorder against.
///
/// Core kinds keep the variant order they always had: they all sort under
/// `declared == false` with an empty word, so their key is the old key with two
/// constant columns added. A corpus that declares nothing does not move (R4).
fn sort_relations(relations: &mut [Relation]) {
    relations.sort_by_cached_key(|r| {
        let declared = matches!(r.kind, EdgeKind::Declared(_));
        (
            declared,
            (!declared).then_some(r.kind),
            if declared {
                r.kind_word.to_string()
            } else {
                String::new()
            },
            r.label.clone(),
            r.identity.clone(),
            r.via.clone(),
            r.source.as_ref().map(SourceRef::anchor),
        )
    });
}

/// Render every card the graph supports, as corpus-relative files ready to write.
///
/// Kept beside the view pipeline rather than inside [`crate::render::build_corpus`]
/// (KTD4): the cards are their own projection, and the view snapshots must not
/// move when a card lands.
pub fn build_card_files(
    graph: &Graph,
    lang: render::Lang,
    style: crate::cli::LinkStyle,
) -> Vec<crate::render::RenderedFile> {
    let cards = build_cards(graph);
    // Built from the finished set, so a link can only ever point at a card that
    // is also being written (U9).
    let index = links::CardIndex::build(&cards);
    render_card_files(&cards, &index, lang, style)
}

/// [`build_card_files`], from an already-built card set and its `CardIndex`
/// (U10) — the signpost (`card::index`) needs both too, so `generate_reporting`
/// builds them once and passes them to both rather than walking the graph, or
/// building the index, twice.
pub fn render_card_files(
    cards: &[Card],
    index: &links::CardIndex,
    lang: render::Lang,
    style: crate::cli::LinkStyle,
) -> Vec<crate::render::RenderedFile> {
    render_card_files_layered(cards, index, lang, style, None)
}

/// The second layer of a run (plan 2a, U6): its level, its model and what
/// was read of it.
#[derive(Clone, Copy)]
pub struct Layer2Run<'a> {
    pub level: u8,
    pub bodies: &'a crate::extract::bodies::BodyLayer,
    pub analysis: &'a layer2::Analysis,
}

impl Layer2Run<'_> {
    /// What the run's second layer gives the card: a type's level, whether
    /// its second layer was read, and its analysis; `None` for every other
    /// card, which the second layer leaves as it was.
    pub fn of(&self, card: &Card) -> Option<layer2_render::Layer2<'_>> {
        if card.kind != CardKind::Type {
            return None;
        }
        let key = crate::graph::NodeKey::new(card.identity.as_str());
        Some(layer2_render::Layer2 {
            level: self.level,
            state: self.bodies.layer_of(&key),
            analysis: self.analysis.of(&key),
        })
    }
}

/// [`render_card_files`] with the run's second layer, when it has one: a
/// type card carries it ([`render::render_card_layered`]); without it, and
/// on every other card, this is [`render_card_files`].
pub fn render_card_files_layered(
    cards: &[Card],
    index: &links::CardIndex,
    lang: render::Lang,
    style: crate::cli::LinkStyle,
    run: Option<&Layer2Run<'_>>,
) -> Vec<crate::render::RenderedFile> {
    // Each card renders from itself and the read-only index alone, so the
    // cards render in parallel; `collect` keeps them in card order.
    use rayon::prelude::*;
    cards
        .par_iter()
        .map(|card| crate::render::RenderedFile {
            path: card.path.clone(),
            contents: render::render_card_layered(
                card,
                lang,
                style,
                index,
                run.and_then(|r| r.of(card)),
            ),
        })
        .collect()
}

/// The keywords of every analyzed type's card with the words its second
/// layer brings (plan 2a, U6: KTD9 point 4), weighed by the first layer's
/// document frequencies; every other card keeps its keywords. Read from the
/// whole analysis, so they are the same on every level.
pub fn add_layer2_keywords(
    cards: &mut [Card],
    insight: &insight::Insight,
    analysis: &layer2::Analysis,
) {
    for card in cards.iter_mut().filter(|c| c.kind == CardKind::Type) {
        let Some(a) = analysis.of(&crate::graph::NodeKey::new(card.identity.as_str())) else {
            continue;
        };
        let words = layer2_render::words(a);
        if !words.is_empty() {
            card.story.keywords = insight.keywords_with(&card.identity, &words);
        }
    }
}

/// A type's header from every part's signature (plan B, U1). The signature is
/// a scalar fact, so a `partial` type's parts arrive as one resolved value and
/// the rest as its dissent; the header needs all of them, because a modifier
/// stated on any part holds for the whole type.
fn type_header(node: &Node) -> Option<header::TypeHeader> {
    let kind = fact_text(node, FactKind::SymbolKind)?;
    let parts = node
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::SymbolSignature)
        .flat_map(|f| {
            std::iter::once((&f.value, &f.provenance))
                .chain(f.dissent.iter().map(|c| (&c.value, &c.provenance)))
        })
        .filter_map(|(value, provenance)| match value {
            FactValue::Text(signature) => Some((provenance.file.as_str(), signature.as_str())),
            _ => None,
        });
    header::from_signatures(kind, parts)
}

fn fact_text(node: &Node, kind: FactKind) -> Option<&str> {
    node.facts.iter().find_map(|f| match (&f.kind, &f.value) {
        (k, FactValue::Text(v)) if *k == kind => Some(v.as_str()),
        _ => None,
    })
}

/// The node's test pairing, carrying the tier the pairing was asserted at.
fn test_pairing(node: &Node) -> Option<TestPairing> {
    let fact = node
        .facts
        .iter()
        .find(|f| f.kind == FactKind::TestByConvention)?;
    let FactValue::Text(statement) = &fact.value else {
        return None;
    };
    Some(TestPairing {
        statement: statement.clone(),
        tier: fact.provenance.tier,
        from_file: None,
    })
}

fn fact_prov(node: &Node, kind: FactKind) -> Option<&Provenance> {
    node.facts
        .iter()
        .find(|f| f.kind == kind)
        .map(|f| &f.provenance)
}

/// Belt-and-braces R22 pass over a quoted doc comment.
///
/// The authoritative filter runs at fact emission
/// ([`crate::extract::external::redact_secret_block`]), so the text arriving here
/// has already been redacted and this call is normally a no-op that only recomputes
/// the `redacted` flag for the card's caveat. It stays because a doc comment can
/// reach a card from a source that did not go through the extractor, and because a
/// second application costs nothing: the filter is idempotent — its own marker
/// contains no secret shape.
fn redact_doc(text: &str) -> (String, bool) {
    use crate::extract::external::{redact_secret_block, SecretPrefixes, REDACTION_MARKER};
    // The BUILT-IN list, not the run's configured one: this pass re-filters text
    // the extractors already filtered with the configured list, so anything a
    // company prefix matched is replaced and carries the marker before it gets
    // here. Threading configuration this far for a belt-and-braces sweep would
    // buy nothing but a longer signature on every card function.
    let (text, matched_now) = redact_secret_block(text, SecretPrefixes::builtin());
    // Text redacted upstream still has to be REPORTED here, or the card would
    // quietly present a hole as the original comment — the reader must learn that
    // a value was withheld and where to look for it.
    let already = text.contains(REDACTION_MARKER);
    (text, matched_now || already)
}

/// Give every card its output path, and every card that has a detail the
/// detail's path, breaking collisions deterministically.
///
/// Two identities that sanitize to paths differing only in case would be one file
/// on Windows and NTFS. Every member of such a group gets the hashed form — not
/// just the ones after the first — so the outcome cannot depend on which card was
/// visited first.
///
/// Cards, details and maps are checked as ONE set (plan B, KTD7). The trees
/// are disjoint by their prefixes, so today a detail can only meet another
/// detail and a map another map — but the set is what gets written, and
/// checking it whole keeps that true when a tree is added. Whether a card has
/// a detail is decided here too ([`detail::wants_detail`]), and whether a
/// module has a map ([`map::modules_with_types`]), so no card names a
/// document outside the set.
fn assign_paths(cards: &mut [Card], vocabulary: &Vocabulary) {
    let mapped: HashSet<String> = map::modules_with_types(cards)
        .into_keys()
        .map(str::to_string)
        .collect();
    // (card, tree, path) for every document the set writes.
    let mut proposed: Vec<(usize, Tree, String)> = Vec::with_capacity(cards.len() * 2);
    for (i, card) in cards.iter().enumerate() {
        proposed.push((
            i,
            Tree::Card,
            card_path(card.kind, &card.identity, vocabulary),
        ));
        if detail::wants_detail(card) {
            proposed.push((
                i,
                Tree::Detail,
                doc_path(Tree::Detail, card.kind, &card.identity, vocabulary),
            ));
        }
        if card.kind == CardKind::Module && mapped.contains(&card.identity) {
            proposed.push((
                i,
                Tree::Map,
                doc_path(Tree::Map, card.kind, &card.identity, vocabulary),
            ));
        }
    }

    let mut folded: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (at, (_, _, path)) in proposed.iter().enumerate() {
        folded.entry(path.to_lowercase()).or_default().push(at);
    }
    for (_, group) in folded {
        if group.len() > 1 {
            for at in group {
                let (i, tree, _) = proposed[at];
                proposed[at].2 = hashed_path(tree, cards[i].kind, &cards[i].identity, vocabulary);
            }
        }
    }

    for (i, tree, path) in proposed {
        match tree {
            Tree::Card => cards[i].path = path,
            Tree::Detail => cards[i].detail = Some(path),
            Tree::Map => cards[i].map = Some(path),
        }
    }
}

/// The reader-facing word for a card kind (KTD4).
///
/// Core kinds keep their authored English word; a declared kind is named by its
/// declaration. Resolved here, while the vocabulary is in reach, because the
/// renderer receives only a finished `Card`.
fn card_word(kind: CardKind, vocabulary: &Vocabulary) -> Cow<'static, str> {
    match kind {
        CardKind::Type => "type".into(),
        CardKind::File => "file".into(),
        CardKind::Module => "module".into(),
        CardKind::Declared(id) => Cow::Owned(
            vocabulary
                .card_name(id)
                .unwrap_or_else(|| crate::graph::undeclared_name(id.get())),
        ),
    }
}

/// The reader-facing word for an edge kind (KTD4).
///
/// Delegates to [`crate::graph::edge_word`] rather than repeating the table.
/// The card module's own promise is that "a card and the export never name one
/// relation two ways", and until review caught it this file and `manifest.rs`
/// held two independently written copies of the same eleven literals — the
/// exact shape that keeps agreeing until one of them is updated alone.
fn edge_word(kind: EdgeKind, vocabulary: &Vocabulary) -> Cow<'static, str> {
    crate::graph::edge_word(kind, vocabulary)
}

/// The tree a document of the card corpus lives in (plan B, KTD7).
///
/// One identity gives one document per tree, and every tree is spelled by the
/// same [`doc_path`]: the same folding, the same 120-byte budget counted over
/// the WHOLE path (the tree prefix included), the same hashed fallback, the
/// same name for the root module. A suffix beside the card (`Foo.detail.md`)
/// would have been shorter, and it would have been the card path of a nested
/// type called `detail`; a tree of its own cannot meet a card's path at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tree {
    /// The card itself: `cards/<kind>/…`.
    Card,
    /// A card's detail (plan B, U8): `cards/detail/<kind>/…`. Only a type card
    /// and a file card have one ([`detail::wants_detail`]), so the declared
    /// kinds' longer directories never meet this prefix.
    Detail,
    /// A module's map (plan B, U10): `cards/map/…`, one tree for the one kind
    /// that has a map.
    Map,
}

impl Tree {
    /// The directory under `cards/` a document of `kind` lives in.
    fn dir(self, kind: CardKind, vocabulary: &Vocabulary) -> Cow<'static, str> {
        match self {
            Tree::Card => kind.dir(vocabulary),
            Tree::Detail => Cow::Owned(format!("detail/{}", kind.dir(vocabulary))),
            Tree::Map => "map".into(),
        }
    }
}

/// Derive a card's corpus-relative path from its identity (R14, KTD3).
///
/// Total and textual: the identity's structural separators (`:` and `#`) become
/// path separators, every remaining character is folded to something a filesystem
/// and a Markdown link both accept, and an over-long or unrepresentable result
/// falls back to [`hashed_path`]. Nothing here interprets what a segment *means*.
pub fn card_path(kind: CardKind, identity: &str, vocabulary: &Vocabulary) -> String {
    doc_path(Tree::Card, kind, identity, vocabulary)
}

/// [`card_path`] for any tree of the card corpus (plan B, KTD7): a detail or a
/// map is named exactly as its card is, under its own prefix.
pub fn doc_path(tree: Tree, kind: CardKind, identity: &str, vocabulary: &Vocabulary) -> String {
    // The root module's key is `.`, which sanitizes to nothing. Name it, rather
    // than pushing the corpus's most-visited card into the hashed fallback.
    if identity == "." {
        return format!("cards/{}/_root.md", tree.dir(kind, vocabulary));
    }

    let body = identity
        .strip_prefix("sym:")
        .unwrap_or(identity)
        .replace([':', '#'], "/");

    let segments: Vec<String> = body
        .split('/')
        .filter(|s| !s.is_empty())
        .map(sanitize_segment)
        .collect();

    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return hashed_path(tree, kind, identity, vocabulary);
    }

    let path = format!(
        "cards/{}/{}.md",
        tree.dir(kind, vocabulary),
        segments.join("/")
    );
    if path.len() > MAX_REL_PATH {
        return hashed_path(tree, kind, identity, vocabulary);
    }
    path
}

/// The fallback name: a readable stub plus a hash of the FULL identity, so it
/// stays unique and stays the same on the next run — and the same stub and
/// hash in every tree, so a hashed card's detail is found beside it by name.
fn hashed_path(tree: Tree, kind: CardKind, identity: &str, vocabulary: &Vocabulary) -> String {
    let stub = identity
        .rsplit(['/', '#', ':'])
        .find(|s| !s.is_empty())
        .map(sanitize_segment)
        .unwrap_or_default();
    // Bounded in bytes, stopping on a character boundary — see
    // `TRUNCATED_SEGMENT`. `take(TRUNCATED_SEGMENT)` over `chars()` counted the
    // wrong unit.
    let stub: String = stub
        .chars()
        .scan(0usize, |used, ch| {
            *used += ch.len_utf8();
            (*used <= TRUNCATED_SEGMENT).then_some(ch)
        })
        .collect();
    let stub = stub.trim_end_matches(['.', '_']).to_string();
    let stub = if stub.is_empty() {
        "card".to_string()
    } else {
        stub
    };
    // `dir()` may ALREADY be a `_hashed` tree, when the kind's own name could not
    // be spelled as a directory. Composing a second `_hashed` under it produced
    // `declared/_hashed/<kind>/_hashed/<stub>-<id>.md` — unique and bounded, but
    // it reads as a mistake and spends budget twice on the same word.
    let dir = tree.dir(kind, vocabulary);
    let sep = if dir.ends_with("_hashed") || dir.contains("_hashed/") {
        ""
    } else {
        "_hashed/"
    };
    format!(
        "cards/{dir}/{sep}{stub}-{}.md",
        crate::graph::fnv1a64_hex(identity)
    )
}

/// Windows device names, which are invalid as a file stem with ANY extension.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Fold one path segment into something both a filesystem and a Markdown link
/// accept.
///
/// `< > : " / \ | ? *` and control characters are invalid on Windows; `(` `)`
/// `[` `]` and spaces are legal there but break an inline Markdown link target,
/// which U9 will build out of this path. A trailing dot or space is stripped
/// because Windows silently drops it — which would make two identities land on
/// one file.
///
/// `#` is folded with them, and the reason is worth the line: an earlier version
/// of this comment said `#` never reached here because it is a separator. That
/// was true of every segment derived from an IDENTITY — `card_path` splits on it
/// — and false of the one segment derived from a declared kind NAME, which
/// arrives through `CardKind::dir`. A `#` inside a relative link target starts a
/// fragment, so one such kind would break every link on every card under it.
/// Folded rather than refused at declaration, because the wire name is the join
/// key a consumer reads out of the manifest and a kind honestly called `C#` has
/// to stay callable that.
fn sanitize_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for ch in segment.chars() {
        let keep = match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '#' => '_',
            '(' | ')' | '[' | ']' | '{' | '}' | ' ' | ',' | '\'' | '`' => '_',
            // `~` never reaches a file name: on NTFS with 8.3 names enabled,
            // `vwList~2.md` IS the short name of a longer sibling such as
            // `vwListDetailRightColl.md`, and writing it overwrites that
            // sibling — which one depends on creation order (found on the
            // reference corpus).
            '~' => '_',
            c if c.is_control() => '_',
            c => c,
        };
        out.push(keep);
    }
    let out = out.trim_end_matches(['.', ' ']).to_string();
    // Windows resolves a device name from the part BEFORE the first dot, so
    // `con.txt` is the console just as much as `con` is. The check used to
    // compare the whole segment and let every extension through — the constant's
    // own doc comment said "with ANY extension" while the code said otherwise,
    // and the external review round read the comment, tested the code, and
    // found the gap.
    //
    // The underscore goes after the STEM rather than at the end: `con.txt_` is
    // still `con` to Windows, so appending would have looked like a fix and
    // changed nothing.
    let (stem, rest) = match out.split_once('.') {
        Some((stem, rest)) => (stem, Some(rest)),
        None => (out.as_str(), None),
    };
    if RESERVED.contains(&stem.to_lowercase().as_str()) {
        return match rest {
            Some(rest) => format!("{stem}_.{rest}"),
            None => format!("{stem}_"),
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Fact, NodeKey};

    /// A `~` in a card file name can be an NTFS 8.3 short name of another
    /// file (`VWLIST~2.MD`), so it never reaches one.
    #[test]
    fn a_tilde_never_reaches_a_file_name() {
        assert_eq!(sanitize_segment("vwList~2"), "vwList_2");
        assert!(!sanitize_segment("~a~").contains('~'));
    }

    /// R2: the two ambiguity caveats used to render as the identical
    /// sentence — a reader could not tell "these relations exist, marked
    /// ambiguous" from "no relation is shown at all" without checking which
    /// fact kind produced it. They must now read differently.
    #[test]
    fn candidate_count_and_ambiguous_reference_caveats_read_differently() {
        let node = |kind: FactKind, text: &str| Node {
            key: NodeKey::new("f"),
            kind: NodeKind::Symbol,
            facts: vec![Fact::new(
                kind,
                FactValue::Text(text.to_string()),
                Provenance::new("f", Some(1), "treesitter".into(), Tier::Measured),
            )],
        };

        let mut below_ceiling = Vec::new();
        push_caveats(
            &node(FactKind::CandidateCount, "Append: 3 candidates"),
            None,
            &mut below_ceiling,
        );
        let mut above_ceiling = Vec::new();
        push_caveats(
            &node(FactKind::AmbiguousReference, "Get: 12 candidates"),
            None,
            &mut above_ceiling,
        );

        assert_ne!(below_ceiling, above_ceiling);
        assert_eq!(
            below_ceiling,
            vec![Caveat {
                text: "Append: 3 candidates — ambiguous: a Maybe also claim where it leaves this card's own type or file"
                    .to_string(),
                kind: CaveatKind::SharedName,
            }],
        );
        assert_eq!(
            above_ceiling,
            vec![Caveat {
                text: "ambiguous reference: Get: 12 candidates — no relation shown, the ambiguity ceiling was exceeded"
                    .to_string(),
                kind: CaveatKind::OverCeiling,
            }],
        );
    }

    /// The unrelated `RedactedLiteral` caveat is untouched by the R2 split
    /// above — same match statement, different arm.
    #[test]
    fn redacted_literal_caveat_is_unaffected_by_the_ambiguity_wording_split() {
        let node = Node {
            key: NodeKey::new("f"),
            kind: NodeKind::Symbol,
            facts: vec![Fact::new(
                FactKind::RedactedLiteral,
                FactValue::Text("sk-".to_string()),
                Provenance::new("f", Some(1), "treesitter".into(), Tier::Measured),
            )],
        };
        let mut caveats = Vec::new();
        push_caveats(&node, None, &mut caveats);
        assert_eq!(caveats.len(), 1);
        assert!(caveats[0].text.contains("sk-"));
        assert!(caveats[0].text.contains("secret pattern"));
        assert_eq!(caveats[0].kind, CaveatKind::RedactedLiteral);
    }

    /// A card with nothing on it but the identity the path is derived from.
    fn bare_card(identity: &str) -> Card {
        Card {
            identity: identity.to_string(),
            path: String::new(),
            kind: CardKind::Type,
            kind_word: "type".into(),
            title: identity.to_string(),
            entity_kind: None,
            visibility: None,
            signature: None,
            header: None,
            source: None,
            doc: None,
            members: Vec::new(),
            outgoing: Vec::new(),
            incoming: Vec::new(),
            externals: Vec::new(),
            maybe_also: Vec::new(),
            test: None,
            caveats: Vec::new(),
            story: Default::default(),
            navigators: Default::default(),
            detail: None,
            map: None,
            generated: false,
        }
    }

    #[test]
    fn a_type_identity_becomes_a_nested_card_path() {
        assert_eq!(
            card_path(
                CardKind::Type,
                "sym:cs:Orbit.Billing#Invoice",
                &Vocabulary::default()
            ),
            "cards/type/cs/Orbit.Billing/Invoice.md"
        );
    }

    #[test]
    fn the_root_module_gets_a_named_card_not_a_hashed_one() {
        // `.` sanitizes to nothing, so without its own case the corpus's entry
        // module would land in `_hashed/` behind an opaque name.
        assert_eq!(
            card_path(CardKind::Module, ".", &Vocabulary::default()),
            "cards/module/_root.md"
        );
    }

    #[test]
    fn file_and_module_cards_live_in_disjoint_trees() {
        assert_eq!(
            card_path(CardKind::File, "src/config.rs", &Vocabulary::default()),
            "cards/file/src/config.rs.md"
        );
        assert_eq!(
            card_path(CardKind::Module, "src", &Vocabulary::default()),
            "cards/module/src.md"
        );
    }

    #[test]
    fn windows_illegal_and_link_breaking_characters_are_folded() {
        let path = card_path(
            CardKind::Type,
            "sym:cs:Ns#Outer.Do(int, string)",
            &Vocabulary::default(),
        );
        assert!(!path.contains('('), "{path}");
        assert!(!path.contains(' '), "{path}");
        assert_eq!(path, "cards/type/cs/Ns/Outer.Do_int__string_.md");
    }

    #[test]
    fn a_reserved_device_name_cannot_become_a_file_stem() {
        assert_eq!(
            card_path(CardKind::Type, "sym:cs:Ns#Con", &Vocabulary::default()),
            "cards/type/cs/Ns/Con_.md"
        );
    }

    #[test]
    fn a_trailing_dot_is_stripped_so_two_identities_cannot_share_a_file() {
        // Windows drops a trailing dot silently; leaving it in would make
        // `Invoice.` and `Invoice` the same file.
        assert_eq!(
            card_path(CardKind::Type, "sym:cs:Ns#Invoice.", &Vocabulary::default()),
            "cards/type/cs/Ns/Invoice.md"
        );

        // Which is the whole point, so put BOTH through the assignment that
        // actually writes the corpus. Stripping alone produces the collision;
        // only `assign_paths` resolves it, and a test that stops at the
        // stripping would pass while one card silently overwrote the other.
        let mut cards = [
            bare_card("sym:cs:Ns#Invoice"),
            bare_card("sym:cs:Ns#Invoice."),
        ];
        assign_paths(&mut cards, &Vocabulary::default());
        assert_ne!(
            cards[0].path,
            cards[1].path,
            "two identities must never land on one file: {:?}",
            cards.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_over_long_identity_falls_back_to_a_hashed_name_under_the_budget() {
        let deep = format!("sym:cs:{}#Type", "Very.Deep.Namespace".repeat(12));
        let path = card_path(CardKind::Type, &deep, &Vocabulary::default());
        assert!(path.starts_with("cards/type/_hashed/"), "{path}");
        assert!(path.len() <= MAX_REL_PATH, "{} chars: {path}", path.len());
    }

    #[test]
    fn a_hashed_path_is_stable_across_calls_and_unique_per_identity() {
        let a = card_path(
            CardKind::Type,
            &format!("sym:cs:{}#A", "N.".repeat(80)),
            &Vocabulary::default(),
        );
        let b = card_path(
            CardKind::Type,
            &format!("sym:cs:{}#A", "N.".repeat(80)),
            &Vocabulary::default(),
        );
        let c = card_path(
            CardKind::Type,
            &format!("sym:cs:{}#B", "N.".repeat(80)),
            &Vocabulary::default(),
        );
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn case_insensitive_collisions_send_every_member_to_a_hashed_name() {
        let mut cards = vec![card_stub("sym:cs:Ns#Foo"), card_stub("sym:cs:Ns#foo")];
        assign_paths(&mut cards, &Vocabulary::default());
        assert!(cards[0].path.contains("_hashed"), "{:?}", cards[0].path);
        assert!(cards[1].path.contains("_hashed"), "{:?}", cards[1].path);
        assert_ne!(cards[0].path, cards[1].path);
    }

    #[test]
    fn a_unique_path_is_left_readable() {
        let mut cards = vec![card_stub("sym:cs:Ns#Foo"), card_stub("sym:cs:Ns#Bar")];
        assign_paths(&mut cards, &Vocabulary::default());
        assert_eq!(cards[0].path, "cards/type/cs/Ns/Foo.md");
        assert_eq!(cards[1].path, "cards/type/cs/Ns/Bar.md");
    }

    // ---- Plan B, U8: the detail's tree (KTD7) ----

    /// A type card that earns a detail: one offered member, nothing else.
    fn card_with_detail(identity: &str) -> Card {
        let mut card = card_stub(identity);
        card.story.api.push(story::ApiRow {
            identity: format!("{identity}.Go()"),
            name: "Go".into(),
            ..Default::default()
        });
        card
    }

    #[test]
    fn a_detail_is_named_as_its_card_in_a_tree_of_its_own() {
        let mut cards = vec![
            card_with_detail("sym:cs:Ns#Foo"),
            card_stub("sym:cs:Ns#Bar"),
        ];
        cards[1].kind = CardKind::File;
        cards[1].identity = "src/q.c".into();
        cards[1].navigators.db.push(detail::DbRow {
            member: None,
            member_identity: "src/q.c".into(),
            access: crate::graph::DbAccessKind::Writes,
            object: "dba.t".into(),
            at: SourceRef {
                file: "src/q.c".into(),
                span: Span::Line(4),
            },
        });
        assign_paths(&mut cards, &Vocabulary::default());
        assert_eq!(cards[0].path, "cards/type/cs/Ns/Foo.md");
        assert_eq!(
            cards[0].detail.as_deref(),
            Some("cards/detail/type/cs/Ns/Foo.md")
        );
        assert_eq!(cards[1].path, "cards/file/src/q.c.md");
        assert_eq!(
            cards[1].detail.as_deref(),
            Some("cards/detail/file/src/q.c.md")
        );
    }

    /// A card with nothing its card does not already say gets no detail, and
    /// so names none.
    #[test]
    fn a_card_with_nothing_more_to_say_has_no_detail() {
        let mut cards = vec![card_stub("sym:cs:Ns#Empty")];
        assign_paths(&mut cards, &Vocabulary::default());
        assert_eq!(cards[0].detail, None);
    }

    /// KTD7: the 120-byte budget counts the detail's longer prefix, so a card
    /// just under it keeps its readable name while its detail falls back to
    /// the hashed one — under the budget, and named by the card's stub.
    #[test]
    fn a_card_just_under_the_budget_gets_a_hashed_detail_under_it() {
        let fixed = "cards/type/cs/Ns/.md".len();
        let identity = format!("sym:cs:Ns#{}", "T".repeat(MAX_REL_PATH - fixed - 2));
        let mut cards = vec![card_with_detail(&identity)];
        assign_paths(&mut cards, &Vocabulary::default());
        let (card, detail) = (&cards[0].path, cards[0].detail.clone().unwrap());
        assert!(!card.contains("_hashed"), "{card}");
        assert!(card.len() > MAX_REL_PATH - "detail/".len(), "{card}");
        assert!(
            detail.starts_with("cards/detail/type/_hashed/TTTT"),
            "{detail}"
        );
        assert!(
            detail.len() <= MAX_REL_PATH,
            "{} bytes: {detail}",
            detail.len()
        );
    }

    /// A card in the hashed fallback has its detail in the same fallback, by
    /// the same stub and digest — a reader finds one from the other by name.
    #[test]
    fn a_hashed_card_has_its_detail_hashed_alike() {
        let deep = format!("sym:cs:{}#Type", "Very.Deep.Namespace".repeat(12));
        let mut cards = vec![
            card_with_detail(&deep),
            card_with_detail("sym:cs:Ns#Foo"),
            card_with_detail("sym:cs:Ns#foo"),
        ];
        assign_paths(&mut cards, &Vocabulary::default());
        for card in &cards {
            assert!(
                card.path.starts_with("cards/type/_hashed/"),
                "{}",
                card.path
            );
            assert_eq!(
                card.detail.as_deref(),
                Some(
                    card.path
                        .replacen("cards/type/", "cards/detail/type/", 1)
                        .as_str()
                )
            );
        }
    }

    /// KTD7: a nested type called `detail` would have been the detail of its
    /// owner under a `Foo.detail.md` suffix. In a tree of its own it is just
    /// another card, and all four documents keep readable, distinct names.
    #[test]
    fn a_nested_type_called_detail_collides_with_nothing() {
        let mut cards = vec![
            card_with_detail("sym:cs:Ns#Foo"),
            card_with_detail("sym:cs:Ns#Foo.detail"),
        ];
        assign_paths(&mut cards, &Vocabulary::default());
        let mut paths: Vec<String> = cards
            .iter()
            .flat_map(|c| [c.path.clone(), c.detail.clone().unwrap()])
            .collect();
        assert!(paths.iter().all(|p| !p.contains("_hashed")), "{paths:?}");
        paths.sort_by_key(|p| p.to_lowercase());
        paths.dedup_by_key(|p| p.to_lowercase());
        assert_eq!(paths.len(), 4, "{paths:?}");
    }

    /// The root module's documents are named as its card is, `_root`, in
    /// every tree — never pushed into the hashed fallback.
    #[test]
    fn the_root_module_gets_a_map_and_a_detail_named_as_its_card() {
        let vocabulary = Vocabulary::default();
        assert_eq!(
            card_path(CardKind::Module, ".", &vocabulary),
            "cards/module/_root.md"
        );
        assert_eq!(
            doc_path(Tree::Map, CardKind::Module, ".", &vocabulary),
            "cards/map/_root.md"
        );
        assert_eq!(
            doc_path(Tree::Detail, CardKind::Module, ".", &vocabulary),
            "cards/detail/module/_root.md"
        );
    }

    #[test]
    fn a_secret_shaped_doc_line_is_replaced_and_the_rest_survives() {
        let (text, redacted) = redact_doc("Usage notes.\npassword = \"hunter2\"\nEnd of notes.");
        assert!(redacted);
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("Usage notes."));
        assert!(text.contains("End of notes."));
    }

    #[test]
    fn a_secret_in_mid_sentence_prose_is_caught_too() {
        // The shared pattern table matches anywhere in the line, not only at the
        // start — so a credential buried in a sentence is redacted just like an
        // assignment on its own line. This is the property that makes reusing the
        // external extractor's table load-bearing rather than incidental.
        let (text, redacted) =
            redact_doc("The default is password = \"x1234secret\" until rotated.");
        assert!(redacted);
        assert!(!text.contains("x1234secret"), "{text}");
    }

    #[test]
    fn an_ordinary_doc_comment_passes_through_byte_for_byte() {
        let original = "Vypočítá součet řádků.\n\nDruhý odstavec.";
        let (text, redacted) = redact_doc(original);
        assert!(!redacted);
        assert_eq!(text, original);
    }

    #[test]
    fn an_anchor_states_only_what_provenance_knows() {
        let range = SourceRef {
            file: "a.cs".into(),
            span: Span::range(12, 88),
        };
        let point = SourceRef {
            file: "a.cs".into(),
            span: Span::Line(12),
        };
        let bare = SourceRef {
            file: "a.cs".into(),
            span: Span::Whole,
        };
        assert_eq!(range.anchor(), "a.cs:12-88");
        assert_eq!(point.anchor(), "a.cs:12");
        assert_eq!(bare.anchor(), "a.cs");
    }

    fn card_stub(identity: &str) -> Card {
        Card {
            identity: identity.to_string(),
            path: String::new(),
            kind: CardKind::Type,
            kind_word: "type".into(),
            title: "stub".into(),
            entity_kind: None,
            visibility: None,
            signature: None,
            header: None,
            source: None,
            doc: None,
            members: Vec::new(),
            outgoing: Vec::new(),
            incoming: Vec::new(),
            externals: Vec::new(),
            maybe_also: Vec::new(),
            test: None,
            caveats: Vec::new(),
            story: Default::default(),
            navigators: Default::default(),
            detail: None,
            map: None,
            generated: false,
        }
    }
}
