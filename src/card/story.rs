//! The explaining half of a card: what the entity is for, who it works with,
//! what it offers, and what exercises it.
//!
//! [`super::build_card`] collects the evidence — every relation with its anchor.
//! A reader cannot use that directly: a class with forty call sites into six
//! collaborators renders forty rows and still does not say "it works with these
//! six". [`Story`] is that sentence, computed from [`Insight`]'s aggregation, so
//! a card leads with the answer and keeps the evidence underneath it.
//!
//! Nothing in a story is authored per repository or inferred beyond lifting and
//! counting. A summary is the first sentence of the entity's own doc comment; a
//! collaborator is a unit some counted relation reaches; a test is a unit in a
//! test file that reaches this one. Every list is ranked by a total order that
//! ends in identity (R18).

use super::header::TypeHeader;
use super::insight::{Counts, ExternalUse, Insight};
use super::keywords::Keyword;
use super::purpose;
use super::{CardKind, MemberRow, Relation};
use crate::graph::{FactKind, FactValue, Node};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How many peers a story keeps per list. The card says how many it left out.
pub const PEER_CAP: usize = 40;

/// Another unit or module this one relates to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Peer {
    pub identity: String,
    pub label: String,
    pub summary: Option<String>,
    /// The declared kind word (`class`, `interface`, …); `None` for a file or a module.
    pub kind: Option<String>,
    pub module: Option<String>,
    pub counts: Counts,
    /// The members of THIS entity the relation leaves from (for `uses`) or the
    /// members of the PEER it arrives from (never tracked: a peer's members are
    /// its own card's business).
    pub via: Vec<String>,
}

/// One offered member: a method, a property, a field, a nested type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiRow {
    pub identity: String,
    pub name: String,
    pub kind: Option<String>,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub summary: Option<String>,
    pub anchor: Option<String>,
    /// The other units this member reaches, on undisputed relations only — a
    /// name match nothing backed counts, an ambiguous one does not (plan A,
    /// KTD2).
    pub reaches: Vec<(String, String)>,
}

/// A file inside a module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileRow {
    pub identity: String,
    pub lines: Option<i64>,
    pub declares: Vec<(String, String)>,
    pub is_test: bool,
}

/// A direct sub-module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleRow {
    pub identity: String,
    pub lines: Option<i64>,
    pub summary: Option<String>,
}

/// What only a module card carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleStory {
    pub parent: Option<String>,
    /// Lines in the files directly here; [`Story::lines`] counts sub-modules too.
    pub own_lines: Option<i64>,
    pub files: Vec<FileRow>,
    pub submodules: Vec<ModuleRow>,
    pub types: usize,
    pub key_types: Vec<Peer>,
    /// Modules this one depends on through at least one undisputed relation —
    /// the same cut as [`Story::uses`], one grain up.
    pub depends_on: Vec<Peer>,
    pub depends_on_total: usize,
    /// Modules reached only through ambiguous name matches.
    pub depends_on_ambiguous: usize,
    pub used_by: Vec<Peer>,
    pub used_by_total: usize,
    /// Modules reaching this one only through ambiguous name matches.
    pub used_by_ambiguous: usize,
}

/// What only a file card carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileStory {
    pub declares: Vec<Peer>,
    pub is_test: bool,
}

