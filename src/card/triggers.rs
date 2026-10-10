//! The registry of triggers, `cards/triggers.md` (plan 2b, U7: R11–R15, R17,
//! R18; KTD10, KTD11), and the line of coverage it shares with the view
//! `entry-points` (U8: KTD12).
//!
//! - **One row per occurrence** of a trigger and functionality it leads to;
//!   nothing is merged. A row is a bullet
//!   `- <kind> · <key> · <state of the path> → <functionality> — `<anchor>``,
//!   so its sentence binds to the anchor on the same line (KTD10). The rows
//!   are sorted by kind, key, then the file and place the trigger is wired at,
//!   its member and the functionality — in code points, never by a hash map.
//! - **A large registry is cut into `## Part N`** by the same greedy packing
//!   as a map ([`fixed_partition`]) on the map's threshold of rows
//!   ([`MAP_PART_ROWS`]); otherwise it has one section, `## Triggers`. Each
//!   `##` is a chunk of its one document in the manifest (`triggers:index`,
//!   see [`super::documents::triggers_document`]).
//! - **The line of coverage** stands above the rows: which kinds of trigger
//!   are covered in which language, composed by the core from what the
//!   components that read bodies declare they report (`emits`), never by a
//!   plugin (KTD11, [`coverage`]).
//!
//! A row links its functionality's card, named by the functionality's key: a
//! card has a `### Context` under every functionality, so a fragment would not
//! say which one, and the slug of a heading is the Markdown renderer's, not
//! the corpus's. The text of a key comes from the code — a menu item's label,
//! a resource key — and goes through the secret filter once more; a row that
//! holds its marker says so before its anchor (R18).
//!
//! The rows are data of their own ([`RegistryRow`]), not the analysis's: the
//! triggers of XAML (step 2c, U9: R9, KTD15) are rows of the same list — a
//! control with `Command=`, a menu item, a `KeyBinding` — each at its
//! element, with the template, style or view it is written in as its member
//! and the functionality an entry point of which its command is; and the
//! registry sorts and renders whatever it is given. A row of XAML is always
//! active: whether the way it opens is dead is told by the composition of a
//! functionality's flows (2b), which reads the rows of C# alone.

use super::explain::{code, cs_n, plural, series, NAMED_IN_SENTENCE};
use super::layer2::dynamic::{CONTROL_KEY, KEY_GESTURE};
use super::layer2::{
    Analysis, FunctionalityRef, MarkupTriggerKind, PathState, Place, TriggerKind, TriggerRow,
};
use super::layer2_render::{anchor, left_out, path_state_phrase, short};
use super::links::{link, mention, CardIndex};
use super::map::MAP_PART_ROWS;
use super::render::{Lang, CARD_SCHEMA_VERSION_LAYER2};
use crate::cli::LinkStyle;
use crate::extract::external::{redact_secret_block, SecretPrefixes};
use crate::graph::NodeKey;
use crate::render::split::{fixed_partition, ChunkWeight};
use crate::render::RenderedFile;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// The corpus-relative path of the registry (KTD10): beside the trees of the
/// card corpus, not under `cards/map/`, where a module named `triggers` would
/// meet it.
pub const REGISTRY_PATH: &str = "cards/triggers.md";

/// The registry's document id in `manifest-documents` (R14).
pub const TRIGGERS_ID: &str = "triggers:index";

/// The registry's document kind.
pub const TRIGGERS_KIND: &str = "triggers";

// ---- The kinds of the registry ----

/// A kind of trigger as the registry lists it, in its order: the kinds of
/// the analysis of C# ([`TriggerKind`]) and the control of XAML (plan 2c,
/// R9), which only markup writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegistryKind {
    Event,
    Key,
    MenuItem,
    /// An element with `Command=` whose type is no menu item (KTD15).
    Control,
    External,
}

