//! Rendering of a card's [`Story`](super::story::Story): the sections that
//! explain, placed above the evidence [`super::render`] has always rendered.
//!
//! Headings stay English constants (R8), and so does the fact vocabulary inside
//! a sentence (`calls 5 · references 2`). The sentences themselves are prose,
//! so `--lang` switches them; they are templates over counts and names the story
//! already holds — nothing is phrased that the aggregation did not measure, and
//! a sentence about an absence says what was SEEN ("no other type was seen
//! using it"), because the relations are a heuristic reading and cannot prove
//! that nothing uses a type.

use super::links::{mention, CardIndex};
use super::render::Lang;
use super::story::{kind_group, kind_rank, ApiRow, Peer, Story};
use super::{Card, CardKind};
use crate::cli::LinkStyle;
use std::fmt::Write;

/// How many peers a sentence names before it says "and N more".
pub(super) const NAMED_IN_SENTENCE: usize = 3;
/// How many peers each side of a card diagram draws.
const DIAGRAM_SIDE: usize = 8;

pub struct Out<'a> {
    pub card: &'a Card,
    /// The document the links are written from: the card's own path, or its
    /// detail's (plan B, U8) — a relative link is only right from where it
    /// is read.
    pub from: &'a str,
    pub links: LinkStyle,
    pub index: &'a CardIndex,
    pub lang: Lang,
}

impl Out<'_> {
    fn link(&self, identity: &str, label: &str) -> String {
        mention(self.from, identity, label, self.links, self.index)
    }

    fn peer_link(&self, p: &Peer) -> String {
        self.link(&p.identity, &p.label)
    }

    fn cs(&self) -> bool {
        self.lang == Lang::Cs
    }

    /// `A, B and C` — or `A, B, C and 4 more` — in the card's language.
    fn series(&self, items: &[String], total: usize) -> String {
        series(self.lang, items, total)
    }

    fn module_label<'m>(&self, module: &'m str) -> &'m str {
        if module == "." {
            if self.cs() {
                "(kořen repozitáře)"
            } else {
                "(repository root)"
            }
        } else {
            module
        }
    }
}

/// `A, B and C` — or `A, B, C and 4 more` — in `lang`: the one way every
/// sentence of a card lists names, the purpose's included.
pub(super) fn series(lang: Lang, items: &[String], total: usize) -> String {
    let (and, more) = match lang {
        Lang::Cs => ("a", "dalších"),
        Lang::En => ("and", "more"),
    };
    let shown = items.len();
    let rest = total.saturating_sub(shown);
    match (shown, rest) {
        (0, _) => String::new(),
        (1, 0) => items[0].clone(),
        (_, 0) => format!(
            "{} {and} {}",
            items[..shown - 1].join(", "),
            items[shown - 1]
        ),
        _ => format!("{} {and} {rest} {more}", items.join(", ")),
    }
}

/// `1,234` — grouped so a reader can compare sizes at a glance.
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

pub(super) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Czech number agreement: `1 typ`, `2 typy`, `5 typů`. `forms` is
/// (one, two-to-four, five-and-more) — pass the case the sentence needs.
pub(super) fn cs_n(n: usize, forms: (&str, &str, &str)) -> String {
    let word = match n {
        1 => forms.0,
        2..=4 => forms.1,
        _ => forms.2,
    };
    format!("{n} {word}")
}

/// A count in the card's language: English `(one, many)`, Czech three forms.
fn count(out: &Out, n: usize, en: (&str, &str), cs: (&str, &str, &str)) -> String {
    if out.cs() {
        cs_n(n, cs)
    } else {
        plural(n, en.0, en.1)
    }
}

/// Lines of source: `1,234 lines` / `1,234 řádků`.
fn lines_word(out: &Out, n: i64) -> String {
    if out.cs() {
        let word = match n {
            1 => "řádek",
            2..=4 => "řádky",
            _ => "řádků",
        };
        format!("{} {word}", grouped(n))
    } else if n == 1 {
        "1 line".to_string()
    } else {
        format!("{} lines", grouped(n))
    }
}

/// Escape a value for a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Inline code that survives a backtick inside the code.
pub(super) fn code(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.contains('`') {
        format!("`` {flat} ``")
    } else {
        format!("`{flat}`")
    }
}

