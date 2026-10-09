//! The lens (U4): composes a view's authored framing with its repo-derived
//! instances into the structure a leaf renders.
//!
//! This is the KTD9 split made concrete: the *problem* and *mechanism* are
//! authored framing copied verbatim from the catalog (identical across every
//! repository), while the *instances* come from the fact graph and the
//! *failure-modes* come from smells (U12). Nothing here consults an LLM (R15).

use crate::graph::Graph;
use crate::taxonomy::views::{Instance, ViewType};

/// One rendered group of instances under "In this repository" (U3).
///
/// Almost every view has exactly one, headingless — the flat list that existed
/// before sectioning. The module-structure tree is the exception: on a wide
/// repository it is cut into several numbered sections (R1). See
/// [`crate::taxonomy::views::module_structure_sections`] for what the headings
/// say and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The `###` sub-heading, or `None` for a single undivided section — which
    /// renders exactly as the flat list always did.
    pub heading: Option<String>,
    pub instances: Vec<Instance>,
}

impl Section {
    /// The whole instance list as one undivided, headingless section — what
    /// every view but a split module-structure tree renders.
    pub fn undivided(instances: Vec<Instance>) -> Self {
        Section {
            heading: None,
            instances,
        }
    }
}

/// A fully composed lens, ready to render as one self-contained leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lens {
    pub view_id: String,
    pub title: String,
    pub guiding_question: String,
    pub problem: String,
    pub mechanism: String,
    /// The instances, grouped into render sections. Sectioning is a render-time
    /// grouping only — see [`Lens::instances`] for the flat view every other
    /// consumer reads.
    pub sections: Vec<Section>,
    /// An optional mermaid diagram body (one or more fenced blocks), rendered as
    /// the leaf's `## Diagram` section above the instances (KTD-C2). The words
    /// authoritative for R7 live in the following instances section, not here.
    pub diagram: Option<String>,
    pub failure_modes: Vec<String>,
}

impl Lens {
    /// Every instance, in order, flattened across sections.
    ///
    /// Sectioning groups the module-structure tree for *rendering*; it never
    /// changes which facts the lens carries. Consumers that want the facts —
    /// the enrich bundle, the manifest — read them through here and see exactly
    /// the list they saw before sections existed.
    pub fn instances(&self) -> impl Iterator<Item = &Instance> {
        self.sections.iter().flat_map(|s| s.instances.iter())
    }
}

/// Compose the lens for a view type over a graph.
pub fn compose(view: &ViewType, graph: &Graph) -> Lens {
    Lens {
        view_id: view.id.to_string(),
        title: view.title.to_string(),
        guiding_question: view.guiding_question.to_string(),
        problem: view.problem.to_string(),
        mechanism: view.mechanism.to_string(),
        // Only the module-structure tree is wide enough to need cutting, and
        // only it carries the indent levels a section rebase is defined against;
        // every other view keeps one undivided headingless section and renders
        // byte-identically to before sectioning existed. Keyed on `view_id` like
        // the failure-modes branch below.
        sections: if view.id == "module-structure" {
            // `taxonomy` decides where the tree divides and hands back its own
            // grouping type; wrapping it here is what keeps that dependency
            // one-directional (`lens` over `taxonomy`, never the reverse).
            crate::taxonomy::sections::module_structure_sections((view.instances)(graph))
                .into_iter()
                .map(|s| Section {
                    heading: s.heading,
                    instances: s.instances,
                })
                .collect()
        } else {
            vec![Section::undivided((view.instances)(graph))]
        },
        // A view's diagram fn is optional; when present it may still decline to
        // draw (no edges) and yield `None` — then no `## Diagram` section renders.
        diagram: view.diagram.and_then(|f| f(graph)),
        // The seed cohesion/coupling smell attaches only to the module-structure
        // view (U12); every other view has no smell, so an empty failure-modes.
        // Recomputed per projection — `compose` runs in both the corpus and the
        // manifest builds — which Tier 0 accepts (KTD-C4 note).
        failure_modes: if view.id == "module-structure" {
            crate::smell::cohesion_coupling_notes(graph)
        } else {
            Vec::new()
        },
    }
}
