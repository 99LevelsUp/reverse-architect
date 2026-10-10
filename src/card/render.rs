//! Deterministic Markdown rendering of a [`Card`] (U8).
//!
//! No LLM, no timestamps, no absolute paths: the same graph renders the same
//! bytes on every run and on every machine (R18).
//!
//! Section headings and every fact-vocabulary word (`heuristic`, `resolved`, the
//! edge names) are English constants, per R8 — they are part of the skeleton a
//! consumer parses, not prose. `--lang` switches only the authored prose line,
//! and an unknown code falls back to English rather than failing a run.
//!
//! Every card opens with YAML front matter naming [`CARD_SCHEMA_VERSION`], so a
//! consumer can tell a skeleton it understands from one it does not — the
//! manifest and the coverage artifact both carry a version, and until now the
//! only artifact whose format was declared parseable carried none.

use super::links::{backlinks, mention, CardIndex};
use super::{
    Card, CardKind, CaveatKind, DocQuote, MaybeAlso, MaybeReason, MemberRow, ModelMark, Relation,
    TestPairing,
};
use crate::cli::LinkStyle;
// `EdgeKind` is no longer named in the rendering path itself: since U3 a
// relation carries its own resolved word (KTD4), so the renderer never asks
// what kind an edge is. The tests below still build relations by hand.
#[cfg(test)]
use crate::graph::EdgeKind;
use crate::graph::{Confidence, Resolution, Rule, SiteId, Tier};
use std::borrow::Cow;

/// The version of the rendered card format, carried by every card.
///
/// Bumped whenever a consumer that parses the skeleton would read an old card
/// differently — a renamed section, a changed bullet key, a reworded line that
/// carried data. NOT bumped for the authored prose line, which is documented as
/// prose and which no consumer may parse.
///
/// The number lives on each card rather than once in the manifest because a card
/// travels alone: pulled out as a retrieval chunk, pasted into a ticket, handed
/// to a model. A version it left behind in another file is a version it does not
/// have. `tool` rides along for the same reason — a stray Markdown file should
/// be able to say what wrote it. It carries no release number: that would churn
/// every card in the corpus on every release, for a fact the reader can get from
/// the manifest.
///
/// The written contract this number refers to (which parts are stable and which
/// are prose) is deliberately not written yet — the format moved three times in
/// the review round that introduced this constant, and a contract is worth
/// writing once it has stopped moving.
///
/// v3: a heuristic `## Relations` row whose name matched more than one
/// repo-wide candidate now says so at the row itself (`, ambiguous — N
/// candidates`), and the corresponding `## Notes` caveat text is reworded to
/// distinguish "these edges exist, treat with caution" (`CandidateCount`)
/// from "no edge is shown, the ambiguity ceiling was exceeded"
/// (`AmbiguousReference`) — under v2 both read as the identical sentence.
///
/// v4: cards explain before they list. A `## Role` paragraph, a
/// `## Collaboration` (or, on a module, `## Dependencies`) diagram, the offered
/// surface as `## API` / `## Functions` with each member's summary and an
/// `## Internals` list, `## Works with` / `## Used by` / `## Inheritance` /
/// `## Tests` tables aggregated to type grain, and — on a module — `## Key
/// types`, `## Contents`, `## Depends on`. The v3 evidence sections follow
/// unchanged, except that `## Members` now appears only on a declared card
/// (a type's and a file's members are the API and Internals lists) and a
/// module's `## Relations` no longer repeats its `contains` rows, which
/// `## Contents` lists. Every file now has a card, so a file mention links.
///
/// v5: relations, not sites, and guesses apart (plan A, U3). An ambiguous
/// relation is no longer a row in `## Relations` or `## Referenced by` marked
/// `ambiguous — N candidates`: it is one line per unsettled usage site in a new
/// `## Maybe also` section after `## Referenced by`, naming the site's source,
/// the kind, `1 of N candidates` (`1 ze N kandidátů` under `--lang cs`) when
/// there is more than one, the alternatives, and why the line is a guess
/// (`ambiguous: <reason>`, authored prose under `--lang`): the rule that
/// disputed it, or the name several declarations share (plan A, R7, U8 — still
/// v5, which no release has carried). A name match nothing backed reads
/// `(heuristic, name only)`. A
/// row is one relation, so its anchor lists every site it was read at; `via`
/// names the member with its parameters (`via `Solve(int)``); and neither list
/// holds a relation between two members of the card's own type or file.
///
/// v6: a card and its detail (plan B). A type's card is the short card: after
/// the title and the glance strip (no `> summary` quote above it, and no lead
/// sentence — the one every type card repeated; the strip names the module
/// by its last segment, linked by its full path), `- Identity:`, then
/// `- Detail: [detail](…)` linking the type's detail when it has one,
/// `## Purpose` (one line per sentence, each ending in its origin,
/// `(from the comment)` / `(from the structure)`, authored prose under
/// `--lang`), `## Role` — whose sentences about units reached only through
/// ambiguous name matches became one count of the neighbours the detail
/// lists beyond those named, with how many of them are such guesses in
/// parentheses (`The detail lists N more neighbours (M uncertain).`, `(all
/// uncertain)`, `(uncertain)`; authored prose under `--lang`, as are the
/// shortened sentences on users and used units, `Used by N in M modules,
/// chiefly …` and `It uses N others, chiefly …`, which a file card shares) —
/// and `## Keywords` (one line, a word's diacritics-free spelling in
/// parentheses beside it). `## Notes` keeps only what is about the card
/// itself — a plugin that failed on the file — and one line counting the
/// names used past the ambiguity ceiling (`names used past the ambiguity
/// ceiling, with no relation shown: N`, authored prose under `--lang`);
/// `## Signature`, `## Test pairing`, `## Collaboration`, `## API`,
/// `## Internals`, `## Works with`, `## Used by`, `## Inheritance`,
/// `## Tests`, `## Relations`, `## Referenced by`, `## Maybe also`,
/// `## External usage` and `## Documentation` left the type card for its
/// detail (`kind: detail` front matter under `cards/detail/`), which lists
/// every relation with its rung, opens with the quoted comment and the
/// signature and ends with the test pairing and `## Notes` holding every
/// other caveat, word for word as the card used to. A type with no detail
/// (one whose card says all a detail would, signature and pairing aside)
/// shows neither. A file card keeps its sections,
/// gains `## Keywords`, and names its own detail — the texts and database
/// accesses no type of the file owns — in a `- Detail:` bullet under
/// `- Identity:`. The `## Notes` caveat for an ambiguous name reads "a Maybe
/// also claim" rather than "listed under Maybe also", since on a type that
/// section is in the detail. A module card whose module holds a type names
/// its map (`kind: map` front matter under `cards/map/`, one row per type,
/// generated types as one counted row, `## Types` or `## Part N`) in a
/// `- Map:` bullet under `- Identity:`, and the signpost's `Types` count in
/// `index.md` links the same map. A run given `--model-purpose` (U13) adds
/// one line at the end of a type's `## Purpose`: the sentence `verify
/// --purpose` had a model write, ending `(written by a model, not read from
/// the code)` (authored prose under `--lang`) in place of an origin; never in
/// the header strip, a map row or the manifest. Without the option no card
/// carries it.
pub const CARD_SCHEMA_VERSION: u32 = 6;

