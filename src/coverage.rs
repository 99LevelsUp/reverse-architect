//! The coverage artifact (U7): what the corpus is allowed to claim.
//!
//! R20 lets the corpus say "this symbol is used nowhere" only inside the range an
//! external index resolved. Everywhere else it must say "not found" and point at
//! the evidence. This module produces that evidence as `coverage.json`: for every
//! tracked file, which extractors touched it, whether the grammar produced
//! structural facts, and whether an index verified it.
//!
//! It is an artifact, not a sentence in `overview.md` (U7 step 2). The prose view
//! stays for a human reader; a consumer that must decide whether an absence claim
//! is licensed needs a machine-readable file with stable ordering.
//!
//! ## A projection, not an extractor
//!
//! The plan filed this under `src/extract/`, but `extract` is documented as
//! "sources that populate the fact graph" and this populates nothing — it reads
//! the finished graph exactly as [`crate::manifest`] does. It lives beside the
//! manifest instead, for the same reason the manifest is not an extractor.
//!
//! ## Why the index diagnostics are a sentence
//!
//! Per-file resolution is structured, because a file is a graph node. The index's
//! own counts — documents that drifted, documents that named nothing git tracks —
//! are NOT recomputable here: a document that matched no tracked file produced no
//! node at all, so nothing in the graph remembers it. Those counts exist only in
//! the summary [`FactKind::ResolvedCoverage`] the scip extractor recorded, and are
//! carried through verbatim rather than re-derived from something that cannot know
//! them.
//!
//! ## What "resolved" does and does not assert
//!
//! A file is `resolved` here when an index covered it AND the drift gate in
//! [`crate::extract::scip`] accepted it. A SCIP index carries no source hashes, so
//! this is agreement between the index and the grammar — not proof the file is
//! byte-identical to what was indexed. R6 asks for the revision the index was
//! built against; the index does not record one, so the artifact records the
//! revision of the repository it was PRODUCED over and leaves the index's own
//! revision to the caller who supplied it.

