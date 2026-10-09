//! Enriched leaf rendering (T1-KTD4). The plain Tier 0 leaf is preserved intact;
//! an `## Interpretation` section, explicitly marked as LLM-narrated (R16), is
//! inserted after the guiding question.

use crate::enrich::Narrative;
use crate::lens::Lens;
use crate::render::markdown::render_leaf;

/// Render an enriched leaf: the Tier 0 leaf with an LLM-narrated Interpretation
/// section spliced in after the guiding-question line.
pub fn render_enriched_leaf(lens: &Lens, narrative: &Narrative) -> String {
    let base = render_leaf(lens);
    let block = format!(
        "## Interpretation\n\n\
         > _LLM-narrated ({backend}, prompt {ver}, lang {lang}) — interpretation around \
         the verified facts below, not a measured fact._\n\n{text}\n\n",
        backend = narrative.backend_id,
        ver = narrative.prompt_version,
        lang = narrative.lang,
        text = narrative.text,
    );

    // Splice after the guiding-question paragraph (the first blank line following
    // the "**Guiding question:**" line). Falls back to prepending after the title
    // if the marker is absent.
    match base.find("**Guiding question:**") {
        Some(gq) => {
            let after_gq = base[gq..]
                .find("\n\n")
                .map(|off| gq + off + 2)
                .unwrap_or(base.len());
            let mut out = String::with_capacity(base.len() + block.len());
            out.push_str(&base[..after_gq]);
            out.push_str(&block);
            out.push_str(&base[after_gq..]);
            out
        }
        None => format!("{block}{base}"),
    }
}

/// Render a degraded leaf (T1-KTD5 / E9): the Tier 0 leaf with an explicit,
/// PERSISTED marker where the Interpretation would sit, so a reader of the
/// enriched corpus can tell an un-narrated leaf from a narrated one and see why —
/// the run report on stderr is ephemeral; this is the durable "coverage as a
/// displayed fact" signal. Deliberately NOT an `## Interpretation` heading, so it
/// never reads as narrated content.
pub fn render_degraded_leaf(lens: &Lens, reason: &str) -> String {
    let base = render_leaf(lens);
    let block = format!(
        "> _No LLM interpretation for this view: {}._\n\n",
        sanitize_reason(reason)
    );
    match base.find("**Guiding question:**") {
        Some(gq) => {
            let after_gq = base[gq..]
                .find("\n\n")
                .map(|off| gq + off + 2)
                .unwrap_or(base.len());
            let mut out = String::with_capacity(base.len() + block.len());
            out.push_str(&base[..after_gq]);
            out.push_str(&block);
            out.push_str(&base[after_gq..]);
            out
        }
        None => format!("{block}{base}"),
    }
}

/// Sanitize a degradation reason before it goes into corpus markdown. The reason
/// can be untrusted (e.g. a claude-cli subprocess's raw stderr): a newline would
/// break out of the `> _..._` blockquote and inject arbitrary markdown — including
/// a `## Interpretation` heading that would violate the "degraded ≠ narrated"
/// invariant. Collapse all whitespace to single spaces, neutralize backticks, and
/// cap the length.
pub fn sanitize_reason(reason: &str) -> String {
    let collapsed = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let deticked = collapsed.replace('`', "'");
    deticked.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::Narrative;
    use crate::lens::Lens;

    fn lens() -> Lens {
        Lens {
            view_id: "overview".into(),
            title: "Overview".into(),
            guiding_question: "What is this repo?".into(),
            problem: "authored problem".into(),
            mechanism: "authored mechanism".into(),
            sections: vec![crate::lens::Section::undivided(vec![
                crate::taxonomy::views::Instance::for_test("The `core` module holds 12 files."),
            ])],
            diagram: None,
            failure_modes: vec![],
        }
    }

    fn narrative() -> Narrative {
        Narrative {
            view_id: "overview".into(),
            text: "The `core` module is the heart of the repository.".into(),
            backend_id: "fake".into(),
            prompt_version: "t1-narrative-v1".into(),
            lang: "en".into(),
            backed_by: vec!["The `core` module holds 12 files.".into()],
        }
    }

    #[test]
    fn enriched_leaf_carries_interpretation_and_the_narrated_marker() {
        let out = render_enriched_leaf(&lens(), &narrative());
        assert!(out.contains("## Interpretation"));
        assert!(out.contains("The `core` module is the heart of the repository."));
        // The LLM-narrated marker keeps a reader from mistaking it for a fact (R16).
        assert!(out.to_lowercase().contains("llm"));
        assert!(out.contains("fake"));
    }

    #[test]
    fn enriched_leaf_still_contains_the_deterministic_facts() {
        let out = render_enriched_leaf(&lens(), &narrative());
        assert!(out.contains("The `core` module holds 12 files."));
        assert!(out.contains("## In this repository"));
    }

    #[test]
    fn degraded_leaf_carries_a_visible_marker_and_no_interpretation() {
        let out = render_degraded_leaf(&lens(), "narrative rejected by guardrail after retries");
        // The un-narrated state is a durable, visible fact in the corpus (E9)...
        assert!(out.contains("No LLM interpretation"));
        assert!(out.contains("narrative rejected by guardrail after retries"));
        // ...but it is NOT an Interpretation section, so it never reads as narrated.
        assert!(!out.contains("## Interpretation"));
        // The deterministic facts are still present.
        assert!(out.contains("The `core` module holds 12 files."));
    }
}