impl From<TriggerKind> for RegistryKind {
    fn from(kind: TriggerKind) -> RegistryKind {
        match kind {
            TriggerKind::Event => RegistryKind::Event,
            TriggerKind::Key => RegistryKind::Key,
            TriggerKind::MenuItem => RegistryKind::MenuItem,
            TriggerKind::External => RegistryKind::External,
        }
    }
}

impl From<MarkupTriggerKind> for RegistryKind {
    fn from(kind: MarkupTriggerKind) -> RegistryKind {
        match kind {
            MarkupTriggerKind::Control => RegistryKind::Control,
            MarkupTriggerKind::MenuItem => RegistryKind::MenuItem,
            MarkupTriggerKind::Key => RegistryKind::Key,
        }
    }
}

// ---- The line of coverage (KTD11) ----

/// A component that read bodies in the run, as the line of coverage sees it:
/// the languages it claims, what it declares it reports (`describe.emits`),
/// and whether it read any file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyComponent {
    pub languages: Vec<String>,
    pub emits: Vec<String>,
    /// It read at least one file in the run.
    pub read: bool,
}

/// The kinds of trigger covered in one language; none — not covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCoverage {
    /// The language's name as a plugin claims it (`csharp`, `xaml`).
    pub language: String,
    /// In the registry's order; empty when nothing covers the language.
    pub kinds: Vec<RegistryKind>,
}

/// Which kinds of trigger are covered in which language (R15), by language.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerCoverage {
    pub languages: Vec<LanguageCoverage>,
}

/// The languages the line always names, covered or not: the two the
/// triggers of a desktop UI are written in. Without a component for XAML the
/// line admits it — `XAML: not covered` — rather than leave it out (AE6).
const NAMED_LANGUAGES: &[&str] = &["csharp", "xaml"];

/// The languages the core turns into rows of the registry, each with the
/// kinds a row of it can have. The analysis of bodies reads C# (2b, U5),
/// walking back over its callers, so an entry from another component is
/// found there whenever its bodies were read. XAML gives the rows of its
/// controls, menu items and key bindings (2c, U9); a handler a XAML file
/// names is a marker of the member, never a row, so XAML covers no event,
/// whatever its component emits. A component of any other language covers
/// nothing — the registry would otherwise say a kind is covered where no row
/// of it can exist.
const ROW_LANGUAGES: &[(&str, &[RegistryKind])] = &[
    (
        "csharp",
        &[
            RegistryKind::Event,
            RegistryKind::Key,
            RegistryKind::MenuItem,
            RegistryKind::External,
        ],
    ),
    (
        "xaml",
        &[
            RegistryKind::Key,
            RegistryKind::MenuItem,
            RegistryKind::Control,
        ],
    ),
];

/// The kinds of trigger a record or a tag a component emits stands for — the
/// core's table, not a plugin's (KTD11). `control-key` stands for a control
/// and a menu item alike: which one an element with a command is, the core
/// tells by its type (2c, KTD15).
fn kind_of(emitted: &str) -> &'static [RegistryKind] {
    match emitted {
        "subscription" => &[RegistryKind::Event],
        "key-compare" | KEY_GESTURE => &[RegistryKind::Key],
        "menu-item" => &[RegistryKind::MenuItem],
        CONTROL_KEY => &[RegistryKind::Control, RegistryKind::MenuItem],
        _ => &[],
    }
}