/// The version of a type's card in a run with the second layer
/// (`--detail-level`, plan 2a, U6: KTD9 point 2, KTD10). Every other card of
/// such a run, every detail and every card of a run without it stays at
/// [`CARD_SCHEMA_VERSION`].
///
/// v7: the front matter adds `detail_level: N` and `layer2: present`,
/// `failed` or `none` under `kind:`. With the type's analysis, `## Purpose`
/// is the purpose from the code with the doc comment as a quote (U7), and
/// after `## Keywords` — whose words include the ones the second layer
/// brings, weighed by the first layer's document frequencies — come a
/// `## Functionality: <key without its sym: prefix>` per core functionality
/// (`- Key:`, `- Words:`, one sentence from the code, `### Entry points`,
/// `### Preconditions`, `### Dead paths`, `### Steps`, `### Flowchart`,
/// `### Members`) and a `## State: <member>` per state (`- Key:`, `- Field:`,
/// `### Values`, `### Read in conditions`, `### Changed by`,
/// `### State diagram`), as deep as the level asks
/// ([`super::layer2_render`]). A type card with `layer2: none` or `failed`
/// is otherwise the card of a run without the second layer.
pub const CARD_SCHEMA_VERSION_LAYER2: u32 = 7;

/// The languages the deterministic prose is authored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Cs,
}

impl Lang {
    /// Parse a `--lang` code. Anything unrecognized is English: a card corpus is
    /// still correct in the wrong language, whereas refusing to render loses the
    /// whole run over a typo.
    pub fn from_code(code: &str) -> Lang {
        match code.trim().to_ascii_lowercase().as_str() {
            "cs" | "cs-cz" | "czech" => Lang::Cs,
            _ => Lang::En,
        }
    }

    /// The one authored sentence on a card: what the reader is looking at and how
    /// far they may trust it.
    /// Takes the card's resolved kind word (KTD4) so a declared kind can be
    /// named. The core kinds ignore it: their sentence is authored, not
    /// assembled, because a card's one framing sentence is where a reader
    /// decides how far to trust the rest.
    ///
    /// A type's short card has none (plan B, R17): the sentence was the same on
    /// every type card, and the budget of a short card goes to what tells one
    /// type from another.
    fn lead(self, kind: CardKind, kind_word: &str) -> Cow<'static, str> {
        match (self, kind) {
            (_, CardKind::Type) => "",
            // NOT "declares no type": whether none was declared or none could be
            // extracted is a question about coverage, and only the coverage
            // artifact may answer it (R20). A card that answers it here would be
            // making an absence claim over data that may never have been read.
            // NOT "no extracted type of its own" either: this file may ALSO
            // declare a type — that type has its own separate card, and this
            // one covers only what no type claims (the file-card plan's
            // KTD1).
            (Lang::En, CardKind::File) => {
                "A file of this repository. The types it declares have cards of their own; this \
                 card holds what no type claims — free functions and similar items — and what \
                 the file as a whole imports and is used by. Whether nothing was declared or \
                 nothing could be extracted is coverage.json's to say. Everything below was \
                 read from the source, and every statement that has a place in it names that \
                 place."
            }
            (Lang::En, CardKind::Module) => {
                "A module: a directory and what it holds. Everything below was read from the \
                 source, and every statement that has a place in it names that place."
            }
            (Lang::Cs, CardKind::File) => {
                "Soubor tohoto repozitáře. Typy, které deklaruje, mají vlastní karty; tahle karta \
                 nese to, co si žádný typ nenárokuje — volné funkce a podobné položky — a co \
                 soubor jako celek importuje a kdo ho používá. Zda nebylo nic deklarováno, nebo \
                 nic extrahováno, říká coverage.json. Vše níže je přečtené ze zdroje a každé \
                 tvrzení, které má ve zdroji místo, toto místo uvádí."
            }
            (Lang::Cs, CardKind::Module) => {
                "Modul: adresář a jeho obsah. Vše níže je přečtené ze zdroje a každé tvrzení, \
                 které má ve zdroji místo, toto místo uvádí."
            }
            // Assembled, not authored — and it deliberately promises less than
            // the core sentences do. The core knows this corpus declared the
            // kind and what it called it; it does not know what the kind MEANS,
            // so the sentence must not imply that it does. Naming the kind and
            // pointing at the manifest is the whole of what can be said.
            (Lang::En, CardKind::Declared(_)) => {
                return Cow::Owned(format!(
                    "A `{kind_word}` — a kind this corpus declares for itself; the manifest's \
                     vocabulary says how it was treated. What is below came from the extractor \
                     that declared it, and where a statement carries a place in the source it \
                     names that place."
                ))
            }
            (Lang::Cs, CardKind::Declared(_)) => {
                return Cow::Owned(format!(
                    "`{kind_word}` — druh, který si tento korpus deklaruje sám; jak s ním bylo \
                     zacházeno, říká slovník v manifestu. Co je níže, dodal extraktor, který ho \
                     deklaroval, a kde tvrzení nese místo ve zdroji, uvádí ho."
                ))
            }
        }
        .into()
    }

    /// The sentence under `## Maybe also`: what the section holds and how far a
    /// line in it may be trusted. Worded as the role text words the same
    /// guesses ("may use it too, through name matches the rules did not
    /// settle"), so the paragraph and the section read as one statement.
    pub(super) fn maybe_also_lead(self) -> &'static str {
        match self {
            Lang::En => {
                "Relations that may hold too, through a name match the rules did not settle: \
                 the name fits more than one declaration, or a rule disputed the one it fits. \
                 Each line is one place in the source and says why; at most one of its \
                 candidates is meant, and possibly none of them."
            }
            Lang::Cs => {
                "Vztahy, které možná platí také — přes shodu jména, kterou pravidla \
                 nerozhodla: jméno sedí na víc deklarací, nebo pravidlo zpochybnilo tu, na \
                 kterou sedí. Každý řádek je jedno místo ve zdroji a říká proč; platí nejvýš \
                 jeden z jeho kandidátů, a možná žádný."
            }
        }
    }

    /// Why a `## Maybe also` line is a guess (plan A, R7): the rule that
    /// disputed it in words a reader can check against the source, or the
    /// shared name. No count here — `1 of N candidates` carries it.
    fn reason(self, reason: MaybeReason) -> &'static str {
        match (self, reason) {
            (Lang::En, MaybeReason::Disputed(Rule::DeclaredReceiver)) => {
                "the receiver is declared as another type"
            }
            (Lang::En, MaybeReason::Disputed(Rule::StaticReceiver)) => {
                "the call names another type"
            }
            (Lang::En, MaybeReason::Disputed(Rule::Arity)) => "the argument count does not fit",
            (Lang::En, MaybeReason::Disputed(Rule::NamespaceVisibility)) => {
                "the target's namespace is not visible in the file"
            }
            (Lang::En, MaybeReason::Disputed(Rule::NestedTypeVisibility)) => {
                "a nested type is not visible here by its simple name"
            }
            (Lang::En, MaybeReason::Disputed(Rule::RoleMismatch)) => {
                "the declaration's role does not fit the usage"
            }
            (Lang::Cs, MaybeReason::Disputed(Rule::DeclaredReceiver)) => {
                "příjemce je deklarovaný jako jiný typ"
            }
            (Lang::Cs, MaybeReason::Disputed(Rule::StaticReceiver)) => "volání jmenuje jiný typ",
            (Lang::Cs, MaybeReason::Disputed(Rule::Arity)) => "počet argumentů nesedí",
            (Lang::Cs, MaybeReason::Disputed(Rule::NamespaceVisibility)) => {
                "namespace cíle není v souboru viditelný"
            }
            (Lang::Cs, MaybeReason::Disputed(Rule::NestedTypeVisibility)) => {
                "vnořený typ tu není vidět holým jménem"
            }
            (Lang::Cs, MaybeReason::Disputed(Rule::RoleMismatch)) => {
                "role deklarace neodpovídá užití"
            }
            (Lang::En, MaybeReason::IndexElsewhere) => "the index points elsewhere",
            (Lang::En, MaybeReason::IndexOutside) => "the index points outside the repository",
            (Lang::Cs, MaybeReason::IndexElsewhere) => "index ukazuje jinam",
            (Lang::Cs, MaybeReason::IndexOutside) => "index ukazuje mimo repozitář",
            // `MaybeReason::of` names a dispute only for the six rules above;
            // any other rule reads as what it is, the shared name.
            (Lang::En, _) => "several declarations share the name",
            (Lang::Cs, _) => "stejné jméno nese víc deklarací",
        }
    }

    /// What `verify`'s model said about a `## Maybe also` line (plan A, U12),
    /// marked as the model's the way an enriched leaf is marked LLM-narrated:
    /// a reader must never take it for something the rules or an index found.
    fn model_mark(self, mark: ModelMark) -> &'static str {
        match (self, mark) {
            (Lang::En, ModelMark::Promoted) => "model-verified: the first candidate",
            (Lang::En, ModelMark::Demoted) => "model-verified: none of them",
            (Lang::Cs, ModelMark::Promoted) => "ověřeno modelem: první kandidát",
            (Lang::Cs, ModelMark::Demoted) => "ověřeno modelem: žádný z nich",
        }
    }

    /// `1 of 2 candidates` / `1 ze 2 kandidátů` — how many declarations one
    /// unsettled site's name fits (plan A, AE3).
    fn one_of(self, n: usize) -> String {
        match self {
            Lang::En => format!("1 of {n} candidates"),
            Lang::Cs => format!("1 {} {n} kandidátů", cs_from(n)),
        }
    }
}

