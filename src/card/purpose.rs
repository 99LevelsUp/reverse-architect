//! What a type is for, said only as far as the code says it (plan B, U7: R5,
//! R6, R14, AE4, KTD5).
//!
//! A purpose is a few sentences, and every one of them carries where it came
//! from ([`Origin`]):
//!
//! 1. the first sentence of the type's own doc comment, verbatim — `comment`;
//! 2. one **stereotype** from the closed table below, or, when there is
//!    neither a comment nor a stereotype, the type's kind and the shape of its
//!    members — `structure`;
//! 3. what it builds on and what it does outside its own code: its bases in
//!    the repository, the database objects it writes, reads, calls and names,
//!    and the resources (URLs, configuration keys) it uses — `structure`.
//!
//! The stereotype table, in the order it is asked — the first that holds wins:
//!
//! | stereotype | holds when |
//! |---|---|
//! | entry point | a member (or the type) carries the `EntryPoint` fact |
//! | factory | a static method returns the type itself |
//! | static helper | the type is declared `static`, or every member but its constructors states `static` or `const`, with at least one method |
//! | data structure | every member is a property, a field or a constructor, with at least one property or field |
//!
//! Nothing here is learned or guessed from a name: every rule reads a fact the
//! extractors emitted (a kind, a signature, a relation, a fact on a member). A
//! type no rule fits and no comment explains gets its kind and its shape and
//! nothing else (R6, AE4). A base type outside the repository is no input: the
//! graph holds no relation to it, and reading one out of the signature would
//! take a per-language parse the closed header reading does not do.
//!
//! The sentences are data ([`Claim`]); [`Sentence::text`] phrases them in the
//! card's language with the same helpers as the rest of the card's prose.

use super::explain::{capitalized, code, cs_n, plural, series};
use super::header::TypeHeader;
use super::insight::Insight;
use super::render::Lang;
use super::story::{kind_rank, Peer};
use super::MemberRow;
use crate::graph::DbAccessKind;

/// How many names one clause lists before it says "and N more".
const NAMED_IN_CLAUSE: usize = 3;

/// Where a purpose sentence comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The type's own doc comment, quoted.
    Comment,
    /// Facts the extractors read off the code: kinds, signatures, relations.
    Structure,
}

impl Origin {
    /// The language-neutral token — the same whatever the card's language.
    pub fn token(self) -> &'static str {
        match self {
            Origin::Comment => "comment",
            Origin::Structure => "structure",
        }
    }

    /// The token as the card's prose says it.
    pub fn phrase(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Origin::Comment, Lang::En) => "from the comment",
            (Origin::Comment, Lang::Cs) => "z komentáře",
            (Origin::Structure, Lang::En) => "from the structure",
            (Origin::Structure, Lang::Cs) => "ze struktury",
        }
    }
}

/// How many members of each group a type has — the groups of the API list
/// ([`kind_rank`]), in that order, empty groups left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shape {
    pub groups: Vec<(u8, usize)>,
}

impl Shape {
    fn of(members: &[MemberRow]) -> Shape {
        let mut counts = [0usize; 9];
        for member in members {
            counts[usize::from(kind_rank(member.member_kind.as_deref()))] += 1;
        }
        Shape {
            groups: (0u8..).zip(counts).filter(|(_, n)| *n > 0).collect(),
        }
    }

    fn count(&self, rank: u8) -> usize {
        self.groups
            .iter()
            .find(|(r, _)| *r == rank)
            .map_or(0, |(_, n)| *n)
    }

