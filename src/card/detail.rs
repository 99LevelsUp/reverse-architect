//! The detail of a card: everything a reader goes to the source for, in a
//! document of its own (plan B, U8: R3, R9, R11, R12, R13, R17).
//!
//! A card says what an entity is and what it works with; the detail says
//! WHERE. Its sections, in order, are the same English skeleton on every
//! detail (R14):
//!
//! - `Anchors` — the texts a reader searches for (a message, a query, a
//!   comment line), each leading to the member and the line it sits on; a
//!   text the secret filter withheld leaves only its place and the reason;
//! - `Database` — per member, the objects its SQL reads, writes, calls as a
//!   procedure or only mentions, with the lines (AE1, AE9);
//! - `Enum decisions` — on an enum's detail, the places other code decides by
//!   its values (AE6): the decisions are read off the deciding methods and
//!   grouped under the enum they name, so the enum's detail answers "where
//!   does `Full` matter" without a search;
//! - `API`, `Relations`, `Referenced by`, `Maybe also` and `Collaboration` —
//!   the full lists the short card leaves out (R17, plan B Key Decision on
//!   "maybe also"), every relation with the rung it stands on;
//! - on a type, `Documentation` and `Signature` first, `Test pairing` and
//!   `Notes` last: the whole doc comment, the declaration header, the test
//!   pairing, and the caveats about what this detail lists — a shared name, a
//!   name past the ambiguity ceiling, a withheld literal or comment line —
//!   which the short card leaves here with what they qualify (plan B, card
//!   trim after U9). The signature and the pairing left the card last, to
//!   bring its median under 1.2 kB: the card's header line states the kind
//!   and modifiers the signature does (R4).
//!
//! A file's detail (KTD7) has only `Anchors` and `Database`: the texts and
//! accesses no type owns — a C or C++ file, which has no plugin, a literal
//! outside every declaration, or a free function's.
//!
//! A detail exists only when it says something its card does not
//! ([`wants_detail`]). A large one is cut into `## Part N` with its sections
//! as `###` inside, by the same greedy packing the views use
//! ([`crate::render::split::fixed_partition`], KTD8): the manifest chunks a
//! document by its `##` headings, so no section stays one giant chunk.
//!
//! Like a card, a detail is rendered from the finished card set and a
//! read-only index alone, so the same graph gives the same bytes (R15).

use super::explain::{self, code, Out};
use super::insight::Insight;
use super::links::{link, mention, CardIndex};
use super::render::{
    doc_quote, maybe_also_line, quoted_doc, resolution_word, signature_block, sites_anchor,
    test_pairing_line, Lang, CARD_SCHEMA_VERSION,
};
use super::story::{kind_group, kind_rank};
use super::{fact_text, params_of, symbol_label, Card, CardKind, Relation, SourceRef};
use crate::cli::LinkStyle;
use crate::graph::{
    AnchorSource, AnchorText, Confidence, DbAccessKind, DecisionShape, EnumDecision, FactKind,
    FactValue, Node, NodeKind, SiteId, Span, ANCHOR_CAP,
};
use crate::render::split::{fixed_partition, ChunkWeight};
use crate::render::RenderedFile;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

/// The size, in bytes, above which a detail is cut into parts (KTD8).
///
/// A part is what one chunk of the manifest carries (`## Part N`), so this is
/// a chunk budget: 8 KiB is some two thousand tokens of English and fewer of
/// Czech, which a retriever or an agent reads in one piece. A detail at or
/// under it stays whole, its sections at `##`. It counts the one-line header
/// too, because a part read alone is read with it.
pub const DETAIL_PART_BYTES: usize = 8 * 1024;

/// The place-level facts of one card, which only its detail shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Navigators {
    /// Every anchor and every withheld text, in source order.
    pub anchors: Vec<AnchorRow>,
    /// Where an owner had more texts than [`ANCHOR_CAP`] kept.
    pub dropped: Vec<DroppedRow>,
    /// One per database access, by member, then access, object and place.
    pub db: Vec<DbRow>,
    /// On an enum's card: the places code decides by its values, the verified
    /// ones first.
    pub decisions: Vec<DecisionRow>,
}

impl Navigators {
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
            && self.dropped.is_empty()
            && self.db.is_empty()
            && self.decisions.is_empty()
    }
}