/// The first letter upper-cased, the way a sentence starts.
pub(super) fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The one-line identity strip under the title: kind, home, size, source.
pub fn glance(out: &Out, story: &Story) -> String {
    let card = out.card;
    let mut parts: Vec<String> = Vec::new();
    match card.kind {
        // The header the signature states (plan B, U1); visibility and kind
        // only where the signature could not be read, because the stated
        // visibility drops `static`, `abstract` and `sealed` (AE3).
        CardKind::Type => parts.push(format!("**{}**", card.kind_phrase())),
        CardKind::File => parts.push(if out.cs() { "**soubor**" } else { "**file**" }.into()),
        CardKind::Module => parts.push(if out.cs() { "**modul**" } else { "**module**" }.into()),
        CardKind::Declared(_) => parts.push(format!("**{}**", card.kind_word)),
    }
    if let Some(module) = &story.module {
        let word = match (card.kind == CardKind::Module, out.cs()) {
            (true, false) => "part of",
            (true, true) => "součást",
            (false, false) => "in",
            (false, true) => "v",
        };
        // A type names its module by the last segment (card trim): the
        // source anchor at the end of the strip spells the path out, and the
        // link still leads to the module by its full one.
        let label = out.module_label(module);
        let label = if card.kind == CardKind::Type {
            label.rsplit('/').next().unwrap_or(label)
        } else {
            label
        };
        parts.push(format!("{word} {}", out.link(module, label)));
    }
    if let Some(ms) = &story.module_story {
        parts.push(count(
            out,
            ms.files.len(),
            ("file", "files"),
            ("soubor", "soubory", "souborů"),
        ));
        if !ms.submodules.is_empty() {
            parts.push(count(
                out,
                ms.submodules.len(),
                ("sub-module", "sub-modules"),
                ("podmodul", "podmoduly", "podmodulů"),
            ));
        }
        if ms.types > 0 {
            parts.push(count(
                out,
                ms.types,
                ("type", "types"),
                ("typ", "typy", "typů"),
            ));
        }
    }
    if let Some(lines) = story.lines {
        parts.push(lines_word(out, lines));
    }
    if card.kind != CardKind::Module {
        if let Some(source) = &card.source {
            parts.push(format!("`{}`", source.anchor()));
        }
    }
    parts.join(" · ")
}

/// A module key as a reader sees it: the root has no name of its own.
pub fn module_label(module: &str) -> &str {
    if module == "." {
        "(repository root)"
    } else {
        module
    }
}