/// The Czech preposition before a spelled-out count: `ze` where the numeral
/// starts with a sound that `z` cannot be said before — `ze dvou`, `ze tří`,
/// `ze čtyř`, `ze šesti`, `ze sedmi`, `ze sta` — and `z` otherwise (`z pěti`,
/// `z osmi`). The numeral's first word is decided by its leading part: the
/// number itself below twenty, then its tens, then its hundreds.
fn cs_from(n: usize) -> &'static str {
    let lead = match n {
        0..=19 => n,
        20..=99 => n / 10,
        100..=199 => return "ze",
        200..=999 => n / 100,
        _ => return "z",
    };
    match lead {
        2 | 3 | 4 | 6 | 7 | 12 | 13 | 14 | 16 | 17 => "ze",
        _ => "z",
    }
}

/// The trust axis in the card's own words — the same three the manifest
/// serializes, so a reader of both never sees one axis spelled two ways.
///
/// A sibling of [`resolution_word`], never derived from it: `Tier` says how much
/// a claim is trusted, `Resolution` how precisely it was pinned down, and the
/// two are independent (KTD1).
fn tier_word(tier: Tier) -> &'static str {
    match tier {
        Tier::Measured => "measured",
        Tier::Derived => "derived",
        Tier::Narrated => "narrated",
    }
}

pub(super) fn resolution_word(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Heuristic => "heuristic",
        Resolution::Resolved => "resolved",
    }
}

/// Render one card to Markdown. Empty sections are omitted entirely — an empty
/// `## Documentation` would read as "documented with nothing".
///
/// `index` decides which mentions become links (U9): a target that owns a card is
/// linked in the configured style, everything else stays text with its anchor.
pub fn render_card(card: &Card, lang: Lang, links: LinkStyle, index: &CardIndex) -> String {
    render_card_layered(card, lang, links, index, None)
}

