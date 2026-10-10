//! The catalog of view types (KTD9) — the single designed artifact that
//! resolves the "narrative without an LLM" paradox.
//!
//! For each view type the catalog pins the *authored* framing (problem,
//! mechanism, guiding question) that is identical across every repository, and
//! a function that derives the per-repo *instances* from the fact graph. In
//! Tier 0 only these two backbone types exist; U10 extends the catalog with the
//! remaining backbone and emergent types.

use crate::extract::parent_key;
#[cfg(test)]
use crate::graph::EdgeKind;
use crate::graph::{
    Confidence, FactKind, FactValue, Graph, LinkRole, NodeKey, NodeKind, StructuralRole,
};

// This import used to be test-only, and the comment here said so proudly: no
// production line in this file named a concrete kind, because U6 replaced every
// such branch with `node_role`/`edge_role` (U6, KTD6), and the compiler agreed.
//
// It is no longer true, and the reason is worth more than the invariant was. Two
// external reviewers found the overview counting declared entities as "tracked
// files" while `coverage.json`, about the same corpus, said declared nodes are not
// tracked files at all. One of the two artifacts had to be lying, and it was this
// one.
//
// What the production code asks now is `matches!(kind, NodeKind::Declared(_))` —
// **core or declared**, never WHICH declared kind. That distinction is the one U6
// actually removed: branching on a plugin's identity is what makes the core
// depend on a plugin, and nothing here does that. Core-or-declared is a property
// the core owns outright; it is what separates "a file this tool read" from "an
// entity a plugin told us about", and no structural role can express it, because
// both populations legitimately hold the same role.
use std::collections::{BTreeMap, BTreeSet};

/// Whether a view is always present (backbone) or appears only when its facts
/// exist (emergent). Only backbone types exist in Tier 0's seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewRole {
    Backbone,
    Emergent,
}

/// A concrete, repo-derived statement rendered under "In this repository".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub text: String,
    /// Markdown nesting depth for the leaf render (U2, R8). 0 is a flat bullet
    /// (every view but the module-structure tree). `render_leaf` emits this many
    /// levels of indentation before the `- ` marker; the enrich fact bundle
    /// ignores it and renders a flat list, so a tree shape never leaks into the
    /// narrative prompt. Must be `pub` — `render_leaf` reads it cross-module.
    pub indent_level: usize,
    /// The weakest rung among the edges this statement rests on (plan A, KTD13),
    /// or `None` when it rests on no rated edge — a count, a fact, a structural
    /// relation.
    ///
    /// Carried so the enrich layer can keep an ambiguous relation out of what
    /// it hands the model (`enrich::bundle`): the narrative must not retell a
    /// guess as a fact. The leaf render ignores it and still shows every
    /// instance. The weakest rather than the strongest, because one statement
    /// names every edge it rests on, so a single ambiguous edge puts a guess in
    /// its text.
    pub rung: Option<Confidence>,
}

impl Instance {
    fn new(text: impl Into<String>) -> Self {
        Instance {
            text: text.into(),
            indent_level: 0,
            rung: None,
        }
    }

    /// Build an instance at a given tree depth (U3). Level 0 is a flat bullet.
    fn nested(text: impl Into<String>, indent_level: usize) -> Self {
        Instance {
            text: text.into(),
            indent_level,
            rung: None,
        }
    }

    /// The same instance, resting on edges whose weakest rung is `rung`.
    fn resting_on(mut self, rung: Option<Confidence>) -> Self {
        self.rung = rung;
        self
    }

    /// Test-only constructor for building lenses in unit tests of other modules.
    #[cfg(test)]
    pub fn for_test(text: impl Into<String>) -> Self {
        Instance {
            text: text.into(),
            indent_level: 0,
            rung: None,
        }
    }
}

/// A view type: authored framing + a rule for deriving instances (KTD9).
pub struct ViewType {
    /// Stable id, used as the leaf's filename and manifest id.
    pub id: &'static str,
    /// Human title.
    pub title: &'static str,
    pub role: ViewRole,
    /// The question this view answers (R6). Authored, identical across repos.
    pub guiding_question: &'static str,
    /// Why this view matters. Authored framing prose (KTD9).
    pub problem: &'static str,
    /// How the view is built. Authored framing prose (KTD9).
    pub mechanism: &'static str,
    /// Derives the per-repo instances from the graph.
    ///
    /// Emergent-gate contract: `gate` keeps an emergent view only when this
    /// returns a non-empty vec, treating "instances non-empty" as "this view's
    /// facts are present". An emergent view's `instances` fn MUST therefore return
    /// empty when its facts are absent — it may not append an unconditional
    /// caveat, or the gate would keep it on every repo. A backbone view is exempt
    /// (always kept) and may append a caveat freely.
    pub instances: fn(&Graph) -> Vec<Instance>,
    /// Optionally derives a mermaid diagram for this view (U11, KTD-C2). Returns
    /// `None` when there is nothing to draw, so no empty `## Diagram` renders.
    /// Views without a diagram set this to `None`.
    pub diagram: Option<fn(&Graph) -> Option<String>>,
}

/// The full catalog select_views draws from: every backbone view plus the
/// emergent views (KTD-C1). `select_views` keeps all backbone views and only
/// the emergent ones whose instances are non-empty.
pub fn catalog() -> Vec<ViewType> {
    let mut c = backbone_catalog();
    c.extend(emergent_catalog());
    c
}

/// Emergent view types — present only when their triggering facts exist
/// (KTD-C1: "fact presence = detection"). Deferred in Tier 0: no floor-
/// reachable seed fact (e.g. a CLI/clap `FactKind`) exists yet, and E11 forbids
/// a view no extractor populates. The gating *mechanism* is live and tested
/// against a synthetic graph (see `taxonomy::tests`); the first production
/// emergent view lands with the extractor that seeds it.
pub fn emergent_catalog() -> Vec<ViewType> {
    Vec::new()
}

