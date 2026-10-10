//! A card's keywords: the few words a search should find the entity by (plan
//! B, U6: R8, R15, KTD4).
//!
//! Four sources, all read from facts the extractors already emitted:
//!
//! - **identifiers** — the name of the type and of every symbol it owns (a
//!   file: its own name and its free declarations), split at `_`, at a case
//!   change and between letters and digits ([`split_identifier`]);
//! - **database objects** — every object a [`FactKind::DbAccess`] fact names,
//!   by its whole name (`ops_schedule_tasks`) and by its parts;
//! - **string literals** and **comment lines** — the [`FactKind::Anchor`]
//!   texts, which the secret filter has already passed (R10); a withheld text
//!   leaves an `AnchorSkipped` or a `RedactedLiteral` fact, and neither is ever
//!   read here;
//! - **doc comments** of the type and of its members — anchors leave them out,
//!   because [`FactKind::DocComment`] carries them — with the lines the secret
//!   filter replaced skipped and every long token shaped like a key dropped
//!   (KTD11 rule 3) BEFORE any word is split out of it, so no piece of a key
//!   becomes a word.
//!
//! Each word is lower-cased and kept as written; short and common words (a
//! stoplist as code) are dropped. A word scores its occurrences in the unit
//! times an integer logarithm of how rare it is across the corpus, so the words
//! typical of THIS unit come first; ties go to code-point order. No floating
//! point anywhere, so the list is the same bytes on every machine (R15).
//!
//! [`FactKind::DbAccess`]: crate::graph::FactKind::DbAccess
//! [`FactKind::Anchor`]: crate::graph::FactKind::Anchor
//! [`FactKind::DocComment`]: crate::graph::FactKind::DocComment

use crate::extract::external::is_long_token;
use std::collections::{BTreeMap, HashMap, HashSet};

/// How many words a card keeps. A word's diacritics-free variant rides along
/// with it and is not counted: it is the same word, written the way a search
/// without a Czech keyboard types it.
pub const KEYWORD_CAP: usize = 12;

/// Words shorter than this, in characters, are no keywords: `id`, `to`, `je`.
const MIN_WORD_CHARS: usize = 3;

/// Words longer than this, in characters, are no keywords either — a run of
/// letters that long is a blob, not a word anybody types.
const MAX_WORD_CHARS: usize = 40;

/// One keyword of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    /// The word as the source writes it, lower-cased (`obnově`).
    pub word: String,
    /// The same word without diacritics (`obnove`) — `None` when that is the
    /// word itself, or when another keyword of the card already is it.
    pub folded: Option<String>,
}