/// One text a reader may search for, or one place a text was withheld.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRow {
    /// The member the text sits in, with its parameters (`Login(string)`);
    /// `None` when the carded entity itself holds it — the type, or the file
    /// outside every declaration.
    pub member: Option<String>,
    /// The first line the text occurs on in its owner.
    pub at: SourceRef,
    pub body: AnchorBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorBody {
    Text(AnchorText),
    /// A text the secret filter withheld (R10): the reason only — the
    /// pattern's name or `long token` — never the text.
    Withheld(String),
}

/// How many texts an owner lost to [`ANCHOR_CAP`] in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedRow {
    pub member: Option<String>,
    pub file: String,
    pub count: i64,
}

/// One database access at one place (R11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRow {
    /// See [`AnchorRow::member`].
    pub member: Option<String>,
    /// The member's identity, what tells two overloads' rows apart when their
    /// names with parameters do not.
    pub member_identity: String,
    pub access: DbAccessKind,
    pub object: String,
    pub at: SourceRef,
}

/// One place code decides by an enum's value (R12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    /// The deciding member as a relation names it (`LoadAction.Run()`).
    pub place: String,
    /// The card the deciding member belongs to, to link the place by.
    pub unit: Option<String>,
    pub at: SourceRef,
    pub shape: DecisionShape,
    /// The enum value decided on.
    pub value: String,
    pub verified: bool,
}

/// Every unit's place-level facts and every enum's decisions, read in one
/// pass over the nodes and handed out card by card.
///
/// The anchors and accesses are lifted to the unit the way the relations are
/// ([`Insight::unit_of`]): a method's to its type, a free function's to its
/// file, the parts of a `partial` type to the one type. The decisions are
/// keyed the other way round, by the enum they name, because the reader of a
/// decision is the enum's detail (AE6), not the deciding type's.
pub(super) struct NavigatorIndex {
    by_unit: HashMap<String, Navigators>,
    decisions: HashMap<String, Vec<DecisionRow>>,
}

