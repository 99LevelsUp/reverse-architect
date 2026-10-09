//! The per-leaf fact bundle handed to the LLM, and the licensed token set the
//! guardrail enforces (T1-KTD3, T1-KTD5). Both are derived deterministically
//! from a composed `Lens`.

use crate::graph::Confidence;
use crate::lens::Lens;
use crate::taxonomy::views::Instance;
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// The facts of one leaf, plus its framing, as handed to the backend.
pub struct FactBundle {
    pub title: String,
    pub guiding_question: String,
    pub facts: Vec<String>,
}

impl FactBundle {
    pub fn from_lens(lens: &Lens) -> Self {
        FactBundle {
            title: lens.title.clone(),
            guiding_question: lens.guiding_question.clone(),
            // Flat across sections: the prompt sees the same fact list whether
            // or not the view's render was cut into sections (U3).
            facts: narrated(lens).map(|i| i.text.clone()).collect(),
        }
    }
}

/// The instances the model is shown: every one but those resting on an
/// ambiguous relation (plan A, KTD13).
///
/// A guess the leaf lists apart is still a guess, and a narrative is read as
/// prose, where no marker survives — so it is left out of the facts rather than
/// handed over with a caveat the model could drop. The one filter both
/// [`FactBundle::from_lens`] and [`licensed_set`] read, so the guardrail
/// licenses exactly the text the prompt carries (Fix D).
fn narrated(lens: &Lens) -> impl Iterator<Item = &Instance> {
    lens.instances()
        .filter(|i| i.rung != Some(Confidence::Ambiguous))
}

/// Everything the narrative is *allowed* to mention (T1-KTD5).
#[derive(Debug, Default)]
pub struct LicensedSet {
    /// Numeric literals in canonical form (percent and thousands separators
    /// stripped, comma-decimals folded to `.`, sign and exponent preserved), e.g.
    /// "12", "24", "-5", "1e9", "1.5" (from a licensed `1,5` too). See
    /// [`normalize_number`].
    pub numbers: BTreeSet<String>,
    /// Code-shaped identifiers: the union of backtick-quoted spans and bare
    /// code-shaped tokens (see `extract_identifiers`) in the leaf text.
    pub identifiers: BTreeSet<String>,
}

fn number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Hex/binary/octal integer literals are matched as WHOLE tokens and are
    // listed BEFORE the decimal alternative so `0xff` is captured whole rather
    // than being cut down to a bare "0" by the decimal branch (regex
    // alternation is leftmost-first, not longest-match). The decimal branch
    // covers optional sign, mantissa with thousands separators and optional
    // fraction, optional exponent; the sign is post-filtered in
    // `extract_numbers` so a hyphen joining two tokens (a range like `12-15`)
    // is not read as a sign. The trailing `\.\d+` branch covers leading-dot
    // decimals (`.5`) that the decimal branch cannot start on.
    RE.get_or_init(|| {
        Regex::new(
            r"0[xX][0-9a-fA-F]+|0[bB][01]+|0[oO][0-7]+|[+-]?\d[\d,]*(?:\.\d+)?(?:[eE][+-]?\d+)?|\.\d+",
        )
        .unwrap()
    })
}