/// The backbone: view types a floor-universal source can populate on any repo
/// (E11). Every entry here is wired to a real extractor, and every backbone
/// view is rendered even when empty — an empty one shows the R18 "no facts"
/// note rather than vanishing.
pub fn backbone_catalog() -> Vec<ViewType> {
    vec![
        ViewType {
            id: "overview",
            title: "Overview",
            role: ViewRole::Backbone,
            guiding_question: "What is this repository, and how is it shaped at the top level?",
            problem: "A newcomer needs a truthful first picture of a repository — what it \
                      contains and how it is shaped — before trusting any deeper claim. Guessed \
                      overviews mislead; this one is built only from the files the source enumerates.",
            mechanism: "The overview counts the enumerated files and modules, lists the top-level \
                        entries and ranks the modules that hold the most files. Every number is \
                        derived deterministically from the file enumeration this run was given — the git \
                        index at HEAD where there is a repository, the tree itself where \
                        there is not — with no history, no inference and no LLM.",
            instances: overview_instances,
            diagram: None,
        },
        ViewType {
            id: "module-structure",
            title: "Module structure",
            role: ViewRole::Backbone,
            guiding_question:
                "How is the code split into modules, and what does each module contain?",
            problem: "To navigate or change a codebase you must know how it is partitioned into \
                      modules and what each holds. Without that map, every change risks landing \
                      in the wrong place.",
            mechanism: "Enumerated directories become modules; each module's contents are the \
                        files and sub-modules the enumeration records beneath it. The structure is read \
                        from the file tree, never assumed.",
            instances: module_structure_instances,
            diagram: Some(module_structure_diagram),
        },
        ViewType {
            id: "dependencies",
            title: "Dependencies",
            role: ViewRole::Backbone,
            guiding_question: "What external code does this repository depend on, and how?",
            problem: "Every dependency is a promise the codebase cannot keep on its own — a \
                      surface for bugs, licensing, and supply-chain risk. To reason about a \
                      repository you must know what it pulls in, and whether each is needed to \
                      ship or only to build and test.",
            mechanism: "Dependencies are read from the ecosystem manifests the enumeration \
                        contains — \
                        `Cargo.toml`, `package.json`, `go.mod`, `*.csproj`. Each is tagged with \
                        the scope it was declared under (normal vs. dev) and the manifest that \
                        declares it. Nothing is inferred from source; a dependency no enumerated \
                        manifest names is never listed.",
            instances: dependencies_instances,
            diagram: Some(dependencies_diagram),
        },
        ViewType {
            id: "entry-points",
            title: "Entry points",
            role: ViewRole::Backbone,
            guiding_question: "Where does execution start, and how is this code entered?",
            problem: "To read a program you need a way in — the point where control first \
                      lands. Missing the real entry points sends a reader down the wrong path; \
                      claiming there are none where the floor simply cannot see them is worse, \
                      because it reads as a fact.",
            mechanism: "Entry points come from two honest sources: language-level starts the \
                        grammar can see (a Rust/Go/C# `main`, a Python `__main__` guard, a C# \
                        top-level program) and manifest declarations (a Cargo `[[bin]]`/`[lib]`, \
                        a `package.json` `main`/`bin`, a `.csproj` `OutputType`). Framework-level \
                        entries — web routes, message handlers, scheduled jobs — are outside what \
                        a deterministic floor can detect, so their absence here is never asserted \
                        as their absence in the code (E9).",
            instances: entry_points_instances,
            diagram: None,
        },
        ViewType {
            id: "test-coverage",
            title: "Test coverage",
            role: ViewRole::Backbone,
            guiding_question:
                "Which sources have a test by convention, and how far does extraction reach?",
            problem: "Tests are evidence a piece of code was meant to hold a contract. Knowing \
                      which sources have a paired test — and being honest about the limits of a \
                      name-based check — tells a reader where the safety net is, without ever \
                      pretending a missing match proves a missing test.",
            mechanism: "A source is paired with a test when the two share a language and a stem \
                        under the ecosystem's naming convention (`foo_test.rs`, `test_foo.py`, \
                        `Foo.test.ts`, `FooTests.cs`). The check reads names and the file tree \
                        only, never contents — so it can confirm a convention-matching test but \
                        never prove one absent, hence \"test by convention not found\", never \
                        \"untested\". A per-language extraction-coverage line reports how many \
                        sources carry tree-sitter structural facts versus only metrics and the file tree.",
            instances: test_coverage_instances,
            diagram: None,
        },
    ]
}

/// The `String` inside a text fact value, if it is one.
fn text_value(value: &FactValue) -> Option<&str> {
    match value {
        FactValue::Text(s) => Some(s.as_str()),
        _ => None,
    }
}

/// An honest caveat carried by the entry-points view on every repo (E9): the
/// deterministic floor cannot see framework-level entries, so a short list is
/// never a proof that no other entry exists.
const FRAMEWORK_ENTRY_CAVEAT: &str =
    "Framework-level entry points — web routes, message handlers, scheduled jobs, \
     dependency-injected services — are invisible to the deterministic floor; this list is what \
     the code and manifests declare, not a proof that no other entry exists.";

/// The id of the view the line of trigger coverage goes into (plan 2b, U8).
pub const ENTRY_POINTS_VIEW: &str = "entry-points";

/// Plan 2b, U8 (KTD12): put `line` — the line of trigger coverage of a run
/// with the second layer — right before the caveat E9 of the view
/// `entry-points`, which stays as it is. Done at render time, so
/// [`ViewType::instances`] keeps its signature and a run without the layer
/// never reaches here. `false` when `instances` hold no caveat: the line is
/// then not placed, and the caller puts it last
/// ([`crate::render::place_entry_points_line`]).
pub fn insert_before_entry_caveat(instances: &mut Vec<Instance>, line: String) -> bool {
    match instances
        .iter()
        .position(|i| i.text == FRAMEWORK_ENTRY_CAVEAT)
    {
        Some(at) => {
            instances.insert(at, Instance::new(line));
            true
        }
        None => false,
    }
}