use crate::extract::languages::Languages;
use crate::graph::{
    Confidence, DbAccessKind, DecisionShape, EdgeKind, EnumDecision, FactKind, FactValue, Graph,
    NodeKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod markup;
pub use markup::{
    markup, markup_state, FailedMarkup, Markup, MarkupSites, MarkupState, UnparsedMarkup,
    MARKUP_LANGUAGE,
};

/// Bumped whenever the shape below changes, alongside the manifest's own version
/// (U7 step 3) — a consumer pins one number, not a guess about field presence.
///
/// v6 (plan B, the one bump for the whole of area B — later units on that
/// branch extend this paragraph rather than bump again): C and C++ are
/// languages (U4). Their files are named in `files[].language` and counted in
/// `languages[]` instead of `unclassified_files`, and since no plugin reads
/// them, `files[].not_structural_because` says so with a reason of its own —
/// [`LEXED_ONLY`] — rather than the false "a grammar covers this file type".
/// No field changed shape, but a consumer that reads the reasons as a closed
/// set meets a string it does not know. Also in v6 (U5): `enum_decisions`,
/// the places C# code decides by the value of an enum the repository
/// declares, counted by what the host could make of them — written by every
/// run, zeros included, like `noise`. Also in v6 (U12): `anchors` (texts kept,
/// dropped by the cap, withheld by reason) and `db_accesses` (by state),
/// written by every run like `enum_decisions`; and `cards` — the type cards
/// whose purpose has no comment sentence, the details written and left out,
/// the maps and their parts, and the sizes in bytes of a map row, a type card
/// and a detail (count, median, p90) — present whenever the run built cards,
/// so an artifact a caller built from the graph alone does not carry it.
/// Also in v6 (U13): `model_purpose`, what generation did with the purpose
/// layer `verify --purpose` wrote — records used, stale and unmatched —
/// present only for a run asked to read it (`--model-purpose`), so every
/// other artifact is byte for byte what it was. And from plan 2a, U4:
/// `bodies`, what the second layer read and paired and where it failed,
/// present only for a run with `--detail-level` on the same terms — no bump,
/// since no other artifact changes by a byte. And from plan 2c, U8 (KTD12):
/// `markup`, the state of XAML coverage for the whole run — not read, read
/// with unsupported constructs, read in full, or the second layer off — with
/// the files XAML failed on, the components that read no bodies, the files
/// with a markup extension the plugin could not take apart (`unparsed`,
/// written only when there is one) and the markup places the second layer
/// could not pair. Present only when a plugin
/// of the run claims XAML or the run has the second layer, so every artifact
/// of a run with neither is byte for byte what it was; for that reason, and
/// because a declaring corpus is already one version up, no bump either.
///
/// v5: `noise` — how much noise the graph and the cards carry (plan A, U4:
/// R21, KTD14). Written by every run, so the version rises for every corpus,
/// not only for the ones with something to report. Two parts are measured
/// since U4: the name-resolved edges per confidence rung, and the card rows the
/// card filter exists to remove (duplicates and reversed rows), counted before
/// and after it. The rest of the section was **reserved** at first — written
/// with zero or empty values — and the units that produce the data fill it
/// without changing the shape: plugins per contract version since U5,
/// deletions per rule and sites over the cap since U7, the index match since
/// U9 — which also gave `resolved_index` the index's SHA-256. The index match
/// took its final shape in U9 (five counters instead of the two it was
/// reserved with); v5 was unreleased then, so neither moved the version.
///
/// Also in v5, from plan A, U5 on: `noise.plugins_by_contract` is filled, and
/// `languages[].contracts` names the plugin contract versions a language was
/// parsed under. Both arrived before v5 was released, so neither moved the
/// version; the per-language field is absent when empty, so an artifact built
/// from a registry no plugin set was recorded into reads as before.
///
/// Also in v5, from plan A, U12 on: `model_verification`, what generation did
/// with the layer `verify` wrote — records used, stale and unmatched. Present
/// only for a run asked to read the layer (`--model-verification`), so every
/// other artifact is byte for byte what it was; v5 was unreleased, so the
/// version did not move.
///
/// v4: `language` is registry-backed, so a language this build was not
/// compiled with is named rather than reported as `null` (R26). That half is
/// reachable today, through the registry `build_coverage` now takes.
///
/// Also in v4, and NOT yet reachable: `files[].not_structural_because` gained
/// the four plugin reasons R14 and R29 need — plugin missing, interface
/// version mismatch, unsatisfied import, failed mid-run. The vocabulary and
/// its derivation exist in the Rust API, but nothing can load a plugin yet,
/// so `build_coverage` passes [`LanguageStatus::Parsed`] for every file and a
/// v4 artifact from this build still carries only the two pre-plugin reasons.
/// U10 wires the failure signal that makes them observable. A consumer should
/// read this entry as "the field's vocabulary is reserved", not as "these
/// strings can appear now" — pinning v4 buys the `language` change, and
/// forward-compatible handling for reasons that arrive later.
///
/// v3: `modules[].resolved` — how many of a module's files an index verified.
/// `fully_resolved: false` alone could not tell eleven of twelve from zero of
/// twelve, and closing that gap from the `files` array meant re-implementing
/// this artifact's own containment rule.
///
/// v2: `unclassified_files` — the tracked files belonging to no language the
/// build recognizes, so `sum(languages[].files)` and `files.len()` close.
pub const SCHEMA_VERSION: u32 = 6;

/// The version a coverage artifact gets when it carries `declared_kinds_excluded`
/// — that is, when the corpus declared node kinds of its own (U6, R12).
///
/// The same *shape* as the manifest's
/// [`crate::manifest::SCHEMA_VERSION_WITH_VOCABULARY`] — a corpus with no
/// declarations keeps [`SCHEMA_VERSION`] and stays byte-identical to what this
/// tool writes without the vocabulary — but **deliberately not the same predicate**,
/// and an earlier draft of this comment said "same rule", which was wrong.
///
/// The manifest rises when the corpus declared **anything**, because it
/// publishes all four categories. This artifact rises only when the corpus
/// declared a **node kind**, because the only thing it has to say about the
/// vocabulary is which nodes it did not count — and a corpus that declared only
/// fact or edge kinds excluded no nodes, so an empty section here would be noise
/// rather than information.
///
/// **A consumer must therefore not assume the two `schema_version` fields rise
/// together.** A manifest at 11 beside a coverage at 6 (10 beside 5 before plan
/// B) is a well-formed pair and means "this corpus declared kinds, none of them
/// node kinds". Found in RG2 by two reviewers independently.
///
/// It moves in step with [`SCHEMA_VERSION`], one above it, because a declaring
/// corpus carries every change the plain one does: 5 with v4's registry-backed
/// languages, 6 with v5's `noise`, 7 with v6's C and C++, enum decisions,
/// anchors, database accesses, card levels and model purpose.
pub const SCHEMA_VERSION_WITH_VOCABULARY: u32 = 7;

/// The coverage artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub schema_version: u32,
    /// Git revision of the repository this run read, when it has one. `None` for a
    /// repository with no commits — stated as absent rather than as an empty
    /// string, which would read like a revision.
    pub repo_revision: Option<String>,
    /// Present only when the run was given an index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_index: Option<ResolvedIndex>,
    /// Per-language totals. Every count here is the size of a subset of `files`,
    /// and the subsets do NOT cover it — see `unclassified_files`.
    pub languages: Vec<LanguageCoverage>,
    /// Tracked files no registered language claims: a `.csproj`, a `.json`, a
    /// file with no extension.
    ///
    /// "Registered", not "compiled in", since U4: a language a plugin brought
    /// takes its files out of this count like any other, so the number stays a
    /// statement about this run rather than about this build.
    ///
    /// Stated as its own number so the totals close. Without it a consumer who
    /// sums `languages[].files` gets less than `files.len()` and has no way to
    /// tell a classification gap from a missing file — the exact silence this
    /// artifact exists to prevent. The invariant is:
    /// `sum(languages[].files) + unclassified_files == files.len()`.
    pub unclassified_files: usize,
    /// Every tracked file, analyzed or not. A file is never omitted: silence is
    /// what this artifact exists to prevent.
    pub files: Vec<FileCoverage>,
    /// Modules, with the one claim a module can carry honestly.
    pub modules: Vec<ModuleCoverage>,
    /// What this artifact did NOT count, when the corpus declared kinds of its
    /// own (U6). Absent when it declared none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_kinds_excluded: Option<DeclaredExclusion>,
    /// How much noise the graph and the cards carry (v5, R21). Every run from
    /// v5 on writes it; it is an `Option` only so an artifact from before v5
    /// still reads, as `None` rather than as counters that claim zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
    /// What generation did with the model verification layer (plan A, U12,
    /// KTD12). Present only when the run was asked to read it — absent, not
    /// zero, otherwise: a run that never opened the layer has nothing to say
    /// about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_verification: Option<ModelVerification>,
    /// The places C# code decides by an enum's value (plan B, U5: KTD3).
    /// Every run from v6 on writes it; an `Option` only so an artifact from
    /// before v6 still reads, as `None` rather than as zeros it never counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_decisions: Option<EnumDecisions>,
    /// The texts the host lexer read for anchors, by what became of them
    /// (plan B, U12: R9, R10, KTD1). Every run from v6 on writes it, zeros
    /// included; an `Option` only so an artifact from before v6 still reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchors: Option<Anchors>,
    /// The database accesses the SQL in literals makes, by state (plan B,
    /// U12: R11, KTD2). Written by every run from v6 on, as `anchors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_accesses: Option<DbAccesses>,
    /// What the three levels — maps, short type cards, details — came to, and
    /// their sizes (plan B, U12: R6, R17, KTD13). Counted over the rendered
    /// card corpus, so generation sets it ([`card_levels`]); absent when the
    /// caller built no cards — zeros there would read as "measured, and
    /// empty", which nothing measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<CardLevels>,
    /// What generation did with the purpose layer `verify --purpose` wrote
    /// (plan B, U13: R7, KTD12). Present only when the run was asked to read
    /// it (`--model-purpose`), as `model_verification` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_purpose: Option<ModelPurpose>,
    /// What the second layer read (plan 2a, U4: KTD6, KTD8): the level, the
    /// plugins, how its bodies paired with the first layer, what paired with
    /// nothing, and every file it failed on. Present only for a run with
    /// `--detail-level` ([`bodies`]), as `model_purpose` is for its flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bodies: Option<Bodies>,
    /// The state of XAML coverage of the run (plan 2c, U8: R19, KTD12) —
    /// what licenses a card to read a missing dynamic-call marker as the
    /// absence of a way from XAML. Present only when a plugin of the run
    /// claims XAML or the run has the second layer ([`markup`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markup: Option<Markup>,
}

/// The second layer of a run, as coverage reports it (plan 2a, U4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bodies {
    /// `--detail-level`.
    pub detail_level: u8,
    /// The plugins that could read bodies, by id, in routing order.
    pub plugins: Vec<String>,
    /// Files whose bodies were read.
    pub files: u64,
    /// Member bodies read, and how they found their declaration: by the
    /// position of its name, or as the nearest declaration around them.
    pub members: u64,
    pub paired_by_name: u64,
    pub paired_by_enclosing: u64,
    /// Bodies that paired with no declaration and have none around them —
    /// kept in the model and listed here, never dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpaired: Vec<UnpairedBody>,
    /// Type shapes, typed members and declaration metas whose name position
    /// is no declaration's.
    pub unpaired_declarations: u64,
    /// Every file the `bodies` pass failed on: it keeps its first layer and
    /// loses its second.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<FailedBodies>,
    /// Declarations written in several files — the parts of a partial type —
    /// rendered without their second layer because one part failed although
    /// another was read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withheld: Vec<String>,
    /// The walk back over the callers (plan 2b, KTD5) stopped at the
    /// functionality nearest each trigger, because it gave more than ten
    /// times as many rows as occurrences of triggers. Absent when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restricted_walk: Option<RestrictedWalk>,
}

/// What the walk back gave a run whose registry was cut at the nearest
/// functionality: rows of C# only, the registry adds those of markup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestrictedWalk {
    /// The rows the full, transitive walk gave.
    pub transitive_rows: u64,
    /// The distinct occurrences of triggers among them (kind, key, member,
    /// place).
    pub trigger_occurrences: u64,
    /// The rows kept at the nearest functionality.
    pub rows: u64,
}

