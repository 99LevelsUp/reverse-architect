//! Markdown emission (U4): renders a lens as a self-contained leaf, and the
//! top-level map that links them. Links are portable relative Markdown by
//! default, or `[[wikilinks]]` for Obsidian (R13).

use crate::cli::LinkStyle;
use crate::lens::{Lens, Section};
use std::fmt::Write;

/// One entry in the top-level map: a leaf's id and title.
pub struct MapEntry {
    pub id: String,
    pub title: String,
}

/// Render the top-level map (`index.md`) linking every leaf (R2).
pub fn render_map(entries: &[MapEntry], links: LinkStyle) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Architecture corpus\n");
    let _ = writeln!(
        s,
        "This corpus was generated deterministically from the source's file \
         enumeration — no inference, no LLM.\n"
    );
    let _ = writeln!(s, "## Views\n");
    for entry in entries {
        let _ = writeln!(s, "- {}", link_to(&entry.id, &entry.title, links));
    }
    s
}

/// Render one lens as a self-contained leaf (R3): guiding question, then the
/// authored problem and mechanism, then the optional diagram, then repo
/// instances, then failure-modes.
pub fn render_leaf(lens: &Lens) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# {}\n", lens.title);
    let _ = writeln!(s, "**Guiding question:** {}\n", lens.guiding_question);
    let _ = writeln!(s, "## Problem\n\n{}\n", lens.problem);
    let _ = writeln!(s, "## Mechanism\n\n{}\n", lens.mechanism);

    // The diagram (U11) is pure mermaid with no duplicated prose — the following
    // instances section stays the authoritative, complete statement in words, so
    // the leaf is retrievable without parsing any mermaid (KTD-C2/R7).
    if let Some(diagram) = &lens.diagram {
        let _ = writeln!(s, "## Diagram\n\n{diagram}\n");
    }

    let _ = writeln!(s, "## In this repository\n");
    if lens.instances().next().is_none() {
        // An empty backbone view is explicit, never silently omitted (R18).
        let _ = writeln!(s, "> No facts were found for this view.\n");
    } else {
        for section in &lens.sections {
            if let Some(heading) = &section.heading {
                let _ = writeln!(s, "### {heading}\n");
            }
            render_section(&mut s, section);
        }
    }

    if !lens.failure_modes.is_empty() {
        let _ = writeln!(s, "## Failure modes\n");
        for mode in &lens.failure_modes {
            let _ = writeln!(s, "- {mode}");
        }
        let _ = writeln!(s);
    }

    s
}

/// Render one section's instances as an indented bullet list.
///
/// The indent is rebased against the section's own shallowest row, so a section
/// cut deep in the tree starts flush left and its heading — not a column of
/// leading whitespace — carries the ancestor context (U3). The rebase happens
/// here at render time and never touches the instances themselves, so every
/// consumer that reads the facts still sees their absolute depth.
///
/// A single undivided section is rooted at level 0, making the rebase a no-op
/// and the output byte-identical to the flat list that predates sectioning.
fn render_section(s: &mut String, section: &Section) {
    let base = section
        .instances
        .iter()
        .map(|i| i.indent_level)
        .min()
        .unwrap_or(0);
    for instance in &section.instances {
        // Structured Markdown nesting (R8): the indent precedes the `- `
        // marker, two spaces per level. Level 0 (every non-tree view) emits
        // exactly `- {text}`, byte-unchanged.
        let indent = "  ".repeat(instance.indent_level - base);
        let _ = writeln!(s, "{indent}- {}", instance.text);
    }
    let _ = writeln!(s);
}

/// A link from the map to a leaf, in the configured style.
fn link_to(id: &str, title: &str, links: LinkStyle) -> String {
    match links {
        LinkStyle::Portable => format!("[{title}]({id}.md)"),
        LinkStyle::Obsidian => format!("[[{id}|{title}]]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taxonomy::views::Instance;

    fn lens_with(instances: Vec<Instance>) -> Lens {
        Lens {
            view_id: "module-structure".into(),
            title: "Module structure".into(),
            guiding_question: "How is the code split?".into(),
            problem: "p".into(),
            mechanism: "m".into(),
            sections: vec![crate::lens::Section::undivided(instances)],
            diagram: None,
            failure_modes: vec![],
        }
    }

    /// A lens whose "In this repository" is already cut into sections — the
    /// shape U4 produces on a wide repository.
    fn lens_with_sections(sections: Vec<Section>) -> Lens {
        Lens {
            sections,
            ..lens_with(vec![])
        }
    }

    /// An instance at a given depth, for building tree fixtures.
    fn at(text: &str, indent_level: usize) -> Instance {
        let mut i = Instance::for_test(text);
        i.indent_level = indent_level;
        i
    }

    /// U3/R1: several sections each render under their own `###` sub-heading,
    /// and the indentation inside a section is rebased against that section's
    /// own root — a section cut at `src/foo/bar` starts flush left rather than
    /// three levels in, with the ancestor context carried by the heading.
    #[test]
    fn sections_render_under_headings_with_rebased_indentation() {
        let leaf = render_leaf(&lens_with_sections(vec![
            Section {
                heading: Some("src/foo/bar".into()),
                instances: vec![
                    at("`src/foo/bar/baz` — 2 files", 3),
                    at("`src/foo/bar/baz/qux` — 1 file", 4),
                ],
            },
            Section {
                heading: Some("vendor (part 2)".into()),
                instances: vec![at("`vendor` — 9 files", 0)],
            },
        ]));

        assert!(leaf.contains("### src/foo/bar\n"), "first heading: {leaf}");
        assert!(
            leaf.contains("### vendor (part 2)\n"),
            "second heading: {leaf}"
        );
        assert!(
            leaf.contains("\n- `src/foo/bar/baz` — 2 files\n"),
            "the section's shallowest row is rebased flush left: {leaf}"
        );
        assert!(
            leaf.contains("\n  - `src/foo/bar/baz/qux` — 1 file\n"),
            "its child keeps exactly one level of relative nesting: {leaf}"
        );
    }

    /// AE5 / zero regression: one headingless section is the pre-sectioning
    /// shape and renders with no sub-heading and no reflow.
    #[test]
    fn a_single_headingless_section_renders_as_a_flat_list() {
        let leaf = render_leaf(&lens_with(vec![at("root", 0), at("child", 1)]));

        assert!(
            !leaf.contains("###"),
            "no sub-heading for a single section: {leaf}"
        );
        assert!(
            leaf.contains("\n- root\n  - child\n"),
            "list unchanged: {leaf}"
        );
    }

    /// R8: the indent precedes the `- ` marker, two spaces per level. A level-0
    /// instance renders exactly `- text` (byte-unchanged from before U2); a
    /// level-2 instance carries four leading spaces before the bullet.
    #[test]
    fn render_leaf_indents_before_the_bullet() {
        let flat = Instance::for_test("root");
        let mut deep = Instance::for_test("nested");
        deep.indent_level = 2;
        let leaf = render_leaf(&lens_with(vec![flat, deep]));

        assert!(
            leaf.contains("\n- root\n"),
            "level 0 is a flat bullet: {leaf}"
        );
        assert!(
            leaf.contains("\n    - nested\n"),
            "level 2 has four leading spaces before the marker: {leaf}"
        );
    }
}