/// Everything a card explains, beside the evidence it lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Story {
    pub summary: Option<String>,
    /// The module this entity lives in (a module card: its parent).
    pub module: Option<String>,
    pub lines: Option<i64>,
    /// The units this one reaches on at least one undisputed relation
    /// ([`Counts::undisputed`]) — a unit reached only through ambiguous name
    /// matches is no peer: naming it would send the reader to a guess (plan A,
    /// R7). Its relations stay in `## Maybe also`; [`Story::uses_ambiguous`]
    /// counts it.
    pub uses: Vec<Peer>,
    pub uses_total: usize,
    /// Units this one reaches only through ambiguous name matches.
    pub uses_ambiguous: usize,
    /// The units reaching this one on at least one undisputed relation, tests
    /// apart — the same cut as [`Story::uses`].
    pub users: Vec<Peer>,
    pub users_total: usize,
    /// Users outside tests with at least one relation that is not an ambiguous
    /// name match — [`Counts::undisputed`], so a name-only user counts here
    /// (plan A, KTD2), not only one an index or a rule backs.
    pub users_certain: usize,
    /// Users outside tests reached only through ambiguous name matches.
    pub users_ambiguous: usize,
    /// Distinct modules among the certain users.
    pub user_modules: usize,
    pub bases: Vec<(Peer, &'static str)>,
    pub derived: Vec<Peer>,
    /// Every type extending or implementing this one, not just the kept ones.
    pub derived_total: usize,
    pub tests: Vec<Peer>,
    pub externals: Vec<ExternalUse>,
    pub externals_total: usize,
    pub api: Vec<ApiRow>,
    pub internals: Vec<ApiRow>,
    pub module_story: Option<ModuleStory>,
    pub file_story: Option<FileStory>,
    /// The words a search should find a type or a file by, most typical first
    /// (plan B, U6) — see [`super::keywords`].
    pub keywords: Vec<Keyword>,
    /// What a type is for, sentence by sentence, each with its origin (plan
    /// B, U7) — see [`super::purpose`]. Empty on every other card.
    pub purpose: Vec<purpose::Sentence>,
    /// What a model said the type is for (plan B, U13: R7, KTD12): set only
    /// when generation was asked to read the purpose layer and its record
    /// still matches the type's facts. A field apart from `purpose` on
    /// purpose: its one projection is a labelled line under `## Purpose`,
    /// while the map row and the manifest header read `purpose` alone, so the
    /// model's words can reach neither.
    pub model_purpose: Option<String>,
}

/// Everything [`build`] needs besides the card's own evidence.
pub struct Context<'a> {
    pub insight: &'a Insight,
    pub nodes: &'a BTreeMap<&'a str, &'a Node>,
    /// Member identity → the units its own outgoing counted relations reach.
    pub member_reach: &'a HashMap<&'a str, BTreeSet<&'a str>>,
    pub file_types: &'a BTreeMap<String, BTreeSet<String>>,
}

pub fn build(
    kind: CardKind,
    key: &str,
    members: &[MemberRow],
    outgoing: &[Relation],
    header: Option<&TypeHeader>,
    cx: &Context,
) -> Story {
    let insight = cx.insight;
    let mut story = Story {
        summary: insight.summary(key).map(str::to_string),
        ..Story::default()
    };
    match kind {
        CardKind::Type => {
            story.module = insight.module_of(key).map(str::to_string);
            story.lines = insight.type_lines(key).map(i64::from);
            fill_relations(&mut story, key, outgoing, cx);
            split_members(&mut story, key, members, cx);
            story.keywords = insight.keywords(key);
            story.purpose = purpose::build(
                &purpose::Input {
                    key,
                    title: insight.label(key),
                    kind: insight.kind(key),
                    header,
                    summary: story.summary.as_deref(),
                    members,
                    bases: &story.bases,
                },
                insight,
            );
        }
        CardKind::File => {
            story.module = insight.module_of(key).map(str::to_string);
            story.lines = insight.file_loc(key);
            fill_relations(&mut story, key, outgoing, cx);
            split_members(&mut story, key, members, cx);
            story.keywords = insight.keywords(key);
            let declares = cx
                .file_types
                .get(key)
                .into_iter()
                .flatten()
                .map(|t| peer(insight, t, Counts::default()))
                .collect();
            story.file_story = Some(FileStory {
                declares,
                is_test: insight.is_test_file(key),
            });
        }
        CardKind::Module => {
            story.lines = insight.module_loc(key);
            story.module_story = Some(module_story(key, cx));
            story.module = story.module_story.as_ref().and_then(|m| m.parent.clone());
            story.externals = insight.module_externals_of(key);
            story.externals_total = story.externals.len();
            story.externals.truncate(PEER_CAP);
        }
        CardKind::Declared(_) => {}
    }
    story
}

fn peer(insight: &Insight, identity: &str, counts: Counts) -> Peer {
    Peer {
        identity: identity.to_string(),
        label: insight.label(identity).to_string(),
        summary: insight.summary(identity).map(str::to_string),
        kind: insight.kind(identity).map(str::to_string),
        module: insight.module_of(identity).map(str::to_string),
        counts,
        via: Vec::new(),
    }
}

/// A ranked list cut in two: the peers with at least one undisputed relation,
/// in rank order, and how many were reached only through ambiguous name
/// matches. Every aggregate a card shows — role text, diagram, peer tables —
/// reads the first half only, so a guess never reads as a collaborator; the
/// count keeps the trace (plan A, R7: separated, not hidden).
fn undisputed_apart(list: Vec<(&str, Counts)>) -> (Vec<(&str, Counts)>, usize) {
    let total = list.len();
    let kept: Vec<(&str, Counts)> = list
        .into_iter()
        .filter(|(_, c)| c.undisputed() > 0)
        .collect();
    let guessed = total - kept.len();
    (kept, guessed)
}