fn dependencies_instances(graph: &Graph) -> Vec<Instance> {
    // Which manifest(s) declare each dependency, via the DependsOn edges
    // (manifest File --DependsOn--> dep node). Conveys the ecosystem without
    // ever parsing the opaque `dep:<eco>:<name>` key apart. The weakest rung
    // among those edges travels with them into the instance (plan A, KTD13).
    let mut declared_by: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut rung_of: BTreeMap<&str, Confidence> = BTreeMap::new();
    for e in graph.edges_sorted() {
        if graph.edge_role(e.edge.kind) == LinkRole::Dependency {
            declared_by
                .entry(e.to.as_str())
                .or_default()
                .insert(e.from.as_str());
            if let Some(rung) = e.edge.provenance.rung() {
                // `Confidence` orders strongest first, so the weakest is the max.
                let weakest = rung_of.entry(e.to.as_str()).or_insert(rung);
                *weakest = (*weakest).max(rung);
            }
        }
    }

    let mut out = Vec::new();
    for node in graph.nodes_sorted() {
        if graph.node_role(node.kind) != StructuralRole::Package {
            continue;
        }
        // Name mirrors the DependencyName fact — never re-parse the dep: key.
        // A declared package has no such fact and falls back to its key, which is
        // the only name this view can honestly give it: the core is not allowed
        // to know what a declared kind calls itself beyond its vocabulary entry.
        let name = node
            .facts
            .iter()
            .find(|f| f.kind == FactKind::DependencyName)
            .and_then(|f| text_value(&f.value))
            .unwrap_or_else(|| node.key.as_str())
            .to_string();
        // Scopes are additive (a dep can be both normal and dev); sort + dedup.
        let scopes: BTreeSet<&str> = node
            .facts
            .iter()
            .filter(|f| f.kind == FactKind::DependencyScope)
            .filter_map(|f| text_value(&f.value))
            .collect();
        let scope_str = if scopes.is_empty() {
            "unscoped".to_string()
        } else {
            scopes.into_iter().collect::<Vec<_>>().join(", ")
        };

        match declared_by.get(node.key.as_str()) {
            Some(manifests) => {
                let rendered: Vec<String> = manifests.iter().map(|m| format!("`{m}`")).collect();
                out.push(
                    Instance::new(format!(
                        "`{name}` ({scope_str}) — declared in {}.",
                        rendered.join(", ")
                    ))
                    .resting_on(rung_of.get(node.key.as_str()).copied()),
                );
            }
            None => out.push(Instance::new(format!("`{name}` ({scope_str})."))),
        }
    }
    out
}

fn entry_points_instances(graph: &Graph) -> Vec<Instance> {
    let mut entries: Vec<Instance> = Vec::new();
    for node in graph.nodes_sorted() {
        // On the ROLE, not the kind (R9). A declared Definition or Leaf that
        // carries the core `EntryPoint`/`EntryPointDecl` fact is a real entry
        // point and belongs here — the fact is core, the node need not be.
        match graph.node_role(node.kind) {
            // Language-level: a definition carrying `EntryPoint = true`.
            StructuralRole::Definition => {
                let is_entry = node
                    .facts
                    .iter()
                    .any(|f| f.kind == FactKind::EntryPoint && f.value == FactValue::Bool(true));
                if !is_entry {
                    continue;
                }
                let name = node
                    .facts
                    .iter()
                    .find(|f| f.kind == FactKind::SymbolName)
                    .and_then(|f| text_value(&f.value))
                    .unwrap_or_else(|| node.key.as_str())
                    .to_string();
                let file = node
                    .facts
                    .iter()
                    .find(|f| f.kind == FactKind::EntryPoint)
                    .map(|f| f.provenance.file.clone())
                    .unwrap_or_default();
                entries.push(Instance::new(format!(
                    "`{name}` in `{file}` — language-level entry point."
                )));
            }
            // Manifest-level: a leaf carrying EntryPointDecl fact(s).
            StructuralRole::Leaf => {
                for f in &node.facts {
                    if f.kind == FactKind::EntryPointDecl {
                        if let Some(decl) = text_value(&f.value) {
                            entries.push(Instance::new(format!(
                                "`{decl}` declared in `{}` — manifest entry point.",
                                node.key.as_str()
                            )));
                        }
                    }
                }
            }
            // A container, a package, an external or a node that takes no part in
            // the backbone views declares no entry point: neither core fact is
            // ever attached to one, so there is nothing here to drop silently.
            StructuralRole::Container
            | StructuralRole::Package
            | StructuralRole::External
            | StructuralRole::Opaque => {}
        }
    }
    // Stable order regardless of fact/node insertion order.
    entries.sort_by(|a, b| a.text.cmp(&b.text));

    let mut out = entries;
    if out.is_empty() {
        // Honest, never "this repo has no entry points" (E9): scoped to what
        // the two floor sources detect, not an absolute claim of absence.
        out.push(Instance::new(
            "No language-level or manifest-declared entry point was detected — a library or a \
             framework-driven project may expose none of these directly.",
        ));
    }
    // The caveat is always last, present or empty — so this backbone view is
    // never empty-silent and never renders the R18 "no facts" note.
    out.push(Instance::new(FRAMEWORK_ENTRY_CAVEAT));
    out
}

fn test_coverage_instances(graph: &Graph) -> Vec<Instance> {
    let mut out = Vec::new();

    // Per-language extraction coverage (Coverage facts on the root module).
    let mut coverage: Vec<&str> = Vec::new();
    for node in graph.nodes_sorted() {
        for f in &node.facts {
            if f.kind == FactKind::Coverage {
                if let Some(t) = text_value(&f.value) {
                    coverage.push(t);
                }
            }
        }
    }
    coverage.sort_unstable();
    for c in coverage {
        out.push(Instance::new(format!("Extraction coverage: {c}.")));
    }

    // Per-source test-pairing verdict — the exact wording is carried verbatim in
    // the fact value ("has a test by convention" / "test by convention not
    // found"), so it can never drift to "untested" (E9/R18).
    // Deliberately still by ROLE alone, unlike the two file COUNTS above. The line
    // is emitted only where a `TestByConvention` fact exists, and that fact is the
    // claim: an extractor that declares its own kind and then asserts a test
    // pairing for it has said something true about its own entity, and dropping it
    // would be the core overruling a claim it did not make. The counts differ
    // because they claim something the core asserts itself.
    for node in graph.nodes_sorted() {
        if graph.node_role(node.kind) != StructuralRole::Leaf {
            continue;
        }
        for f in &node.facts {
            if f.kind == FactKind::TestByConvention {
                if let Some(t) = text_value(&f.value) {
                    out.push(Instance::new(format!("`{}`: {t}.", node.key.as_str())));
                }
            }
        }
    }
    out
}