impl RestrictedWalk {
    /// What coverage says of the walk `walk`: `None` unless it was cut.
    pub fn of(walk: &crate::card::layer2::WalkCounts) -> Option<RestrictedWalk> {
        walk.nearest.then_some(RestrictedWalk {
            transitive_rows: walk.transitive_rows as u64,
            trigger_occurrences: walk.occurrences as u64,
            rows: walk.rows as u64,
        })
    }
}

/// One body the second layer could pair with nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnpairedBody {
    pub path: String,
    pub line: u32,
    pub column: u32,
    /// The kind of declaration it is the body of (`method`, `getter`, …).
    pub kind: String,
}

/// One file the `bodies` pass failed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedBodies {
    pub path: String,
    pub plugin: String,
    /// Redacted, as the plugin's every text.
    pub reason: String,
}

/// The second layer's section of coverage, read off the run's model.
pub fn bodies(layer: &crate::extract::bodies::BodyLayer) -> Bodies {
    let counts = layer.counts();
    Bodies {
        detail_level: layer.level(),
        plugins: layer.readers().iter().map(|r| r.id.clone()).collect(),
        files: layer.files().count() as u64,
        members: counts.members,
        paired_by_name: counts.paired_by_name,
        paired_by_enclosing: counts.paired_by_enclosing,
        unpaired: layer
            .unpaired()
            .iter()
            .map(|u| UnpairedBody {
                path: u.path.clone(),
                line: u.position.line,
                column: u.position.column,
                kind: u.kind.word().to_string(),
            })
            .collect(),
        unpaired_declarations: counts.unpaired_declarations,
        failed: layer
            .failures()
            .map(|(path, f)| FailedBodies {
                path: path.to_string(),
                plugin: f.plugin.clone(),
                reason: f.reason.clone(),
            })
            .collect(),
        withheld: layer
            .withheld()
            .into_iter()
            .map(|k| k.as_str().to_string())
            .collect(),
        restricted_walk: None,
    }
}

/// The purpose layer's records, by what became of each one (plan B, U13).
/// The three partition the layer: every record is exactly one of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPurpose {
    /// Records shown on their type's card: the facts the model was given are
    /// the type's facts still, in the language and from the backend the
    /// layer names.
    pub used: u64,
    /// Records whose type is still there but whose facts — or the language
    /// asked for — changed since the model answered. Not shown; `verify
    /// --purpose` asks again.
    pub stale: u64,
    /// Records whose type has no card any more.
    pub unmatched: u64,
}

/// The anchors of the graph (plan B, U12), read off the facts the external
/// extractor left. `kept` and `dropped_by_cap` count texts; `withheld` counts
/// places, by the reason the detail shows for each.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchors {
    /// Texts kept as anchors — one per distinct text per owner per file,
    /// as the detail lists them.
    pub kept: u64,
    /// Texts an owner had beyond [`crate::graph::ANCHOR_CAP`] in one file and
    /// lost to it (KTD1).
    pub dropped_by_cap: u64,
    /// Places a text was withheld, by reason (R10): the secret pattern's name
    /// (`password`, `private key`, …) or `long token`, for a comment line or
    /// a literal alike — the two facts the detail shows as one kind of row.
    /// The reason is all that is said; the text never reaches this artifact.
    pub withheld: BTreeMap<String, u64>,
}

/// The database accesses of the graph by state (plan B, U12: KTD2), one per
/// occurrence. `mentions` is the share whose statement's target or alias the
/// host could not tell, where a card claims no more than that the code names
/// the object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbAccesses {
    pub reads: u64,
    pub writes: u64,
    pub calls: u64,
    pub mentions: u64,
}

/// The card levels of one run (plan B, U12).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardLevels {
    /// Type cards — the short cards, the level the size budget is set on.
    pub type_cards: u64,
    /// …whose purpose has no sentence from the type's doc comment, only what
    /// the structure says (R6).
    pub purpose_without_comment: u64,
    /// …that carry no keyword.
    pub without_keywords: u64,
    pub details: DetailLevel,
    pub maps: MapLevel,
    pub sizes: LevelSizes,
}

/// The details a run wrote, and the ones it left out (R3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailLevel {
    /// Type cards with a detail.
    pub types: u64,
    /// Type cards without one: the card says everything a detail would.
    pub types_without: u64,
    /// File cards with a detail — the texts and accesses no type owns.
    pub files: u64,
    /// Details cut into `## Part N` (KTD8).
    pub split: u64,
}

/// The maps a run wrote (R1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapLevel {
    pub maps: u64,
    /// Maps cut into `## Part N` (KTD8).
    pub split: u64,
    /// The parts of the maps that were cut.
    pub parts: u64,
}

/// Sizes in bytes per level (KTD13) — bytes, not tokens: the repository has
/// no tokenizer, and Czech tokenizes worse than English, so a budget in bytes
/// is the one a fixture can hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelSizes {
    /// One row of a map, its line break included ([`crate::card::map::MAP_ROW_BYTES`]
    /// counts the same way) — the merged row of generated types too.
    pub map_row: Sizes,
    /// One type card, whole.
    pub type_card: Sizes,
    /// One detail, whole — a type's or a file's.
    pub detail: Sizes,
}

/// The distribution of one level's sizes, in integers only, so two runs give
/// the same bytes (R15). Both statistics are nearest-rank: the value at
/// position `ceil(p × count / 100)` of the ascending sizes, a size that
/// occurred — the median of an even count is the lower of the middle two.
/// All three are 0 when there is nothing of the level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sizes {
    pub count: u64,
    pub median: u64,
    pub p90: u64,
}

impl Sizes {
    fn of(mut sizes: Vec<u64>) -> Sizes {
        sizes.sort_unstable();
        let count = sizes.len() as u64;
        // `ceil(p × n / 100)`, 1-based; 0 only for an empty level.
        let rank = |p: u64| (p * count).div_ceil(100);
        let at = |p: u64| rank(p).checked_sub(1).map_or(0, |i| sizes[i as usize]);
        Sizes {
            count,
            median: at(50),
            p90: at(90),
        }
    }
}

