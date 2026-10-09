//! The structural anti-hallucination guardrail (T1-KTD5). It rejects fabricated
//! numeric literals (including signed and exponent forms) and fabricated
//! code-shaped or backtick-quoted identifiers not present in the leaf's licensed
//! set. Token extraction is shared with `bundle::licensed_set` (via
//! `extract_numbers`/`extract_identifiers`), so licensing and checking can never
//! drift. It never inspects prose vocabulary, so it is language-agnostic
//! (T1-KTD10); a bare natural-language-shaped name (e.g. "tokio") is a documented
//! residual, governed by the prompt and the LLM-narrated marker rather than this
//! structural check.

use crate::enrich::bundle::{extract_identifiers, extract_numbers, LicensedSet};
use regex::Regex;
use std::sync::OnceLock;

/// The outcome of checking one candidate narrative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// An unlicensed token (numbers checked before identifiers), or the sentinel
    /// `<markdown-heading>`; named so the retry prompt can cite it.
    Rejected {
        token: String,
    },
}

fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*#{1,6}\s").unwrap())
}

/// Check a candidate narrative against the licensed set. Returns an unlicensed
/// token (numbers checked before identifiers), the `<markdown-heading>` sentinel
/// if the narrative contains a markdown heading (the prompt forbids them, and one
/// would pollute the rendered `## Interpretation` block), or `Ok`.
pub fn check(narrative: &str, licensed: &LicensedSet) -> Verdict {
    for line in narrative.lines() {
        if heading_re().is_match(line) {
            return Verdict::Rejected {
                token: "<markdown-heading>".into(),
            };
        }
    }
    // Same extraction bundle::licensed_set uses — licensing and checking are
    // symmetric by construction.
    for n in extract_numbers(narrative) {
        if !licensed.numbers.contains(&n) {
            return Verdict::Rejected { token: n };
        }
    }
    for ident in extract_identifiers(narrative) {
        if !licensed.identifiers.contains(&ident) {
            return Verdict::Rejected { token: ident };
        }
    }
    Verdict::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::bundle::LicensedSet;

    fn licensed() -> LicensedSet {
        let mut s = LicensedSet::default();
        s.numbers.insert("12".into());
        s.identifiers.insert("core".into());
        s.identifiers.insert("serde_json".into());
        s
    }

    #[test]
    fn narrative_using_only_licensed_tokens_passes() {
        let narrative = "The `core` module, with 12 files, leans on `serde_json`.";
        assert_eq!(check(narrative, &licensed()), Verdict::Ok);
    }

    #[test]
    fn fabricated_number_is_rejected() {
        let narrative = "The `core` module holds 9999 files.";
        assert_eq!(
            check(narrative, &licensed()),
            Verdict::Rejected {
                token: "9999".into()
            }
        );
    }

    #[test]
    fn fabricated_identifier_is_rejected() {
        let narrative = "The `core` module wraps `tokio` internally.";
        assert_eq!(
            check(narrative, &licensed()),
            Verdict::Rejected {
                token: "tokio".into()
            }
        );
    }

    #[test]
    fn ordinary_prose_without_backticks_is_never_flagged() {
        // Plain English words (no backticks, no digits) are not candidates.
        let narrative = "This repository is small and cohesive overall.";
        assert_eq!(check(narrative, &licensed()), Verdict::Ok);
    }

    #[test]
    fn non_english_prose_with_licensed_tokens_passes() {
        // T1-KTD10: guardrail is language-agnostic — only tokens are checked.
        let narrative = "Modul `core` obsahuje 12 souborů a používá `serde_json`.";
        assert_eq!(check(narrative, &licensed()), Verdict::Ok);
    }

    #[test]
    fn comma_decimal_narrative_passes_against_dot_licensed_number() {
        // T1-KTD10 (--lang cs): a leaf fact stating `1.5` licenses the canonical
        // "1.5"; a Czech narrative rendering it as `1,5` must be accepted, not
        // degraded. Before the locale-aware normalize, `1,5` folded to "15" and
        // was wrongly rejected as a fabrication.
        let mut licensed = LicensedSet::default();
        licensed.numbers.insert("1.5".into());
        licensed.identifiers.insert("core".into());
        let narrative = "Modul `core` má poměr 1,5 napříč soubory.";
        assert_eq!(check(narrative, &licensed), Verdict::Ok);
    }

    #[test]
    fn fabricated_comma_decimal_is_still_rejected() {
        // The fix must not blunt the guardrail: an unlicensed comma-decimal is
        // canonicalized and still rejected on its canonical form.
        let mut licensed = LicensedSet::default();
        licensed.numbers.insert("1.5".into());
        let narrative = "Poměr je ve skutečnosti 2,5.";
        assert_eq!(
            check(narrative, &licensed),
            Verdict::Rejected {
                token: "2.5".into()
            }
        );
    }

    #[test]
    fn identifier_digits_never_license_a_fabricated_number() {
        // Regression: a leaf mentioning `base64` licenses the identifier "base64",
        // but the digits inside it must never license an invented number like 64.
        let mut s = LicensedSet::default();
        s.identifiers.insert("base64".into());
        let narrative = "The module uses `base64` and touches 64 files.";
        assert_eq!(
            check(narrative, &s),
            Verdict::Rejected { token: "64".into() }
        );
    }

    /// Build a licensed set the way production does — from a real leaf — so these
    /// tests prove licensing and checking use the SAME extraction (symmetry).
    fn lens_licensing(instances: &[&str]) -> LicensedSet {
        use crate::lens::Lens;
        let lens = Lens {
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
        };
        crate::enrich::bundle::licensed_set(&lens)
    }

    // ---- Fix A: signed / exponent numeric literals ----

    #[test]
    fn signed_number_is_rejected_when_only_unsigned_is_licensed() {
        // A leaf licensing "5" must NOT accept a narrative asserting "-5".
        let licensed = lens_licensing(&["The net change is 5 files."]);
        assert_eq!(
            check("The net change is -5 files.", &licensed),
            Verdict::Rejected { token: "-5".into() }
        );
    }

    #[test]
    fn exponent_number_is_rejected_when_only_mantissa_parts_licensed() {
        // A leaf licensing "1" and "9" must NOT accept a narrative asserting "1e9".
        let licensed = lens_licensing(&["It has 1 core and 9 files."]);
        assert_eq!(
            check("It scales to 1e9 requests.", &licensed),
            Verdict::Rejected {
                token: "1e9".into()
            }
        );
    }

    #[test]
    fn exponent_number_is_accepted_when_genuinely_licensed() {
        let licensed = lens_licensing(&["It scales to 1e9 requests."]);
        assert_eq!(
            check("Handles up to 1e9 operations.", &licensed),
            Verdict::Ok
        );
    }

    #[test]
    fn plain_range_hyphen_is_not_treated_as_a_sign() {
        // A range "12-15" over a leaf licensing 12 and 15 must not spuriously
        // reject: the hyphen is not a sign because a digit precedes it.
        let licensed = lens_licensing(&["Between 12 and 15 files."]);
        assert_eq!(check("A range of 12-15 files.", &licensed), Verdict::Ok);
    }

    // ---- Fix B: bare (unbackticked) code-shaped identifiers ----

    #[test]
    fn bare_licensed_code_identifier_is_accepted() {
        // `serde_json` backticked in the leaf licenses the bare form too.
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        assert_eq!(
            check("The parser leans on serde_json heavily.", &licensed),
            Verdict::Ok
        );
    }

    #[test]
    fn bare_fabricated_code_identifier_is_rejected() {
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        assert_eq!(
            check("It calls fake_module directly.", &licensed),
            Verdict::Rejected {
                token: "fake_module".into()
            }
        );
    }

    #[test]
    fn lowercase_natural_word_is_accepted_residual() {
        // "tokio" is lowercase and not code-shaped: an accepted, documented
        // residual — the structural check cannot catch it without a dictionary.
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        assert_eq!(
            check("This wraps tokio internally.", &licensed),
            Verdict::Ok
        );
    }

    #[test]
    fn fabricated_camelcase_identifier_is_rejected() {
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        assert_eq!(
            check("It instantiates FakeThing at startup.", &licensed),
            Verdict::Rejected {
                token: "FakeThing".into()
            }
        );
    }

    // ---- Fix F: numeric alternate spellings (hex/bin/octal, leading-dot, unicode-minus) ----

    #[test]
    fn hex_literal_is_rejected_when_only_leading_digit_is_licensed() {
        // A leaf licensing "0" must NOT accept a narrative asserting "0xff":
        // the decimal branch alone would cut "0xff" down to a bare "0".
        let licensed = lens_licensing(&["It starts at 0 errors."]);
        assert_eq!(
            check("It jumps to 0xff errors.", &licensed),
            Verdict::Rejected {
                token: "0xff".into()
            }
        );
    }

    #[test]
    fn hex_literal_is_accepted_when_genuinely_licensed() {
        let licensed = lens_licensing(&["It caps out at 0xff errors."]);
        assert_eq!(check("It reports 0xff errors.", &licensed), Verdict::Ok);
    }

    #[test]
    fn leading_dot_decimal_is_rejected_when_only_integer_part_is_licensed() {
        // A leaf licensing "5" must NOT accept a narrative asserting ".5".
        let licensed = lens_licensing(&["The build takes 5 seconds."]);
        assert_eq!(
            check("Startup adds .5 seconds.", &licensed),
            Verdict::Rejected { token: ".5".into() }
        );
    }

    #[test]
    fn unicode_minus_number_is_rejected_when_only_unsigned_is_licensed() {
        // A leaf licensing "5" must NOT accept a narrative asserting "−5" (U+2212).
        let licensed = lens_licensing(&["The net change is 5 files."]);
        assert_eq!(
            check("The net change is \u{2212}5 files.", &licensed),
            Verdict::Rejected { token: "-5".into() }
        );
    }

    // ---- Fix G: scoped-package identifiers ----

    #[test]
    fn fabricated_scoped_package_identifier_is_rejected() {
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        assert_eq!(
            check("It secretly depends on @scope/pkg.", &licensed),
            Verdict::Rejected {
                token: "@scope/pkg".into()
            }
        );
    }

    #[test]
    fn licensed_scoped_package_identifier_is_accepted() {
        let licensed = lens_licensing(&["Depends on @scope/pkg for utils."]);
        assert_eq!(check("It depends on @scope/pkg.", &licensed), Verdict::Ok);
    }

    #[test]
    fn bare_kebab_prose_is_not_flagged_as_code_shaped() {
        // Regression guard: `-` alone must never trigger the code-shaped rule,
        // or ordinary hyphenated prose (and bare kebab identifiers) would be
        // wrongly rejected.
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        assert_eq!(
            check("This API is read-only for safety.", &licensed),
            Verdict::Ok
        );
    }

    // ---- Fix E: markdown headings ----

    #[test]
    fn markdown_heading_line_is_rejected() {
        let licensed = lens_licensing(&["Parsing uses `serde_json`."]);
        let narrative = "An intro paragraph.\n## Sneaky\nMore prose.";
        assert_eq!(
            check(narrative, &licensed),
            Verdict::Rejected {
                token: "<markdown-heading>".into()
            }
        );
    }
}
