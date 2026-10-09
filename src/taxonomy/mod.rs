//! Taxonomy (U4): selects which view types populate the corpus.
//!
//! Backbone views are always present (an empty one renders the R18 "no facts"
//! note); emergent views appear only when their triggering facts exist — for
//! Tier 0 "has instances" *is* "facts present" (KTD-C1), so the gate is a single
//! predicate over `instances(graph)`, with no separate `detects` field.

pub mod profile;
pub mod sections;
pub mod views;

use crate::graph::Graph;
use views::{catalog, ViewRole, ViewType};

/// The views to render for this repository.
///
/// With no profile (`None`): every backbone view, plus each emergent view whose
/// `instances(graph)` is non-empty (KTD-C1) — the automatic taxonomy.
///
/// With a named profile (`Some`): the same automatic set, narrowed to the
/// profile's view ids (R18). A profile is an *override that selects among the
/// automatic set*, so it can only ever be a subset — it never adds a view the
/// gate excluded (E11). Both projections (`render::build_corpus` and
/// `manifest::build_manifest`) call this with the same profile, so the corpus
/// and manifest can never disagree on the selected view set (KTD-C5).
///
/// The profile name is assumed valid: `Config::resolve` validates it against
/// [`profile::view_ids`] and fails early on an unknown name, so an unknown name
/// reaching here is a broken invariant, not user error.
pub fn select_views(
    graph: &Graph,
    profile: Option<&str>,
) -> Result<Vec<ViewType>, profile::UnknownProfile> {
    let gated = gate(catalog(), graph);
    match profile {
        None => Ok(gated),
        Some(name) => {
            // Same mapping `Config::resolve` validated against — so the applied
            // set can never drift. Returns an error (not a panic, not a silent
            // empty corpus) on an unknown name, so the public projection
            // functions are safe for library callers too.
            let ids = profile::view_ids_checked(name)?;
            Ok(gated.into_iter().filter(|v| ids.contains(&v.id)).collect())
        }
    }
}

/// Every filename the tool can generate in a corpus directory: `index.md`,
/// `manifest.json`, and one `<id>.md` per catalog view type (backbone AND
/// emergent, independent of gating or profile). Used to clear only tool-owned
/// files before a re-run — never user-authored Markdown (a README, a plan doc)
/// that happens to share the output directory. Because it spans the whole
/// catalog, a narrower `--profile` re-run still removes leaves the wider prior
/// run wrote.
pub fn generated_filenames() -> Vec<String> {
    let mut names = vec!["index.md".to_string(), "manifest.json".to_string()];
    for view in catalog() {
        names.push(format!("{}.md", view.id));
    }
    // The architecture pages the card layer writes beside the views.
    names.extend(
        crate::card::pages::PAGE_FILENAMES
            .iter()
            .map(|n| n.to_string()),
    );
    names.sort();
    names.dedup();
    names
}

/// Keep every backbone view unconditionally and each emergent view only when it
/// has instances on this graph. Split out so the gating mechanism is testable
/// against a synthetic catalog independent of the production catalog.
fn gate(catalog: Vec<ViewType>, graph: &Graph) -> Vec<ViewType> {
    catalog
        .into_iter()
        .filter(|v| v.role == ViewRole::Backbone || !(v.instances)(graph).is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeKey, NodeKind};
    use views::Instance;

    /// A synthetic backbone view: instances always empty, so the test proves an
    /// *empty backbone* is still kept (it would render the R18 note).
    fn always_empty(_graph: &Graph) -> Vec<Instance> {
        Vec::new()
    }

    /// A synthetic emergent view whose instances are present iff the graph holds
    /// any Dependency node. CRITICAL (per review): it returns an empty vec when
    /// the triggering facts are absent — the whole point of the gate.
    fn dependency_triggered(graph: &Graph) -> Vec<Instance> {
        graph
            .nodes_sorted()
            .iter()
            .filter(|n| n.kind == NodeKind::Dependency)
            .map(|n| Instance::for_test(n.key.as_str().to_string()))
            .collect()
    }

    fn synthetic_catalog() -> Vec<ViewType> {
        vec![
            ViewType {
                id: "b",
                title: "B",
                role: ViewRole::Backbone,
                guiding_question: "",
                problem: "",
                mechanism: "",
                instances: always_empty,
                diagram: None,
            },
            ViewType {
                id: "e",
                title: "E",
                role: ViewRole::Emergent,
                guiding_question: "",
                problem: "",
                mechanism: "",
                instances: dependency_triggered,
                diagram: None,
            },
        ]
    }

    #[test]
    fn unknown_profile_is_an_error_not_a_panic() {
        // The public projection functions must not panic on an unvalidated
        // profile name reaching them from a library caller (Codex #3).
        let g = Graph::new();
        assert!(select_views(&g, Some("nonesuch")).is_err());
        assert!(select_views(&g, None).is_ok());
        assert!(select_views(&g, Some("minimal")).is_ok());
    }

    #[test]
    fn emergent_view_appears_only_when_its_facts_are_present() {
        // A graph WITH the triggering fact (a Dependency node).
        let mut with_dep = Graph::new();
        with_dep
            .ensure_node(NodeKey::new("dep:cargo:serde"), NodeKind::Dependency)
            .unwrap();
        let selected = gate(synthetic_catalog(), &with_dep);
        let ids: Vec<&str> = selected.iter().map(|v| v.id).collect();
        assert!(ids.contains(&"b"), "backbone is always present");
        assert!(ids.contains(&"e"), "emergent present when its facts exist");
    }

    #[test]
    fn empty_emergent_is_omitted_while_empty_backbone_is_kept() {
        // A graph WITHOUT the triggering fact.
        let mut empty = Graph::new();
        empty
            .ensure_node(NodeKey::new("."), NodeKind::Module)
            .unwrap();
        let selected = gate(synthetic_catalog(), &empty);
        let ids: Vec<&str> = selected.iter().map(|v| v.id).collect();
        assert!(
            ids.contains(&"b"),
            "empty backbone is still kept (shows R18 note)"
        );
        assert!(
            !ids.contains(&"e"),
            "empty emergent is omitted (no false presence)"
        );
    }
}