    /// `3 methods, 1 property` / `3 metody, 1 vlastnost`.
    fn text(&self, lang: Lang) -> String {
        self.groups
            .iter()
            .map(|&(rank, n)| {
                let (en, cs) = group_words(rank);
                match lang {
                    Lang::En => plural(n, en.0, en.1),
                    Lang::Cs => cs_n(n, cs),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The group of [`kind_rank`] as a counted noun, English `(one, many)` and
/// Czech `(1, 2–4, 5+)`.
fn group_words(
    rank: u8,
) -> (
    (&'static str, &'static str),
    (&'static str, &'static str, &'static str),
) {
    match rank {
        0 => (
            ("constructor", "constructors"),
            ("konstruktor", "konstruktory", "konstruktorů"),
        ),
        1 => (("method", "methods"), ("metoda", "metody", "metod")),
        2 => (("function", "functions"), ("funkce", "funkce", "funkcí")),
        3 => (
            ("property", "properties"),
            ("vlastnost", "vlastnosti", "vlastností"),
        ),
        4 => (("event", "events"), ("událost", "události", "událostí")),
        5 => (("field", "fields"), ("pole", "pole", "polí")),
        6 => (("value", "values"), ("hodnota", "hodnoty", "hodnot")),
        7 => (
            ("nested type", "nested types"),
            ("vnořený typ", "vnořené typy", "vnořených typů"),
        ),
        _ => (
            ("other member", "other members"),
            ("další člen", "další členy", "dalších členů"),
        ),
    }
}

/// How a base relates to the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseWord {
    Extends,
    Implements,
}

/// What one purpose sentence states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// The first sentence of the doc comment, as the source wrote it.
    Comment(String),
    /// The members the extractor marked as the program's entry point.
    EntryPoint { members: Vec<String> },
    /// Static methods that return the type itself.
    Factory { methods: Vec<String> },
    /// Nothing but static members, `methods` of them methods.
    StaticHelper { methods: usize },
    /// Nothing but data — properties and fields, constructors aside.
    DataStructure { shape: Shape },
    /// No comment and no stereotype: the declared kind and the members.
    KindAndShape { kind: String, shape: Shape },
    /// What the type builds on and what it does outside its own code: bases
    /// in the repository, database objects by access, other resources.
    Reaches {
        bases: Vec<(String, BaseWord)>,
        /// Per access, in the order writes, reads, calls, mentions: the
        /// objects, and how many there are in all.
        db: Vec<(DbAccessKind, Vec<String>, usize)>,
        resources: Vec<String>,
        resources_total: usize,
    },
}

/// One sentence of a purpose, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub origin: Origin,
    pub claim: Claim,
}

impl Sentence {
    fn new(claim: Claim) -> Sentence {
        let origin = match claim {
            Claim::Comment(_) => Origin::Comment,
            _ => Origin::Structure,
        };
        Sentence { origin, claim }
    }

    /// The sentence in `lang`, its provenance left to the caller
    /// ([`Origin::phrase`]). Names are code spans; the kind word stays the
    /// fact vocabulary's English, as everywhere else in a card's prose.
    pub fn text(&self, lang: Lang) -> String {
        let cs = lang == Lang::Cs;
        match &self.claim {
            Claim::Comment(text) => text.clone(),
            Claim::EntryPoint { members } => {
                let names = names(lang, members, members.len());
                if cs {
                    format!("Vstupní bod programu: {names}.")
                } else {
                    format!("Entry point of the program: {names}.")
                }
            }
            Claim::Factory { methods } => {
                let names = names(lang, methods, methods.len());
                match (cs, methods.len()) {
                    (false, 1) => {
                        format!("Creates its own instances through the static method {names}.")
                    }
                    (false, _) => {
                        format!("Creates its own instances through the static methods {names}.")
                    }
                    (true, 1) => format!("Vytváří vlastní instance statickou metodou {names}."),
                    (true, _) => format!("Vytváří vlastní instance statickými metodami {names}."),
                }
            }
            Claim::StaticHelper { methods } => {
                if cs {
                    format!(
                        "Statický pomocník: jen statické členy — {}.",
                        cs_n(*methods, ("metoda", "metody", "metod"))
                    )
                } else {
                    format!(
                        "Static helper: static members only — {}.",
                        plural(*methods, "method", "methods")
                    )
                }
            }
            Claim::DataStructure { shape } => {
                if cs {
                    format!("Datová struktura: {}; žádné metody.", shape.text(lang))
                } else {
                    format!("Data structure: {}; no methods.", shape.text(lang))
                }
            }
            Claim::KindAndShape { kind, shape } => match (cs, shape.groups.is_empty()) {
                (false, true) => format!("{} with no members.", code(kind)),
                (true, true) => format!("{} bez členů.", code(kind)),
                (_, false) => format!("{}: {}.", code(kind), shape.text(lang)),
            },
            Claim::Reaches {
                bases,
                db,
                resources,
                resources_total,
            } => {
                let mut clauses: Vec<String> = Vec::new();
                for word in [BaseWord::Implements, BaseWord::Extends] {
                    let of: Vec<String> = bases
                        .iter()
                        .filter(|(_, w)| *w == word)
                        .map(|(label, _)| label.clone())
                        .collect();
                    if of.is_empty() {
                        continue;
                    }
                    let verb = match (cs, word) {
                        (false, BaseWord::Implements) => "implements",
                        (false, BaseWord::Extends) => "extends",
                        (true, BaseWord::Implements) => "implementuje",
                        (true, BaseWord::Extends) => "rozšiřuje",
                    };
                    clauses.push(format!("{verb} {}", names(lang, &of, of.len())));
                }
                for (access, objects, total) in db {
                    let verb = match (cs, access) {
                        (false, DbAccessKind::Writes) => "writes",
                        (false, DbAccessKind::Reads) => "reads",
                        (false, DbAccessKind::Calls) => "calls",
                        (false, DbAccessKind::Mentions) => "names",
                        (true, DbAccessKind::Writes) => "zapisuje do",
                        (true, DbAccessKind::Reads) => "čte",
                        (true, DbAccessKind::Calls) => "volá",
                        (true, DbAccessKind::Mentions) => "uvádí",
                    };
                    clauses.push(format!("{verb} {}", names(lang, objects, *total)));
                }
                if !resources.is_empty() {
                    let verb = if cs { "používá" } else { "uses" };
                    clauses.push(format!(
                        "{verb} {}",
                        names(lang, resources, *resources_total)
                    ));
                }
                format!("{}.", capitalized(&clauses.join("; ")))
            }
        }
    }
}

/// Names as code spans in a series.
fn names(lang: Lang, items: &[String], total: usize) -> String {
    let shown: Vec<String> = items.iter().map(|n| code(n)).collect();
    series(lang, &shown, total)
}

/// Everything [`build`] reads about one type.
pub struct Input<'a> {
    pub key: &'a str,
    pub title: &'a str,
    pub kind: Option<&'a str>,
    pub header: Option<&'a TypeHeader>,
    /// The first sentence of the doc comment ([`super::story::Story::summary`]).
    pub summary: Option<&'a str>,
    pub members: &'a [MemberRow],
    /// The bases in the repository ([`super::story::Story::bases`]).
    pub bases: &'a [(Peer, &'static str)],
}