/// [`render_card`] with the run's second layer (plan 2a, U6, U7: KTD9,
/// KTD10), which only a type's card carries ([`CARD_SCHEMA_VERSION_LAYER2`]):
/// its front matter names the level and whether the type has its second
/// layer, and with the type's analysis `## Purpose` is the purpose from the
/// code with the doc comment as a quote ([`super::purpose::layered`]) and the
/// functionalities and states follow `## Keywords`
/// ([`super::layer2_render::sections`]). With `None`, and on any other card,
/// the card is exactly [`render_card`]'s. The detail never reads the
/// analysis (KTD9).
pub fn render_card_layered(
    card: &Card,
    lang: Lang,
    links: LinkStyle,
    index: &CardIndex,
    layer2: Option<super::layer2_render::Layer2<'_>>,
) -> String {
    use super::explain;
    let mut s = String::new();
    let out = explain::Out {
        card,
        from: &card.path,
        links,
        index,
        lang,
    };
    let story = &card.story;
    let layer2 = layer2.filter(|_| card.kind == CardKind::Type);
    let analysis = layer2.and_then(|l| l.analysis);

    // YAML front matter, before anything else: the one part of a card addressed
    // to a machine rather than a reader. The `--links obsidian` layout renders it
    // as properties, every other reader hides it, and a parser can read it
    // without understanding a word of Markdown.
    let schema = match layer2 {
        Some(_) => CARD_SCHEMA_VERSION_LAYER2,
        None => CARD_SCHEMA_VERSION,
    };
    s.push_str(&format!(
        "---\nschema: {schema}\ntool: reverse-architect\nkind: {}\n",
        yaml_word(&card.kind_word)
    ));
    if let Some(l) = &layer2 {
        s.push_str(&format!(
            "detail_level: {}\nlayer2: {}\n",
            l.level,
            l.word()
        ));
    }
    s.push_str("---\n\n");

    // A type's card is the short card (plan B, U9: R2, R17): its header line,
    // its purpose, the role sentences and its keywords, with everything it
    // used to list moved to its detail.
    let short = card.kind == CardKind::Type;

    s.push_str(&format!("# {}\n\n", card.title));
    // The comment's first sentence; on the short card it is the first
    // sentence of `## Purpose`, with its origin, and not said twice.
    if let (false, Some(summary)) = (short, &story.summary) {
        s.push_str(&format!("> {summary}\n\n"));
    }
    let glance = explain::glance(&out, story);
    if !glance.is_empty() {
        s.push_str(&glance);
        s.push_str("\n\n");
    }
    let lead = lang.lead(card.kind, &card.kind_word);
    if !lead.is_empty() {
        s.push_str(&lead);
        s.push_str("\n\n");
    }

    // Kind, visibility and source are in the strip under the title; a declared
    // card has no such strip content of its own, so it keeps the bullets.
    s.push_str(&format!("- Identity: `{}`\n", card.identity));
    // Plan B, U8 and U9 (KTD7): a card names its detail in one line, by what
    // the detail holds for its kind — a file's texts and database accesses no
    // type of the file owns, a type's places, members and every relation.
    if let Some(detail) = &card.detail {
        let label = match (card.kind, lang) {
            (CardKind::File, Lang::En) => Some("anchors and database"),
            (CardKind::File, Lang::Cs) => Some("kotvy a databáze"),
            // One word on the short card (card trim): the bullet's key says
            // where it leads, and the README says what a detail holds.
            (CardKind::Type, _) => Some("detail"),
            // `wants_detail` gives no other kind a detail.
            _ => None,
        };
        if let Some(label) = label {
            s.push_str(&format!(
                "- Detail: {}\n",
                super::links::link(&card.path, detail, label, links)
            ));
        }
    }
    // Plan B, U10 (KTD7): a module card names its map — one row per type —
    // by how many types it lists.
    if let (Some(map), Some(ms)) = (&card.map, &story.module_story) {
        let label = match lang {
            Lang::En => super::explain::plural(ms.types, "type", "types"),
            Lang::Cs => super::explain::cs_n(ms.types, ("typ", "typy", "typů")),
        };
        s.push_str(&format!(
            "- Map: {}\n",
            super::links::link(&card.path, map, &label, links)
        ));
    }
    if matches!(card.kind, CardKind::Declared(_)) {
        if let Some(kind) = &card.entity_kind {
            s.push_str(&format!("- Kind: {kind}\n"));
        }
        if let Some(visibility) = &card.visibility {
            s.push_str(&format!("- Visibility: {visibility}\n"));
        }
        if let Some(source) = &card.source {
            s.push_str(&format!("- Source: `{}`\n", source.anchor()));
        }
    }

    match card.kind {
        // The short card's body. The collaboration diagram, the API and the
        // peer tables are the detail's (R17).
        CardKind::Type => {
            match analysis {
                Some(analysis) => s.push_str(&explain::purpose_layered(
                    &out,
                    story,
                    &super::purpose::layered(card, &analysis.purpose),
                )),
                None => s.push_str(&explain::purpose(&out, story)),
            }
            s.push_str(&explain::role(&out, story));
            s.push_str(&explain::keywords(story));
            // KTD10: the second layer's sections after the keywords, before
            // the notes.
            if let (Some(l), Some(analysis)) = (&layer2, analysis) {
                s.push_str(&super::layer2_render::sections(
                    card, lang, l.level, analysis,
                ));
            }
        }
        CardKind::File => {
            s.push_str(&explain::role(&out, story));
            if let Some(d) = explain::diagram(
                lang,
                "Collaboration",
                explain::file_name(&card.title),
                &story.users,
                &story.uses,
            ) {
                s.push_str(&d);
            }
            s.push_str(&explain::api(&out, story, "Functions"));
            s.push_str(&explain::peer_tables(&out, story));
            s.push_str(&explain::keywords(story));
        }
        CardKind::Module => s.push_str(&explain::module_body(&out, story)),
        CardKind::Declared(_) => {}
    }

    // A type's short card leaves its signature to the detail (card trim): the
    // header line under the title already states its kind and modifiers
    // (R4), and the rest of it — the bases — the purpose says.
    if let (false, Some(signature)) = (short, &card.signature) {
        s.push_str("\n## Signature\n\n");
        s.push_str(&signature_block(signature));
    }

    // The whole comment, when it says more than the one-sentence summary at the
    // top already did. A type's short card leaves it to the detail (plan B,
    // card trim): a comment is read whole or not at all, and on a documented
    // type it was the largest section the short card had left — most often
    // the purpose's first sentence again, inside its `<summary>` tags.
    if !short {
        if let Some(doc) = quoted_doc(card) {
            s.push_str("\n## Documentation\n\n");
            s.push_str(&doc_quote(doc));
        }
    }

    // A type's and a file's members are the API and Internals lists above; only
    // a declared card, which has no such lists, keeps the plain member listing.
    if matches!(card.kind, CardKind::Declared(_)) && !card.members.is_empty() {
        s.push_str("\n## Members\n\n");
        for member in &card.members {
            s.push_str(&member_line(card, member, links, index));
        }
    }

    // The evidence: every relation with the place it was read from. A module's
    // `contains` rows are its `## Contents` table and are not repeated here.
    // A type's short card lists none of it — its detail lists all four
    // sections below with the rung of every row (plan B, AE2) — so the card
    // ends with the facts only it carries.
    if short {
        s.push_str(&trailer(card, lang));
        return s;
    }
    let evidence: Vec<&Relation> = card
        .outgoing
        .iter()
        .filter(|r| !(card.kind == CardKind::Module && r.kind_word == "contains"))
        .collect();
    if !evidence.is_empty() {
        s.push_str("\n## Relations\n\n");
        for relation in evidence {
            s.push_str(&relation_line(card, relation, links, index));
        }
    }

    // R11 (and R13's "both directions" for a module): who points HERE. One
    // section on every card kind — a module's incoming dependencies and a type's
    // callers are the same question asked of different entities.
    let (shown, omitted) = backlinks(card);
    if !shown.is_empty() {
        s.push_str("\n## Referenced by\n\n");
        for relation in shown {
            s.push_str(&relation_line(card, relation, links, index));
        }
        if !omitted.is_empty() {
            // Never a silent truncation, and never a silent CATEGORY: what was
            // dropped is stated per relation kind, so a reader can tell that the
            // callers listed are all the callers and only the implementors were
            // capped — a single total would have read as "some of everything".
            let dropped: Vec<String> = omitted
                .iter()
                .map(|(word, count)| format!("{count} more {word}"))
                .collect();
            s.push_str(&format!(
                "\nAt most {} entries per relation kind are listed. Not listed here: {} — \
                 the full set is in the graph export.\n",
                super::links::BACKLINK_CAP,
                dropped.join(", ")
            ));
        }
    }

    // Plan A, R7: what MAY hold, apart from what does. After the two lists it
    // qualifies, and absent when there is nothing to qualify — an empty
    // section would read as "checked, and nothing uncertain".
    if !card.maybe_also.is_empty() {
        s.push_str("\n## Maybe also\n\n");
        s.push_str(lang.maybe_also_lead());
        s.push_str("\n\n");
        for claim in &card.maybe_also {
            s.push_str(&maybe_also_line(&card.path, claim, lang, links, index));
        }
    }

    if !card.externals.is_empty() {
        s.push_str("\n## External usage\n\n");
        for relation in &card.externals {
            s.push_str(&relation_line(card, relation, links, index));
        }
    }

    s.push_str(&trailer(card, lang));
    s
}

/// The doc comment a card quotes in full: `None` when there is none, or when
/// the card already says all of it — on a type's short card as a purpose
/// sentence, on any other card as the `> summary` under the title. A summary
/// the secret filter cut into is no purpose sentence, so that comment is
/// quoted, marker and all. A type quotes it in its detail, and the detail
/// exists for it by this same answer ([`super::detail::wants_detail`]).
pub(super) fn quoted_doc(card: &Card) -> Option<&DocQuote> {
    let doc = card.doc.as_ref()?;
    let flat = doc.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let repeats_summary = if card.kind == CardKind::Type {
        card.story.purpose.iter().any(|sentence| {
            matches!(&sentence.claim, super::purpose::Claim::Comment(text) if *text == flat)
        })
    } else {
        card.story
            .summary
            .as_deref()
            .is_some_and(|summary| summary == flat)
    };
    (!repeats_summary).then_some(doc)
}

/// A quoted comment as `## Documentation` reads it: every line quoted, blank
/// lines kept, and the place it was read from under it.
pub(super) fn doc_quote(doc: &DocQuote) -> String {
    let mut s = String::new();
    for line in doc.text.lines() {
        if line.trim().is_empty() {
            s.push_str(">\n");
        } else {
            s.push_str(&format!("> {line}\n"));
        }
    }
    s.push_str(&format!("\n— `{}`\n", doc.source.anchor()));
    s
}

/// A signature as `## Signature` holds it: one fenced block, read whole.
pub(super) fn signature_block(signature: &str) -> String {
    format!("```text\n{signature}\n```\n")
}

/// The one line under `## Test pairing`.
///
/// The tier belongs on this line and on no other: everything else a card
/// shows was read from the source, this one was concluded from how files are
/// named. Without the word, a conclusion reads as an observation — and it is
/// the only line with no anchor to give that away. …and when the pairing was
/// measured on the declaring FILE rather than on this entity, the line says
/// so. The heuristic pairs files, so a file declaring three types would
/// otherwise hand all three the same sentence as if it were about each of
/// them.
pub(super) fn test_pairing_line(test: &TestPairing) -> String {
    let measured_on = match &test.from_file {
        Some(file) => format!(" — for the file `{file}`"),
        None => String::new(),
    };
    format!(
        "- {}{measured_on} ({})\n",
        test.statement,
        tier_word(test.tier)
    )
}