/// The line of coverage of a run whose components read bodies as
/// `components` say (KTD11). A language the core turns into rows
/// ([`ROW_LANGUAGES`]) is covered by the kinds its rows can have that the
/// `emits` of its components that read a file map to — and C# by the entry
/// from another component, which the core finds whenever it read the
/// bodies. A component that read nothing, or that declares no `emits`, covers
/// nothing; a language with no component that covers it is not covered.
/// Every name of [`NAMED_LANGUAGES`] is listed, and every language a
/// component claims; by name.
pub fn coverage(components: &[BodyComponent]) -> TriggerCoverage {
    let mut by: BTreeMap<String, BTreeSet<RegistryKind>> = NAMED_LANGUAGES
        .iter()
        .map(|l| (l.to_string(), BTreeSet::new()))
        .collect();
    for c in components {
        for language in &c.languages {
            let kinds = by.entry(language.clone()).or_default();
            let Some((_, rows)) = ROW_LANGUAGES.iter().find(|(l, _)| l == language) else {
                continue;
            };
            if !c.read || c.emits.is_empty() {
                continue;
            }
            kinds.extend(
                c.emits
                    .iter()
                    .flat_map(|e| kind_of(e))
                    .filter(|k| rows.contains(k)),
            );
            if rows.contains(&RegistryKind::External) {
                kinds.insert(RegistryKind::External);
            }
        }
    }
    TriggerCoverage {
        languages: by
            .into_iter()
            .map(|(language, kinds)| LanguageCoverage {
                language,
                kinds: kinds.into_iter().collect(),
            })
            .collect(),
    }
}

/// A language's name for a reader: `C#`, `XAML`, else the name it is
/// claimed by.
fn language_name(language: &str) -> &str {
    match language {
        "csharp" => "C#",
        "xaml" => "XAML",
        other => other,
    }
}

/// A kind of trigger, lower-case, in `lang`.
fn kind_word(kind: RegistryKind, lang: Lang) -> &'static str {
    match (kind, lang) {
        (RegistryKind::Event, Lang::En) => "event",
        (RegistryKind::Event, Lang::Cs) => "událost",
        (RegistryKind::Key, Lang::En) => "key",
        (RegistryKind::Key, Lang::Cs) => "klávesa",
        (RegistryKind::MenuItem, Lang::En) => "menu item",
        (RegistryKind::MenuItem, Lang::Cs) => "položka menu",
        (RegistryKind::Control, Lang::En) => "control",
        (RegistryKind::Control, Lang::Cs) => "ovládací prvek",
        (RegistryKind::External, Lang::En) => "entry from another component",
        (RegistryKind::External, Lang::Cs) => "vstup z jiné komponenty",
    }
}