/// The card levels of a rendered corpus (plan B, U12). `card_files` is the
/// rendered `cards`, one per card in the same order (as
/// [`crate::card::render_card_files`] returns them); `details` and `maps` the
/// rendered details and maps.
///
/// Passed the rendered files rather than re-rendering anything: the sizes are
/// of the bytes written, and the parts are counted from the `## Part N`
/// headings the files carry.
pub fn card_levels(
    cards: &[crate::card::Card],
    card_files: &[crate::render::RenderedFile],
    details: &[crate::render::RenderedFile],
    maps: &[crate::render::RenderedFile],
) -> CardLevels {
    use crate::card::{purpose::Origin, CardKind};
    let mut levels = CardLevels::default();
    let mut card_sizes: Vec<u64> = Vec::new();
    for (card, file) in cards.iter().zip(card_files) {
        match card.kind {
            CardKind::Type => {
                levels.type_cards += 1;
                card_sizes.push(file.contents.len() as u64);
                levels.purpose_without_comment += u64::from(
                    !card
                        .story
                        .purpose
                        .iter()
                        .any(|s| s.origin == Origin::Comment),
                );
                levels.without_keywords += u64::from(card.story.keywords.is_empty());
                match card.detail {
                    Some(_) => levels.details.types += 1,
                    None => levels.details.types_without += 1,
                }
            }
            CardKind::File => levels.details.files += u64::from(card.detail.is_some()),
            CardKind::Module | CardKind::Declared(_) => {}
        }
    }
    let parts = |contents: &str| {
        contents
            .lines()
            .filter(|l| l.starts_with("## Part "))
            .count() as u64
    };
    let mut detail_sizes: Vec<u64> = Vec::new();
    for detail in details {
        detail_sizes.push(detail.contents.len() as u64);
        levels.details.split += u64::from(parts(&detail.contents) > 0);
    }
    let mut row_sizes: Vec<u64> = Vec::new();
    for map in maps {
        levels.maps.maps += 1;
        let n = parts(&map.contents);
        if n > 0 {
            levels.maps.split += 1;
            levels.maps.parts += n;
        }
        // A row and its line break.
        row_sizes.extend(crate::card::map::rows(&map.contents).map(|row| row.len() as u64 + 1));
    }
    levels.sizes = LevelSizes {
        map_row: Sizes::of(row_sizes),
        type_card: Sizes::of(card_sizes),
        detail: Sizes::of(detail_sizes),
    };
    levels
}

/// The decisions by an enum's value, by what the host made of each place
/// (plan B, U5: KTD3). The four partition the places whose qualifier names an
/// enum the repository declares: every such place is exactly one of them. A
/// place whose qualifier names no such enum — `case Consts.Max:` — is no
/// decision by an enum and is not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumDecisions {
    /// `case` labels and switch expression arms tied to the one type of that
    /// name the place can see, an enum with that value.
    pub verified: u64,
    /// Labels and arms tied to one enum, but where another type of the name
    /// is visible, an alias points elsewhere, or the enum is not visible at
    /// all — the place may not be the enum's.
    pub unverified: u64,
    /// Comparisons — `==`, `!=`, `is` — tied to one enum. Never verified: a
    /// local or a lambda parameter can stand in for the type, which the host
    /// cannot see without a syntax tree.
    pub comparisons: u64,
    /// Places tied to no enum: of the enums of that name, none has the value,
    /// or several have it and visibility does not choose. They leave no fact
    /// but this count.
    pub unmatched: u64,
}

/// The layer's records, by what became of each one (plan A, U12). The three
/// partition the layer: every record is exactly one of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelVerification {
    /// Records applied to the cards: the site is still an ambiguous site no
    /// index judged, and its request — code around it, candidates and their
    /// signatures, prompt, redaction, backend — is unchanged.
    pub used: u64,
    /// Records whose site is still there but whose request changed since the
    /// model answered. Not applied; `verify` asks again.
    pub stale: u64,
    /// Records whose site is gone, or is no longer one the layer may speak
    /// about: the rules settled it, or an index judged it.
    pub unmatched: u64,
}

/// The noise counters (plan A, U4: R21, KTD14) — numbers a test can put a
/// threshold on, so a resolver or card regression is a red test rather than a
/// card a reader has to notice is wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Noise {
    /// Name-resolved edges per confidence rung, counted from the graph.
    pub edges_by_rung: EdgesByRung,
    /// The card rows the card filter exists to remove, counted by the card
    /// layer before and after it. Absent when the caller built no cards —
    /// zeros there would read as "measured, and clean", which nothing measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_rows: Option<CardRows>,
    /// Candidates a rule with the right to delete removed, per rule id —
    /// one per (site, candidate), counted by the shared resolver since plan A,
    /// U7. Empty while no rule holds a right (R11): the shipped rights table
    /// grants none until a measurement earns one.
    pub deleted_by_rule: BTreeMap<String, u64>,
    /// Usage sites that ended over the ambiguity ceiling — one ambiguity fact
    /// each and no edges — counted per site, not per fact (since U7).
    pub over_cap_sites: u64,
    /// How many plugins of the run run under each contract version (`"0.1"`
    /// → count), bundled and declared alike — a plugin every one of whose
    /// extensions another plugin took first included, since it was loaded
    /// all the same. Filled since plan A, U5; empty when the artifact was
    /// built from a registry no plugin set was recorded into.
    pub plugins_by_contract: BTreeMap<String, u64>,
    /// How far the heuristic usage sites and an external index paired up
    /// (plan A, U9, KTD8). Absent without an index — absent, not zero,
    /// because a run without one has no rate to report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_match: Option<IndexMatch>,
}

/// Every edge whose provenance carries a confidence rung — the edges name
/// resolution produced, by the tree-sitter resolver or by an index — counted
/// once per rung (R2). Structural edges (`contains`, `declares`, `has_member`,
/// `imports`) carry no rung and are not counted.
///
/// A struct with all four rungs rather than a map of the rungs that occurred,
/// so the four fields always sum to the number of name-resolved edges and a
/// consumer never has to infer a zero from an absent key. The field names are
/// the manifest's own words for the rungs; a test holds the two together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgesByRung {
    pub resolved_by_index: u64,
    pub rules_backed: u64,
    pub name_only: u64,
    pub ambiguous: u64,
}

/// The noise in the cards' two relation lists, `Relations` (outgoing) and
/// `Referenced by` (incoming), summed over every card. `externals` and
/// `Maybe also` are not counted: the card filter does not clean them, so a
/// count there would guard nothing.
///
/// Each counter is taken twice with the same predicate: over the lists as the
/// card layer first assembles them from the graph, and over the lists the card
/// finally carries. The first shows what the resolver and the graph hand the
/// cards — a rise there is a resolver regression even while the cards stay
/// clean. The second guards the card itself and must be 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardRows {
    /// Rows of one list that repeat a relation an earlier row of the same list
    /// already states: the same edge kind, the same neighbour, and the same
    /// member of this card (`via`, by identity, so two overloads are two
    /// members). A relation stated three times counts 2. This is what the
    /// card's per-relation dedup removes; on a merged graph, which holds one
    /// edge per relation, it is 0 before the filter as well.
    pub duplicate_rows: BeforeAfter,
    /// Rows whose neighbour belongs to the card's own unit — a call between
    /// two members of one type, or of one file's free functions. Such a call
    /// is the unit's inside, and before the filter it showed twice, once in
    /// each list, with the `Referenced by` half reading backwards (R17).
    /// Counted per row, so one such call counts 2 before the filter.
    pub reversed_rows: BeforeAfter,
}

/// One counter, before and after the card filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeforeAfter {
    pub before_filter: u64,
    pub after_filter: u64,
}