fn overview_instances(graph: &Graph) -> Vec<Instance> {
    let nodes = graph.nodes_sorted();
    // A "tracked file" is a file an extractor actually read, which is what
    // `coverage.json` counts — and coverage says in so many words that a declared
    // node is not one. Counting by role alone made the two artifacts contradict
    // each other about the same corpus. RG2 saw the contradiction and accepted it,
    // reasoning that unifying meant changing the sentence and so breaking R4; that
    // was wrong, and two external reviewers said so independently. The fix is the
    // FILTER, not the wording, and a corpus that declares nothing holds no
    // declared node — so its number does not move.
    //
    // This asks core-or-declared, not WHICH declared kind. R9 forbids the second;
    // the first is a property the core owns and is the only thing that separates
    // "a file this tool read" from "an entity a plugin told us about".
    let leaves = || {
        nodes
            .iter()
            .filter(|n| graph.node_role(n.kind) == StructuralRole::Leaf)
    };
    let file_count = leaves()
        .filter(|n| !matches!(n.kind, NodeKind::Declared(_)))
        .count();
    let declared_leaves = leaves()
        .filter(|n| matches!(n.kind, NodeKind::Declared(_)))
        .count();
    // Exclude the synthetic root module `.` so the count matches the module
    // structure view, which lists only real directories.
    let module_count = nodes
        .iter()
        .filter(|n| graph.node_role(n.kind) == StructuralRole::Container && n.key.as_str() != ".")
        .count();

    let mut out = vec![Instance::new(format!(
        "{file_count} tracked file{} across {module_count} module{}.",
        plural(file_count),
        plural(module_count),
    ))];
    // Said rather than folded into the number above: they are real entities the
    // corpus documents, they are not files, and leaving them out of both places
    // would be the silent omission R10 forbids.
    if declared_leaves > 0 {
        out.push(Instance::new(format!(
            "Plus {declared_leaves} entr{} of declared kinds, which no language \
             extractor read and which coverage therefore does not count.",
            if declared_leaves == 1 { "y" } else { "ies" },
        )));
    }

    let top_level = top_level_entries(graph);
    if !top_level.is_empty() {
        out.push(Instance::new(format!(
            "Top-level entries: {}.",
            top_level.join(", ")
        )));
    }

    // Rank modules by how many files they hold — where the mass of the code
    // actually sits. This stays readable even on large repos, where a flat
    // module list does not (auto-split/ranking of that list arrives in U10/U11).
    let largest = largest_modules(graph, 5);
    if !largest.is_empty() {
        let rendered: Vec<String> = largest
            .iter()
            .map(|(module, count)| {
                format!(
                    "`{module}` ({count} {})",
                    leaf_word(declared_leaves > 0, *count)
                )
            })
            .collect();
        out.push(Instance::new(format!(
            "Largest modules: {}.",
            rendered.join(", ")
        )));
    }

    // The extraction-coverage level (E9): how far structural extraction actually
    // reached, stated as an honesty fact rather than a quality guess. Sourced
    // from the same Coverage facts the test-coverage view uses.
    let mut coverage: Vec<&str> = Vec::new();
    for node in nodes.iter() {
        for fact in &node.facts {
            if fact.kind == FactKind::Coverage {
                if let Some(t) = text_value(&fact.value) {
                    coverage.push(t);
                }
            }
        }
    }
    coverage.sort_unstable();
    for c in coverage {
        out.push(Instance::new(format!("Extraction coverage: {c}.")));
    }

    out
}

/// The module structure as a hierarchical, annotated tree (U3, R1–R6). The
/// module forest is nested by path; siblings are byte-ordered by whole key
/// (R3/KTD6 — a pre-order DFS, not today's flat global sort); files are counts,
/// not nodes (R4). Each line is a full-path label plus an annotation: a
/// container (≥1 direct child module) shows both its direct-entry count and its
/// recursive file count; a leaf (no child module) collapses to the one recursive
/// file count (KTD3 — keyed on child-module presence, never on the counts
/// coinciding). The synthetic root `.` is never rendered (R1).
pub(crate) fn module_structure_instances(graph: &Graph) -> Vec<Instance> {
    // Direct entries per module: Contains edges out of it (its direct files AND
    // sub-modules). One pass over the edge set — `edges_sorted` sorts the whole
    // set, so it must not be called per module.
    let mut direct_entries: BTreeMap<&str, usize> = BTreeMap::new();
    let edges = graph.edges_sorted();
    for e in &edges {
        // The TARGET too. Two reviewers found this independently in RG2, and they
        // were right: `module_structure_diagram`, `top_level_entries` and
        // `largest_modules` each got this filter and the direct-entry count did
        // not, so one view's prose counted an `Opaque` child that the same view's
        // diagram, its child list and its file count all excluded. The earlier
        // fix closed three of four readers of the same question.
        if graph.edge_role(e.edge.kind) == LinkRole::Containment && takes_part(graph, e.to.as_str())
        {
            *direct_entries.entry(e.from.as_str()).or_insert(0) += 1;
        }
    }

    // Recursive file counts from the shared U1 helper — the same source the
    // overview ranking uses, so a module's file count is identical across views (R5).
    let recursive_files = recursive_module_file_counts(graph);

    // parent → sorted child modules (the synthetic root `.` excluded as a node,
    // but used as the key under which top-level modules hang). `nodes_sorted`
    // yields keys in bytewise order, so each child list is already byte-sorted by
    // whole key — the per-level order (R3).
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in graph.nodes_sorted() {
        if graph.node_role(node.kind) != StructuralRole::Container || node.key.as_str() == "." {
            continue;
        }
        let key = node.key.as_str().to_string();
        let parent = parent_key(&key); // "." for a top-level module
        children.entry(parent).or_default().push(key);
    }

    // Parent-closed invariant (R1.1): a module nested under a real directory has
    // that directory as a module node. `filetree` builds a module for every
    // ancestor prefix of every file, so this holds by construction — a gap is a
    // generator bug, never silently re-parented or dropped. Cheap insurance in
    // debug builds (also covered by an explicit test). The synthetic root `.` is
    // exempt; see `parent_closed`.
    debug_assert!(
        parent_closed(graph),
        "module-structure tree has an orphan: a module whose parent dir is not a module node"
    );

    // Pre-order DFS from the top-level modules (children of `.`), depth = indent.
    let mut out = Vec::new();
    let declared_leaf = any_declared_leaf(graph);
    if let Some(roots) = children.get(".") {
        for root in roots {
            push_module_subtree(
                root,
                &children,
                &direct_entries,
                &recursive_files,
                declared_leaf,
                &mut out,
            );
        }
    }
    out
}