fn backtick_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`([^`]+)`").unwrap())
}

/// A candidate code-shaped token: a run of identifier-ish characters. The `.`/`:`
/// are included so `Cargo.toml` and `std::collections` survive as one token;
/// `@`/`/` are included so a scoped npm-style package (`@scope/pkg`) survives as
/// one token instead of decomposing into lowercase prose fragments; `-` is
/// included so a whole hyphenated token (kebab or prose) is captured as one unit
/// for `is_code_shaped` to classify. Leading/trailing `.`/`:` (sentence
/// punctuation) are trimmed by the caller.
fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9_@][A-Za-z0-9_.:@/-]*").unwrap())
}

/// File extensions that mark a bare token as code-shaped (a file name).
const CODE_EXTENSIONS: &[&str] = &[
    ".rs", ".ts", ".tsx", ".js", ".py", ".go", ".cs", ".toml", ".json", ".md",
];

/// Normalize a numeric literal for comparison. Drops `%`; preserves sign and
/// exponent. Commas are locale-ambiguous, so they are classified deterministically
/// rather than blindly stripped (which would systematically degrade decimals in
/// comma-decimal locales, undercutting `--lang cs`/EU output, T1-KTD10):
///
/// - A **single** comma followed by exactly one or two digits at the very end of
///   the number is a **decimal** separator (`1,5` → `1.5`, `12,34` → `12.34`) — the
///   cs/EU comma-decimal form; it becomes `.` so it matches a licensed `1.5`.
/// - Any other comma is a **thousands** separator and is stripped (`1,234` → `1234`,
///   `1,234,567` → `1234567`, `1,234.5` → `1234.5`).
///
/// The 3-digit-group heuristic makes `1,234` mean 1234, not 1.234 — genuinely
/// ambiguous, but the deliberate choice (per plan): a comma with a 3-digit tail is
/// far more likely a thousands group than a 3-place decimal in these corpora.
/// Commas only ever appear in the integer part of the mantissa (the number regex
/// puts `[\d,]*` before the `.` fraction), so only that part is inspected.
pub fn normalize_number(raw: &str) -> String {
    let s = raw.trim_end_matches('%');
    if !s.contains(',') {
        return s.to_string();
    }
    // Split off any fraction/exponent tail (never contains a comma).
    let (int_part, tail) = match s.find(['.', 'e', 'E']) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let is_decimal_comma = tail.is_empty()
        && int_part.matches(',').count() == 1
        && int_part.rsplit(',').next().is_some_and(|after| {
            (1..=2).contains(&after.len()) && after.bytes().all(|b| b.is_ascii_digit())
        });
    if is_decimal_comma {
        // Exactly one comma, so replace is unambiguous; tail is empty here.
        int_part.replace(',', ".")
    } else {
        format!("{}{tail}", int_part.replace(',', ""))
    }
}

/// Extract the canonical numeric literals from `text` (T1-KTD5). Shared by
/// licensing (`licensed_set`) and checking (`guardrail::check`) so the two can
/// never drift. Backtick spans are masked out first, so identifier-internal
/// digits (`base64`, `sha256`, `x86_64`) never enter the number set — otherwise a
/// fabricated count could reuse "64" and slip past the guardrail. A leading sign
/// is honored only when the character immediately before the match is neither a
/// letter nor a digit; a hyphen joining two tokens (a range like `12-15`, or
/// `word-5`) is therefore not misread as a sign.
pub(crate) fn extract_numbers(text: &str) -> BTreeSet<String> {
    // Normalize the Unicode minus sign (U+2212) to ASCII '-' first, so `−5` is
    // treated identically to `-5` instead of smuggling a bare "5" past a
    // licensed-set check keyed on the signed literal.
    let normalized = text.replace('\u{2212}', "-");
    let masked = backtick_re().replace_all(&normalized, " ");
    let mut set = BTreeSet::new();
    for m in number_re().find_iter(&masked) {
        let raw = m.as_str();
        let keep_sign = match masked[..m.start()].chars().next_back() {
            Some(c) => !c.is_alphanumeric(),
            None => true,
        };
        let canonical = if !keep_sign && (raw.starts_with('+') || raw.starts_with('-')) {
            &raw[1..]
        } else {
            raw
        };
        set.insert(normalize_number(canonical));
    }
    set
}

/// Extract code-shaped identifiers from `text` (T1-KTD5). Shared by licensing and
/// checking so the two can never drift. The result is the UNION of:
/// (1) every backtick-quoted span, and
/// (2) every bare "code-shaped" token — one that contains `_`, contains `::`, is
///     CamelCase, or ends in a known code extension.
/// Ordinary lowercase natural-language words (e.g. "tokio") are deliberately NOT
/// code-shaped: they cannot be told from prose without a dictionary, so a bare
/// lowercase name is an accepted, documented residual governed by the prompt and
/// the LLM-narrated marker rather than this structural check.
pub(crate) fn extract_identifiers(text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    // (1) backtick spans, verbatim.
    for c in backtick_re().captures_iter(text) {
        set.insert(c[1].to_string());
    }
    // (2) bare code-shaped tokens, scanned over the backtick-masked text so a
    //     span already captured above is not re-tokenized.
    let masked = backtick_re().replace_all(text, " ");
    for m in token_re().find_iter(&masked) {
        let tok = m.as_str().trim_matches(|ch| ch == '.' || ch == ':');
        if !tok.is_empty() && is_code_shaped(tok) {
            set.insert(tok.to_string());
        }
    }
    set
}

/// A bare token is code-shaped when it structurally cannot be an ordinary word.
/// A lone hyphen is deliberately NOT sufficient (bare kebab like `fake-module`,
/// and ordinary hyphenated prose like `read-only`/`language-agnostic`, must stay
/// unflagged); only the combination of `@` AND `/` — the shape of a scoped
/// package name (`@scope/pkg`) — is structural enough to flag on its own.
fn is_code_shaped(tok: &str) -> bool {
    tok.contains('_')
        || tok.contains("::")
        || is_camel_case(tok)
        || CODE_EXTENSIONS.iter().any(|ext| tok.ends_with(ext))
        || (tok.contains('@') && tok.contains('/'))
}

/// CamelCase: an internal lowercase→uppercase boundary (`FooBar`, `FakeThing`) or
/// a multi-capital→lowercase boundary (`HTTPServer`). A sentence-initial capital
/// (`This`) has neither, so ordinary prose is never flagged.
fn is_camel_case(tok: &str) -> bool {
    let chars: Vec<char> = tok.chars().collect();
    chars
        .windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
        || chars.windows(3).any(|w| {
            w[0].is_ascii_uppercase() && w[1].is_ascii_uppercase() && w[2].is_ascii_lowercase()
        })
}

/// Derive the licensed set from exactly the deterministic text the model is shown
/// (Fix D): the leaf title, guiding question, and instances — the same fields
/// `FactBundle::from_lens` puts in the prompt. Authored `problem`/`mechanism`/
/// `failure_modes` text is NOT part of the prompt, so a number appearing only
/// there must not license a narrative number. A narrative number/identifier
/// absent from this licensed set is a fabrication (T1-KTD5).
pub fn licensed_set(lens: &Lens) -> LicensedSet {
    let mut source = String::new();
    source.push_str(&lens.title);
    source.push(' ');
    source.push_str(&lens.guiding_question);
    for i in narrated(lens) {
        source.push(' ');
        source.push_str(&i.text);
    }

    LicensedSet {
        numbers: extract_numbers(&source),
        identifiers: extract_identifiers(&source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lens::Lens;

    fn lens_with(instances: &[&str]) -> Lens {
        Lens {
            view_id: "overview".into(),
            title: "Overview".into(),
            guiding_question: "What is this repo?".into(),
            problem: "authored problem".into(),
            mechanism: "authored mechanism".into(),
            sections: vec![crate::lens::Section::undivided(
                instances
                    .iter()
                    .map(|t| crate::taxonomy::views::Instance::for_test(*t))
                    .collect(),
            )],
            diagram: None,
            failure_modes: vec![],
        }
    }

    /// U3: sectioning groups a view for *rendering*, so the prompt must be
    /// blind to it — a lens whose instances were cut into sections hands over
    /// the same flat fact list, in the same order, as the undivided one. If this
    /// ever diverges, the narrative's licensed set changes with the layout.
    #[test]
    fn a_sectioned_lens_yields_the_same_flat_fact_bundle() {
        let flat = lens_with(&["`a` — 1 file", "`b` — 2 files", "`c` — 3 files"]);
        let mut sectioned = flat.clone();
        let all: Vec<_> = flat.instances().cloned().collect();
        sectioned.sections = vec![
            crate::lens::Section {
                heading: Some("first".into()),
                instances: all[..1].to_vec(),
            },
            crate::lens::Section {
                heading: Some("second".into()),
                instances: all[1..].to_vec(),
            },
        ];

        assert_eq!(
            FactBundle::from_lens(&flat).facts,
            FactBundle::from_lens(&sectioned).facts
        );
        assert_eq!(
            licensed_set(&flat).numbers,
            licensed_set(&sectioned).numbers
        );
    }

    #[test]
    fn licensed_numbers_come_from_instance_text() {
        let lens = lens_with(&["The module `core` holds 12 files."]);
        let set = licensed_set(&lens);
        assert!(set.numbers.contains("12"));
        assert!(!set.numbers.contains("9999"));
    }

    #[test]
    fn licensed_identifiers_are_backtick_spans() {
        let lens = lens_with(&["The module `core` imports `serde_json`."]);
        let set = licensed_set(&lens);
        assert!(set.identifiers.contains("core"));
        assert!(set.identifiers.contains("serde_json"));
        assert!(!set.identifiers.contains("nonexistent"));
    }

    #[test]
    fn percent_is_stripped_when_licensing_numbers() {
        let lens = lens_with(&["Coverage by convention is 24%."]);
        let set = licensed_set(&lens);
        assert!(set.numbers.contains("24"));
    }

    #[test]
    fn licensed_source_excludes_authored_problem_and_mechanism() {
        // Fix D: the licensed source is exactly what the model is shown (title +
        // guiding question + instances). A number that appears ONLY in authored
        // problem/mechanism text is not licensed, so a narrative asserting it is
        // rejected downstream.
        let mut lens = lens_with(&["The module `core` holds 12 files."]);
        lens.problem = "Historically the team fixed 4242 issues.".into();
        lens.mechanism = "The pipeline runs across 7777 stages.".into();
        let set = licensed_set(&lens);
        assert!(set.numbers.contains("12"));
        assert!(!set.numbers.contains("4242"));
        assert!(!set.numbers.contains("7777"));
    }

    #[test]
    fn comma_decimal_normalizes_to_dot() {
        // cs/EU comma-decimal: a comma with a 1–2 digit tail is a decimal point.
        assert_eq!(normalize_number("1,5"), "1.5");
        assert_eq!(normalize_number("12,34"), "12.34");
        assert_eq!(normalize_number("-1,5"), "-1.5");
    }

    #[test]
    fn thousands_comma_is_stripped() {
        // A 3-digit group (or multiple commas, or a following fraction) is thousands.
        assert_eq!(normalize_number("1,234"), "1234");
        assert_eq!(normalize_number("1,234,567"), "1234567");
        assert_eq!(normalize_number("1,234.5"), "1234.5");
    }

    #[test]
    fn comma_free_numbers_pass_through() {
        assert_eq!(normalize_number("12"), "12");
        assert_eq!(normalize_number("24%"), "24");
        assert_eq!(normalize_number("1e9"), "1e9");
        assert_eq!(normalize_number("0xff"), "0xff");
        assert_eq!(normalize_number(".5"), ".5");
    }

    #[test]
    fn comma_decimal_narrative_matches_dot_licensed_number() {
        // The bug this closes: a leaf fact stating `1.5` and a cs narrative writing
        // `1,5` must license/canonicalize identically, so the guardrail accepts it
        // instead of degrading the leaf (T1-KTD10, --lang cs).
        let lens = lens_with(&["The ratio is 1.5 across modules."]);
        let set = licensed_set(&lens);
        assert!(set.numbers.contains("1.5"));
        // A narrative token rendered comma-decimal normalizes to the same canonical.
        assert!(set.numbers.contains(&normalize_number("1,5")));
    }

    #[test]
    fn bundle_carries_facts_and_question() {
        let lens = lens_with(&["fact one", "fact two"]);
        let bundle = FactBundle::from_lens(&lens);
        assert_eq!(bundle.guiding_question, "What is this repo?");
        assert_eq!(bundle.title, "Overview");
        assert_eq!(bundle.facts.len(), 2);
    }

    /// Plan A, U2 (KTD13): an ambiguous relation reaches neither the backend
    /// request nor the guardrail's licensed set. The rung travels from the edge
    /// into the lens instance where the view assembles it, and the enrich layer
    /// filters on it — while the leaf itself still lists the relation.
    #[test]
    fn an_ambiguous_relation_reaches_neither_the_prompt_nor_the_licensed_set() {
        use crate::graph::{
            Edge, EdgeKind, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Rule,
            Tier,
        };
        let prov = || Provenance::new("Cargo.toml", Some(1), "test".into(), Tier::Measured);
        let mut g = Graph::new();
        let manifest = NodeKey::new("Cargo.toml");
        g.ensure_node(manifest.clone(), NodeKind::File).unwrap();
        for (name, provenance) in [
            ("settled_dep", prov()),
            (
                "guessed_dep",
                prov()
                    .rated(Confidence::Ambiguous, Rule::NameMatch)
                    .ambiguous(2),
            ),
        ] {
            let key = NodeKey::new(format!("dep:cargo:{name}"));
            g.ensure_node(key.clone(), NodeKind::Dependency).unwrap();
            g.add_fact(
                &key,
                Fact::new(
                    FactKind::DependencyName,
                    FactValue::Text(name.into()),
                    prov(),
                ),
            );
            g.add_edge(&manifest, &key, Edge::new(EdgeKind::DependsOn, provenance));
        }
        let view = crate::taxonomy::views::backbone_catalog()
            .into_iter()
            .find(|v| v.id == "dependencies")
            .unwrap();
        let lens = crate::lens::compose(&view, &g);

        let listed: Vec<(&str, Option<Confidence>)> = lens
            .instances()
            .map(|i| (i.text.as_str(), i.rung))
            .collect();
        assert!(
            listed
                .iter()
                .any(|(t, r)| t.contains("guessed_dep") && *r == Some(Confidence::Ambiguous)),
            "the leaf still lists the relation, carrying its rung: {listed:?}"
        );

        let prompt = crate::enrich::prompt::build_prompt(&FactBundle::from_lens(&lens), "en", None);
        assert!(prompt.contains("settled_dep"), "{prompt}");
        assert!(!prompt.contains("guessed_dep"), "{prompt}");

        let licensed = licensed_set(&lens);
        assert!(licensed.identifiers.contains("settled_dep"));
        assert!(
            !licensed.identifiers.contains("guessed_dep"),
            "{:?}",
            licensed.identifiers
        );
    }
}