/// How far the usage sites paired with an index (plan A, U9, KTD8). The four
/// outcomes partition `considered`, so the match rate is `matched /
/// considered` and nothing is left for a reader to infer:
/// `considered == matched + external + unverified_document + unmapped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMatch {
    /// Usage sites in files the index has a document for. A site in a file
    /// the index never saw — another language, a project nobody indexed — is
    /// not a pairing that failed, so it is not counted at all.
    pub considered: u64,
    /// …paired with exactly one occurrence whose symbol the index defines in
    /// a verified document of the repository.
    pub matched: u64,
    /// …paired with exactly one occurrence of a symbol from outside the
    /// repository. A verdict all the same: every repository candidate of such
    /// a site is only a guess.
    pub external: u64,
    /// …in a document the drift check did not verify, which is not paired at
    /// all — an index that disagrees with the tree about a file answers no
    /// question about it.
    pub unverified_document: u64,
    /// …in a verified document and not paired: no occurrence, several, a range
    /// the merged index marks as conflicting, or a symbol with no single
    /// verified definition. Refutes nothing.
    pub unmapped: u64,
}

/// The declared kinds this artifact left out, named.
///
/// Coverage is a statement about **tracked source files** and how far the
/// language extractors reached over them. A declared node is not a tracked file
/// — nothing parsed it, no grammar produced structure for it — so counting it
/// would change the meaning of every percentage this artifact has ever
/// published. Excluding it is therefore right, and the whole content of this
/// struct is that excluding it **silently** would not be: a reader comparing
/// `files.len()` against the node count of a corpus with declared kinds would
/// otherwise find a gap with nothing to explain it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeclaredExclusion {
    /// Qualified names (`namespace:name`) of the node kinds this corpus
    /// declared, in name order. Names, not counts alone: "three nodes were not
    /// counted" is not something a reader can act on, and `open:tabulka` is.
    pub node_kinds: Vec<String>,
    /// How many nodes of those kinds the graph held.
    pub nodes: usize,
    /// In words, so the number above cannot be read as a coverage gap. The same
    /// discipline `not_structural_because` and `ResolvedIndex.summary` follow.
    pub note: String,
    /// Keys of declared containers the module tree cannot hold, because their
    /// parent is no container of it ([`Graph::orphan_containers`], R-30). They
    /// stay in the graph and the manifest; the backbone views leave them out.
    /// Absent when there is none, so a corpus without one keeps its bytes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_containers: Vec<String>,
}

/// What the run was told about the index, and what consuming it came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedIndex {
    /// The indexer as the index itself named it (`"scip-dotnet 0.2.14"`).
    pub indexed_by: String,
    /// The scip extractor's own summary, verbatim — see the module docs for why
    /// this is a sentence and not three numbers.
    pub summary: String,
    /// SHA-256 of the index file the run read, lowercase hex (plan A, U9):
    /// the same path can hold a different index on the next run, and the
    /// pairing this artifact reports is only reproducible against these bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageCoverage {
    pub language: String,
    /// Tracked files of this language.
    pub files: usize,
    /// …of which carry tree-sitter structural facts.
    pub structural: usize,
    /// …of which an index verified.
    pub resolved: usize,
    /// The plugin contract versions this language's files were parsed under
    /// (`0.1`, `0.2` — plan A, U5), ascending. One entry in the ordinary
    /// case; two when its extensions are parsed by plugins of different
    /// versions (a declared plugin taking `.ts` beside the bundled one keeping
    /// `.tsx`), both stated rather than one picked. A 0.1 plugin's sites
    /// carry no context, so this is what says whether a language's edges
    /// could have been sifted at all. Absent when no plugin of the run was
    /// recorded in the registry the artifact was built from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contracts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileCoverage {
    pub path: String,
    /// `None` for a file no registered language claims. Registry-backed
    /// since U4, so a language a plugin brought is named here like any
    /// other — the field no longer means "one of the six we compiled in".
    pub language: Option<String>,
    /// Which extractors asserted anything about this file.
    pub extractors: Vec<String>,
    /// Whether the grammar produced structural facts (the file declares at least
    /// one symbol). This is the honest reading of "analyzed": a supported file
    /// with no declaration is covered by a grammar and still has no structure.
    pub structural: bool,
    /// `resolved` only for a file an index covered and the drift gate accepted.
    pub resolution: String,
    /// Why a file carries no structural facts. Present exactly when
    /// `structural` is false, so a reader never has to infer the reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_structural_because: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleCoverage {
    pub path: String,
    /// Files anywhere beneath this module.
    pub files: usize,
    /// …of which an index verified.
    ///
    /// `fully_resolved` answers whether there is a licence; this answers how far
    /// off one is, which is the question a reader of coverage actually arrives
    /// with. Eleven of twelve and zero of twelve are the same `false`, and
    /// telling them apart used to mean re-implementing this artifact's own
    /// module-to-file containment rule against the `files` array.
    pub resolved: usize,
    /// True only when EVERY file beneath it is resolved. A partially covered
    /// module is not marked — that is the whole point of the flag: it is the
    /// licence for an absence claim across the module, and a partial licence is
    /// no licence (R20).
    ///
    /// Derivable from the two counts above, and kept anyway: deriving a licence
    /// to claim absence is exactly the arithmetic a consumer should not be doing
    /// for itself.
    pub fully_resolved: bool,
}

/// No registered language claims this file's extension.
pub const NO_GRAMMAR: &str = "no grammar covers this file type";
/// A language claims it, its extractor ran, and the file declares nothing.
pub const NO_DECLARATION: &str = "a grammar covers this file type but it declares no symbol";
/// A language claims it, but no plugin reads its declarations — only the host
/// lexer reads its literals and comments (plan B, U4: C and C++).
pub const LEXED_ONLY: &str =
    "no plugin reads this file type's declarations; only its literals and comments are read";

/// Why a file's language produced no structure, beyond "it declared nothing".
///
/// This exists because the old two-arm derivation keyed on the language alone,
/// and a failed plugin still HAS a language: such a file would have been told
/// "a grammar covers this file type but it declares no symbol", which is a
/// false statement about a file nothing ever parsed (R14, R29).
///
/// Only [`LanguageStatus::Parsed`] is produced today — nothing can yet load a
/// plugin, so nothing can yet fail to. The other four are the vocabulary U10
/// fills in when the plugin lifecycle lands; they are written and tested here
/// so U10 wires a failure signal into a shape that already exists rather than
/// redesigning the artifact around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageStatus {
    /// The extractor for this language ran to completion on this file.
    Parsed,
    /// A plugin was declared for this language and never loaded (R14).
    PluginMissing { plugin: String },
    /// The plugin loaded but implements a different interface version (R14).
    VersionMismatch {
        plugin: String,
        wanted: String,
        found: String,
    },
    /// The plugin needs a host interface this build does not provide (R14).
    ///
    /// Names the interface and version rather than just failing, so an operator
    /// can tell an out-of-date host from a plugin that was never installed —
    /// two very different fixes behind one "it did not run".
    UnsatisfiedImport {
        plugin: String,
        interface: String,
        version: String,
    },
    /// The plugin loaded, ran, and failed on this unit of work (R29), with the
    /// pass and the reason — already redacted by the extractor that recorded
    /// it, as every extractor-supplied string is.
    Failed { plugin: String, reason: String },
    /// No plugin reads this language; the host lexer read its literals and
    /// comments and nothing else (plan B, U4). Saying "a grammar covers this
    /// file type but it declares no symbol" about a C file would be false.
    LexedOnly,
}