/// The last sections of every card: the test pairing and the notes.
///
/// A type's short card has no test pairing — it is in the detail (card
/// trim): it stood on almost every type card, most often to say that no test
/// was found by convention. It keeps only the
/// notes about itself ([`CaveatKind::on_short_card`]) and counts the names
/// past the ambiguity ceiling in one line, because those qualify its role
/// sentences: a type "seen using nothing" may only have used names too common
/// to resolve. Every other caveat qualifies something its detail shows, and
/// the detail lists it (plan B, card trim).
fn trailer(card: &Card, lang: Lang) -> String {
    let mut s = String::new();
    let short = card.kind == CardKind::Type;
    if let (false, Some(test)) = (short, &card.test) {
        s.push_str("\n## Test pairing\n\n");
        s.push_str(&test_pairing_line(test));
    }

    let mut notes: Vec<&str> = card
        .caveats
        .iter()
        .filter(|c| !short || c.kind.on_short_card())
        .map(|c| c.text.as_str())
        .collect();
    let over_ceiling = if short {
        over_ceiling_note(card, lang)
    } else {
        None
    };
    notes.extend(over_ceiling.as_deref());
    if !notes.is_empty() {
        s.push_str("\n## Notes\n\n");
        for note in notes {
            s.push_str(&format!("- {note}\n"));
        }
    }

    s
}

/// The one line a type's short card says about the names past the ambiguity
/// ceiling, which its detail lists one by one; `None` when there are none.
fn over_ceiling_note(card: &Card, lang: Lang) -> Option<String> {
    let n = card
        .caveats
        .iter()
        .filter(|c| c.kind == CaveatKind::OverCeiling)
        .count();
    if n == 0 {
        return None;
    }
    let mut line = match lang {
        Lang::En => format!("names used past the ambiguity ceiling, with no relation shown: {n}"),
        Lang::Cs => {
            format!("použitá jména nad stropem nejednoznačnosti, bez uvedeného vztahu: {n}")
        }
    };
    if card.detail.is_some() {
        line.push_str(match lang {
            Lang::En => " — the detail's notes list them",
            Lang::Cs => " — vypisují je poznámky v detailu",
        });
    }
    Some(line)
}