/// Append `key`'s tree line, then recurse into its child modules in order
/// (pre-order DFS, R3). `indent_level` is the path depth (`/`-segment count), so
/// a top-level module is level 0.
fn push_module_subtree(
    key: &str,
    children: &BTreeMap<String, Vec<String>>,
    direct_entries: &BTreeMap<&str, usize>,
    recursive_files: &BTreeMap<String, usize>,
    declared_leaf: bool,
    out: &mut Vec<Instance>,
) {
    let depth = key.matches('/').count();
    let child_modules = children.get(key).filter(|c| !c.is_empty());
    // Rely on the U4 invariants (not the churned snapshot) as the real guard on
    // this count; a missing key means zero files beneath it.
    let files = recursive_files.get(key).copied().unwrap_or(0);

    let text = if child_modules.is_some() {
        // Container: both numbers — direct entries (files + sub-modules) and the
        // recursive file total. Both always shown, even when they coincide (KTD3).
        let entries = direct_entries.get(key).copied().unwrap_or(0);
        format!(
            "`{key}` — {} {}, {} {}",
            entries,
            entry_word(entries),
            files,
            leaf_word(declared_leaf, files),
        )
    } else {
        // Leaf module (no child modules): the two counts are equal, so collapse
        // to the single recursive file count.
        format!("`{key}` — {} {}", files, leaf_word(declared_leaf, files))
    };
    out.push(Instance::nested(text, depth));

    if let Some(child_modules) = child_modules {
        for child in child_modules {
            push_module_subtree(
                child,
                children,
                direct_entries,
                recursive_files,
                declared_leaf,
                out,
            );
        }
    }
}

/// True iff every module whose parent is a real directory has that directory as
/// a module node (R1.1). `filetree` guarantees this; the check is cheap
/// insurance against a future extractor that could emit an orphan module such
/// as `a/b/c` with no `a/b`.
///
/// The synthetic root `.` is deliberately **not** required to exist as a node.
/// `module_structure_instances` keys its child lists by `parent_key`, so a
/// top-level module hangs under the *string* `"."` whether or not a `.` node was
/// ever created — measured 10.09.2026: a graph without it renders all five
/// backbone views identically to one with it. Demanding the node made the debug
/// assert fire on a graph that produced correct output, which is a trap for any
/// caller building a graph outside this crate.
fn parent_closed(graph: &Graph) -> bool {
    let modules: BTreeSet<&str> = graph
        .nodes_sorted()
        .iter()
        .filter(|n| graph.node_role(n.kind) == StructuralRole::Container)
        .map(|n| n.key.as_str())
        .collect();
    modules.iter().filter(|k| **k != ".").all(|k| {
        let parent = parent_key(k);
        parent == "." || modules.contains(parent.as_str())
    })
}

/// Whether the node under `key` takes part in the backbone views at all (R9).
///
/// The one question every view that names a node BY KEY has to ask. **Five
/// readers, and they must not drift apart** — `module_structure_instances`'s
/// direct-entry count and its child list, `module_structure_diagram`,
/// `top_level_entries`, `largest_modules` and `dependencies_diagram`. The first
/// round of U6 gave it to three of them; RG2 found the other two, each as a view
/// contradicting itself about one node. If a sixth reader of "is this node in
/// the backbone" appears, it belongs on this list. A key with
/// no node keeps today's answer — `yes` — because a view may legitimately name a
/// path no node was ever created for (`recursive_module_file_counts` credits
/// ancestor prefixes, and a hand-built graph need not have a node for each).
/// Only a node that EXISTS and declared itself `Opaque` is held back.
///
/// Core kinds all carry a backbone role, so this holds nothing back in any
/// corpus this tool builds today (R4).
fn takes_part(graph: &Graph, key: &str) -> bool {
    graph
        .node(&NodeKey::new(key))
        .is_none_or(|n| graph.node_role(n.kind) != StructuralRole::Opaque)
}

/// `entry`/`entries` for the direct-entry count (R5 normative wording).
fn entry_word(n: usize) -> &'static str {
    if n == 1 {
        "entry"
    } else {
        "entries"
    }
}

/// `file`/`files` for a recursive file count (R5 normative wording).
fn file_word(n: usize) -> &'static str {
    if n == 1 {
        "file"
    } else {
        "files"
    }
}

/// The noun for a recursive leaf count, chosen for what the corpus actually holds.
///
/// `recursive_module_file_counts` counts every leaf by ROLE, so in a corpus with
/// declarations the population under a container is not all files — and two
/// external reviewers, reading the rendered view against `coverage.json`, found
/// the word claiming it was. The count is right; the noun was not.
///
/// Decided from the whole graph rather than per module, because the alternative
/// is a per-module split the reader has no use for: what a reader needs to know
/// is whether "files" can be taken at face value in this corpus at all. A corpus
/// that declares nothing holds no declared leaf and keeps the word it always had,
/// so its bytes do not move (R4).
fn leaf_word(any_declared_leaf: bool, n: usize) -> &'static str {
    if any_declared_leaf {
        entry_word(n)
    } else {
        file_word(n)
    }
}

/// Whether this corpus holds a leaf that no language extractor read.
///
/// One pass over the nodes, resolved ONCE per view and threaded — not asked per
/// module. Asking it inside `push_module_subtree` would have made an O(n) scan
/// run for every module in the tree, which on a real repository is the O(n*m) the
/// performance lens exists to catch.
fn any_declared_leaf(graph: &Graph) -> bool {
    graph.nodes_sorted().iter().any(|node| {
        graph.node_role(node.kind) == StructuralRole::Leaf
            && matches!(node.kind, NodeKind::Declared(_))
    })
}