/// Why a file carries no structural facts, or `None` when it carries some.
///
/// Keyed on the status as well as the language — see [`LanguageStatus`].
pub fn not_structural_because(
    structural: bool,
    language: Option<&str>,
    status: &LanguageStatus,
) -> Option<String> {
    if structural {
        return None;
    }
    Some(match status {
        LanguageStatus::Parsed => match language {
            None => NO_GRAMMAR.to_string(),
            Some(_) => NO_DECLARATION.to_string(),
        },
        LanguageStatus::PluginMissing { plugin } => {
            format!("the plugin `{plugin}` declared for this language was not loaded")
        }
        LanguageStatus::VersionMismatch {
            plugin,
            wanted,
            found,
        } => format!(
            "the plugin `{plugin}` implements interface version {found}, but this host requires {wanted}"
        ),
        LanguageStatus::UnsatisfiedImport {
            plugin,
            interface,
            version,
        } => format!(
            "the plugin `{plugin}` requires the interface `{interface}` version {version}, which this host does not provide"
        ),
        LanguageStatus::Failed { plugin, reason } => {
            format!("the plugin `{plugin}` failed while parsing this file ({reason})")
        }
        LanguageStatus::LexedOnly => LEXED_ONLY.to_string(),
    })
}

/// Build the coverage artifact from the finished graph.
///
/// `repo_revision` is passed in rather than read here, so this stays a pure
/// projection over the graph (the same discipline that keeps `manifest` free of
/// I/O). `card_rows` is passed in for the same reason (KTD14): the card noise
/// is a property of the cards, which the card layer counts while it builds
/// them ([`crate::card::build_cards_with_insight`]), and re-deriving it here
/// would mean re-implementing the card filter this counter exists to check.
/// `None` when the caller built no cards.
pub fn build_coverage(
    graph: &Graph,
    repo_revision: Option<String>,
    languages: &Languages,
    card_rows: Option<CardRows>,
) -> Coverage {
    // Files that declare at least one symbol — the grammar produced structure.
    let mut declaring: BTreeSet<&str> = BTreeSet::new();
    // Module → the files beneath it, at any depth.
    let mut module_files: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    // Extractors that asserted something ABOUT a file through an edge rather than
    // a fact on it. filetree is the case that matters: it creates the File node
    // and its `Contains` edge but puts no fact on the node, so a facts-only view
    // would report the file as untouched by the extractor that found it.
    let mut edge_extractors: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in graph.edges_sorted() {
        for endpoint in [edge.from.as_str(), edge.to.as_str()] {
            if graph
                .node(&crate::graph::NodeKey::new(endpoint))
                .is_some_and(|n| n.kind == NodeKind::File)
            {
                edge_extractors
                    .entry(endpoint)
                    .or_default()
                    .insert(&*edge.edge.provenance.extractor);
            }
        }
        match edge.edge.kind {
            EdgeKind::Declares => {
                declaring.insert(edge.from.as_str());
            }
            // Only file children here; module children are folded in below, once
            // every module's own direct files are known.
            EdgeKind::Contains
                if graph
                    .node(edge.to)
                    .is_some_and(|n| n.kind == NodeKind::File) =>
            {
                module_files
                    .entry(edge.from.as_str())
                    .or_default()
                    .push(edge.to.as_str());
            }
            // Every other edge kind, declared ones included. This artifact
            // counts files and the modules over them; an edge that is neither a
            // declaration nor containment of a file says nothing about either.
            // Listed as a wildcard rather than an enumeration because the set is
            // open — a declared kind cannot be named here — but the reason is
            // the same for every member of it (KTD8).
            _ => {}
        }
    }

    let mut files: Vec<FileCoverage> = Vec::new();
    let mut resolved_files: BTreeSet<&str> = BTreeSet::new();
    let mut per_language: BTreeMap<&str, LanguageCoverage> = BTreeMap::new();
    let mut resolved_index: Option<ResolvedIndex> = None;

    // Deliberately on the concrete kind and NOT on the structural role (U6
    // step 4). The views ask about roles because they draw the shape of the
    // repository, and a declared kind may legitimately take part in that shape.
    // This artifact measures something narrower: how far the language extractors
    // reached over TRACKED FILES. A declared leaf is not a tracked file, it was
    // never handed to a grammar, and admitting it through the `Leaf` role would
    // silently redefine every percentage below. What the exclusion owes the
    // reader is a statement, not a different number — see `DeclaredExclusion`.
    for node in graph.nodes_sorted() {
        if node.kind == NodeKind::Module {
            // The run-level index summary rides on the root module.
            for fact in &node.facts {
                if fact.kind == FactKind::ResolvedCoverage {
                    if let FactValue::Text(summary) = &fact.value {
                        resolved_index = Some(ResolvedIndex {
                            indexed_by: summary
                                .split_once(':')
                                .map(|(tool, _)| tool.trim().to_string())
                                .unwrap_or_else(|| summary.clone()),
                            summary: summary.clone(),
                            sha256: graph.sites().index().map(|i| i.sha256.clone()),
                        });
                    }
                }
            }
            continue;
        }
        if node.kind != NodeKind::File {
            continue;
        }

        let path = node.key.as_str();
        let mut extractors: BTreeSet<&str> = node
            .facts
            .iter()
            .map(|f| &*f.provenance.extractor)
            .collect();
        // Facts on the node say who measured it; edges say who found it and who
        // read its structure (the declarations themselves are facts on the SYMBOL
        // nodes, so tree-sitter's contribution to a file is visible only through
        // the `Declares` edge).
        if let Some(from_edges) = edge_extractors.get(path) {
            extractors.extend(from_edges.iter().copied());
        }
        let structural = declaring.contains(path);
        let resolved = node
            .facts
            .iter()
            .any(|f| f.kind == FactKind::ResolvedByIndex);
        if resolved {
            resolved_files.insert(path);
        }

        // A plugin that failed on the file said so on the file node; the first
        // failure is the one reported.
        let status = node
            .facts
            .iter()
            .find(|f| f.kind == FactKind::ExtractionFailed)
            .map_or_else(
                || {
                    if languages.is_lexed_only(path) {
                        LanguageStatus::LexedOnly
                    } else {
                        LanguageStatus::Parsed
                    }
                },
                |f| LanguageStatus::Failed {
                    plugin: f.provenance.extractor.to_string(),
                    reason: match &f.value {
                        FactValue::Text(t) => t.clone(),
                        other => format!("{other:?}"),
                    },
                },
            );

        let lang = languages.name_for(path);
        if let Some(name) = lang {
            let entry = per_language
                .entry(name)
                .or_insert_with(|| LanguageCoverage {
                    language: name.to_string(),
                    files: 0,
                    structural: 0,
                    resolved: 0,
                    contracts: Vec::new(),
                });
            entry.files += 1;
            entry.structural += usize::from(structural);
            entry.resolved += usize::from(resolved);
            if let Some(contract) = languages.contract_for(path) {
                if let Err(at) = entry
                    .contracts
                    .binary_search_by(|c| c.as_str().cmp(contract))
                {
                    entry.contracts.insert(at, contract.to_string());
                }
            }
        }

        files.push(FileCoverage {
            path: path.to_string(),
            language: lang.map(str::to_string),
            extractors: extractors.into_iter().map(str::to_string).collect(),
            structural,
            resolution: resolution_word(resolved),
            not_structural_because: not_structural_because(structural, lang, &status),
        });
    }

    // Fold each module's descendants upward, so a parent counts what its children
    // hold. Keys are `/`-separated paths, and the root is `.`.
    let modules = modules_with_descendants(graph, &module_files, &resolved_files);

    let plugins_by_contract = languages.plugins_by_contract();
    let languages: Vec<LanguageCoverage> = per_language.into_values().collect();
    let unclassified_files = files.len() - languages.iter().map(|l| l.files).sum::<usize>();

    let declared_kinds_excluded = declared_exclusion(graph);
    let (anchors, db_accesses) = anchors_and_accesses(graph);
    let schema_version = if declared_kinds_excluded.is_some() {
        SCHEMA_VERSION_WITH_VOCABULARY
    } else {
        SCHEMA_VERSION
    };

    Coverage {
        schema_version,
        repo_revision,
        resolved_index,
        languages,
        unclassified_files,
        files,
        modules,
        declared_kinds_excluded,
        // Generation sets it after the fact when it read the layer; this
        // projection reads the graph alone.
        model_verification: None,
        enum_decisions: Some(enum_decisions(graph)),
        anchors: Some(anchors),
        db_accesses: Some(db_accesses),
        // Generation sets it from the cards it rendered ([`card_levels`]).
        cards: None,
        // Generation sets it when it read the purpose layer, as above.
        model_purpose: None,
        // Generation sets it for a run with the second layer ([`bodies`]).
        bodies: None,
        // Generation sets it for a run that reads XAML or has the second
        // layer ([`markup`]).
        markup: None,
        noise: Some(Noise {
            edges_by_rung: edges_by_rung(graph),
            card_rows,
            deleted_by_rule: graph
                .sites()
                .deleted_by_rule()
                .iter()
                .map(|(rule, n)| (rule.id().to_string(), *n))
                .collect(),
            over_cap_sites: graph.sites().over_cap_sites(),
            plugins_by_contract,
            index_match: graph.sites().index().map(|i| IndexMatch {
                considered: i.considered,
                matched: i.matched,
                external: i.external,
                unverified_document: i.unverified_document,
                unmapped: i.unmapped,
            }),
        }),
    }
}