/// A kind word as a YAML scalar: plain when it is a bare word, quoted otherwise.
fn yaml_word(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        word.to_string()
    } else {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn member_line(card: &Card, member: &MemberRow, links: LinkStyle, index: &CardIndex) -> String {
    // A nested type is a member row AND a card of its own, so its row links. A
    // method or a field has no card and stays code style.
    let mut line = format!(
        "- {}",
        mention(&card.path, &member.identity, &member.name, links, index)
    );
    if let Some(kind) = &member.member_kind {
        line.push_str(&format!(" — {kind}"));
    }
    if let Some(visibility) = &member.visibility {
        line.push_str(&format!(", {visibility}"));
    }
    if let Some(source) = &member.source {
        line.push_str(&format!(" — `{}`", source.anchor()));
    }
    line.push('\n');
    if let Some(signature) = &member.signature {
        line.push_str(&format!("  - signature: `{signature}`\n"));
    }
    line
}

fn relation_line(card: &Card, relation: &Relation, links: LinkStyle, index: &CardIndex) -> String {
    // A name match nothing backed or disputed stays in the main lists, but
    // never reads as backed (plan A, R7): it says so at the claim. Cow avoids
    // allocating for every other rung. An ambiguous relation never reaches
    // this line — it is a `## Maybe also` claim (`maybe_also_line`).
    let precision: Cow<str> = match relation.rung {
        Some(Confidence::NameOnly) => {
            format!("{}, name only", resolution_word(relation.resolution)).into()
        }
        _ => resolution_word(relation.resolution).into(),
    };
    let mut line = format!(
        "- {} {} ({})",
        &relation.kind_word,
        mention(
            &card.path,
            &relation.identity,
            &relation.label,
            links,
            index
        ),
        precision
    );
    if let Some(via) = &relation.via {
        // Name the member the relation really belongs to, so a reader is never
        // left thinking the type declaration itself does this.
        line.push_str(&format!(" via `{via}`"));
    }
    // Every site the relation was read at, when it came from usage sites; the
    // provenance anchor otherwise. A one-site relation reads exactly as the
    // anchor did.
    match sites_anchor(&relation.sites) {
        Some(sites) => line.push_str(&format!(" — {sites}")),
        None => {
            if let Some(source) = &relation.source {
                line.push_str(&format!(" — `{}`", source.anchor()));
            }
        }
    }
    line.push('\n');
    line
}

/// One unsettled usage site (plan A, R19): the member it sits in, what it
/// does, how many declarations its name fits when more than one, which, and
/// why it is a guess (R7) — on one line, never one row per candidate. Read
/// caller to callee whichever side of it this card is on (R18).
///
/// A lone candidate a rule disputed has no "1 of 1": the count says nothing
/// there, the reason says everything.
pub(super) fn maybe_also_line(
    from: &str,
    claim: &MaybeAlso,
    lang: Lang,
    links: LinkStyle,
    index: &CardIndex,
) -> String {
    let alternatives: Vec<String> = claim
        .alternatives
        .iter()
        .map(|a| mention(from, &a.identity, &a.label, links, index))
        .collect();
    let count = if claim.candidates > 1 {
        format!("{}: ", lang.one_of(claim.candidates))
    } else {
        String::new()
    };
    let model = match claim.model {
        Some(mark) => format!("; {}", lang.model_mark(mark)),
        None => String::new(),
    };
    let mut line = format!(
        "- {} {} {count}{} ({}, ambiguous: {}{model})",
        mention(
            from,
            &claim.source.identity,
            &claim.source.label,
            links,
            index
        ),
        &claim.kind_word,
        alternatives.join(", "),
        resolution_word(claim.resolution),
        lang.reason(claim.reason)
    );
    if let Some(sites) = sites_anchor(&claim.sites) {
        line.push_str(&format!(" — {sites}"));
    }
    line.push('\n');
    line
}

/// The sites a relation was read at, as anchors: `` `src/A.cs:12` ``, one file's
/// several lines folded into one anchor (`` `src/A.cs:12, 30, 44` ``), files apart
/// (`` `a.cs:3`, `b.cs:7` ``). A site with a column reads `line:column`. `None`
/// when there is no site to name.
pub(super) fn sites_anchor(sites: &[SiteId]) -> Option<String> {
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for site in sites {
        let place = match site.column {
            Some(column) => format!("{}:{column}", site.line),
            None => site.line.to_string(),
        };
        match groups.last_mut() {
            Some((file, places)) if *file == site.file => places.push(place),
            _ => groups.push((site.file.as_str(), vec![place])),
        }
    }
    if groups.is_empty() {
        return None;
    }
    Some(
        groups
            .iter()
            .map(|(file, places)| format!("`{file}:{}`", places.join(", ")))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{DocQuote, SourceRef};
    use crate::graph::Span;

    /// Render with no other card in the corpus — every mention therefore stays
    /// text, which keeps these tests about the skeleton rather than about links.
    fn plain(card: &Card, lang: Lang) -> String {
        render_card(card, lang, LinkStyle::Portable, &CardIndex::default())
    }

    fn bare(kind: CardKind) -> Card {
        Card {
            identity: "sym:cs:Ns#Invoice".into(),
            path: "cards/type/cs/Ns/Invoice.md".into(),
            kind,
            kind_word: crate::card::card_word(kind, &crate::graph::Vocabulary::default()),
            title: "Invoice".into(),
            entity_kind: Some("class".into()),
            visibility: Some("public".into()),
            signature: None,
            header: None,
            source: Some(SourceRef {
                file: "src/Invoice.cs".into(),
                span: Span::range(12, 88),
            }),
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
    fn an_absent_doc_comment_omits_the_whole_section() {
        let out = plain(&bare(CardKind::Type), Lang::En);
        assert!(!out.contains("## Documentation"), "{out}");
    }

    /// A file card quotes its comment; a type's short card leaves it to the
    /// detail (plan B, card trim — see `tests/card.rs`).
    #[test]
    fn a_doc_comment_is_quoted_verbatim_with_its_own_anchor() {
        let mut card = bare(CardKind::File);
        card.doc = Some(DocQuote {
            text: "Faktura.\n\nDruhý odstavec s diakritikou: příliš žluťoučký kůň.".into(),
            source: SourceRef {
                file: "src/Invoice.cs".into(),
                span: Span::range(8, 11),
            },
            redacted: false,
        });
        let out = plain(&card, Lang::En);
        assert!(out.contains("> Faktura."), "{out}");
        assert!(
            out.contains("> Druhý odstavec s diakritikou: příliš žluťoučký kůň."),
            "{out}"
        );
        assert!(out.contains("— `src/Invoice.cs:8-11`"), "{out}");
    }

    #[test]
    fn lang_switches_the_prose_and_leaves_the_english_skeleton_alone() {
        let mut card = bare(CardKind::Type);
        card.signature = Some("public class Invoice".into());
        card.test = Some(super::super::TestPairing {
            // The real wording the pairing extractor emits, backtick and all —
            // an invented one would let this test pass while the extractor and
            // the renderer disagreed about the contract between them.
            statement: format!(
                "{}: `tests/InvoiceTests.cs`",
                crate::extract::test_pairing::HAS_TEST
            ),
            tier: Tier::Derived,
            from_file: Some("src/Invoice.cs".into()),
        });
        let en = plain(&card, Lang::En);
        let cs = plain(&card, Lang::Cs);

        assert!(
            en.contains("No other type or file in this repository was seen using `Invoice`."),
            "{en}"
        );
        assert!(
            cs.contains("V tomto repozitáři nebylo vidět, že by `Invoice` používal"),
            "{cs}"
        );
        // v6: the short card has no lead sentence in either language (R17).
        assert!(!en.contains("A type this repository declares."), "{en}");
        assert!(
            !cs.contains("Typ, který tento repozitář deklaruje."),
            "{cs}"
        );
        // v4: a type card's `- Kind:` bullet became the glance strip under the
        // title (`**public class**`), and `## Role` joined the skeleton — both
        // must stay English whatever `--lang` says, like every other heading.
        for heading in ["## Role", "- Identity:", "**public class**"] {
            assert!(en.contains(heading), "{en}");
            assert!(cs.contains(heading), "{cs}");
        }
        // The card trim: the signature and the test pairing are the detail's
        // (`tests/card.rs`), on a card with no detail too.
        for gone in ["## Signature", "## Test pairing"] {
            assert!(!en.contains(gone), "{en}");
            assert!(!cs.contains(gone), "{cs}");
        }
    }

    /// The card trim: the role counts the units that use a type and the ones
    /// it uses in the short form — a unit is a type, or a file for its free
    /// functions, so the count names no kind — with the number agreeing in
    /// either language.
    #[test]
    fn the_role_counts_users_and_used_units_in_the_short_form() {
        use crate::card::insight::Counts;
        use crate::card::story::Peer;
        let peers = |n: usize| -> Vec<Peer> {
            (0..n)
                .map(|i| Peer {
                    identity: format!("sym:cs:Ns#P{i}"),
                    label: format!("P{i}"),
                    counts: Counts {
                        calls: 1,
                        ..Counts::default()
                    },
                    ..Peer::default()
                })
                .collect()
        };
        let role = |n: usize, modules: usize, lang: Lang| {
            let mut card = bare(CardKind::Type);
            card.story.users = peers(n);
            card.story.users_certain = n;
            card.story.user_modules = modules;
            card.story.uses = peers(n);
            card.story.uses_total = n;
            plain(&card, lang)
        };
        let cases = [
            (
                1,
                1,
                Lang::En,
                "Used by 1 in 1 module, chiefly `P0`.\n",
                "It uses 1 other, chiefly `P0` (calls 1).\n",
            ),
            (
                5,
                2,
                Lang::En,
                "Used by 5 in 2 modules, chiefly `P0`, `P1`, `P2` and 2 more.\n",
                "It uses 5 others, chiefly `P0` (calls 1), `P1` (calls 1), `P2` (calls 1) and 2 \
                 more.\n",
            ),
            (
                1,
                1,
                Lang::Cs,
                "Používá ho 1 další v 1 modulu, hlavně `P0`.\n",
                "Pracuje s 1 dalším, hlavně `P0` (calls 1).\n",
            ),
            (
                3,
                2,
                Lang::Cs,
                "Používají ho 3 další v 2 modulech, hlavně `P0`, `P1` a `P2`.\n",
                "Pracuje s 3 dalšími, hlavně `P0` (calls 1), `P1` (calls 1) a `P2` (calls 1).\n",
            ),
            (
                5,
                5,
                Lang::Cs,
                "Používá ho 5 dalších v 5 modulech, hlavně `P0`, `P1`, `P2` a 2 dalších.\n",
                "Pracuje s 5 dalšími, hlavně `P0` (calls 1), `P1` (calls 1), `P2` (calls 1) a 2 \
                 dalších.\n",
            ),
        ];
        for (n, modules, lang, users, uses) in cases {
            let out = role(n, modules, lang);
            assert!(out.contains(users), "{users}\n{out}");
            assert!(out.contains(uses), "{uses}\n{out}");
        }
    }

    /// Plan B, U1: a type with no header read from a signature keeps the strip
    /// it always had, visibility and kind — and a header, where there is one,
    /// replaces exactly that part of the strip.
    #[test]
    fn a_type_without_a_signature_header_keeps_visibility_and_kind() {
        let mut card = bare(CardKind::Type);
        let strip = |out: &str| {
            out.lines()
                .find(|l| l.starts_with("**"))
                .map(str::to_string)
        };
        assert_eq!(
            strip(&plain(&card, Lang::En)).as_deref(),
            Some("**public class** · `src/Invoice.cs:12-88`")
        );
        card.header = Some(crate::card::header::TypeHeader {
            modifiers: vec!["public".into(), "static".into()],
            kind: "class".into(),
        });
        assert_eq!(
            strip(&plain(&card, Lang::En)).as_deref(),
            Some("**public static class** · `src/Invoice.cs:12-88`")
        );
    }

    #[test]
    fn an_unknown_lang_code_falls_back_to_english() {
        assert_eq!(Lang::from_code("kl"), Lang::En);
        assert_eq!(Lang::from_code("CS"), Lang::Cs);
        assert_eq!(Lang::from_code(" cs-CZ "), Lang::Cs);
    }

    /// A name match nothing backed or disputed stays in the main list, and says
    /// so at the row (plan A, R7) — never read as a backed relation.
    #[test]
    fn a_name_only_relation_says_so_at_the_row_with_its_anchor() {
        let mut card = bare(CardKind::File);
        card.outgoing.push(Relation {
            kind: EdgeKind::Calls,
            kind_word: "calls".into(),
            label: "Total".into(),
            identity: "sym:cs:Ns#Invoice.Total()".into(),
            resolution: Resolution::Heuristic,
            via: None,
            source: Some(SourceRef {
                file: "src/Invoice.cs".into(),
                span: Span::Line(30),
            }),
            ambiguous_candidates: None,
            rung: Some(Confidence::NameOnly),
            via_identity: None,
            sites: Vec::new(),
        });
        let out = plain(&card, Lang::En);
        assert!(
            out.contains("- calls `Total` (heuristic, name only) — `src/Invoice.cs:30`"),
            "{out}"
        );
        assert!(!out.contains("## Maybe also"), "{out}");
    }

    /// A relation a rule backed carries no label: only a name match nothing
    /// backed is marked, and the candidate count a rule settled is not shown.
    #[test]
    fn a_backed_relation_carries_no_label() {
        let mut card = bare(CardKind::File);
        card.outgoing.push(Relation {
            kind: EdgeKind::Calls,
            kind_word: "calls".into(),
            label: "Solve".into(),
            identity: "sym:cs:Ns#GeoSolver.Solve()".into(),
            resolution: Resolution::Heuristic,
            via: None,
            source: Some(SourceRef {
                file: "src/GeoSolver.cs".into(),
                span: Span::Line(1170),
            }),
            ambiguous_candidates: Some(2),
            rung: Some(Confidence::RulesBacked),
            via_identity: None,
            sites: Vec::new(),
        });
        let out = plain(&card, Lang::En);
        assert!(!out.contains("ambiguous"), "{out}");
        assert!(
            out.contains("- calls `Solve` (heuristic) — `src/GeoSolver.cs:1170`"),
            "{out}"
        );
    }

    fn site(file: &str, line: u32) -> SiteId {
        SiteId {
            file: file.into(),
            line,
            column: None,
        }
    }

    /// A row is a relation: one caller calling one member from three places is
    /// one row naming all three sites (plan A, R18), one file's lines folded
    /// into one anchor and another file's kept apart.
    #[test]
    fn a_relation_read_at_several_sites_names_every_one_of_them() {
        let mut card = bare(CardKind::File);
        card.incoming.push(Relation {
            kind: EdgeKind::Calls,
            kind_word: "calls".into(),
            label: "CleanServer.Run(GeoSolver)".into(),
            identity: "sym:cs:Ns#CleanServer.Run(GeoSolver)".into(),
            resolution: Resolution::Heuristic,
            via: Some("FoundTables()".into()),
            source: Some(SourceRef {
                file: "src/CleanServer.cs".into(),
                span: Span::Line(7),
            }),
            ambiguous_candidates: None,
            rung: Some(Confidence::RulesBacked),
            via_identity: Some("sym:cs:Ns#GeoSolver.FoundTables()".into()),
            sites: vec![
                site("src/CleanServer.cs", 7),
                site("src/CleanServer.cs", 8),
                site("src/CleanServer.cs", 9),
                site("src/Other.cs", 3),
            ],
        });
        let out = plain(&card, Lang::En);
        assert!(
            out.contains(
                "- calls `CleanServer.Run(GeoSolver)` (heuristic) via `FoundTables()` \
                 — `src/CleanServer.cs:7, 8, 9`, `src/Other.cs:3`\n"
            ),
            "{out}"
        );
    }

    /// Two calls of one name on one line are two sites once their columns are
    /// known (plan A, KTD7) — `sb.Append(a).Append(b)` — and the anchor says so.
    #[test]
    fn two_sites_on_one_line_read_apart_by_their_columns() {
        let mut card = bare(CardKind::File);
        let at = |column| SiteId {
            column: Some(column),
            ..site("src/Report.cs", 12)
        };
        card.outgoing.push(Relation {
            kind: EdgeKind::Calls,
            kind_word: "calls".into(),
            label: "Builder.Append(string)".into(),
            identity: "sym:cs:Ns#Builder.Append(string)".into(),
            resolution: Resolution::Heuristic,
            via: Some("Print()".into()),
            source: None,
            ambiguous_candidates: None,
            rung: Some(Confidence::RulesBacked),
            via_identity: Some("sym:cs:Ns#Invoice.Print()".into()),
            sites: vec![at(8), at(20)],
        });
        let out = plain(&card, Lang::En);
        assert!(out.contains("— `src/Report.cs:12:8, 12:20`\n"), "{out}");
    }

    /// The ambiguous alternatives of one usage site (plan A, AE3, R19): ONE line
    /// in its own section after `## Referenced by`, naming the site's source,
    /// the count and every alternative — and the count in the card's language,
    /// while the heading stays the English skeleton.
    fn with_one_unsettled_site() -> Card {
        let mut card = bare(CardKind::File);
        card.incoming.push(Relation {
            kind: EdgeKind::Calls,
            kind_word: "calls".into(),
            label: "CleanServer.Run(GeoSolver)".into(),
            identity: "sym:cs:Ns#CleanServer.Run(GeoSolver)".into(),
            resolution: Resolution::Heuristic,
            via: Some("FoundTables()".into()),
            source: None,
            ambiguous_candidates: None,
            rung: Some(Confidence::NameOnly),
            via_identity: Some("sym:cs:Ns#GeoSolver.FoundTables()".into()),
            sites: vec![site("src/CleanServer.cs", 7)],
        });
        let mention = |label: &str| crate::card::Mention {
            label: label.into(),
            identity: format!("sym:cs:Ns#{label}"),
        };
        card.maybe_also.push(MaybeAlso {
            kind: EdgeKind::Calls,
            kind_word: "calls".into(),
            incoming: true,
            source: mention("CleanServer.Run(GeoSolver)"),
            alternatives: vec![
                mention("GeoSolver.Solve(int)"),
                mention("GeoSolver.Solve(string)"),
            ],
            candidates: 2,
            reason: MaybeReason::SharedName,
            resolution: Resolution::Heuristic,
            sites: vec![site("src/CleanServer.cs", 10)],
            model: None,
        });
        card
    }

    /// Plan A, U12: the model's verdict is a mark inside the line's
    /// parentheses, in the card's language, after the reason — the line is
    /// still a guess, and still lists every candidate.
    #[test]
    fn a_model_verdict_marks_the_line_and_keeps_every_candidate() {
        let mut card = with_one_unsettled_site();
        card.maybe_also[0].model = Some(ModelMark::Promoted);
        let en = plain(&card, Lang::En);
        assert!(
            en.contains(
                "`GeoSolver.Solve(int)`, `GeoSolver.Solve(string)` (heuristic, ambiguous: \
                 several declarations share the name; model-verified: the first candidate) — "
            ),
            "{en}"
        );
        let cs = plain(&card, Lang::Cs);
        assert!(cs.contains("; ověřeno modelem: první kandidát)"), "{cs}");

        card.maybe_also[0].model = Some(ModelMark::Demoted);
        let en = plain(&card, Lang::En);
        assert!(en.contains("; model-verified: none of them)"), "{en}");
        assert_eq!(en.matches("Solve(").count(), 2, "{en}");
        let cs = plain(&card, Lang::Cs);
        assert!(cs.contains("; ověřeno modelem: žádný z nich)"), "{cs}");
    }

    #[test]
    fn an_unsettled_site_is_one_maybe_also_claim_after_referenced_by() {
        let card = with_one_unsettled_site();
        let en = plain(&card, Lang::En);
        let line = "- `CleanServer.Run(GeoSolver)` calls 1 of 2 candidates: \
                    `GeoSolver.Solve(int)`, `GeoSolver.Solve(string)` (heuristic, ambiguous: \
                    several declarations share the name) — `src/CleanServer.cs:10`\n";
        assert!(en.contains(line), "{en}");
        assert_eq!(
            en.matches("Solve(").count(),
            2,
            "one line, not one per candidate: {en}"
        );
        let (referenced, maybe) = (
            en.find("## Referenced by")
                .expect("a Referenced by section"),
            en.find("## Maybe also").expect("a Maybe also section"),
        );
        assert!(referenced < maybe, "Maybe also follows Referenced by: {en}");
        assert!(
            !en[referenced..maybe].contains("Solve("),
            "the guess is not a Referenced-by row: {en}"
        );

        let cs = plain(&card, Lang::Cs);
        assert!(cs.contains("## Maybe also"), "{cs}");
        assert!(
            cs.contains("calls 1 ze 2 kandidátů: `GeoSolver.Solve(int)`"),
            "{cs}"
        );
        assert!(
            cs.contains("ambiguous: stejné jméno nese víc deklarací)"),
            "{cs}"
        );
        assert!(cs.contains("Vztahy, které možná platí také"), "{cs}");
    }

    /// Plan A, R7 and AE2: a lone candidate a rule disputed reads with the
    /// rule's reason and without a count — "1 of 1 candidates" said nothing
    /// and hid why the line was a guess at all.
    #[test]
    fn a_disputed_single_candidate_reads_its_reason_and_no_count() {
        let mut card = with_one_unsettled_site();
        let claim = &mut card.maybe_also[0];
        claim.alternatives.truncate(1);
        claim.candidates = 1;
        claim.reason = MaybeReason::Disputed(Rule::DeclaredReceiver);

        let en = plain(&card, Lang::En);
        let line = "- `CleanServer.Run(GeoSolver)` calls `GeoSolver.Solve(int)` (heuristic, \
                    ambiguous: the receiver is declared as another type) — `src/CleanServer.cs:10`\n";
        assert!(en.contains(line), "{en}");
        assert!(!en.contains("1 of 1"), "{en}");

        let cs = plain(&card, Lang::Cs);
        assert!(
            cs.contains("(heuristic, ambiguous: příjemce je deklarovaný jako jiný typ)"),
            "{cs}"
        );
        assert!(!cs.contains("1 z 1"), "{cs}");
    }

    /// A rule that only ever disputes names its reason beside the count, at
    /// any count; every reason the catalog can produce has its own words in
    /// both languages, distinct from the shared name.
    #[test]
    fn every_disputing_rule_has_its_own_reason_in_both_languages() {
        let mut card = with_one_unsettled_site();
        card.maybe_also[0].reason = MaybeReason::Disputed(Rule::Arity);
        let en = plain(&card, Lang::En);
        assert!(
            en.contains(
                "calls 1 of 2 candidates: `GeoSolver.Solve(int)`, \
                         `GeoSolver.Solve(string)` (heuristic, ambiguous: the argument count \
                         does not fit)"
            ),
            "{en}"
        );

        let disputing = [
            Rule::DeclaredReceiver,
            Rule::StaticReceiver,
            Rule::Arity,
            Rule::NamespaceVisibility,
            Rule::NestedTypeVisibility,
            Rule::RoleMismatch,
        ];
        for lang in [Lang::En, Lang::Cs] {
            let shared = lang.reason(MaybeReason::SharedName);
            let words: std::collections::BTreeSet<&str> = disputing
                .iter()
                .map(|r| MaybeReason::Disputed(*r))
                .chain([MaybeReason::IndexElsewhere, MaybeReason::IndexOutside])
                .map(|reason| lang.reason(reason))
                .collect();
            assert_eq!(words.len(), disputing.len() + 2, "{lang:?}: {words:?}");
            assert!(!words.contains(shared), "{lang:?}");
        }
    }

    #[test]
    fn a_card_without_unsettled_sites_has_no_maybe_also_section() {
        let mut card = with_one_unsettled_site();
        card.maybe_also.clear();
        let out = plain(&card, Lang::En);
        assert!(out.contains("## Referenced by"), "{out}");
        assert!(!out.contains("## Maybe also"), "{out}");
    }

    /// `ze` before a numeral whose first sound `z` cannot be said before, `z`
    /// otherwise — `1 ze 2`, `1 z 5`, `1 ze 7`, `1 z 8`, `1 ze 23`.
    #[test]
    fn the_czech_count_takes_the_preposition_its_numeral_needs() {
        let words: Vec<String> = [2, 3, 5, 7, 8, 12, 15, 23, 51, 100, 500]
            .iter()
            .map(|&n| Lang::Cs.one_of(n))
            .collect();
        assert_eq!(
            words,
            [
                "1 ze 2 kandidátů",
                "1 ze 3 kandidátů",
                "1 z 5 kandidátů",
                "1 ze 7 kandidátů",
                "1 z 8 kandidátů",
                "1 ze 12 kandidátů",
                "1 z 15 kandidátů",
                "1 ze 23 kandidátů",
                "1 z 51 kandidátů",
                "1 ze 100 kandidátů",
                "1 z 500 kandidátů",
            ]
        );
    }

    #[test]
    fn every_card_kind_states_who_points_at_it() {
        let incoming = Relation {
            kind: EdgeKind::Imports,
            kind_word: "imports".into(),
            label: "src/other.rs".into(),
            identity: "src/other.rs".into(),
            resolution: Resolution::Heuristic,
            via: None,
            source: None,
            ambiguous_candidates: None,
            rung: None,
            via_identity: None,
            sites: Vec::new(),
        };

        // U8 rendered the incoming side only for a module (R13's "both
        // directions"); U9 asks the same question of every entity (R11), so a
        // file card carries it too — and a type's, since plan B, U9, in its
        // detail: the short card lists no relation at all.
        for kind in [CardKind::Module, CardKind::File] {
            let mut card = bare(kind);
            card.incoming.push(incoming.clone());
            assert!(
                plain(&card, Lang::En).contains("## Referenced by"),
                "{kind:?} must state who points at it"
            );
        }
        let mut short = bare(CardKind::Type);
        short.incoming.push(incoming);
        assert!(!plain(&short, Lang::En).contains("## Referenced by"));

        // …and a card nothing points at shows no empty section.
        assert!(!plain(&bare(CardKind::File), Lang::En).contains("## Referenced by"));
    }

    #[test]
    fn a_relation_to_a_carded_target_becomes_a_link_and_keeps_its_anchor() {
        let mut card = bare(CardKind::File);
        card.outgoing.push(Relation {
            kind: EdgeKind::References,
            kind_word: "references".into(),
            label: "Money".into(),
            identity: "sym:cs:Ns#Money".into(),
            resolution: Resolution::Heuristic,
            via: None,
            source: Some(SourceRef {
                file: "src/Invoice.cs".into(),
                span: Span::Line(30),
            }),
            ambiguous_candidates: None,
            rung: Some(Confidence::NameOnly),
            via_identity: None,
            sites: Vec::new(),
        });

        let cards = [card.clone(), {
            let mut money = bare(CardKind::Type);
            money.identity = "sym:cs:Ns#Money".into();
            money.path = "cards/type/cs/Ns/Money.md".into();
            money
        }];
        let index = CardIndex::build(&cards);
        let out = render_card(&card, Lang::En, LinkStyle::Portable, &index);

        assert!(
            out.contains(
                "- references [Money](Money.md) (heuristic, name only) — `src/Invoice.cs:30`"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_capped_backlink_section_states_how_many_it_left_out() {
        let mut card = bare(CardKind::File);
        for i in 0..(super::super::links::BACKLINK_CAP + 3) {
            card.incoming.push(Relation {
                kind: EdgeKind::Calls,
                kind_word: "calls".into(),
                label: format!("Caller{i}"),
                identity: format!("sym:cs:Ns#Caller{i}"),
                resolution: Resolution::Heuristic,
                via: None,
                source: None,
                ambiguous_candidates: None,
                rung: Some(Confidence::NameOnly),
                via_identity: None,
                sites: Vec::new(),
            });
        }

        let out = plain(&card, Lang::En);
        assert!(
            out.contains("Not listed here: 3 more calls"),
            "a cap must never be silent, and must name the kind it thinned: {out}"
        );
        assert_eq!(
            out.matches("- calls ").count(),
            super::super::links::BACKLINK_CAP
        );
    }

    #[test]
    fn two_renders_of_one_card_are_byte_identical() {
        let card = bare(CardKind::Type);
        assert_eq!(plain(&card, Lang::En), plain(&card, Lang::En));
    }
}