fn module_peer(insight: &Insight, identity: &str, counts: Counts) -> Peer {
    Peer {
        identity: identity.to_string(),
        label: identity.to_string(),
        summary: insight.summary(identity).map(str::to_string),
        kind: None,
        module: None,
        counts,
        via: Vec::new(),
    }
}

fn fill_relations(story: &mut Story, key: &str, outgoing: &[Relation], cx: &Context) {
    let insight = cx.insight;

    // Which of this entity's members reach each peer — read from the lifted
    // relations the evidence already carries, so the two can never disagree.
    let mut via: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for relation in outgoing {
        if let (Some(member), Some(unit)) =
            (relation.via.as_deref(), insight.unit_of(&relation.identity))
        {
            via.entry(unit).or_default().insert(member);
        }
    }

    let (uses, guessed_uses) = undisputed_apart(insight.uses(key));
    story.uses_total = uses.len();
    story.uses_ambiguous = guessed_uses;
    story.uses = uses
        .iter()
        .take(PEER_CAP)
        .map(|(unit, counts)| {
            let mut p = peer(insight, unit, *counts);
            p.via = via
                .get(unit)
                .into_iter()
                .flatten()
                .map(|m| m.to_string())
                .collect();
            p
        })
        .collect();

    let all_users = insight.users(key);
    story.users_ambiguous = all_users
        .iter()
        .filter(|(u, c)| c.undisputed() == 0 && !insight.is_test_unit(u))
        .count();
    let (users, _) = undisputed_apart(all_users);
    story.users_total = users.len();
    let production: Vec<&(&str, Counts)> = users
        .iter()
        .filter(|(u, _)| !insight.is_test_unit(u))
        .collect();
    story.users_certain = production.len();
    story.user_modules = production
        .iter()
        .filter_map(|(u, _)| insight.module_of(u))
        .collect::<BTreeSet<_>>()
        .len();
    story.tests = users
        .iter()
        .filter(|(u, _)| insight.is_test_unit(u))
        .take(PEER_CAP)
        .map(|(u, c)| peer(insight, u, *c))
        .collect();
    story.users = users
        .iter()
        .filter(|(u, _)| !insight.is_test_unit(u))
        .take(PEER_CAP)
        .map(|(u, c)| peer(insight, u, *c))
        .collect();

    story.bases = insight
        .bases_of(key)
        .into_iter()
        .map(|(base, word)| (peer(insight, base, Counts::default()), word))
        .collect();
    story.derived_total = insight.derived_of(key).len();
    story.derived = insight
        .derived_of(key)
        .into_iter()
        .take(PEER_CAP)
        .map(|d| peer(insight, d, Counts::default()))
        .collect();

    story.externals = insight.externals_of(key);
    story.externals_total = story.externals.len();
    story.externals.truncate(PEER_CAP);
}

/// Split members into the offered surface and the internals, with each row's
/// own summary and the units it reaches.
fn split_members(story: &mut Story, owner: &str, members: &[MemberRow], cx: &Context) {
    let insight = cx.insight;
    let file = insight.file_of(owner).unwrap_or(owner);
    for member in members {
        let node = cx.nodes.get(member.identity.as_str()).copied();
        let summary = node
            .and_then(|n| fact_text(n, FactKind::DocComment))
            .and_then(super::insight::doc_summary);
        let reaches = cx
            .member_reach
            .get(member.identity.as_str())
            .into_iter()
            .flatten()
            .filter(|unit| **unit != owner)
            .map(|unit| (unit.to_string(), insight.label(unit).to_string()))
            .collect();
        let row = ApiRow {
            identity: member.identity.clone(),
            name: member.name.clone(),
            kind: member.member_kind.clone(),
            visibility: member.visibility.clone(),
            signature: member.signature.clone(),
            summary,
            anchor: member.source.as_ref().map(super::SourceRef::anchor),
            reaches,
        };
        if offered(&row, file) {
            story.api.push(row);
        } else {
            story.internals.push(row);
        }
    }
    let order = |r: &ApiRow| {
        (
            kind_rank(r.kind.as_deref()),
            r.name.clone(),
            r.identity.clone(),
        )
    };
    story.api.sort_by_cached_key(order);
    story.internals.sort_by_cached_key(order);
}