/// The decisions by an enum's value — see [`EnumDecisions`]. Read off the
/// facts the external extractor left: one per place it tied to an enum, and
/// one count per file of the places it could tie to none.
fn enum_decisions(graph: &Graph) -> EnumDecisions {
    let mut counts = EnumDecisions::default();
    for node in graph.nodes_unordered() {
        for fact in &node.facts {
            match (fact.kind, &fact.value) {
                (FactKind::EnumDecision, FactValue::Text(value)) => {
                    let Some(decision) = EnumDecision::decode(value) else {
                        continue;
                    };
                    *match (decision.shape, decision.verified) {
                        (DecisionShape::Comparison, _) => &mut counts.comparisons,
                        (_, true) => &mut counts.verified,
                        (_, false) => &mut counts.unverified,
                    } += 1;
                }
                (FactKind::EnumDecisionsUnmatched, FactValue::Int(n)) => {
                    counts.unmatched += u64::try_from(*n).unwrap_or(0);
                }
                // Every other fact, declared kinds included: this counter
                // reads the two kinds above and nothing else.
                _ => {}
            }
        }
    }
    counts
}

/// The anchors and the database accesses — see [`Anchors`] and
/// [`DbAccesses`]. Counted over every node, the ones no card shows included:
/// this says what the lexer read, not what the details list.
fn anchors_and_accesses(graph: &Graph) -> (Anchors, DbAccesses) {
    let mut anchors = Anchors::default();
    let mut db = DbAccesses::default();
    for node in graph.nodes_unordered() {
        for fact in &node.facts {
            match (fact.kind, &fact.value) {
                (FactKind::Anchor, _) => anchors.kept += 1,
                (FactKind::AnchorsDropped, FactValue::Int(n)) => {
                    anchors.dropped_by_cap += u64::try_from(*n).unwrap_or(0);
                }
                (FactKind::AnchorSkipped | FactKind::RedactedLiteral, FactValue::Text(reason)) => {
                    *anchors.withheld.entry(reason.clone()).or_default() += 1;
                }
                (FactKind::DbAccess, FactValue::Text(value)) => {
                    let Some((access, _)) = DbAccessKind::decode(value) else {
                        continue;
                    };
                    *match access {
                        DbAccessKind::Reads => &mut db.reads,
                        DbAccessKind::Writes => &mut db.writes,
                        DbAccessKind::Calls => &mut db.calls,
                        DbAccessKind::Mentions => &mut db.mentions,
                    } += 1;
                }
                // Every other fact, declared kinds included: these counters
                // read the kinds above and nothing else.
                _ => {}
            }
        }
    }
    (anchors, db)
}

/// The name-resolved edges per rung — see [`EdgesByRung`]. The edges only, not
/// the dissent they carry: a losing claim is a record of who else asserted the
/// relation, not a second relation. Counted in no particular order: a count
/// does not depend on one, so it does not pay for a sort of every edge.
fn edges_by_rung(graph: &Graph) -> EdgesByRung {
    let mut counts = EdgesByRung::default();
    for edge in graph.edges_unordered() {
        let Some(rung) = edge.edge.provenance.rung() else {
            continue;
        };
        *match rung {
            Confidence::ResolvedByIndex => &mut counts.resolved_by_index,
            Confidence::RulesBacked => &mut counts.rules_backed,
            Confidence::NameOnly => &mut counts.name_only,
            Confidence::Ambiguous => &mut counts.ambiguous,
        } += 1;
    }
    counts
}

/// What this artifact left out because the corpus declared it, or `None` when it
/// declared nothing (R12: the version rises only for the artifacts that carry
/// this).
///
/// Reported even when the count is zero. A corpus that declared `open:tabulka`
/// and happens to hold no node of that kind still has a reader who needs to know
/// this artifact would not have counted one — the declaration is what makes the
/// exclusion a rule, and a rule that only appears once it bites is a rule nobody
/// can plan around.
fn declared_exclusion(graph: &Graph) -> Option<DeclaredExclusion> {
    let vocabulary = graph.vocabulary();
    let mut node_kinds: Vec<String> = vocabulary
        .node_declarations()
        .filter_map(|(id, _)| vocabulary.node_name(*id))
        .collect();
    if node_kinds.is_empty() {
        return None;
    }
    // By name, like the manifest's vocabulary section: declaration order is by
    // digest, which is deterministic but arbitrary to a reader (U4).
    node_kinds.sort();

    let nodes = graph
        .nodes_sorted()
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Declared(_)))
        .count();

    Some(DeclaredExclusion {
        node_kinds,
        nodes,
        note: "Coverage counts tracked source files and the modules over them. Nodes of a \
               declared kind were not parsed by any language extractor and are not counted \
               here; their absence from `files` is not a coverage gap."
            .to_string(),
        orphan_containers: graph.orphan_containers(),
    })
}