/// `## Role` for a type or a file: who uses it, what it works with, what it
/// builds on, what exercises it.
///
/// On a type's short card (plan B, U9: R2) the sentences name the peers they
/// lead with and nothing uncertain: the two sentences a file card keeps about
/// units reached only through ambiguous name matches become one count of what
/// the detail lists beyond the named ([`further`]), how many of those are
/// such guesses included.
pub fn role(out: &Out, story: &Story) -> String {
    let card = out.card;
    let name = code(&card.title);
    let cs = out.cs();
    let short = card.kind == CardKind::Type;
    let mut lines: Vec<String> = Vec::new();

    if let Some(fs) = &story.file_story {
        if !fs.declares.is_empty() {
            let names: Vec<String> = fs
                .declares
                .iter()
                .take(6)
                .map(|p| out.peer_link(p))
                .collect();
            let n = count(
                out,
                fs.declares.len(),
                ("type", "types"),
                ("typ", "typy", "typů"),
            );
            let list = out.series(&names, fs.declares.len());
            lines.push(if cs {
                format!("{name} deklaruje {n}: {list}.")
            } else {
                format!("{name} declares {n}: {list}.")
            });
        }
        if fs.is_test {
            lines.push(
                if cs {
                    "Je to testovací soubor podle pojmenování a složek svého ekosystému."
                } else {
                    "It is a test file, by the naming and folder conventions of its ecosystem."
                }
                .into(),
            );
        }
    }

    if story.users_certain > 0 {
        let named: Vec<String> = story
            .users
            .iter()
            .filter(|p| p.counts.undisputed() > 0)
            .take(NAMED_IN_SENTENCE)
            .map(|p| out.peer_link(p))
            .collect();
        let list = out.series(&named, story.users_certain);
        // The count names no kind: a user is a unit — a type, or a file for
        // its free functions ([`super::insight::Insight::unit_of`]) — so
        // "types" would be wrong on a corpus with free functions, and "types
        // or files" spent bytes on every card to say so (card trim).
        let k = story.users_certain;
        lines.push(if cs {
            let verb = if (2..=4).contains(&k) {
                "Používají"
            } else {
                "Používá"
            };
            format!(
                "{verb} ho {} v {}, hlavně {list}.",
                cs_n(k, ("další", "další", "dalších")),
                cs_n(story.user_modules, ("modulu", "modulech", "modulech"))
            )
        } else {
            format!(
                "Used by {k} in {}, chiefly {list}.",
                plural(story.user_modules, "module", "modules")
            )
        });
    } else if matches!(card.kind, CardKind::Type) && story.users_ambiguous == 0 {
        lines.push(if cs {
            format!("V tomto repozitáři nebylo vidět, že by {name} používal jiný typ nebo soubor.")
        } else {
            format!("No other type or file in this repository was seen using {name}.")
        });
    }
    if story.users_ambiguous > 0 && !short {
        let k = story.users_ambiguous;
        lines.push(if cs {
            let (who, verb) = match k {
                1 => ("další", "používá"),
                2..=4 => ("další", "používají"),
                _ => ("dalších", "používá"),
            };
            format!(
                "Ještě {k} {who} ho možná {verb} také — přes shodu jména, kterou pravidla nerozhodla."
            )
        } else {
            format!(
                "{k} {} may use it too, through name matches the rules did not settle.",
                if k == 1 { "other" } else { "others" }
            )
        });
    }

    if !story.uses.is_empty() {
        let named: Vec<String> = story
            .uses
            .iter()
            .take(NAMED_IN_SENTENCE)
            .map(|p| format!("{} ({})", out.peer_link(p), p.counts.describe()))
            .collect();
        let list = out.series(&named, story.uses_total);
        // Units again, as for the users; and "of this repository" goes
        // without saying — what lies outside it is the sentence of its own
        // below ([`outside`]).
        lines.push(if cs {
            format!(
                "Pracuje s {}, hlavně {list}.",
                cs_n(story.uses_total, ("dalším", "dalšími", "dalšími"))
            )
        } else {
            format!(
                "It uses {}, chiefly {list}.",
                plural(story.uses_total, "other", "others")
            )
        });
    }
    // The outgoing mirror of "may use it too": the units reached only through
    // ambiguous name matches are no peers above, and this count — with the
    // `## Maybe also` lines — is all the card says of them.
    if story.uses_ambiguous > 0 && !short {
        let k = story.uses_ambiguous;
        lines.push(if cs {
            format!(
                "Možná pracuje také s {k} {} — přes shodu jména, kterou pravidla nerozhodla.",
                if k == 1 { "dalším" } else { "dalšími" }
            )
        } else {
            format!(
                "It may work with {k} {} too, through name matches the rules did not settle.",
                if k == 1 { "other" } else { "others" }
            )
        });
    }

    if !story.bases.is_empty() {
        let parts: Vec<String> = story
            .bases
            .iter()
            .map(|(p, word)| {
                let word = match (cs, *word) {
                    (true, "extends") => "rozšiřuje",
                    (true, "implements") => "implementuje",
                    (_, w) => w,
                };
                format!("{word} {}", out.peer_link(p))
            })
            .collect();
        let list = out.series(&parts, parts.len());
        lines.push(if cs {
            format!("{}.", capitalized(&list))
        } else {
            format!("It {list}.")
        });
    }
    if !story.derived.is_empty() {
        let named: Vec<String> = story
            .derived
            .iter()
            .take(NAMED_IN_SENTENCE + 2)
            .map(|p| out.peer_link(p))
            .collect();
        let list = out.series(&named, story.derived_total);
        lines.push(if cs {
            format!(
                "Staví na něm {}: {list}.",
                cs_n(story.derived_total, ("typ", "typy", "typů"))
            )
        } else {
            format!(
                "{} build on it: {list}.",
                plural(story.derived_total, "type", "types")
            )
        });
    }
    if !story.externals.is_empty() {
        lines.push(outside(out, story));
    }
    if !story.tests.is_empty() {
        let named: Vec<String> = story
            .tests
            .iter()
            .take(NAMED_IN_SENTENCE)
            .map(|p| out.peer_link(p))
            .collect();
        let list = out.series(&named, story.tests.len());
        lines.push(if cs {
            format!("Procvičují ho testy: {list}.")
        } else {
            format!("Tests that exercise it: {list}.")
        });
    }
    if short {
        if let Some(sentence) = further_sentence(out.lang, further(story)) {
            lines.push(sentence);
        }
    }

    if lines.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n## Role\n\n");
    for line in lines {
        let _ = writeln!(s, "{line}");
    }
    s
}

/// What a type's role sentences leave to its detail, counted in peers — the
/// unit the sentences name (plan B, U9: R2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Further {
    /// Neighbours the detail lists that no sentence names: those past the
    /// number a sentence names before it says "and N more", and every unit
    /// reached only through ambiguous name matches.
    pub more: usize,
    /// Of `more`, the units reached only through ambiguous name matches —
    /// the detail's `Maybe also` names them, the card never does.
    pub uncertain: usize,
}

/// [`Further`] of a story, by the thresholds [`role`] names peers by — so the
/// count on the card is exactly what the sentences above it leave out.
pub(super) fn further(story: &Story) -> Further {
    let past = |total: usize, named: usize| total.saturating_sub(named);
    let uncertain = story.users_ambiguous + story.uses_ambiguous;
    Further {
        more: past(story.users_certain, NAMED_IN_SENTENCE)
            + past(story.uses_total, NAMED_IN_SENTENCE)
            + past(story.derived_total, NAMED_IN_SENTENCE + 2)
            + past(story.externals_total, NAMED_IN_SENTENCE)
            + past(story.tests.len(), NAMED_IN_SENTENCE)
            + uncertain,
        uncertain,
    }
}

/// Whether the `## Role` sentences ([`role`]) name every peer the card has —
/// no "and N more" in any of them, and no guess counted apart (plan B, U8).
///
/// The detail's relation sections list member-level rows with their sites,
/// but a type whose every peer the role already names is not sent to a
/// detail for them alone: what the detail would add is the lines, and R3
/// asks for a detail only when it carries something the card does not say.
/// It is [`further`] at zero, so the card's count and the decision to write
/// a detail cannot disagree: a card that counts a neighbour more always has
/// the detail that lists it.
pub(super) fn role_names_every_peer(story: &Story) -> bool {
    further(story).more == 0
}