impl TriggerCoverage {
    /// `C#: event, key, menu item, entry from another component; XAML: not
    /// covered` in `lang`.
    pub fn summary(&self, lang: Lang) -> String {
        self.languages
            .iter()
            .map(|l| {
                let kinds = if l.kinds.is_empty() {
                    match lang {
                        Lang::En => "not covered".to_string(),
                        Lang::Cs => "nepokryto".to_string(),
                    }
                } else {
                    l.kinds
                        .iter()
                        .map(|k| kind_word(*k, lang))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                format!("{}: {kinds}", language_name(&l.language))
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The line the view `entry-points` carries before its caveat E9 (U8:
    /// KTD12), in English like every view, linking the registry from the
    /// view at `from` (the corpus root).
    pub fn entry_points_line(&self, from: &str, links: LinkStyle) -> String {
        format!(
            "Coverage of triggers by the second layer, by language — {}. The {} lists every \
             trigger it found.",
            self.summary(Lang::En),
            link(from, REGISTRY_PATH, "registry of triggers", links)
        )
    }
}

// ---- The rows (R11, R12) ----

/// One row of the registry: a trigger and a functionality it leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRow {
    pub kind: RegistryKind,
    /// A key with its modifiers, a menu item's label as written, an event's
    /// name, an external entry's member key, a control's identity, a key
    /// binding's gesture — as the code has it, before the secret filter.
    pub key: String,
    /// The member the trigger is written in; an external entry's root; the
    /// template, style or view of markup (or its file).
    pub member: NodeKey,
    /// Where it is wired; `None` for a root whose body has no place.
    pub place: Option<Place>,
    pub state: PathState,
    pub target: FunctionalityRef,
}

impl From<&TriggerRow> for RegistryRow {
    fn from(r: &TriggerRow) -> RegistryRow {
        RegistryRow {
            kind: r.trigger.kind.into(),
            key: r.trigger.key.clone(),
            member: r.trigger.member.clone(),
            place: r.trigger.place.clone(),
            state: r.state.clone(),
            target: r.target.clone(),
        }
    }
}

/// The rows of the registry and its line of coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    /// Sorted (see the module docs).
    pub rows: Vec<RegistryRow>,
    pub coverage: TriggerCoverage,
}

impl Registry {
    /// A registry of `rows`, from any source, sorted.
    pub fn new(rows: impl IntoIterator<Item = RegistryRow>, coverage: TriggerCoverage) -> Registry {
        let mut rows: Vec<RegistryRow> = rows.into_iter().collect();
        rows.sort_by(|a, b| {
            (a.kind, &a.key, a.place.as_ref(), &a.member, &a.target).cmp(&(
                b.kind,
                &b.key,
                b.place.as_ref(),
                &b.member,
                &b.target,
            ))
        });
        Registry { rows, coverage }
    }

    /// The registry of the triggers the analysis of C# bodies found (U5),
    /// and of the triggers of markup (step 2c, U9).
    pub fn of(analysis: &Analysis, coverage: TriggerCoverage) -> Registry {
        Registry::new(
            analysis
                .triggers
                .iter()
                .map(RegistryRow::from)
                .chain(markup_rows(analysis)),
            coverage,
        )
    }
}

/// The rows of the triggers of markup (U9: R9, KTD15): one per trigger and
/// functionality its command is an entry point of — the member the walk
/// told at the element (U6), a command property 2a bound — each at the
/// element's lines. A trigger whose command no functionality enters at has
/// no row, as a trigger of C# that leads to none.
fn markup_rows(analysis: &Analysis) -> Vec<RegistryRow> {
    let triggers = analysis.dynamic.triggers();
    if triggers.is_empty() {
        return Vec::new();
    }
    // The functionalities each member is an entry point of, built once.
    let mut entered: BTreeMap<&NodeKey, BTreeSet<FunctionalityRef>> = BTreeMap::new();
    for ty in &analysis.types {
        for f in &ty.functionalities {
            for e in f.entry_points.iter().filter(|e| e.discriminator.is_none()) {
                entered
                    .entry(&e.member)
                    .or_default()
                    .insert(FunctionalityRef {
                        ty: ty.key.clone(),
                        key: f.key.clone(),
                    });
            }
        }
    }
    let mut out = Vec::new();
    for t in triggers {
        let targets: BTreeSet<&FunctionalityRef> = t
            .targets
            .iter()
            .filter_map(|m| entered.get(m))
            .flatten()
            .collect();
        for target in targets {
            out.push(RegistryRow {
                kind: t.kind.into(),
                key: t.key.clone(),
                member: t.member.clone(),
                place: Some(t.place.clone()),
                state: PathState::Active,
                target: target.clone(),
            });
        }
    }
    out
}

/// A part's weight in rows.
struct Rows(usize);

impl<T> ChunkWeight<T> for Rows {
    fn peek(&self, _: &T) -> usize {
        self.0 + 1
    }
    fn add(&mut self, _: &T) {
        self.0 += 1;
    }
    fn reset(&mut self) {
        self.0 = 0;
    }
}

/// A text of the code through the secret filter, its whitespace collapsed.
fn scrubbed(text: &str) -> String {
    let (text, _) = redact_secret_block(text, SecretPrefixes::builtin());
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The registry as the file `cards/triggers.md`; `index` and `links` write
/// the links to the functionalities' cards.
pub fn render_registry(
    registry: &Registry,
    index: &CardIndex,
    lang: Lang,
    links: LinkStyle,
) -> RenderedFile {
    let cs = lang == Lang::Cs;
    let n = registry.rows.len();
    let mut s = format!(
        "---\nschema: {CARD_SCHEMA_VERSION_LAYER2}\ntool: reverse-architect\nkind: {TRIGGERS_KIND}\n---\n\n# Registry of triggers\n\n"
    );
    let count = if cs {
        cs_n(n, ("řádek", "řádky", "řádků"))
    } else {
        plural(n, "row", "rows")
    };
    let _ = writeln!(
        s,
        "**{}** · {count}\n",
        if cs {
            "rejstřík spouštěčů"
        } else {
            "registry of triggers"
        }
    );
    let _ = writeln!(
        s,
        "**{}** {}.\n",
        if cs { "Pokrytí:" } else { "Coverage:" },
        registry.coverage.summary(lang)
    );
    s.push_str(if cs {
        "Jeden řádek na každý výskyt spouštěče a každou funkčnost, na kterou vede; nic se \
         neslučuje. Řádek uvádí druh, klíč, stav cesty (aktivní, mrtvá s místem, které ji vypíná, \
         nebo nerozřešená), funkčnost s odkazem na kartu jejího typu, kde ji popisuje `### Context`, \
         a místo, kde je spouštěč zapojený. Jazyk nebo druh, který řádek pokrytí neuvádí jako \
         pokrytý, může mít spouštěče, které tu nejsou.\n"
    } else {
        "One row per occurrence of a trigger and each functionality it leads to; nothing is \
         merged. A row gives the kind, the key, the state of the path (active, dead with the place \
         that turns it off, or unresolved), the functionality with a link to its type's card, where \
         its `### Context` describes it, and where the trigger is wired. A language or a kind the \
         line of coverage does not name as covered can have triggers this registry does not \
         list.\n"
    });

    let rows: Vec<String> = registry
        .rows
        .iter()
        .map(|r| row(r, index, lang, links))
        .collect();
    if rows.len() <= MAP_PART_ROWS {
        s.push_str("\n## Triggers\n\n");
        if rows.is_empty() {
            s.push_str(if cs {
                "V tomto běhu nevede na žádnou funkčnost druhé vrstvy žádný spouštěč.\n"
            } else {
                "No trigger leads to a functionality of the second layer in this run.\n"
            });
        }
        rows.iter().for_each(|r| s.push_str(r));
    } else {
        let parts = fixed_partition(rows.iter(), MAP_PART_ROWS, &mut Rows(0));
        for (i, part) in parts.iter().enumerate() {
            let _ = write!(s, "\n## Part {}\n\n", i + 1);
            part.iter().for_each(|r| s.push_str(r));
        }
    }
    RenderedFile {
        path: REGISTRY_PATH.to_string(),
        contents: s,
    }
}

/// One row, its line break included (KTD10).
fn row(r: &RegistryRow, index: &CardIndex, lang: Lang, links: LinkStyle) -> String {
    let cs = lang == Lang::Cs;
    let member = code(short(r.member.as_str()));
    let key = match r.kind {
        RegistryKind::External => member,
        _ => format!(
            "{} {} {member}",
            code(&scrubbed(short(&r.key))),
            if cs { "v" } else { "in" }
        ),
    };
    let mut places: Vec<&Place> = r.place.iter().collect();
    let phrase = path_state_phrase(lang, &r.state).unwrap_or_default();
    let state = match &r.state {
        PathState::Active => (if cs { "aktivní" } else { "active" }).to_string(),
        PathState::Dead { member, place } => {
            places.push(place);
            format!("{phrase} {}", code(short(member.as_str())))
        }
        PathState::Unresolved { callers } => {
            let names: Vec<String> = callers
                .iter()
                .take(NAMED_IN_SENTENCE)
                .map(|k| code(short(k.as_str())))
                .collect();
            format!("{phrase} {}", series(lang, &names, callers.len()))
        }
    };
    let target = mention(
        REGISTRY_PATH,
        r.target.ty.as_str(),
        short(&r.target.key),
        links,
        index,
    );
    let text = format!("{} · {key} · {state} → {target}", kind_word(r.kind, lang));
    let note = left_out(lang, &text);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let anchors: Vec<String> = places
        .into_iter()
        .map(anchor)
        .filter(|a| seen.insert(a.clone()))
        .map(|a| format!("`{a}`"))
        .collect();
    let anchors = if anchors.is_empty() {
        String::new()
    } else {
        format!(" — {}", anchors.join(", "))
    };
    format!("- {text}{note}{anchors}\n")
}