/// The same two words the graph serializes, so a consumer reading both the
/// manifest and this artifact never sees one axis spelled two ways.
fn resolution_word(resolved: bool) -> String {
    match resolved {
        true => "resolved",
        false => "heuristic",
    }
    .to_string()
}

/// Every module with the files beneath it at any depth, and whether an index
/// verified all of them.
fn modules_with_descendants(
    graph: &Graph,
    direct_files: &BTreeMap<&str, Vec<&str>>,
    resolved_files: &BTreeSet<&str>,
) -> Vec<ModuleCoverage> {
    let module_keys: Vec<&str> = graph
        .nodes_sorted()
        .into_iter()
        .filter(|n| n.kind == NodeKind::Module)
        .map(|n| n.key.as_str())
        .collect();

    module_keys
        .iter()
        .map(|module| {
            // A descendant is any file under this module's path prefix. The root
            // module `.` holds everything; every other module holds what sits
            // beneath `<module>/`.
            let descendants: Vec<&str> = direct_files
                .iter()
                .filter(|(owner, _)| is_self_or_descendant(module, owner))
                .flat_map(|(_, files)| files.iter().copied())
                .collect();
            let resolved = descendants
                .iter()
                .filter(|f| resolved_files.contains(*f))
                .count();
            ModuleCoverage {
                path: module.to_string(),
                files: descendants.len(),
                resolved,
                // An empty module is not "fully resolved": there is nothing an
                // absence claim over it could rest on.
                fully_resolved: !descendants.is_empty() && resolved == descendants.len(),
            }
        })
        .collect()
}

/// Whether `candidate` is `module` itself or sits beneath it.
fn is_self_or_descendant(module: &str, candidate: &str) -> bool {
    if module == "." {
        return true;
    }
    candidate == module || candidate.starts_with(&format!("{module}/"))
}

/// Serialize the artifact. Pretty-printed and stably ordered, so two runs over
/// one repository produce identical bytes (R18).
pub fn to_json(coverage: &Coverage) -> String {
    serde_json::to_string_pretty(coverage).expect("coverage serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_prefix_match_respects_path_boundaries() {
        // `src` must not claim `srcgen/a.rs` just because the string starts the
        // same way — that would inflate its file count and could hand it a
        // "fully resolved" licence it never earned.
        assert!(is_self_or_descendant("src", "src/a.rs"));
        assert!(is_self_or_descendant("src", "src"));
        assert!(!is_self_or_descendant("src", "srcgen/a.rs"));
        // The root module holds the whole tree.
        assert!(is_self_or_descendant(".", "anything/at/all.rs"));
    }

    /// The rung counters are keyed by the manifest's words for the rungs, so a
    /// consumer reading both artifacts never sees one rung spelled two ways.
    #[test]
    fn rung_counter_names_are_the_manifests_rung_words() {
        let json = serde_json::to_value(EdgesByRung::default()).unwrap();
        let fields: BTreeSet<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let words: BTreeSet<&str> = [
            Confidence::ResolvedByIndex,
            Confidence::RulesBacked,
            Confidence::NameOnly,
            Confidence::Ambiguous,
        ]
        .into_iter()
        .map(crate::manifest::confidence)
        .collect();
        assert_eq!(fields, words);
    }

    /// Nearest rank, worked by hand: of ten sizes the median is the 5th and
    /// p90 the 9th; of four, the 2nd (the lower middle) and the 4th
    /// (`ceil(3.6)`); of one, that one; of none, zeros. The order the sizes
    /// come in does not matter.
    #[test]
    fn median_and_p90_are_nearest_rank_integers() {
        let of = |v: &[u64]| {
            let s = Sizes::of(v.to_vec());
            (s.count, s.median, s.p90)
        };
        assert_eq!(of(&[100, 10, 90, 20, 80, 30, 70, 40, 60, 50]), (10, 50, 90));
        assert_eq!(of(&[4, 1, 3, 2]), (4, 2, 4));
        assert_eq!(of(&[7]), (1, 7, 7));
        assert_eq!(of(&[]), (0, 0, 0));
        // Eleven: the 6th and the 10th (`ceil(9.9)`).
        assert_eq!(of(&(1..=11).collect::<Vec<u64>>()), (11, 6, 10));
    }

    /// R19, KTD12: the four states of XAML coverage, and what lowers one. A
    /// component that claims XAML and serves no `bodies` — a plugin on
    /// contract 0.2 — lowers a run with the second layer to "read with
    /// unsupported constructs" even when no file failed; this repository has
    /// no XAML component on 0.2 to run (`plugins/prebuilt/xaml.wasm` was built
    /// on 0.3), so the case is the one the facts of such a run give. A
    /// markup extension the plugin could not take apart lowers it too.
    #[test]
    fn only_a_failed_file_a_component_without_bodies_or_unparsed_markup_lowers_the_state() {
        use MarkupState::*;
        assert_eq!(markup_state(false, true, true, true, true), SecondLayerOff);
        assert_eq!(
            markup_state(false, false, false, false, false),
            SecondLayerOff
        );
        assert_eq!(markup_state(true, false, false, false, false), NotRead);
        assert_eq!(markup_state(true, true, false, false, false), ReadInFull);
        assert_eq!(
            markup_state(true, true, true, false, false),
            ReadWithUnsupportedConstructs
        );
        assert_eq!(
            markup_state(true, true, false, true, false),
            ReadWithUnsupportedConstructs
        );
        assert_eq!(
            markup_state(true, true, false, false, true),
            ReadWithUnsupportedConstructs
        );
        let words: Vec<String> = [
            NotRead,
            ReadWithUnsupportedConstructs,
            ReadInFull,
            SecondLayerOff,
        ]
        .iter()
        .map(|s| {
            serde_json::to_value(s)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
        assert_eq!(
            words,
            [
                "not-read",
                "read-with-unsupported-constructs",
                "read-in-full",
                "second-layer-off"
            ]
        );
    }

    /// A run with neither a plugin for XAML nor the second layer has no
    /// section; one with a plugin for XAML and no second layer says so, and
    /// counts no markup places.
    #[test]
    fn the_section_is_absent_without_xaml_and_the_second_layer() {
        let graph = Graph::new();
        let languages = Languages::bundled();
        let none = BTreeSet::new();
        assert_eq!(
            markup(&graph, &languages, &[], &none, None, None, &[]),
            None
        );
        let unparsed = [UnparsedMarkup {
            path: "a.xaml".into(),
            count: 1,
        }];
        let off = markup(
            &graph,
            &languages,
            &["xaml".into()],
            &none,
            None,
            None,
            &unparsed,
        )
        .unwrap();
        assert_eq!(off.state, MarkupState::SecondLayerOff);
        assert_eq!(off.sites, None);
        // Read only with the second layer, like the markup places.
        assert!(off.unparsed.is_empty());
    }

    #[test]
    fn resolution_words_match_the_graphs_own_vocabulary() {
        assert_eq!(resolution_word(true), "resolved");
        assert_eq!(resolution_word(false), "heuristic");
    }
}