/// The short card's one sentence about what the detail lists beyond the
/// role's names (plan B Key Decision on "maybe also": the card carries the
/// count of further relations and how many of them are uncertain). `None`
/// when the role named everything.
///
/// The count is in neighbours — types, files and outside resources, the
/// grain the sentences above it name — not in the detail's rows, which are
/// one per member, kind and target and would count one neighbour many times.
///
/// "Uncertain" is one word for "reached only through name matches the rules
/// did not settle": the card trim cut the sentence to it. The README's card
/// format defines the word, and the detail's `Maybe also` names those
/// neighbours, each with why the rules left it open.
fn further_sentence(lang: Lang, further: Further) -> Option<String> {
    let Further { more, uncertain } = further;
    if more == 0 {
        return None;
    }
    Some(match lang {
        Lang::En => {
            let head = format!(
                "The detail lists {}",
                plural(more, "more neighbour", "more neighbours")
            );
            match (uncertain, more) {
                (0, _) => format!("{head}."),
                (1, 1) => format!("{head} (uncertain)."),
                (u, m) if u == m => format!("{head} (all uncertain)."),
                (u, _) => format!("{head} ({u} uncertain)."),
            }
        }
        Lang::Cs => {
            let head = format!(
                "Detail uvádí ještě {}",
                cs_n(
                    more,
                    ("dalšího souseda", "další sousedy", "dalších sousedů")
                )
            );
            match (uncertain, more) {
                (0, _) => format!("{head}."),
                (1, 1) => format!("{head} (nejistý)."),
                (u, m) if u == m => format!("{head} (všichni nejistí)."),
                (u, _) => format!(
                    "{head} (z toho {}).",
                    cs_n(u, ("nejistý", "nejistí", "nejistých"))
                ),
            }
        }
    })
}

/// `## Purpose` on a type's short card (plan B, U9: R5–R7): one line per
/// sentence of [`Story::purpose`], each followed by where it came from in the
/// card's language. Empty when the story has no purpose.
pub fn purpose(out: &Out, story: &Story) -> String {
    if story.purpose.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n## Purpose\n\n");
    for sentence in &story.purpose {
        let _ = writeln!(
            s,
            "{} ({})",
            sentence.text(out.lang),
            sentence.origin.phrase(out.lang)
        );
    }
    // The model's sentence (plan B, U13: R7), when the purpose layer was
    // read: its own line under the sentences the code gave, labelled as the
    // model's the way a "maybe also" line is marked model-verified, so a
    // reader never takes it for something read off the code.
    if let Some(sentence) = &story.model_purpose {
        let label = match out.lang {
            super::render::Lang::En => "written by a model, not read from the code",
            super::render::Lang::Cs => "napsal model, není přečteno z kódu",
        };
        let _ = writeln!(s, "{sentence} ({label})");
    }
    s
}

/// `## Keywords` (plan B, U9: R8): the words of [`Story::keywords`] on one
/// line, most typical first, each with its diacritics-free spelling beside
/// it where that differs — `obnově (obnove)` — so a search typed without a
/// Czech keyboard finds the card too. The words are data, the same in either
/// language. Empty when there are none.
pub fn keywords(story: &Story) -> String {
    if story.keywords.is_empty() {
        return String::new();
    }
    let words: Vec<String> = story
        .keywords
        .iter()
        .map(|k| match &k.folded {
            Some(folded) => format!("{} ({folded})", k.word),
            None => k.word.clone(),
        })
        .collect();
    format!("\n## Keywords\n\n{}\n", words.join(", "))
}

/// "Outside the repository it reaches …" — shared by type, file and module.
fn outside(out: &Out, story: &Story) -> String {
    let named: Vec<String> = story
        .externals
        .iter()
        .take(NAMED_IN_SENTENCE)
        .map(|e| code(&e.label))
        .collect();
    let list = out.series(&named, story.externals_total);
    if out.cs() {
        format!("Mimo repozitář sahá na {list}.")
    } else {
        format!("Outside the repository it reaches {list}.")
    }
}

/// The line the collaboration diagram draws for one peer, by the strongest rung
/// among the relations it aggregates (plan A, U2): solid when an index or a rule
/// backs one of them (or one carries no rung, as before rungs existed), dotted
/// when the best of them is a name match nothing backed. A peer reached only
/// through ambiguous name matches is never drawn — the story keeps it out of
/// every peer list (plan A, R7) — so there is no third style.
fn line_style(counts: &super::insight::Counts) -> &'static str {
    if counts.certain() > 0 {
        "-->"
    } else {
        "-.->"
    }
}

/// A small `flowchart LR`: users on the left, this entity in the middle, what
/// it reaches on the right. The line says how far each relation can be trusted
/// — see `line_style`.
pub fn diagram(
    lang: Lang,
    heading: &str,
    title: &str,
    users: &[Peer],
    uses: &[Peer],
) -> Option<String> {
    diagram_body(lang, title, users, uses, Rest::TablesBelow)
        .map(|body| format!("\n## {heading}\n\n{body}"))
}