/// The purpose of one type, in the order the module doc gives.
pub fn build(input: &Input, insight: &Insight) -> Vec<Sentence> {
    let mut out: Vec<Sentence> = Vec::new();
    // A summary holding the secret filter's marker is a hole, not a sentence
    // about the type; the card's quoted comment says what was withheld.
    let comment = input
        .summary
        .filter(|s| !s.contains(crate::extract::external::REDACTION_MARKER));
    if let Some(summary) = comment {
        out.push(Sentence::new(Claim::Comment(summary.to_string())));
    }
    match stereotype(input, insight) {
        Some(claim) => out.push(Sentence::new(claim)),
        None if comment.is_none() => out.push(Sentence::new(Claim::KindAndShape {
            kind: input.kind.unwrap_or("type").to_string(),
            shape: Shape::of(input.members),
        })),
        None => {}
    }
    if let Some(reaches) = reaches(input, insight) {
        out.push(Sentence::new(reaches));
    }
    out
}

/// The first stereotype of the table in the module doc that holds.
fn stereotype(input: &Input, insight: &Insight) -> Option<Claim> {
    let entries: Vec<String> = insight
        .entry_points(input.key)
        .map(str::to_string)
        .collect();
    if !entries.is_empty() {
        return Some(Claim::EntryPoint { members: entries });
    }

    let own = bare(input.title);
    let mut factories: Vec<String> = input
        .members
        .iter()
        .filter(|m| kind_rank(m.member_kind.as_deref()) == 1)
        .filter(|m| {
            m.signature
                .as_deref()
                .and_then(static_return)
                .is_some_and(|returns| returns_own_type(returns, own, input.key))
        })
        .map(|m| m.name.clone())
        .collect();
    factories.dedup();
    if !factories.is_empty() {
        return Some(Claim::Factory { methods: factories });
    }

    let shape = Shape::of(input.members);
    let methods = shape.count(1) + shape.count(2);
    // A declared `static` speaks for the members only where the language
    // makes it: a C# static class can hold nothing else. On a Java nested
    // class it says only that the class is no inner one — a static builder
    // is all instance methods — so everywhere else the members must say it.
    let declared_static = input.key.starts_with("sym:csharp:")
        && input
            .header
            .is_some_and(|h| h.modifiers.iter().any(|m| m == "static"));
    let all_static = input
        .members
        .iter()
        .filter(|m| !matches!(kind_rank(m.member_kind.as_deref()), 0 | 7))
        .all(|m| m.signature.as_deref().is_some_and(states_static));
    if methods > 0 && (declared_static || all_static) {
        return Some(Claim::StaticHelper { methods });
    }

    let data = shape.count(3) + shape.count(5);
    let only_data = shape
        .groups
        .iter()
        .all(|(rank, _)| matches!(rank, 0 | 3 | 5));
    if data > 0 && only_data {
        return Some(Claim::DataStructure { shape });
    }
    None
}

