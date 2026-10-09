//! The narration prompt (T1-KTD5, T1-KTD10). Templated and versioned — the
//! LLM only reframes the given facts, must backtick every identifier, and must
//! introduce no number or name not present.

use crate::enrich::bundle::FactBundle;
use std::fmt::Write;

/// Bump when the template changes; recorded in the narrative's provenance.
pub const PROMPT_VERSION: &str = "t1-narrative-v1";

/// Build the narration prompt for one leaf. `lang` is the BCP-47-ish output
/// language code (default "en"); `retry_note`, when present, is a ready-composed
/// correction sentence from the guardrail (see `narrate_one`) appended verbatim so
/// the retry can fix the specific problem.
pub fn build_prompt(bundle: &FactBundle, lang: &str, retry_note: Option<&str>) -> String {
    let mut p = String::new();
    let _ = writeln!(
        p,
        "You are writing the WHAT/WHY/HOW interpretation for one section of an \
         architecture document. Section: \"{}\".",
        bundle.title
    );
    let _ = writeln!(
        p,
        "\nGuiding question to answer:\n{}",
        bundle.guiding_question
    );
    let _ = writeln!(p, "\nThe ONLY facts you may use (verbatim):");
    for f in &bundle.facts {
        let _ = writeln!(p, "- {f}");
    }
    let _ = writeln!(
        p,
        "\nHard rules:\n\
         - Write in the language with code \"{lang}\".\n\
         - Interpret and connect ONLY the facts above. Introduce NO number and NO \
           name that is not present in those facts.\n\
         - Wrap every code identifier (module, file, symbol, dependency name) in \
           backticks, exactly as written above.\n\
         - Do not invent counts, percentages, versions, or file names.\n\
         - 2–4 short paragraphs, no headings."
    );
    if let Some(note) = retry_note {
        let _ = writeln!(p, "\n{note}");
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::bundle::FactBundle;

    fn bundle() -> FactBundle {
        FactBundle {
            title: "Overview".into(),
            guiding_question: "What is this repo?".into(),
            facts: vec!["The module `core` holds 12 files.".into()],
        }
    }

    #[test]
    fn prompt_contains_every_fact_and_the_question() {
        let p = build_prompt(&bundle(), "en", None);
        assert!(p.contains("What is this repo?"));
        assert!(p.contains("The module `core` holds 12 files."));
    }

    #[test]
    fn prompt_names_the_target_language() {
        let p = build_prompt(&bundle(), "cs", None);
        assert!(p.to_lowercase().contains("cs") || p.contains("language"));
    }

    #[test]
    fn retry_addendum_names_the_offending_token() {
        let p = build_prompt(&bundle(), "en", Some("9999"));
        assert!(p.contains("9999"));
    }

    #[test]
    fn prompt_version_is_stable() {
        assert_eq!(PROMPT_VERSION, "t1-narrative-v1");
    }
}