/// Where the peers a diagram leaves out are listed instead.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Rest {
    /// The card's `## Works with` / `## Used by` tables, below the diagram.
    TablesBelow,
    /// The detail's relation sections, above it (plan B, U8).
    RelationsAbove,
}

/// The diagram [`diagram`] draws, without its heading — the detail places it
/// under a heading of its own level (plan B, U8).
pub(super) fn diagram_body(
    lang: Lang,
    title: &str,
    users: &[Peer],
    uses: &[Peer],
    rest: Rest,
) -> Option<String> {
    if users.is_empty() && uses.is_empty() {
        return None;
    }
    let esc = crate::render::mermaid::escape_label;
    let mut s = String::from("```mermaid\nflowchart LR\n");
    let _ = writeln!(s, "    self[\"{}\"]", esc(title));
    let _ = writeln!(s, "    style self stroke-width:3px");
    for (i, p) in users.iter().take(DIAGRAM_SIDE).enumerate() {
        let _ = writeln!(s, "    u{i}[\"{}\"]", esc(&p.label));
        let arrow = line_style(&p.counts);
        let _ = writeln!(s, "    u{i} {arrow}|{}| self", p.counts.dominant_word());
    }
    for (i, p) in uses.iter().take(DIAGRAM_SIDE).enumerate() {
        let _ = writeln!(s, "    d{i}[\"{}\"]", esc(&p.label));
        let arrow = line_style(&p.counts);
        let _ = writeln!(s, "    self {arrow}|{}| d{i}", p.counts.dominant_word());
    }
    s.push_str("```\n");
    let hidden_users = users.len().saturating_sub(DIAGRAM_SIDE);
    let hidden_uses = uses.len().saturating_sub(DIAGRAM_SIDE);
    if hidden_users + hidden_uses > 0 {
        let _ = match (lang, rest) {
            (Lang::Cs, Rest::TablesBelow) => writeln!(
                s,
                "\nDiagram kreslí {DIAGRAM_SIDE} nejsilnějších vztahů na každé straně; zbytek uvádějí tabulky níže."
            ),
            (Lang::Cs, Rest::RelationsAbove) => writeln!(
                s,
                "\nDiagram kreslí {DIAGRAM_SIDE} nejsilnějších vztahů na každé straně; všechny uvádějí vztahy výše."
            ),
            (Lang::En, Rest::TablesBelow) => writeln!(
                s,
                "\nThe diagram draws the {DIAGRAM_SIDE} strongest relations on each side; the tables below list the rest."
            ),
            (Lang::En, Rest::RelationsAbove) => writeln!(
                s,
                "\nThe diagram draws the {DIAGRAM_SIDE} strongest relations on each side; the relations above list them all."
            ),
        };
    }
    Some(s)
}

/// `## API` and `## Internals` for a type or a file's free declarations.
pub fn api(out: &Out, story: &Story, heading: &str) -> String {
    let mut s = String::new();
    if !story.api.is_empty() {
        let _ = write!(s, "\n## {heading}\n");
        let mut current: Option<u8> = None;
        for row in &story.api {
            let rank = kind_rank(row.kind.as_deref());
            if current != Some(rank) {
                let _ = write!(s, "\n### {}\n\n", kind_group(rank));
                current = Some(rank);
            }
            s.push_str(&api_row(out, row));
        }
    }
    if !story.internals.is_empty() {
        s.push_str("\n## Internals\n\n");
        let _ = if out.cs() {
            writeln!(s, "Nenabízeno jinému kódu ({}):\n", story.internals.len())
        } else {
            writeln!(
                s,
                "{} not offered to other code:\n",
                plural(story.internals.len(), "member", "members")
            )
        };
        for row in &story.internals {
            let mut line = format!("- {}", code(&row.name));
            if let Some(kind) = &row.kind {
                let _ = write!(line, " — {kind}");
            }
            if let Some(summary) = &row.summary {
                let _ = write!(line, ": {summary}");
            }
            if let Some(anchor) = &row.anchor {
                let _ = write!(line, " · `{anchor}`");
            }
            let _ = writeln!(s, "{line}");
        }
    }
    s
}

pub(super) fn api_row(out: &Out, row: &ApiRow) -> String {
    // A nested type owns a card: its row links by name and shows the signature
    // beside it. Any other member is its signature, in code.
    let mut line = match out.index.path_of(&row.identity) {
        Some(_) => {
            let mut l = format!("- {}", out.link(&row.identity, &row.name));
            if let Some(sig) = &row.signature {
                let _ = write!(l, " {}", code(sig));
            }
            l
        }
        None => format!(
            "- {}",
            row.signature
                .as_deref()
                .map(code)
                .unwrap_or_else(|| code(&row.name))
        ),
    };
    if let Some(summary) = &row.summary {
        let _ = write!(line, " — {summary}");
    }
    if let Some(anchor) = &row.anchor {
        let _ = write!(line, " · `{anchor}`");
    }
    line.push('\n');
    if !row.reaches.is_empty() {
        let names: Vec<String> = row
            .reaches
            .iter()
            .take(8)
            .map(|(identity, label)| out.link(identity, label))
            .collect();
        let verb = if out.cs() { "používá" } else { "uses" };
        let _ = writeln!(line, "  - {verb} {}", out.series(&names, row.reaches.len()));
    }
    line
}