impl NavigatorIndex {
    pub(super) fn build(nodes: &BTreeMap<&str, &Node>, insight: &Insight) -> NavigatorIndex {
        let mut index = NavigatorIndex {
            by_unit: HashMap::new(),
            decisions: HashMap::new(),
        };
        for node in nodes.values() {
            if !matches!(node.kind, NodeKind::Symbol | NodeKind::File)
                || !node.facts.iter().any(|f| is_navigator(f.kind))
            {
                continue;
            }
            let key = node.key.as_str();
            let unit = insight.unit_of(key);
            let member = (unit != Some(key)).then(|| member_label(node));
            for fact in &node.facts {
                if !is_navigator(fact.kind) {
                    continue;
                }
                let at = SourceRef::from(&fact.provenance);
                match (&fact.kind, &fact.value) {
                    (FactKind::EnumDecision, FactValue::Text(value)) => {
                        let Some(decision) = EnumDecision::decode(value) else {
                            continue;
                        };
                        let EnumDecision {
                            verified,
                            shape,
                            enum_key,
                            member: value,
                        } = decision;
                        index
                            .decisions
                            .entry(enum_key.as_str().to_string())
                            .or_default()
                            .push(DecisionRow {
                                place: symbol_label(node).unwrap_or_else(|| key.to_string()),
                                unit: unit.map(str::to_string),
                                at,
                                shape,
                                value,
                                verified,
                            });
                    }
                    (kind, value) => {
                        // A fact no card shows: the node belongs to no unit.
                        let Some(unit) = unit else { continue };
                        let nav = index.by_unit.entry(unit.to_string()).or_default();
                        match (kind, value) {
                            (FactKind::Anchor, FactValue::Text(v)) => {
                                if let Some(text) = AnchorText::decode(v) {
                                    nav.anchors.push(AnchorRow {
                                        member: member.clone(),
                                        at,
                                        body: AnchorBody::Text(text),
                                    });
                                }
                            }
                            // Both ways a text is withheld (R10): a comment
                            // line or a long token (`AnchorSkipped`), and a
                            // literal the pattern table matched whole
                            // (`RedactedLiteral`). Each carries its reason.
                            (
                                FactKind::AnchorSkipped | FactKind::RedactedLiteral,
                                FactValue::Text(reason),
                            ) => nav.anchors.push(AnchorRow {
                                member: member.clone(),
                                at,
                                body: AnchorBody::Withheld(reason.clone()),
                            }),
                            (FactKind::AnchorsDropped, FactValue::Int(count)) => {
                                nav.dropped.push(DroppedRow {
                                    member: member.clone(),
                                    file: at.file,
                                    count: *count,
                                })
                            }
                            (FactKind::DbAccess, FactValue::Text(v)) => {
                                if let Some((access, object)) = DbAccessKind::decode(v) {
                                    nav.db.push(DbRow {
                                        member: member.clone(),
                                        member_identity: key.to_string(),
                                        access,
                                        object: object.to_string(),
                                        at,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // Content order, never the order the nodes or their facts came in
        // (R15).
        for nav in index.by_unit.values_mut() {
            nav.anchors.sort_by(|a, b| {
                place_key(&a.at)
                    .cmp(&place_key(&b.at))
                    .then_with(|| a.member.cmp(&b.member))
                    .then_with(|| body_key(&a.body).cmp(&body_key(&b.body)))
            });
            nav.dropped
                .sort_by(|a, b| (&a.member, &a.file, a.count).cmp(&(&b.member, &b.file, b.count)));
            nav.db.sort_by(|a, b| {
                (&a.member, &a.member_identity, a.access, &a.object)
                    .cmp(&(&b.member, &b.member_identity, b.access, &b.object))
                    .then_with(|| place_key(&a.at).cmp(&place_key(&b.at)))
            });
        }
        for rows in index.decisions.values_mut() {
            rows.sort_by(|a, b| {
                (!a.verified, &a.value, &a.place)
                    .cmp(&(!b.verified, &b.value, &b.place))
                    .then_with(|| place_key(&a.at).cmp(&place_key(&b.at)))
                    .then_with(|| a.shape.cmp(&b.shape))
            });
        }
        index
    }

    /// The share of the card `identity` of `kind`: its unit's anchors and
    /// accesses, and — a type only — the decisions on it as an enum.
    pub(super) fn take(&mut self, kind: CardKind, identity: &str) -> Navigators {
        let mut nav = self.by_unit.remove(identity).unwrap_or_default();
        if kind == CardKind::Type {
            nav.decisions = self.decisions.remove(identity).unwrap_or_default();
        }
        nav
    }
}

/// The fact kinds a detail's navigator sections read.
fn is_navigator(kind: FactKind) -> bool {
    matches!(
        kind,
        FactKind::Anchor
            | FactKind::AnchorSkipped
            | FactKind::AnchorsDropped
            | FactKind::RedactedLiteral
            | FactKind::DbAccess
            | FactKind::EnumDecision
    )
}

/// A member as a navigator row names it: its name and, for a callable, its
/// parameter list (`Run()`), as `via` names it on a relation.
fn member_label(node: &Node) -> String {
    let name = fact_text(node, FactKind::SymbolName).unwrap_or(node.key.as_str());
    match params_of(node.key.as_str()) {
        Some(params) => format!("{name}{params}"),
        None => name.to_string(),
    }
}

/// `(file, first line)` — the order a reader walks a file in.
fn place_key(at: &SourceRef) -> (&str, u32) {
    let line = match at.span {
        Span::Whole => 0,
        Span::Line(line) => line,
        Span::Range { start, .. } => start,
    };
    (at.file.as_str(), line)
}

/// A total order over what an anchor row says, so two rows at one place in
/// one member never fall back to the order their facts came in (R15).
fn body_key(body: &AnchorBody) -> (u8, &str, Option<AnchorSource>, u32, u32) {
    match body {
        AnchorBody::Text(text) => (
            0,
            text.text.as_str(),
            Some(text.source),
            text.occurrences,
            text.length,
        ),
        AnchorBody::Withheld(reason) => (1, reason.as_str(), None, 0, 0),
    }
}

/// Whether `card` gets a detail: only when the detail would say something
/// the card does not (R3, plan B Key Decision on details).
///
/// - A file: when it holds texts or database accesses no type owns — its own,
///   outside every declaration, or its free functions' (KTD7).
/// - A type: when it has anchors, accesses or decisions on it as an enum; an
///   API — every member list left the short card (R17); a "maybe also" claim,
///   which left it too; more peers than the role sentences name
///   ([`explain::role_names_every_peer`]); a doc comment the purpose does not
///   already say whole ([`quoted_doc`]); or a caveat the short card does not
///   show ([`super::CaveatKind::on_short_card`]) — the comment and those
///   caveats left the card too. A type whose every relation the card's
///   sentences already name gets no detail for its relations alone — its
///   lines are not worth a document (plan B, U8 test scenarios).
/// - Any other card: never. A module has a map (U10), and a declared card
///   kind keeps every list on itself.
///
/// Once a detail exists it carries every section, not only the ones that
/// earned it. The signature and the test pairing earn none: every type has a
/// signature and almost every one a pairing, so counting them would write a
/// detail for every type (plan B Key Decision on details). A type with no
/// detail shows its kind and modifiers in the card's header line and its
/// signature nowhere else.
pub fn wants_detail(card: &Card) -> bool {
    let nav = &card.navigators;
    match card.kind {
        CardKind::File => !nav.anchors.is_empty() || !nav.dropped.is_empty() || !nav.db.is_empty(),
        CardKind::Type => {
            !nav.is_empty()
                || !card.story.api.is_empty()
                || !card.story.internals.is_empty()
                || !card.maybe_also.is_empty()
                || !explain::role_names_every_peer(&card.story)
                || quoted_doc(card).is_some()
                || card.caveats.iter().any(|c| !c.kind.on_short_card())
        }
        CardKind::Module | CardKind::Declared(_) => false,
    }
}

/// Render the detail of every card that has one, as corpus-relative files
/// ready to write, in card order. Each detail renders from its card and the
/// read-only index alone, so they render in parallel.
pub fn render_detail_files(
    cards: &[Card],
    index: &CardIndex,
    lang: Lang,
    links: LinkStyle,
) -> Vec<RenderedFile> {
    use rayon::prelude::*;
    cards
        .par_iter()
        .filter_map(|card| {
            Some(RenderedFile {
                path: card.detail.clone()?,
                contents: render_detail(card, lang, links, index)?,
            })
        })
        .collect()
}

/// One card's detail, or `None` for a card that has none.
pub fn render_detail(
    card: &Card,
    lang: Lang,
    links: LinkStyle,
    index: &CardIndex,
) -> Option<String> {
    render_within(card, lang, links, index, DETAIL_PART_BYTES)
}

fn render_within(
    card: &Card,
    lang: Lang,
    links: LinkStyle,
    index: &CardIndex,
    threshold: usize,
) -> Option<String> {
    let from = card.detail.as_deref()?;
    let out = Out {
        card,
        from,
        links,
        index,
        lang,
    };
    let mut head = format!(
        "---\nschema: {CARD_SCHEMA_VERSION}\ntool: reverse-architect\nkind: detail\nof: {}\n---\n\n# {}\n\n",
        card.kind_word, card.title
    );
    // The one-line header (R4): the card's own strip — kind as the signature
    // states it, module, size, source — and the way back to the card.
    let mut header = explain::glance(&out, &card.story);
    let back = link(from, &card.path, &card.title, links);
    if header.is_empty() {
        header = format!("card: {back}");
    } else {
        let _ = write!(header, " · card: {back}");
    }
    header.push('\n');
    head.push_str(&header);
    let sections = sections(&out);
    Some(head + &layout(&header, &sections, threshold))
}

/// One `##` section of a detail: its heading word, an optional lead sentence,
/// and its items — the indivisible units the parts are cut between.
struct Section {
    name: &'static str,
    lead: Option<&'static str>,
    items: Vec<String>,
}

fn sections(out: &Out) -> Vec<Section> {
    let card = out.card;
    let lang = out.lang;
    let mut sections = Vec::new();
    if card.kind == CardKind::Type {
        // The comment is one item: a quote is read whole, so no part ever
        // cuts it.
        sections.push(Section {
            name: "Documentation",
            lead: None,
            items: quoted_doc(card).map(doc_quote).into_iter().collect(),
        });
        // One item too: the fence must never be cut between parts.
        sections.push(Section {
            name: "Signature",
            lead: None,
            items: card
                .signature
                .as_deref()
                .map(signature_block)
                .into_iter()
                .collect(),
        });
    }
    sections.extend([
        Section {
            name: "Anchors",
            lead: Some(anchors_lead(lang)),
            items: anchor_items(out),
        },
        Section {
            name: "Database",
            lead: Some(database_lead(lang)),
            items: db_items(out),
        },
    ]);
    if card.kind == CardKind::Type {
        sections.push(Section {
            name: "Enum decisions",
            lead: Some(decisions_lead(lang)),
            items: card
                .navigators
                .decisions
                .iter()
                .map(|row| decision_item(out, row))
                .collect(),
        });
        sections.push(Section {
            name: "API",
            lead: None,
            items: api_items(out),
        });
        sections.push(Section {
            name: "Relations",
            lead: None,
            items: card
                .outgoing
                .iter()
                .chain(&card.externals)
                .map(|r| relation_item(out, r))
                .collect(),
        });
        sections.push(Section {
            name: "Referenced by",
            lead: None,
            items: card
                .incoming
                .iter()
                .map(|r| relation_item(out, r))
                .collect(),
        });
        sections.push(Section {
            name: "Maybe also",
            lead: Some(lang.maybe_also_lead()),
            items: card
                .maybe_also
                .iter()
                .map(|claim| maybe_also_line(out.from, claim, lang, out.links, out.index))
                .collect(),
        });
        sections.push(Section {
            name: "Collaboration",
            lead: None,
            items: explain::diagram_body(
                lang,
                &card.title,
                &card.story.users,
                &card.story.uses,
                explain::Rest::RelationsAbove,
            )
            .into_iter()
            .collect(),
        });
        sections.push(Section {
            name: "Test pairing",
            lead: None,
            items: card.test.iter().map(test_pairing_line).collect(),
        });
        // Every caveat the short card leaves here; the ones it shows itself
        // are about the card, and stay with it.
        sections.push(Section {
            name: "Notes",
            lead: None,
            items: card
                .caveats
                .iter()
                .filter(|c| !c.kind.on_short_card())
                .map(|c| format!("- {}\n", c.text))
                .collect(),
        });
    }
    sections.retain(|s| !s.items.is_empty());
    sections
}

/// A section's heading as it is rendered: `##` in a whole detail, `###`
/// inside a part, with the lead under it where it is the section's first
/// appearance.
fn heading(level: &str, section: &Section, with_lead: bool) -> String {
    let mut s = format!("\n{level} {}\n\n", section.name);
    if let (true, Some(lead)) = (with_lead, section.lead) {
        s.push_str(lead);
        s.push_str("\n\n");
    }
    s
}

/// The longest a `## Part N` heading is spelled — counted into every part's
/// weight, so a part's size never depends on how many parts precede it.
const PART_HEADING_BYTES: usize = "\n## Part 9999\n".len();

/// The sections laid out whole when they fit `threshold` with the header, cut
/// into `## Part N` otherwise (KTD8).
fn layout(header: &str, sections: &[Section], threshold: usize) -> String {
    let mut whole = String::new();
    for section in sections {
        whole.push_str(&heading("##", section, true));
        for item in &section.items {
            whole.push_str(item);
        }
    }
    if header.len() + whole.len() <= threshold {
        return whole;
    }

    // (section, first item of the section, item)
    let items = sections.iter().enumerate().flat_map(|(i, section)| {
        section
            .items
            .iter()
            .enumerate()
            .map(move |(n, item)| (i, n == 0, item.as_str()))
    });
    let mut weight = PartWeight {
        sections,
        base: header.len() + PART_HEADING_BYTES,
        bytes: 0,
        open: None,
    };
    weight.reset();
    let parts = fixed_partition(items, threshold, &mut weight);

    let mut s = String::new();
    for (n, part) in parts.iter().enumerate() {
        let _ = write!(s, "\n## Part {}\n", n + 1);
        let mut open: Option<usize> = None;
        for &(section, first, item) in part {
            if open != Some(section) {
                s.push_str(&heading("###", &sections[section], first));
                open = Some(section);
            }
            s.push_str(item);
        }
    }
    s
}

/// The bytes a part holds: the header and the part's heading, every item, and
/// each section's heading where the section opens in the part — exactly what
/// [`layout`] writes, so a part over the threshold is one item that alone
/// outweighs it.
struct PartWeight<'a> {
    sections: &'a [Section],
    base: usize,
    bytes: usize,
    open: Option<usize>,
}

impl PartWeight<'_> {
    fn of(&self, &(section, first, item): &(usize, bool, &str)) -> usize {
        let opening = if self.open == Some(section) {
            0
        } else {
            heading("###", &self.sections[section], first).len()
        };
        opening + item.len()
    }
}

impl<'i> ChunkWeight<(usize, bool, &'i str)> for PartWeight<'_> {
    fn peek(&self, unit: &(usize, bool, &'i str)) -> usize {
        self.bytes + self.of(unit)
    }
    fn add(&mut self, unit: &(usize, bool, &'i str)) {
        self.bytes += self.of(unit);
        self.open = Some(unit.0);
    }
    fn reset(&mut self) {
        self.bytes = self.base;
        self.open = None;
    }
}

fn anchors_lead(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "Texts a reader may search for — string literals and comment lines — and where each \
             sits. A text the secret filter withheld shows only its place and the reason."
        }
        Lang::Cs => {
            "Texty, podle kterých se dá hledat — řetězcové literály a řádky komentářů — a kde \
             který leží. Text, který vynechal filtr tajemství, uvádí jen místo a důvod."
        }
    }
}

fn database_lead(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "What each member's SQL does with the database objects it names, with the lines: \
             `reads`, `writes`, `calls` a procedure, or only `mentions` an object where the \
             statement's target could not be told."
        }
        Lang::Cs => {
            "Co SQL kterého člena dělá s databázovými objekty, které jmenuje, s řádky: `reads` \
             čte, `writes` zapisuje, `calls` volá proceduru a `mentions` jen uvádí objekt tam, \
             kde cíl příkazu nešlo určit."
        }
    }
}

fn decisions_lead(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "Where code decides by this enum's value. A `case` label or a switch arm is verified \
             when its qualifier can name this enum alone; the places marked unverified follow — \
             every comparison, which a local of the same name could stand behind, and any label \
             the visibility rules did not settle."
        }
        Lang::Cs => {
            "Kde se kód rozhoduje podle hodnoty tohoto enumu. Návěští `case` a rameno switch \
             jsou ověřená, když jejich kvalifikátor může jmenovat jen tento enum; místa označená \
             unverified následují — každé porovnání, za kterým může stát stejně pojmenovaná \
             lokální proměnná, a návěští, které pravidla viditelnosti nerozhodla."
        }
    }
}

/// ` → `Login(string)` · `src/A.cs:42`` — the member a row belongs to, when
/// it is not the carded entity itself, and its place.
fn place(member: Option<&str>, at: &str) -> String {
    match member {
        Some(member) => format!(" → {} · `{at}`", code(member)),
        None => format!(" · `{at}`"),
    }
}

fn anchor_items(out: &Out) -> Vec<String> {
    let nav = &out.card.navigators;
    let cs = out.lang == Lang::Cs;
    let mut items: Vec<String> = nav
        .anchors
        .iter()
        .map(|row| {
            let at = place(row.member.as_deref(), &row.at.anchor());
            match &row.body {
                AnchorBody::Text(text) => {
                    let mut line = format!("- {}", code(&text.text));
                    if text.source == AnchorSource::Comment {
                        line.push_str(" (comment)");
                    }
                    if text.occurrences > 1 {
                        let _ = write!(line, " ×{}", text.occurrences);
                    }
                    if text.length as usize > text.text.chars().count() {
                        let _ = if cs {
                            write!(line, " (zkráceno z {} znaků)", text.length)
                        } else {
                            write!(line, " (cut from {} characters)", text.length)
                        };
                    }
                    format!("{line}{at}\n")
                }
                AnchorBody::Withheld(reason) => {
                    let word = if cs { "vynecháno" } else { "withheld" };
                    format!("- {word}: {reason}{at}\n")
                }
            }
        })
        .collect();
    items.extend(nav.dropped.iter().map(|row| {
        let at = place(row.member.as_deref(), &row.file);
        if cs {
            format!(
                "- nezachováno dalších textů: {} (nejvýš {ANCHOR_CAP} na vlastníka a soubor){at}\n",
                row.count
            )
        } else {
            format!(
                "- {} more texts not kept (at most {ANCHOR_CAP} per owner and file){at}\n",
                row.count
            )
        }
    }));
    items
}

/// One item per member: the member, then each object it reaches and how,
/// with every place (R11). The access words are the fact vocabulary, English
/// in either language.
fn db_items(out: &Out) -> Vec<String> {
    let card = out.card;
    let own = match card.kind {
        CardKind::File => match out.lang {
            // Not "outside any declaration": a C file has no plugin, so a
            // query inside a function is the file's too — no declaration
            // around it was read, which is all this may claim.
            Lang::En => "the file (no declaration read around it)".to_string(),
            Lang::Cs => "soubor (bez přečtené deklarace kolem)".to_string(),
        },
        _ => code(&card.title),
    };
    let mut items: Vec<String> = Vec::new();
    let rows = &card.navigators.db;
    let mut i = 0;
    while i < rows.len() {
        let member = (&rows[i].member, &rows[i].member_identity);
        let end = rows[i..]
            .iter()
            .position(|r| (&r.member, &r.member_identity) != member)
            .map_or(rows.len(), |n| i + n);
        let mut item = format!(
            "- {}\n",
            rows[i].member.as_deref().map_or_else(|| own.clone(), code)
        );
        let mut j = i;
        while j < end {
            let what = (rows[j].access, &rows[j].object);
            let stop = rows[j..end]
                .iter()
                .position(|r| (r.access, &r.object) != what)
                .map_or(end, |n| j + n);
            let sites: Vec<SiteId> = rows[j..stop]
                .iter()
                .map(|r| SiteId {
                    file: r.at.file.clone(),
                    line: place_key(&r.at).1,
                    column: None,
                })
                .collect();
            let _ = write!(item, "  - {} {}", what.0.word(), code(what.1));
            if let Some(sites) = sites_anchor(&sites) {
                let _ = write!(item, " — {sites}");
            }
            item.push('\n');
            j = stop;
        }
        items.push(item);
        i = end;
    }
    items
}

fn decision_item(out: &Out, row: &DecisionRow) -> String {
    let shape = match row.shape {
        DecisionShape::Label => "case label",
        DecisionShape::Arm => "switch arm",
        DecisionShape::Comparison => "comparison",
    };
    let place = match &row.unit {
        Some(unit) => mention(out.from, unit, &row.place, out.links, out.index),
        None => code(&row.place),
    };
    let mut line = format!(
        "- {} — {shape} in {place} — `{}`",
        code(&row.value),
        row.at.anchor()
    );
    if !row.verified {
        line.push_str(" (unverified)");
    }
    line.push('\n');
    line
}

/// The offered members under their kind, then the members not offered — the
/// card's former `## API` and `## Internals`, one item per member.
fn api_items(out: &Out) -> Vec<String> {
    let story = &out.card.story;
    let mut items: Vec<String> = Vec::new();
    let mut group: Option<u8> = None;
    for row in &story.api {
        let rank = kind_rank(row.kind.as_deref());
        let mut item = String::new();
        if group != Some(rank) {
            if !items.is_empty() {
                item.push('\n');
            }
            let _ = write!(item, "**{}**\n\n", kind_group(rank));
            group = Some(rank);
        }
        item.push_str(&explain::api_row(out, row));
        items.push(item);
    }
    for (n, row) in story.internals.iter().enumerate() {
        let mut item = String::new();
        if n == 0 {
            if !items.is_empty() {
                item.push('\n');
            }
            item.push_str("**Internals**\n\n");
        }
        let _ = write!(item, "- {}", code(&row.name));
        if let Some(kind) = &row.kind {
            let _ = write!(item, " — {kind}");
        }
        if let Some(summary) = &row.summary {
            let _ = write!(item, ": {summary}");
        }
        if let Some(anchor) = &row.anchor {
            let _ = write!(item, " · `{anchor}`");
        }
        item.push('\n');
        items.push(item);
    }
    items
}

/// A relation as the card's `## Relations` row reads, with the rung it
/// stands on spelled out (plan A, R1): `rules-backed`, `name only`,
/// `resolved by index`; a relation no name resolution produced has no rung
/// and reads as its precision alone.
fn relation_item(out: &Out, relation: &Relation) -> String {
    let resolution = resolution_word(relation.resolution);
    let precision = match relation.rung {
        Some(Confidence::ResolvedByIndex) => "resolved by index".to_string(),
        Some(Confidence::RulesBacked) => format!("{resolution}, rules-backed"),
        Some(Confidence::NameOnly) => format!("{resolution}, name only"),
        // Never in the main lists — a "maybe also" claim carries it.
        Some(Confidence::Ambiguous) => format!("{resolution}, ambiguous"),
        None => resolution.to_string(),
    };
    let mut line = format!(
        "- {} {} ({precision})",
        relation.kind_word,
        mention(
            out.from,
            &relation.identity,
            &relation.label,
            out.links,
            out.index
        )
    );
    if let Some(via) = &relation.via {
        let _ = write!(line, " via `{via}`");
    }
    match sites_anchor(&relation.sites) {
        Some(sites) => {
            let _ = write!(line, " — {sites}");
        }
        None => {
            if let Some(source) = &relation.source {
                let _ = write!(line, " — `{}`", source.anchor());
            }
        }
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(name: &'static str, items: &[&str]) -> Section {
        Section {
            name,
            lead: Some("Lead."),
            items: items.iter().map(|i| i.to_string()).collect(),
        }
    }

    /// The items of a laid-out detail, in order.
    fn items_of(text: &str) -> Vec<&str> {
        text.lines().filter(|l| l.starts_with("- ")).collect()
    }

    /// The `## Part N` blocks of a laid-out detail.
    fn parts_of(text: &str) -> Vec<&str> {
        text.split("\n## Part ").skip(1).collect()
    }

    #[test]
    fn a_detail_under_the_threshold_keeps_its_sections_at_level_two() {
        let sections = [section("Anchors", &["- a\n"]), section("API", &["- b\n"])];
        let text = layout("header\n", &sections, 1000);
        assert_eq!(
            text,
            "\n## Anchors\n\nLead.\n\n- a\n\n## API\n\nLead.\n\n- b\n"
        );
    }

    /// KTD8: past the threshold the detail is cut between items into `## Part
    /// N`, sections `###` inside; a section running into the next part opens
    /// there again under its heading, without repeating its lead. No part
    /// outweighs the threshold unless it is one item that alone does, and the
    /// parts together hold every item exactly once, in order.
    #[test]
    fn a_large_detail_is_cut_between_items_and_loses_none() {
        let relations: Vec<String> = (0..40).map(|i| format!("- relation {i:02}\n")).collect();
        let big = "- x".repeat(60) + "\n";
        let mut items: Vec<&str> = relations.iter().map(String::as_str).collect();
        items.insert(20, &big);
        let sections = [section("Anchors", &["- a\n"]), section("Relations", &items)];
        let header = "header\n";
        let threshold = 160;
        let text = layout(header, &sections, threshold);

        let parts = parts_of(&text);
        assert!(parts.len() > 2, "{text}");
        assert!(!text.contains("\n## Anchors"), "{text}");
        assert_eq!(text.matches("\n### Anchors\n").count(), 1, "{text}");
        assert!(text.matches("\n### Relations\n").count() > 1, "{text}");
        assert_eq!(text.matches("Lead.").count(), 2, "{text}");
        for (n, part) in parts.iter().enumerate() {
            assert!(part.starts_with(&format!("{}\n", n + 1)), "{part}");
            let weight = header.len() + "\n## Part ".len() + part.len();
            assert!(
                weight <= threshold || items_of(part).len() == 1,
                "part {} weighs {weight}: {part}",
                n + 1
            );
        }
        let mut expected = vec!["- a"];
        expected.extend(items.iter().map(|i| i.trim_end()));
        assert_eq!(items_of(&text), expected);
    }

    #[test]
    fn the_same_sections_are_cut_the_same_way_twice() {
        let items: Vec<String> = (0..30).map(|i| format!("- item {i}\n")).collect();
        let refs: Vec<&str> = items.iter().map(String::as_str).collect();
        let sections = [section("Relations", &refs)];
        assert_eq!(layout("h\n", &sections, 120), layout("h\n", &sections, 120));
    }
}