/// The module-structure diagram: the `Contains` tree (module → file/sub-module),
/// including the synthetic root `.` so the top-level seam subdivides on auto-
/// split. `None` when the repo is flat (no `Contains` edges to draw).
fn module_structure_diagram(graph: &Graph) -> Option<String> {
    // Only containment (a container holding leaves/sub-containers). file→symbol
    // edges are `Declares`, whose role is `Declaration`, so the role filter alone
    // excludes them — no node-kind workaround needed. (Previously `Contains` was
    // overloaded for both, so this had to also check the source was a Module or
    // the diagram exploded into one block per symbol.)
    //
    // BOTH ENDPOINTS must take part in the backbone views. Found by U6's own
    // fixture: with only the edge filtered, a declared node in the `Opaque` role
    // stayed out of the prose list and was still drawn in the picture above it —
    // the view saying two different things about the same node. `Opaque` is the
    // default for a declared kind, so that asymmetry would have been the FIRST
    // thing a plugin author met. Core kinds all carry a backbone role, so this
    // changes no existing diagram (R4).
    let edges: Vec<(&str, &str)> = graph
        .edges_sorted()
        .iter()
        .filter(|e| graph.edge_role(e.edge.kind) == LinkRole::Containment)
        .filter(|e| takes_part(graph, e.from.as_str()) && takes_part(graph, e.to.as_str()))
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    crate::render::mermaid::diagram(&edges)
}

/// The dependencies diagram: the `DependsOn` relation, drawn manifest-file →
/// dependency (its true shape — never module → module). Labels are the raw node
/// keys, so a dependency shows as `dep:<eco>:<name>`; the human name lives in the
/// authoritative instances prose. `None` when no manifest declares a dependency.
fn dependencies_diagram(graph: &Graph) -> Option<String> {
    let edges: Vec<(&str, &str)> = graph
        .edges_sorted()
        .iter()
        .filter(|e| graph.edge_role(e.edge.kind) == LinkRole::Dependency)
        // Both endpoints, exactly as the containment diagram does. Without it
        // this picture draws a node `dependencies_instances` never lists — the
        // same prose/diagram disagreement, one view over.
        .filter(|e| takes_part(graph, e.from.as_str()) && takes_part(graph, e.to.as_str()))
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    crate::render::mermaid::diagram(&edges)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The immediate children of the root module, sorted.
fn top_level_entries(graph: &Graph) -> Vec<String> {
    let root = NodeKey::new(".");
    let mut entries: Vec<String> = graph
        .edges_sorted()
        .iter()
        .filter(|e| e.from == &root && graph.edge_role(e.edge.kind) == LinkRole::Containment)
        // The TARGET too, not just the edge. Found by U6's own fixture: with the
        // edge alone filtered, an `Opaque` node the module tree left out was
        // still listed here as a top-level entry (R9).
        .filter(|e| takes_part(graph, e.to.as_str()))
        .map(|e| e.to.as_str().to_string())
        .collect();
    entries.sort();
    entries.dedup();
    entries
}

/// The untruncated per-module recursive file count: for every module that holds
/// at least one file anywhere beneath it, the number of files under its subtree.
/// The root `.` is excluded and zero-file modules are omitted (both fall out of
/// the ancestor-prefix walk, which only credits `/`-prefixed ancestors of files).
///
/// This is the single source both the overview ranking (`largest_modules`) and
/// the module-structure tree draw from — so a module's recursive count is
/// identical in both views (R5). It is NOT the all-module oracle: a module with
/// no files never appears here, so a caller needing "every module" must union
/// this with the module node set.
pub(crate) fn recursive_module_file_counts(graph: &Graph) -> BTreeMap<String, usize> {
    // One pass over files, crediting each file to every ancestor directory —
    // O(files × path-depth) instead of the old O(modules × files) prefix scan,
    // which is billions of `starts_with` checks on a 100k-file monorepo
    // (backlog: large-repo module-view readability). A module's count is the
    // number of files anywhere beneath it, matching the old recursive prefix
    // semantics; the root `.` has no `/`-prefixed ancestor and stays excluded,
    // and modules with no files never enter the map.
    //
    // The walk is over the KEY, not over `Contains` edges, so a declared Leaf is
    // credited to its ancestors only if its key is `/`-hierarchical — the same
    // precondition R16 puts on a declared Container, now inherited by its leaves.
    // A plugin whose leaf keys are not path-shaped gets a count of zero here
    // rather than a wrong one.
    //
    // Counts every LEAF, declared ones included, and that is deliberate: U6 made
    // this role-based so a declared container is not rendered as holding nothing.
    // What a declared leaf is not, is a *file* — so the two consumers pick their
    // word from [`leaf_word`] rather than saying "files" over a population that
    // is not all files. The number is the same either way; only the noun moves.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in graph.nodes_sorted() {
        if graph.node_role(node.kind) != StructuralRole::Leaf {
            continue;
        }
        let path = node.key.as_str();
        // Walk ancestor prefixes: "a/b/c.rs" → "a/b", then "a".
        let mut end = path.len();
        while let Some(slash) = path[..end].rfind('/') {
            *counts.entry(path[..slash].to_string()).or_insert(0) += 1;
            end = slash;
        }
    }
    counts
}

/// The `n` modules holding the most files (recursively), ranked by file count
/// then by key for determinism. The root module `.` is excluded. A thin ranker
/// over [`recursive_module_file_counts`] — rank + truncate, no counting logic.
fn largest_modules(graph: &Graph, n: usize) -> Vec<(String, usize)> {
    // Highest count first; ties broken by key. The map iterates key-sorted
    // (BTreeMap), so the explicit key tie-break keeps output deterministic
    // regardless of sort stability.
    //
    // Filtered HERE rather than inside `recursive_module_file_counts`: that walk
    // is the measured hot path with a documented contract (a count per ancestor
    // prefix), and it has a second consumer that wants exactly that. The view
    // that RANKS the result is the one that owes the reader consistency — an
    // `Opaque` node ranked as a largest module while the module tree omits it
    // was the second half of the same leak the diagram had.
    let mut ranked: Vec<(String, usize)> = recursive_module_file_counts(graph)
        .into_iter()
        .filter(|(key, _)| takes_part(graph, key))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(n);
    ranked
}