/// The bases and side effects sentence, or `None` when there are none.
fn reaches(input: &Input, insight: &Insight) -> Option<Claim> {
    let bases: Vec<(String, BaseWord)> = input
        .bases
        .iter()
        .map(|(peer, word)| {
            let word = if *word == "extends" {
                BaseWord::Extends
            } else {
                BaseWord::Implements
            };
            (peer.label.clone(), word)
        })
        .collect();

    let mut db: Vec<(DbAccessKind, Vec<String>, usize)> = Vec::new();
    for access in [
        DbAccessKind::Writes,
        DbAccessKind::Reads,
        DbAccessKind::Calls,
        DbAccessKind::Mentions,
    ] {
        let objects: Vec<&str> = insight
            .db_accesses(input.key)
            .filter(|(a, _)| *a == access)
            .map(|(_, object)| object)
            .collect();
        if !objects.is_empty() {
            db.push((
                access,
                objects
                    .iter()
                    .take(NAMED_IN_CLAUSE)
                    .map(|o| o.to_string())
                    .collect(),
                objects.len(),
            ));
        }
    }

    // Resources only: a database object is said above, by how it is reached,
    // and a foreign symbol is a library the code calls, not a side effect.
    let resources: Vec<String> = insight
        .externals_of(input.key)
        .into_iter()
        .filter(|e| matches!(e.kind.as_deref(), Some("url") | Some("config_key")))
        .map(|e| e.label)
        .collect();
    let resources_total = resources.len();

    if bases.is_empty() && db.is_empty() && resources.is_empty() {
        return None;
    }
    Some(Claim::Reaches {
        bases,
        db,
        resources: resources.into_iter().take(NAMED_IN_CLAUSE).collect(),
        resources_total,
    })
}

/// A type name without its generic arguments, nullable mark or qualifier:
/// `Ns.Money<T>?` → `Money`.
fn bare(name: &str) -> &str {
    let name = name.split('<').next().unwrap_or(name).trim_end_matches('?');
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

/// Whether a static method's written return type is the type it is declared
/// in (R6: say only what is known). Unqualified, the name is the type's own
/// — inside its body the type's simple name means the type itself. A
/// qualified name must spell the end of the type's own full name, which the
/// key holds (`N.Foo` and `global::N.Foo` in `sym:csharp:N#Foo`); a namesake
/// of another namespace (`Other.Foo`) is some other type, and a qualifier the
/// key cannot tie to it decides for no factory. `global::` spells the name
/// from the root, so it must be the type's full name exactly: `global::Foo`
/// inside `N.Foo` is the root-level `Foo`, another type.
fn returns_own_type(returns: &str, own: &str, key: &str) -> bool {
    let written = without_generics(returns.trim_end_matches('?'));
    let absolute = written.strip_prefix("global::");
    let written = absolute.unwrap_or(&written);
    if absolute.is_none() && !written.contains(['.', ':']) {
        return written == own;
    }
    // `sym:<lang>:<namespace>#<type path>` → `<namespace>.<type path>`.
    let Some((namespace, path)) = key
        .splitn(3, ':')
        .nth(2)
        .and_then(|rest| rest.split_once('#'))
    else {
        return false;
    };
    let path = without_generics(path);
    let written = written.replace("::", ".");
    if absolute.is_some() {
        // A C# type outside every namespace is keyed under its file's path
        // (`sym:csharp:src/Foo#Foo`), which no namespace name can hold: its
        // full name is its type path alone. A file at the repository root
        // reads as a namespace here, and the claim is then not made.
        let full = if namespace.is_empty() || namespace.contains('/') {
            path
        } else {
            format!("{}.{path}", namespace.replace("::", "."))
        };
        return full == written;
    }
    let full = format!("{}.{path}", namespace.replace("::", "."));
    full == written || full.ends_with(&format!(".{written}"))
}

/// A type name without its generic arguments: `Outer<T>.Inner<U>` →
/// `Outer.Inner`.
fn without_generics(name: &str) -> String {
    let mut depth = 0usize;
    name.chars()
        .filter(|&c| {
            match c {
                '<' => depth += 1,
                '>' => depth = depth.saturating_sub(1),
                _ => return depth == 0,
            }
            false
        })
        .collect()
}

/// The words of a signature before its parameter list.
fn head(signature: &str) -> Vec<&str> {
    signature
        .split('(')
        .next()
        .unwrap_or(signature)
        .split_whitespace()
        .collect()
}

/// Whether a member's signature states `static` or `const` among its leading
/// words. A language that marks neither (Rust, Python) never qualifies.
fn states_static(signature: &str) -> bool {
    head(signature)
        .iter()
        .any(|w| matches!(*w, "static" | "const"))
}

/// The return type of a method whose signature states `static`: the word
/// right before the method's name. `None` for an instance method, or where
/// the head has no word before the name.
fn static_return(signature: &str) -> Option<&str> {
    let words = head(signature);
    if !words.contains(&"static") || words.len() < 2 {
        return None;
    }
    Some(words[words.len() - 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_return_is_the_word_before_the_name() {
        assert_eq!(
            static_return("public static Money From(decimal amount)"),
            Some("Money")
        );
        assert_eq!(static_return("public Money Add(Money other)"), None);
        assert_eq!(bare("Orbit.Money<T>?"), "Money");
    }
}