/// `## Works with` / `## Used by` tables.
pub fn peer_tables(out: &Out, story: &Story) -> String {
    let mut s = String::new();
    if !story.uses.is_empty() {
        s.push_str("\n## Works with\n\n| Type or file | Module | Relationship | Through |\n|---|---|---|---|\n");
        for p in &story.uses {
            let through = if p.via.is_empty() {
                String::new()
            } else {
                let names: Vec<String> = p.via.iter().take(4).map(|v| code(v)).collect();
                out.series(&names, p.via.len())
            };
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                out.peer_link(p),
                module_cell(out, p),
                p.counts.describe(),
                cell(&through)
            );
        }
        more(out, &mut s, story.uses_total, story.uses.len());
    }
    if !story.users.is_empty() {
        s.push_str("\n## Used by\n\n| Type or file | Module | Relationship |\n|---|---|---|\n");
        for p in &story.users {
            let _ = writeln!(
                s,
                "| {} | {} | {} |",
                out.peer_link(p),
                module_cell(out, p),
                p.counts.describe()
            );
        }
        let non_test_total = story.users_total.saturating_sub(story.tests.len());
        more(out, &mut s, non_test_total, story.users.len());
    }
    if !story.bases.is_empty() || !story.derived.is_empty() {
        s.push_str("\n## Inheritance\n\n");
        for (p, word) in &story.bases {
            let word = match (out.cs(), *word) {
                (true, "extends") => "rozšiřuje",
                (true, "implements") => "implementuje",
                (_, w) => w,
            };
            let _ = writeln!(s, "- {word} {}{}", out.peer_link(p), summary_tail(p));
        }
        let derived_word = if out.cs() {
            "staví na něm"
        } else {
            "extended or implemented by"
        };
        for p in &story.derived {
            let _ = writeln!(
                s,
                "- {derived_word} {}{}",
                out.peer_link(p),
                summary_tail(p)
            );
        }
        more(out, &mut s, story.derived_total, story.derived.len());
    }
    if !story.tests.is_empty() {
        s.push_str("\n## Tests\n\n");
        for p in &story.tests {
            let _ = writeln!(s, "- {} — {}", out.peer_link(p), p.counts.describe());
        }
    }
    s
}

fn module_cell(out: &Out, p: &Peer) -> String {
    match &p.module {
        Some(m) => out.link(m, out.module_label(m)),
        None => String::new(),
    }
}

fn summary_tail(p: &Peer) -> String {
    match &p.summary {
        Some(summary) => format!(" — {summary}"),
        None => String::new(),
    }
}

fn more(out: &Out, s: &mut String, total: usize, shown: usize) {
    if total > shown {
        let _ = if out.cs() {
            writeln!(
                s,
                "\nNeuvedeno: {} dalších — úplná sada je v exportu grafu.",
                total - shown
            )
        } else {
            writeln!(
                s,
                "\n{} more not listed — the full set is in the graph export.",
                total - shown
            )
        };
    }
}