/// Build a graph the way `filetree` does: a `.` root, a module for every
/// ancestor dir of every file, and `Contains` edges parent→child (module and
/// file). This gives the module-structure tree correct direct-entry counts.
///
/// Module-level rather than inside `mod tests` because [`super::sections`]
/// builds its fixtures the same way, and both need the tree to come out of the
/// real derivation rather than hand-assembled `Instance` values.
#[cfg(test)]
pub(crate) fn graph_from_files(paths: &[&str]) -> Graph {
    graph_from_files_opt(paths, true)
}

/// [`graph_from_files`], but `with_root` chooses whether the synthetic `.` node
/// is created — the difference F2 turns on.
#[cfg(test)]
pub(crate) fn graph_from_files_opt(paths: &[&str], with_root: bool) -> Graph {
    use crate::graph::{Edge, Provenance, Tier};
    let mut g = Graph::new();
    if with_root {
        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    }

    let mut modules: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        let parts: Vec<&str> = p.split('/').collect();
        for i in 1..parts.len() {
            modules.insert(parts[..i].join("/"));
        }
    }
    for m in &modules {
        g.ensure_node(NodeKey::new(m.clone()), NodeKind::Module)
            .unwrap();
    }

    let contains = || {
        Edge::new(
            EdgeKind::Contains,
            Provenance::new("t", None, "test".into(), Tier::Measured),
        )
    };
    for m in &modules {
        g.add_edge(
            &NodeKey::new(parent_key(m)),
            &NodeKey::new(m.clone()),
            contains(),
        );
    }
    for p in paths {
        g.ensure_node(NodeKey::new(*p), NodeKind::File).unwrap();
        g.add_edge(&NodeKey::new(parent_key(p)), &NodeKey::new(*p), contains());
    }
    g
}