/// Whether a member is part of what its owner offers to other code.
///
/// Read from the stated visibility where the source states one, and from the
/// language's own default where it does not — a C# or Rust member without a
/// modifier is private, a TypeScript class member is public, Python marks
/// privacy with a leading underscore and Go with case.
pub fn offered(row: &ApiRow, file: &str) -> bool {
    if let Some(v) = row.visibility.as_deref() {
        let v = v.to_ascii_lowercase();
        if v.contains("private") || v.contains("internal") || v.starts_with("pub(") {
            return false;
        }
        return true;
    }
    let ext = file.rsplit('.').next().unwrap_or("");
    match ext {
        "py" => {
            !row.name.starts_with('_') || row.name.starts_with("__") && row.name.ends_with("__")
        }
        "go" => row.name.chars().next().is_some_and(char::is_uppercase),
        "rs" | "cs" | "java" => false,
        _ => true,
    }
}

/// Section order inside an API list.
pub fn kind_rank(kind: Option<&str>) -> u8 {
    match kind {
        Some("constructor") => 0,
        Some("method") => 1,
        // A stored procedure (SQL) is called like a function and reads as one.
        Some("function") | Some("procedure") => 2,
        Some("property") => 3,
        Some("event") => 4,
        Some("field") => 5,
        Some("variant") => 6,
        Some(k)
            if crate::extract::treesitter::symbol_role(k)
                == Some(crate::extract::treesitter::Role::Type) =>
        {
            7
        }
        _ => 8,
    }
}

/// The heading an API group renders under.
pub fn kind_group(rank: u8) -> &'static str {
    match rank {
        0 => "Constructors",
        1 => "Methods",
        2 => "Functions",
        3 => "Properties",
        4 => "Events",
        5 => "Fields",
        6 => "Variants",
        7 => "Nested types",
        _ => "Other members",
    }
}

fn module_story(key: &str, cx: &Context) -> ModuleStory {
    let insight = cx.insight;
    let parent = (key != ".").then(|| crate::extract::parent_key(key));

    let files: Vec<FileRow> = insight
        .module_files
        .get(key)
        .into_iter()
        .flatten()
        .map(|file| FileRow {
            identity: file.clone(),
            lines: insight.file_loc(file),
            declares: cx
                .file_types
                .get(file)
                .into_iter()
                .flatten()
                .map(|t| (t.clone(), insight.label(t).to_string()))
                .collect(),
            is_test: insight.is_test_file(file),
        })
        .collect();

    let submodules = insight
        .module_children
        .get(key)
        .into_iter()
        .flatten()
        .map(|m| ModuleRow {
            identity: m.clone(),
            lines: insight.module_loc(m),
            summary: insight.summary(m).map(str::to_string),
        })
        .collect();

    let mut types: Vec<(&str, usize)> = insight
        .module_units
        .get(key)
        .into_iter()
        .flatten()
        .filter(|u| insight.kind(u).is_some())
        .map(|u| (u.as_str(), undisputed_apart(insight.users(u)).0.len()))
        .collect();
    let type_count = types.len();
    types.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let key_types = types
        .iter()
        .take(15)
        .map(|(u, n)| {
            let mut p = peer(insight, u, Counts::default());
            // Reuse the counts slot for the ranking figure: how many units use it.
            p.counts.references = *n as u32;
            p
        })
        .collect();

    let (uses, depends_on_ambiguous) = undisputed_apart(insight.module_uses(key));
    let (users, used_by_ambiguous) = undisputed_apart(insight.module_users(key));
    ModuleStory {
        parent,
        own_lines: insight.module_own_loc(key),
        files,
        submodules,
        types: type_count,
        key_types,
        depends_on_total: uses.len(),
        depends_on_ambiguous,
        depends_on: uses
            .iter()
            .take(PEER_CAP)
            .map(|(m, c)| module_peer(insight, m, *c))
            .collect(),
        used_by_total: users.len(),
        used_by_ambiguous,
        used_by: users
            .iter()
            .take(PEER_CAP)
            .map(|(m, c)| module_peer(insight, m, *c))
            .collect(),
    }
}

fn fact_text(node: &Node, kind: FactKind) -> Option<&str> {
    node.facts.iter().find_map(|f| match (&f.kind, &f.value) {
        (k, FactValue::Text(v)) if *k == kind => Some(v.as_str()),
        _ => None,
    })
}