/// The explaining sections of a module card.
pub fn module_body(out: &Out, story: &Story) -> String {
    let Some(ms) = &story.module_story else {
        return String::new();
    };
    let card = out.card;
    let cs = out.cs();
    let name = code(out.module_label(&card.title));
    let mut s = String::new();

    // Role.
    let mut lines: Vec<String> = Vec::new();
    let size = match (ms.own_lines, story.lines) {
        (Some(own), Some(all)) if all > own && !ms.submodules.is_empty() => {
            if cs {
                format!(
                    " ({} přímo zde, {} i s podmoduly)",
                    lines_word(out, own),
                    lines_word(out, all)
                )
            } else {
                format!(
                    " ({} here, {} with its sub-modules)",
                    lines_word(out, own),
                    lines_word(out, all)
                )
            }
        }
        (Some(own), _) => format!(" ({})", lines_word(out, own)),
        (None, Some(all)) => format!(" ({})", lines_word(out, all)),
        (None, None) => String::new(),
    };
    let mut holds = vec![count(
        out,
        ms.files.len(),
        ("file", "files"),
        ("soubor", "soubory", "souborů"),
    )];
    if !ms.submodules.is_empty() {
        holds.push(count(
            out,
            ms.submodules.len(),
            ("sub-module", "sub-modules"),
            ("podmodul", "podmoduly", "podmodulů"),
        ));
    }
    let holds = out.series(&holds, holds.len());
    let declaring = if ms.types == 0 {
        String::new()
    } else if cs {
        format!(", deklaruje {}", cs_n(ms.types, ("typ", "typy", "typů")))
    } else {
        format!(", declaring {}", plural(ms.types, "type", "types"))
    };
    lines.push(if cs {
        format!("{name} obsahuje {holds}{size}{declaring}.")
    } else {
        format!("{name} holds {holds}{size}{declaring}.")
    });
    if !ms.depends_on.is_empty() {
        let named: Vec<String> = ms
            .depends_on
            .iter()
            .take(NAMED_IN_SENTENCE)
            .map(|p| {
                format!(
                    "{} ({})",
                    out.link(&p.identity, out.module_label(&p.label)),
                    p.counts.undisputed()
                )
            })
            .collect();
        let list = out.series(&named, ms.depends_on_total);
        lines.push(if cs {
            format!(
                "Závisí na {} — hlavně {list}.",
                cs_n(
                    ms.depends_on_total,
                    ("dalším modulu", "dalších modulech", "dalších modulech")
                )
            )
        } else {
            format!(
                "It depends on {} — chiefly {list}.",
                plural(ms.depends_on_total, "other module", "other modules")
            )
        });
    } else if ms.types > 0 && ms.depends_on_ambiguous == 0 {
        lines.push(
            if cs {
                "Nebylo vidět, že by závisel na jiném modulu tohoto repozitáře."
            } else {
                "It was not seen depending on any other module of this repository."
            }
            .into(),
        );
    }
    if !ms.used_by.is_empty() {
        let named: Vec<String> = ms
            .used_by
            .iter()
            .take(NAMED_IN_SENTENCE)
            .map(|p| out.link(&p.identity, out.module_label(&p.label)))
            .collect();
        let list = out.series(&named, ms.used_by_total);
        lines.push(if cs {
            format!(
                "Používá ho {}: {list}.",
                cs_n(ms.used_by_total, ("modul", "moduly", "modulů"))
            )
        } else {
            format!(
                "It is used by {}: {list}.",
                plural(ms.used_by_total, "module", "modules")
            )
        });
    }
    if ms.depends_on_ambiguous > 0 {
        let k = ms.depends_on_ambiguous;
        lines.push(if cs {
            format!(
                "Možná závisí také na {} — přes shodu jména, kterou pravidla nerozhodla.",
                cs_n(k, ("dalším modulu", "dalších modulech", "dalších modulech"))
            )
        } else {
            format!(
                "It may depend on {} too, through name matches the rules did not settle.",
                plural(k, "other module", "other modules")
            )
        });
    }
    if ms.used_by_ambiguous > 0 {
        let k = ms.used_by_ambiguous;
        lines.push(if cs {
            let (who, verb) = match k {
                1 => ("další modul", "používá"),
                2..=4 => ("další moduly", "používají"),
                _ => ("dalších modulů", "používá"),
            };
            format!(
                "Ještě {k} {who} ho možná {verb} také — přes shodu jména, kterou pravidla nerozhodla."
            )
        } else {
            format!(
                "{} may use it too, through name matches the rules did not settle.",
                plural(k, "other module", "other modules")
            )
        });
    }
    let central: Vec<String> = ms
        .key_types
        .iter()
        .filter(|p| p.counts.references > 0)
        .take(NAMED_IN_SENTENCE)
        .map(|p| out.peer_link(p))
        .collect();
    if !central.is_empty() {
        let list = out.series(&central, central.len());
        lines.push(if cs {
            format!("Jeho nejpoužívanější typy jsou {list}.")
        } else {
            format!("Its most used types are {list}.")
        });
    }
    if !story.externals.is_empty() {
        lines.push(outside(out, story));
    }
    s.push_str("\n## Role\n\n");
    for line in lines {
        let _ = writeln!(s, "{line}");
    }

    // Diagram.
    let relabel = |list: &[Peer]| -> Vec<Peer> {
        list.iter()
            .map(|p| Peer {
                label: out.module_label(&p.label).to_string(),
                ..p.clone()
            })
            .collect()
    };
    if let Some(d) = diagram(
        out.lang,
        "Dependencies",
        out.module_label(&card.title),
        &relabel(&ms.used_by),
        &relabel(&ms.depends_on),
    ) {
        s.push_str(&d);
    }

    if !ms.key_types.is_empty() {
        s.push_str("\n## Key types\n\n| Type | Kind | Used by | Summary |\n|---|---|---|---|\n");
        for p in &ms.key_types {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                out.peer_link(p),
                p.kind.as_deref().unwrap_or(""),
                p.counts.references,
                cell(p.summary.as_deref().unwrap_or(""))
            );
        }
        if ms.types > ms.key_types.len() {
            let rest = ms.types - ms.key_types.len();
            let _ = if cs {
                writeln!(
                    s,
                    "\nDeklaruje se zde ještě {rest} dalších typů; soubory níže uvádějí všechny."
                )
            } else {
                writeln!(
                    s,
                    "\n{rest} more types are declared here; the files below list every one."
                )
            };
        }
    }

    if !ms.submodules.is_empty() || !ms.files.is_empty() {
        s.push_str("\n## Contents\n");
        if !ms.submodules.is_empty() {
            s.push_str("\n| Sub-module | Lines | Summary |\n|---|---|---|\n");
            for m in &ms.submodules {
                let _ = writeln!(
                    s,
                    "| {} | {} | {} |",
                    out.link(&m.identity, &m.identity),
                    m.lines.map(grouped).unwrap_or_default(),
                    cell(m.summary.as_deref().unwrap_or(""))
                );
            }
        }
        if !ms.files.is_empty() {
            s.push_str("\n| File | Lines | Declares |\n|---|---|---|\n");
            for f in &ms.files {
                let declares: Vec<String> = f
                    .declares
                    .iter()
                    .take(6)
                    .map(|(identity, label)| out.link(identity, label))
                    .collect();
                let mut file = out.link(&f.identity, file_name(&f.identity));
                if f.is_test {
                    file.push_str(" (test)");
                }
                let _ = writeln!(
                    s,
                    "| {} | {} | {} |",
                    file,
                    f.lines.map(grouped).unwrap_or_default(),
                    out.series(&declares, f.declares.len())
                );
            }
        }
    }

    if !ms.depends_on.is_empty() {
        s.push_str("\n## Depends on\n\n| Module | Relationship |\n|---|---|\n");
        for p in &ms.depends_on {
            let _ = writeln!(
                s,
                "| {} | {} |",
                out.link(&p.identity, out.module_label(&p.label)),
                p.counts.describe()
            );
        }
        more(out, &mut s, ms.depends_on_total, ms.depends_on.len());
    }
    if !ms.used_by.is_empty() {
        s.push_str("\n## Used by\n\n| Module | Relationship |\n|---|---|\n");
        for p in &ms.used_by {
            let _ = writeln!(
                s,
                "| {} | {} |",
                out.link(&p.identity, out.module_label(&p.label)),
                p.counts.describe()
            );
        }
        more(out, &mut s, ms.used_by_total, ms.used_by.len());
    }
    if !story.externals.is_empty() {
        s.push_str("\n## Outside the repository\n\n| External | Kind | Uses |\n|---|---|---|\n");
        for e in &story.externals {
            let _ = writeln!(
                s,
                "| {} | {} | {} |",
                code(&e.label),
                e.kind.as_deref().unwrap_or(""),
                e.uses
            );
        }
        more(out, &mut s, story.externals_total, story.externals.len());
    }
    s
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn czech_counts_agree_with_the_number() {
        let forms = ("typ", "typy", "typů");
        assert_eq!(cs_n(1, forms), "1 typ");
        assert_eq!(cs_n(3, forms), "3 typy");
        assert_eq!(cs_n(5, forms), "5 typů");
        assert_eq!(cs_n(0, forms), "0 typů");
    }

    /// The card trim: the count of further neighbours carries how many are
    /// uncertain in parentheses — none, the one, all of them, or some — with
    /// the number agreeing in either language.
    #[test]
    fn the_further_count_says_how_many_are_uncertain() {
        let cases = [
            (0, 0, Lang::En, None),
            (1, 0, Lang::En, Some("The detail lists 1 more neighbour.")),
            (3, 0, Lang::En, Some("The detail lists 3 more neighbours.")),
            (
                1,
                1,
                Lang::En,
                Some("The detail lists 1 more neighbour (uncertain)."),
            ),
            (
                2,
                2,
                Lang::En,
                Some("The detail lists 2 more neighbours (all uncertain)."),
            ),
            (
                5,
                1,
                Lang::En,
                Some("The detail lists 5 more neighbours (1 uncertain)."),
            ),
            (
                5,
                3,
                Lang::En,
                Some("The detail lists 5 more neighbours (3 uncertain)."),
            ),
            (0, 0, Lang::Cs, None),
            (
                1,
                0,
                Lang::Cs,
                Some("Detail uvádí ještě 1 dalšího souseda."),
            ),
            (
                1,
                1,
                Lang::Cs,
                Some("Detail uvádí ještě 1 dalšího souseda (nejistý)."),
            ),
            (
                3,
                3,
                Lang::Cs,
                Some("Detail uvádí ještě 3 další sousedy (všichni nejistí)."),
            ),
            (
                4,
                1,
                Lang::Cs,
                Some("Detail uvádí ještě 4 další sousedy (z toho 1 nejistý)."),
            ),
            (
                7,
                2,
                Lang::Cs,
                Some("Detail uvádí ještě 7 dalších sousedů (z toho 2 nejistí)."),
            ),
            (
                9,
                5,
                Lang::Cs,
                Some("Detail uvádí ještě 9 dalších sousedů (z toho 5 nejistých)."),
            ),
        ];
        for (more, uncertain, lang, expected) in cases {
            assert_eq!(
                further_sentence(lang, Further { more, uncertain }).as_deref(),
                expected,
                "{more} more, {uncertain} uncertain, {lang:?}"
            );
        }
    }
}