/// [`graph_from_files`] over generated paths — the fixtures too wide to spell out.
#[cfg(test)]
pub(crate) fn graph_from_generated(paths: &[String]) -> Graph {
    let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    graph_from_files(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ranking: count-desc, key-asc tie-break, truncate to `n` — the branches
    /// the tiny fixture repos never reach.
    #[test]
    fn largest_modules_ranks_ties_and_truncates() {
        let mut g = Graph::new();
        for (module, files) in [("a", 6), ("b", 5), ("c", 4), ("d", 3), ("f", 2), ("e", 2)] {
            g.ensure_node(NodeKey::new(module), NodeKind::Module)
                .unwrap();
            for i in 0..files {
                g.ensure_node(NodeKey::new(format!("{module}/f{i}.rs")), NodeKind::File)
                    .unwrap();
            }
        }

        let ranked = largest_modules(&g, 5);

        // Top 5 of 6: highest counts first; the 2-file tie broken by key ("e"
        // beats "f"), and "f" truncated away entirely.
        assert_eq!(
            ranked,
            vec![
                ("a".to_string(), 6),
                ("b".to_string(), 5),
                ("c".to_string(), 4),
                ("d".to_string(), 3),
                ("e".to_string(), 2),
            ]
        );
    }

    /// The module-structure diagram is the directory tree only — the file→symbol
    /// `Declares` edges U6 emits must be excluded, or the diagram explodes. Since
    /// they are a distinct edge kind, the `Contains` filter alone excludes them.
    #[test]
    fn module_structure_diagram_excludes_file_to_symbol_edges() {
        use crate::graph::{Edge, Provenance, Tier};
        let mut g = Graph::new();
        g.ensure_node(NodeKey::new("src"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("src/a.rs"), NodeKind::File)
            .unwrap();
        g.ensure_node(NodeKey::new("sym:src/a.rs#foo"), NodeKind::Symbol)
            .unwrap();
        let prov = || Provenance::new("src/a.rs", None, "test".into(), Tier::Measured);
        // module → file (Contains) belongs in the diagram; file → symbol
        // (Declares) does NOT.
        g.add_edge(
            &NodeKey::new("src"),
            &NodeKey::new("src/a.rs"),
            Edge::new(EdgeKind::Contains, prov()),
        );
        g.add_edge(
            &NodeKey::new("src/a.rs"),
            &NodeKey::new("sym:src/a.rs#foo"),
            Edge::new(EdgeKind::Declares, prov()),
        );

        let diagram = module_structure_diagram(&g).expect("a diagram is produced");
        assert!(diagram.contains("src/a.rs"), "the file node should appear");
        assert!(
            !diagram.contains("sym:"),
            "file→symbol edges must be excluded from the module-structure diagram"
        );
    }

    /// Nested modules count files recursively, and empty modules are excluded.
    #[test]
    fn largest_modules_counts_recursively_and_skips_empty() {
        let mut g = Graph::new();
        g.ensure_node(NodeKey::new("src"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("src/inner"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("empty"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("src/inner/a.rs"), NodeKind::File)
            .unwrap();
        g.ensure_node(NodeKey::new("src/b.rs"), NodeKind::File)
            .unwrap();

        let ranked = largest_modules(&g, 5);

        assert_eq!(
            ranked,
            vec![("src".to_string(), 2), ("src/inner".to_string(), 1)]
        );
    }

    /// The shared helper is the untruncated all-file-bearing-module source: it
    /// returns EVERY module with files (not just the top-N `largest_modules`
    /// keeps), excludes `.`, and omits zero-file modules. Regression against the
    /// old truncation the tree must not inherit.
    #[test]
    fn recursive_module_file_counts_returns_every_file_bearing_module() {
        let mut g = Graph::new();
        // Six file-bearing modules — more than largest_modules' default top-5.
        for (module, files) in [("a", 6), ("b", 5), ("c", 4), ("d", 3), ("e", 2), ("f", 1)] {
            g.ensure_node(NodeKey::new(module), NodeKind::Module)
                .unwrap();
            for i in 0..files {
                g.ensure_node(NodeKey::new(format!("{module}/f{i}.rs")), NodeKind::File)
                    .unwrap();
            }
        }
        // A zero-file module and the synthetic root must not appear.
        g.ensure_node(NodeKey::new("empty"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

        let counts = recursive_module_file_counts(&g);

        assert_eq!(counts.len(), 6, "all six file-bearing modules, not top-5");
        assert_eq!(counts.get("a"), Some(&6));
        assert_eq!(counts.get("f"), Some(&1));
        assert!(!counts.contains_key("empty"), "zero-file module omitted");
        assert!(!counts.contains_key("."), "synthetic root excluded");
    }

    fn texts(instances: &[Instance]) -> Vec<&str> {
        instances.iter().map(|i| i.text.as_str()).collect()
    }

    /// A 3-level repo renders nested and alphabetical, with indent = path depth,
    /// and the synthetic root `.` is never a line (R1, R3, R8).
    #[test]
    fn module_structure_renders_a_nested_alphabetical_tree() {
        let g = graph_from_files(&[
            "src/lib.rs",
            "src/extract/git.rs",
            "README.md",
            "docs/intro.md",
        ]);
        let out = module_structure_instances(&g);

        assert_eq!(
            texts(&out),
            vec![
                "`docs` — 1 file",
                "`src` — 2 entries, 2 files",
                "`src/extract` — 1 file",
            ]
        );
        // Indent tracks depth: top-level modules at 0, the sub-module at 1.
        assert_eq!(
            out.iter().map(|i| i.indent_level).collect::<Vec<_>>(),
            vec![0, 0, 1]
        );
        // `.` is never rendered as a line.
        assert!(out.iter().all(|i| !i.text.contains("`.`")));
    }

    /// A graph assembled outside this crate need not create the synthetic root:
    /// the tree is identical with and without it, and the debug assert must not
    /// fire on the second one (measured 10.09.2026 — the release build already
    /// rendered both the same, so the assert was stricter than the renderer).
    #[test]
    fn a_graph_without_the_synthetic_root_renders_the_same_tree() {
        let paths = ["src/lib.rs", "src/extract/git.rs", "docs/intro.md"];
        let with_root = graph_from_files_opt(&paths, true);
        let without_root = graph_from_files_opt(&paths, false);

        assert!(
            parent_closed(&without_root),
            "the missing synthetic root must not read as an orphan"
        );
        assert_eq!(
            texts(&module_structure_instances(&without_root)),
            texts(&module_structure_instances(&with_root)),
            "the tree must not depend on a node the renderer never reads"
        );
    }

    /// The insurance the assert exists for is untouched: a module nested under a
    /// directory that has no module node of its own is still an orphan.
    #[test]
    fn a_module_under_a_missing_directory_is_still_an_orphan() {
        let mut g = Graph::new();
        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
        g.ensure_node(NodeKey::new("a"), NodeKind::Module).unwrap();
        // `a/b` is deliberately absent.
        g.ensure_node(NodeKey::new("a/b/c"), NodeKind::Module)
            .unwrap();

        assert!(
            !parent_closed(&g),
            "a module whose parent directory has no node is an orphan"
        );
    }

    /// KTD3: a container shows BOTH numbers even when they coincide — the
    /// collapse rule is keyed on having no child module, not on equal counts.
    #[test]
    fn container_with_child_module_shows_both_counts_even_when_equal() {
        // `a` holds a direct file AND a sub-module: 2 direct entries, and 2 files
        // recursively — numerically equal, yet both must render.
        let g = graph_from_files(&["a/file.rs", "a/b/x.rs"]);
        let out = module_structure_instances(&g);

        assert!(
            texts(&out).contains(&"`a` — 2 entries, 2 files"),
            "container shows both counts even when equal: {:?}",
            texts(&out)
        );
        assert!(
            texts(&out).contains(&"`a/b` — 1 file"),
            "leaf collapses: {:?}",
            texts(&out)
        );
    }

    /// A leaf collapses to one recursive file count, and singular `1 entry`/
    /// `1 file` wording renders (R5). A container whose only entry is a single
    /// sub-module reads `1 entry, 1 file`.
    #[test]
    fn leaf_collapses_and_singular_wording_renders() {
        let g = graph_from_files(&["a/b/x.rs"]);
        let out = module_structure_instances(&g);

        assert_eq!(texts(&out), vec!["`a` — 1 entry, 1 file", "`a/b` — 1 file"]);
    }

    /// A flat repo (only top-level modules, no nesting) degenerates to a forest
    /// of level-0 leaf lines — no crash, no indent.
    #[test]
    fn flat_repo_is_a_forest_of_level_zero_leaves() {
        let g = graph_from_files(&["x/a.rs", "y/b.rs"]);
        let out = module_structure_instances(&g);

        assert_eq!(texts(&out), vec!["`x` — 1 file", "`y` — 1 file"]);
        assert!(out.iter().all(|i| i.indent_level == 0));
    }

    /// A repo whose only node is the synthetic root `.` (all files at the top
    /// level, no directories) yields NO instances — the renderer then shows the
    /// R18 "no facts" note rather than a blank or contradictory section (R6).
    #[test]
    fn root_only_repo_yields_no_instances() {
        let g = graph_from_files(&["a.rs", "b.rs"]);
        assert!(module_structure_instances(&g).is_empty());
    }

    /// KTD6: sibling order is a bytewise, per-level pre-order DFS — NOT today's
    /// flat global key sort. With prefix-sharing keys the two differ: under the
    /// tree, `src/api` (a child of `src`) precedes the separate root `src-old`,
    /// whereas a flat sort would place `src-old` before `src/api` (`-` 0x2D < `/`
    /// 0x2F). This is the one observable behavior change the nesting introduces.
    #[test]
    fn sibling_order_is_per_level_dfs_not_flat_global_sort() {
        let g = graph_from_files(&["src/api/a.rs", "src/z.rs", "src-old/b.rs"]);
        let instances = module_structure_instances(&g);
        let out = texts(&instances);

        let pos = |needle: &str| out.iter().position(|t| t.starts_with(needle)).unwrap();
        assert!(
            pos("`src`") < pos("`src/api`") && pos("`src/api`") < pos("`src-old`"),
            "pre-order DFS keeps a subtree contiguous before the next root: {out:?}"
        );
    }

    /// Nested files credit every ancestor recursively — the count of a parent
    /// includes files held only by its descendants.
    #[test]
    fn recursive_module_file_counts_credits_ancestors() {
        let mut g = Graph::new();
        g.ensure_node(NodeKey::new("src"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("src/inner"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("src/inner/a.rs"), NodeKind::File)
            .unwrap();
        g.ensure_node(NodeKey::new("src/b.rs"), NodeKind::File)
            .unwrap();

        let counts = recursive_module_file_counts(&g);

        assert_eq!(counts.get("src"), Some(&2));
        assert_eq!(counts.get("src/inner"), Some(&1));
    }
}