/// The parts of an identifier, lower-cased, in order: split at every character
/// that is neither a letter nor a digit, where a lower-case letter meets an
/// upper-case one, where an acronym meets the next word (`XMLParser` → `xml`,
/// `parser`), and between letters and digits (`Base64Encode` → `base`, `64`,
/// `encode`). Every part is returned — which of them make keywords is decided
/// apart from the split.
pub fn split_identifier(name: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let chars: Vec<char> = name.chars().collect();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some(&prev) = i.checked_sub(1).and_then(|p| chars.get(p)) {
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            let boundary = prev.is_alphanumeric()
                && ((prev.is_lowercase() && c.is_uppercase())
                    || (prev.is_uppercase() && c.is_uppercase() && next_lower)
                    || (prev.is_numeric() != c.is_numeric()));
            if boundary && !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Whether a lower-cased part is a keyword at all: long enough, not too long,
/// not a bare number, not a common word.
pub(crate) fn is_word(part: &str) -> bool {
    let chars = part.chars().count();
    (MIN_WORD_CHARS..=MAX_WORD_CHARS).contains(&chars)
        && part.chars().any(char::is_alphabetic)
        && !is_stopword(part)
}

/// The common words no search is helped by: English and Czech function words,
/// the keywords of the languages the lexer reads, the SQL verbs and clauses
/// every query shares, and the tags of doc comments. Matched on the lower-cased
/// word.
fn is_stopword(word: &str) -> bool {
    matches!(
        word,
        // English
        "about" | "above" | "after" | "all" | "also" | "and" | "any" | "are" | "because"
            | "been" | "before" | "being" | "both" | "but" | "can" | "could" | "did"
            | "does" | "each" | "else" | "for" | "from" | "had" | "has" | "have" | "her"
            | "here" | "his" | "how" | "into" | "its" | "may" | "more" | "most" | "must"
            | "not" | "now" | "off" | "one" | "only" | "other" | "our" | "out" | "over"
            | "same" | "should" | "some" | "such" | "than" | "that" | "the" | "their"
            | "them" | "then" | "there" | "these" | "they" | "this" | "those" | "through"
            | "too" | "two" | "under" | "use" | "used" | "uses" | "using" | "very" | "was"
            | "were" | "what" | "when" | "where" | "which" | "while" | "who" | "why"
            | "will" | "with" | "without" | "would" | "yet" | "you" | "your"
            // Czech
            | "ale" | "aby" | "ani" | "bez" | "byl" | "byla" | "bylo" | "byly" | "být"
            | "což" | "jak" | "jako" | "jaké" | "jejich" | "jeho" | "její" | "jen" | "již"
            | "jsou" | "kde" | "kdy" | "když" | "který" | "která" | "které" | "kteří"
            | "mezi" | "nebo" | "není" | "než" | "pak" | "pod" | "podle" | "pokud" | "pro"
            | "proto" | "při" | "také" | "tak" | "tam" | "tato" | "této" | "tento"
            | "tím" | "toho" | "tom" | "tomu" | "toto" | "tyto" | "už" | "vše" | "všech"
            | "zda" | "že"
            // Language keywords
            | "abstract" | "async" | "await" | "base" | "bool" | "break" | "byte" | "case"
            | "catch" | "char" | "class" | "const" | "continue" | "decimal" | "def"
            | "default" | "delegate" | "double" | "enum" | "event" | "explicit" | "export"
            | "extends" | "extern" | "false" | "final" | "finally" | "float" | "func"
            | "function" | "get" | "impl" | "implements" | "import" | "int" | "interface"
            | "internal" | "let" | "long" | "mut" | "namespace" | "new" | "null" | "object"
            | "override" | "package" | "private" | "protected" | "pub" | "public"
            | "readonly" | "ref" | "return" | "sealed" | "self" | "set" | "short"
            | "static" | "string" | "struct" | "super" | "switch" | "throw" | "true"
            | "try" | "typeof" | "uint" | "ulong" | "unsafe" | "var" | "virtual" | "void"
            | "volatile"
            // SQL
            | "begin" | "declare" | "delete" | "distinct" | "end" | "exec" | "execute"
            | "exists" | "group" | "having" | "inner" | "insert" | "isnull" | "join"
            | "left" | "merge" | "nolock" | "order" | "outer" | "right" | "select" | "top"
            | "union" | "update" | "values"
            // Doc-comment tags
            | "param" | "paramref" | "returns" | "see" | "seealso" | "summary" | "throws"
            | "typeparam" | "remarks" | "inheritdoc" | "cref"
    )
}

/// The word without diacritics, when that is a different word made only of
/// ASCII letters, digits and `_` — a script `deunicode` can only spell out
/// phonetically gets no variant.
fn folded(word: &str) -> Option<String> {
    let folded = deunicode::deunicode(word).to_lowercase();
    (folded != word
        && !folded.is_empty()
        && folded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(folded)
}

/// The rarity weight of a word: `⌊4 · log₂(documents / df)⌋`, computed as the
/// integer `⌊log₂(documents⁴ / df⁴)⌋`. The fourth power gives the logarithm a
/// quarter-bit of resolution without a float — whole bits would put every word
/// in more than half of the units at the same weight as one in all of them.
/// A word in every unit weighs 0 and can never outrank a word some units lack.
fn rarity(documents: u32, df: u32) -> u32 {
    // `u32::MAX⁴` is below `2¹²⁸`, so neither power can overflow.
    let ratio = u128::from(documents).pow(4) / u128::from(df.max(1)).pow(4);
    if ratio == 0 {
        0
    } else {
        ratio.ilog2()
    }
}

/// The words of every unit, and how many units each word occurs in — built
/// once per run in [`super::insight::Insight::build`], so a card reads its own
/// short list and never the whole vocabulary.
///
/// A **document** is a unit that holds at least one word: a type, or a file
/// with something of its own (a free function, a comment outside every
/// declaration — every word of a C file, which no plugin reads). The
/// document frequency of a word counts those units.
#[derive(Debug, Default)]
pub struct KeywordIndex {
    words: Vec<Box<str>>,
    rarity: Vec<u32>,
    /// Unit → `(word, occurrences in the unit)`.
    terms: HashMap<String, Vec<(u32, u32)>>,
    /// Word → its index in `words`: the rarity of a word a later layer
    /// brings is looked up here.
    ids: HashMap<Box<str>, u32>,
    /// The documents the rarities are weighed over.
    documents: u32,
}

impl KeywordIndex {
    /// The keywords of `unit`, at most [`KEYWORD_CAP`] of them, the most
    /// typical first. Empty for a unit without a word.
    pub fn keywords(&self, unit: &str) -> Vec<Keyword> {
        self.keywords_with(unit, &UnitWords::default())
    }

    /// [`Self::keywords`] with the words the second layer brings to `unit`
    /// (plan 2a, U6: KTD9 point 4). Their occurrences add to the unit's own;
    /// the rarity of every word stays the one the first layer's document
    /// frequencies give it, and a word only the second layer brings weighs as
    /// one found in a single document. No other unit's keywords change, and
    /// with no words this is [`Self::keywords`].
    pub fn keywords_with(&self, unit: &str, extra: &UnitWords) -> Vec<Keyword> {
        let mut scored: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
        for &(id, occurrences) in self.terms.get(unit).into_iter().flatten() {
            scored.insert(
                &*self.words[id as usize],
                (u64::from(occurrences), self.rarity[id as usize]),
            );
        }
        for (word, &times) in &extra.words {
            let weight = match self.ids.get(word.as_str()) {
                Some(&id) => self.rarity[id as usize],
                None => rarity(self.documents, 1),
            };
            scored.entry(word.as_str()).or_insert((0, weight)).0 += u64::from(times);
        }
        let mut ranked: Vec<(u64, &str)> = scored
            .into_iter()
            .map(|(word, (occurrences, weight))| (occurrences * u64::from(weight), word))
            .collect();
        // `str` orders by bytes, and UTF-8 bytes order as code points do.
        ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Keyword> = Vec::new();
        for (_, word) in ranked {
            if out.len() == KEYWORD_CAP {
                break;
            }
            // `obnove` written plainly is the variant `obnově` already brought.
            if !seen.insert(word.to_string()) {
                continue;
            }
            let folded = folded(word).filter(|f| seen.insert(f.clone()));
            out.push(Keyword {
                word: word.to_string(),
                folded,
            });
        }
        out
    }
}

/// [`KeywordIndex`] while the run's facts are read into it.
#[derive(Default)]
pub(super) struct KeywordIndexBuilder {
    ids: HashMap<Box<str>, u32>,
    words: Vec<Box<str>>,
    terms: HashMap<String, HashMap<u32, u32>>,
}

impl KeywordIndexBuilder {
    /// An identifier of `unit` — a type's, a member's, a file's name.
    pub(super) fn identifier(&mut self, unit: &str, name: &str) {
        for part in split_identifier(name) {
            self.add(unit, &part, 1);
        }
    }

    /// A database object `unit` reaches: its own name — the last dotted
    /// segment, without quoting brackets — whole and in parts. The schema is
    /// no keyword: nearly every object shares it.
    pub(super) fn db_object(&mut self, unit: &str, object: &str) {
        let name = object
            .rsplit('.')
            .next()
            .unwrap_or(object)
            .trim_matches(|c| matches!(c, '[' | ']' | '"' | '`'))
            .to_lowercase();
        if is_word(&name) && name.contains('_') {
            self.add_word(unit, &name, 1);
        }
        self.identifier(unit, &name);
    }

    /// Prose of `unit` that occurs `times` times — a literal, a comment line, a
    /// doc comment. A whitespace-separated token shaped like a key or a hash is
    /// dropped whole before anything is split out of it (KTD11 rule 3).
    pub(super) fn text(&mut self, unit: &str, text: &str, times: u32) {
        for token in text.split_whitespace() {
            if is_long_token(token.trim_matches(|c: char| !c.is_ascii_alphanumeric())) {
                continue;
            }
            for part in split_identifier(token) {
                self.add(unit, &part, times);
            }
        }
    }

    fn add(&mut self, unit: &str, part: &str, times: u32) {
        if is_word(part) {
            self.add_word(unit, part, times);
        }
    }

    fn add_word(&mut self, unit: &str, word: &str, times: u32) {
        let id = match self.ids.get(word) {
            Some(&id) => id,
            None => {
                let id = u32::try_from(self.words.len()).unwrap_or(u32::MAX);
                self.words.push(word.into());
                self.ids.insert(word.into(), id);
                id
            }
        };
        let unit_terms = match self.terms.get_mut(unit) {
            Some(terms) => terms,
            None => self.terms.entry(unit.to_string()).or_default(),
        };
        *unit_terms.entry(id).or_default() += times;
    }

    /// Count each word's documents once, and weigh it.
    pub(super) fn finish(self) -> KeywordIndex {
        let documents = u32::try_from(self.terms.len()).unwrap_or(u32::MAX);
        let mut df: Vec<u32> = vec![0; self.words.len()];
        for unit_terms in self.terms.values() {
            for &id in unit_terms.keys() {
                df[id as usize] += 1;
            }
        }
        KeywordIndex {
            rarity: df.iter().map(|&d| rarity(documents, d)).collect(),
            words: self.words,
            terms: self
                .terms
                .into_iter()
                .map(|(unit, unit_terms)| (unit, unit_terms.into_iter().collect()))
                .collect(),
            ids: self.ids,
            documents,
        }
    }
}

/// The words a later layer brings to one unit (plan 2a, U6: KTD9 point 4),
/// read as the first layer reads its own: an identifier split into its parts,
/// prose split by token with every token shaped like a key dropped whole,
/// and every part kept only when it is a keyword at all. Ordered by word, so
/// what is added is the same whatever order it was read in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitWords {
    words: BTreeMap<String, u32>,
}

impl UnitWords {
    /// An identifier: a member's, a type's, a resource key's.
    pub fn identifier(&mut self, name: &str) {
        for part in split_identifier(name) {
            self.add(&part);
        }
    }

    /// Prose: a quoted label.
    pub fn text(&mut self, text: &str) {
        for token in text.split_whitespace() {
            if is_long_token(token.trim_matches(|c: char| !c.is_ascii_alphanumeric())) {
                continue;
            }
            for part in split_identifier(token) {
                self.add(&part);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    fn add(&mut self, part: &str) {
        if is_word(part) {
            *self.words.entry(part.to_string()).or_default() += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_split_at_case_acronyms_digits_and_separators() {
        assert_eq!(
            split_identifier("FindAllColumns"),
            ["find", "all", "columns"]
        );
        assert_eq!(
            split_identifier("ops_schedule_tasks"),
            ["ops", "schedule", "tasks"]
        );
        assert_eq!(split_identifier("XMLParser"), ["xml", "parser"]);
        assert_eq!(split_identifier("GeoSolver"), ["geo", "solver"]);
        assert_eq!(split_identifier("Base64Encode"), ["base", "64", "encode"]);
        assert_eq!(split_identifier("dba.ops_x"), ["dba", "ops", "x"]);
        assert_eq!(split_identifier("ObnovaZálohy"), ["obnova", "zálohy"]);
    }

    #[test]
    fn rarity_is_zero_for_a_word_in_every_document_and_grows_as_it_thins() {
        assert_eq!(rarity(10, 10), 0);
        assert_eq!(rarity(1, 1), 0);
        // 4 · log₂ 2 = 4; 4 · log₂ 10 ≈ 13.3.
        assert_eq!(rarity(10, 5), 4);
        assert_eq!(rarity(10, 1), 13);
        assert!(rarity(17_800, 1) > rarity(17_800, 2));
    }

    #[test]
    fn a_diacritic_word_brings_its_plain_variant_and_no_string_repeats() {
        let mut b = KeywordIndexBuilder::default();
        b.text("u", "Chyba při obnově zálohy", 1);
        b.text("w", "obnově obnove", 1);
        b.text("v", "jiný text", 1);
        let index = b.finish();
        let u = index.keywords("u");
        assert!(
            u.contains(&Keyword {
                word: "obnově".into(),
                folded: Some("obnove".into())
            }),
            "{u:?}"
        );
        // Both spellings occur in `w`; `obnove` is still listed once.
        let w = index.keywords("w");
        let spelled: Vec<&str> = w
            .iter()
            .flat_map(|k| std::iter::once(k.word.as_str()).chain(k.folded.as_deref()))
            .filter(|s| *s == "obnove")
            .collect();
        assert_eq!(spelled, ["obnove"], "{w:?}");
    }

    /// The cap counts words, not variants: twenty words, the rarest twelve
    /// kept, each with its plain spelling beside it where it has one.
    #[test]
    fn a_unit_keeps_at_most_the_cap_rarest_first() {
        let mut b = KeywordIndexBuilder::default();
        let words: Vec<String> = (0..20)
            .map(|i| format!("slovo{}ž", "a".repeat(i)))
            .collect();
        b.text("u", &words.join(" "), 1);
        // The last eight are shared with another unit, so they weigh less.
        b.text("v", &words[12..].join(" "), 1);
        let kept = b.finish().keywords("u");
        assert_eq!(kept.len(), KEYWORD_CAP);
        let mut expected = words[..12].to_vec();
        expected.sort();
        let got: Vec<String> = kept.iter().map(|k| k.word.clone()).collect();
        assert_eq!(got, expected);
        assert!(kept.iter().all(|k| k.folded.is_some()));
    }

    /// KTD9 point 4: a later layer's words weigh by the first layer's
    /// document frequencies — a word it alone brings as one in a single
    /// document — add to the unit's occurrences, and change no other unit;
    /// with none, the keywords are the first layer's.
    #[test]
    fn a_later_layers_words_weigh_by_the_first_layers_frequencies() {
        let mut b = KeywordIndexBuilder::default();
        b.text("u", "common shared", 1);
        b.text("v", "common shared other", 1);
        b.text("w", "common", 1);
        let index = b.finish();
        assert_eq!(
            index.keywords_with("u", &UnitWords::default()),
            index.keywords("u")
        );

        let mut extra = UnitWords::default();
        extra.identifier("ParserCommon");
        extra.text("Kontrola chyb x");
        let words: Vec<String> = index
            .keywords_with("u", &extra)
            .iter()
            .map(|k| k.word.clone())
            .collect();
        // `parser`, `kontrola` and `chyb` are in no document: df 1, the rarest.
        // `shared` weighs by its two documents of three; `common`, in all
        // three, weighs 0 however often it occurs.
        assert_eq!(words, ["chyb", "kontrola", "parser", "shared", "common"]);
        assert_eq!(rarity(3, 1), 6);
        assert_eq!(
            index.keywords("v"),
            index.keywords_with("v", &UnitWords::default())
        );
        // A unit with no word of its own gets the words alone.
        let alone: Vec<String> = index
            .keywords_with("nothing", &extra)
            .iter()
            .map(|k| k.word.clone())
            .collect();
        assert_eq!(alone, ["chyb", "kontrola", "parser", "common"]);
    }
}
