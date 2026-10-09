//! The seed cohesion/coupling smell (U12, KTD-C4).
//!
//! Over the merged fact graph this computes two structural signals per module —
//! *cohesion* (how densely a module's own files reference each other) and
//! *coupling* (how densely two modules reference across their boundary) — and
//! turns extreme values into **advisory** notes the module-structure lens hangs
//! in its failure-modes section.
//!
//! Three disciplines shape the design:
//!
//! * **R5 — recommendation, never a verdict.** Every note is phrased "Consider
//!   …"; the smell never says a module is wrong, bad, or broken. It surfaces a
//!   candidate for a human to weigh, nothing more.
//! * **KTD-C4 — a minimum-size guard.** Density is statistically meaningless at
//!   small N: a 2-file module with no internal edge, or two modules joined by a
//!   single edge, would false-alarm and no threshold could rescue them. Below
//!   [`MIN_MODULE_FILES`] the smell stays silent rather than cry wolf.
//! * **KTD7 — determinism.** Modules are grouped and iterated through
//!   [`BTreeMap`]s and the notes are sorted, so the same graph yields
//!   byte-identical notes in a stable order across runs.
//!
//! "Module" here means a tracked directory — the same unit the module-structure
//! view uses — and a file belongs to exactly one module: its immediate parent
//! directory. That disjoint partition keeps the density arithmetic clean. The
//! synthetic root module `.` is excluded (a file with no `/` in its key), so
//! top-level files never form a pseudo-module, matching the module-structure
//! view which also skips the root.

use crate::graph::{EdgeKind, Graph, NodeKind};
use std::collections::BTreeMap;

/// Minimum number of files a module must directly contain before its cohesion or
/// coupling density is evaluated (KTD-C4 size guard). Below this, density is
/// statistically meaningless and every threshold false-alarms — a 2-file module
/// with no internal edge would read as zero cohesion, and two tiny modules with
/// one cross-edge as high coupling. Seed value; the exact guard is an open
/// question tuned empirically (plan).
pub const MIN_MODULE_FILES: usize = 3;

/// Cohesion density (fraction of the possible intra-module directed edges that
/// actually exist) at or below which a module is flagged as a *candidate* for
/// splitting — its files barely reference one another, a hint they may be
/// unrelated concerns cohabiting a directory. Seed value; open question.
///
/// At [`MIN_MODULE_FILES`] `= 3` the possible directed edge count is `3·2 = 6`,
/// so a 3-file module clears this seed with a single internal edge (`1/6 ≈
/// 0.17`) and only fires at *exactly* zero internal references. That makes the
/// seed deliberately conservative but also noisy in the wild (small modules with
/// no cross-file imports are common) — hence "open question, tune empirically".
pub const LOW_COHESION_DENSITY: f64 = 0.15;

/// Coupling density (fraction of the possible directed edges *between* two
/// modules that actually exist, counting both directions) at or above which the
/// two modules are flagged as a *candidate* for merging — the boundary between
/// them carries so much traffic it may be adding separation without payoff. Seed
/// value; open question.
pub const HIGH_COUPLING_DENSITY: f64 = 0.5;

/// Compute the advisory cohesion/coupling notes for the merged graph.
///
/// Returns a sorted `Vec<String>` (possibly empty). The lens appends these to a
/// module-structure leaf's `failure_modes`; every other view gets none. The
/// notes are advisory only — they never change a fact or fail the run.
pub fn cohesion_coupling_notes(graph: &Graph) -> Vec<String> {
    // Partition files into modules by immediate parent directory. `member_count`
    // is a module → direct-file-count map; `file_module` maps a file key to its
    // owning module so an edge can be classified as intra- or inter-module.
    // Both are BTreeMaps for a deterministic walk (KTD7).
    let mut member_count: BTreeMap<&str, usize> = BTreeMap::new();
    let mut file_module: BTreeMap<&str, &str> = BTreeMap::new();
    // On the concrete kind, NOT on the `Leaf` role — the same call `coverage`
    // makes and for the same reason (U6 step 3). This note is a claim about how
    // SOURCE FILES in a directory reference each other; it reads a file's key as
    // a `/`-path and its `Imports` edges as source-level references. A declared
    // leaf is neither, and admitting it through the role would let a plugin's
    // node change a cohesion verdict about code it has nothing to do with.
    // Excluding it costs nothing: the note is advisory and never a fact.
    for node in graph.nodes_sorted() {
        if node.kind != NodeKind::File {
            continue;
        }
        let path = node.key.as_str();
        // A file with no `/` sits in the synthetic root module `.`, which the
        // module-structure view excludes — so it belongs to no evaluated module.
        if let Some(slash) = path.rfind('/') {
            let module = &path[..slash];
            *member_count.entry(module).or_insert(0) += 1;
            file_module.insert(path, module);
        }
    }

    // Count intra-module and inter-module reference edges in one pass. Only
    // `Imports` (file → file) carries the source-level reference signal;
    // `Contains` (module → file) and `DependsOn` (file → external dep) self-
    // exclude because one endpoint is never a file-in-a-module, but the explicit
    // filter documents the intent. Inter-module pairs are keyed order-normalized
    // (smaller module first) so A↔B and B↔A land in one bucket.
    let mut intra: BTreeMap<&str, usize> = BTreeMap::new();
    let mut inter: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut saw_imports = false;
    for e in graph.edges_sorted() {
        // `Imports` deliberately carries no role (KTD6): it is the source-level
        // reference signal this note is built on, not a shape in the tree. A
        // declared edge kind is therefore not counted here, and a plugin that
        // wants its relation to affect cohesion has to emit `Imports` itself.
        if e.edge.kind != EdgeKind::Imports {
            continue;
        }
        saw_imports = true;
        let (Some(&from_mod), Some(&to_mod)) = (
            file_module.get(e.from.as_str()),
            file_module.get(e.to.as_str()),
        ) else {
            continue;
        };
        if from_mod == to_mod {
            *intra.entry(from_mod).or_insert(0) += 1;
        } else {
            let pair = if from_mod < to_mod {
                (from_mod, to_mod)
            } else {
                (to_mod, from_mod)
            };
            *inter.entry(pair).or_insert(0) += 1;
        }
    }

    // Zero-signal guard (E9): the tree-sitter floor emits no `Imports` edges for
    // some languages at all (Go, C#) and only best-effort for others. With no
    // import signal in the whole graph, cohesion density is undefined, not zero —
    // rendering "consider splitting (density 0.00)" on every module of a C# or Go
    // repo would present a floor blind spot as a measurement. Stay silent.
    if !saw_imports {
        return Vec::new();
    }

    let mut notes = Vec::new();

    // Cohesion: a module whose files barely reference each other.
    for (&module, &count) in &member_count {
        if count < MIN_MODULE_FILES {
            continue; // KTD-C4 size guard.
        }
        // Possible directed edges among `count` distinct files.
        let possible = count * (count - 1);
        let actual = intra.get(module).copied().unwrap_or(0);
        let density = actual as f64 / possible as f64;
        if density <= LOW_COHESION_DENSITY {
            notes.push(format!(
                "Consider splitting `{module}`: its {count} files reference each other only \
                 loosely (cohesion density {density:.2}), which can mean the module holds \
                 concerns that would read more clearly apart."
            ));
        }
    }

    // Coupling: two modules that reference across their boundary very densely.
    for (&(a, b), &count) in &inter {
        let (Some(&na), Some(&nb)) = (member_count.get(a), member_count.get(b)) else {
            continue;
        };
        if na < MIN_MODULE_FILES || nb < MIN_MODULE_FILES {
            continue; // KTD-C4 size guard — both sides must clear it.
        }
        // Possible directed edges between the two file sets, both directions.
        let possible = 2 * na * nb;
        let density = count as f64 / possible as f64;
        if density >= HIGH_COUPLING_DENSITY {
            notes.push(format!(
                "Consider merging `{a}` and `{b}` into a common module: they reference each \
                 other so densely (coupling density {density:.2}) that the boundary between them \
                 may be adding little separation."
            ));
        }
    }

    // Total, stable order regardless of which signal produced a note (KTD7).
    notes.sort();
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Edge, EdgeKind, NodeKey, NodeKind, Provenance, Tier};

    fn prov() -> Provenance {
        Provenance::new("src/lib.rs", Some(1), "treesitter".into(), Tier::Measured)
    }

    /// Add a file node under `module` and return its key.
    fn add_file(g: &mut Graph, module: &str, name: &str) -> NodeKey {
        let key = NodeKey::new(format!("{module}/{name}"));
        g.ensure_node(key.clone(), NodeKind::File).unwrap();
        key
    }

    fn imports(g: &mut Graph, from: &NodeKey, to: &NodeKey) {
        assert!(g.add_edge(from, to, Edge::new(EdgeKind::Imports, prov())));
    }

    /// A module above the guard whose files never reference each other → a
    /// "consider splitting" recommendation.
    #[test]
    fn low_cohesion_module_suggests_splitting() {
        let mut g = Graph::new();
        add_file(&mut g, "src/widgets", "a.rs");
        add_file(&mut g, "src/widgets", "b.rs");
        add_file(&mut g, "src/widgets", "c.rs");
        // A stray import in a below-guard module gives the graph an import signal,
        // so the zero-signal guard (E9) does not suppress — this is a genuine
        // low-cohesion case, not a no-instrumentation repo.
        let s0 = add_file(&mut g, "src/sig", "s0.rs");
        let s1 = add_file(&mut g, "src/sig", "s1.rs");
        imports(&mut g, &s0, &s1);

        let notes = cohesion_coupling_notes(&g);
        assert_eq!(notes.len(), 1, "exactly one note: {notes:?}");
        assert!(notes[0].contains("Consider splitting"));
        assert!(notes[0].contains("`src/widgets`"));
        assert!(notes[0].contains("cohesion density 0.00"));
    }

    /// E9: a graph with zero `Imports` edges anywhere (a Go- or C#-only repo,
    /// which the floor never instruments) yields NO cohesion notes — an undefined
    /// density must not be rendered as a measurement, even for large modules.
    #[test]
    fn zero_import_signal_yields_no_notes() {
        let mut g = Graph::new();
        for n in ["a.cs", "b.cs", "c.cs", "d.cs"] {
            add_file(&mut g, "Services", n); // 4 files, no imports at all
        }
        assert!(
            cohesion_coupling_notes(&g).is_empty(),
            "no import signal in the graph → stay silent"
        );
    }

    /// Two modules above the guard with dense cross-references → a "consider
    /// merging" recommendation. Denominator is `2·na·nb = 18`, so ≥9 directed
    /// cross-edges reach the 0.5 seed.
    #[test]
    fn high_coupling_pair_suggests_merging() {
        let mut g = Graph::new();
        let a: Vec<NodeKey> = ["a0.rs", "a1.rs", "a2.rs"]
            .iter()
            .map(|n| add_file(&mut g, "src/alpha", n))
            .collect();
        let b: Vec<NodeKey> = ["b0.rs", "b1.rs", "b2.rs"]
            .iter()
            .map(|n| add_file(&mut g, "src/beta", n))
            .collect();
        // Give each module enough internal edges to stay cohesive (so only the
        // coupling note fires), then saturate the cross-boundary edges.
        imports(&mut g, &a[0], &a[1]);
        imports(&mut g, &a[1], &a[2]);
        imports(&mut g, &b[0], &b[1]);
        imports(&mut g, &b[1], &b[2]);
        for fa in &a {
            for fb in &b {
                imports(&mut g, fa, fb); // 9 directed cross-edges → density 0.50
            }
        }

        let notes = cohesion_coupling_notes(&g);
        assert_eq!(notes.len(), 1, "only the coupling note: {notes:?}");
        assert!(notes[0].contains("Consider merging"));
        assert!(notes[0].contains("`src/alpha`"));
        assert!(notes[0].contains("`src/beta`"));
        assert!(notes[0].contains("coupling density"));
    }

    /// R5: notes are recommendations, never verdicts. No judgment vocabulary.
    #[test]
    fn notes_are_recommendations_never_verdicts() {
        let mut g = Graph::new();
        add_file(&mut g, "src/widgets", "a.rs");
        add_file(&mut g, "src/widgets", "b.rs");
        add_file(&mut g, "src/widgets", "c.rs");
        // Import signal elsewhere so the zero-signal guard doesn't suppress.
        let s0 = add_file(&mut g, "src/sig", "s0.rs");
        let s1 = add_file(&mut g, "src/sig", "s1.rs");
        imports(&mut g, &s0, &s1);

        let notes = cohesion_coupling_notes(&g);
        assert!(!notes.is_empty());
        for note in &notes {
            // Phrased as a recommendation (accept any case).
            assert!(
                note.to_lowercase().contains("consider"),
                "note recommends: {note}"
            );
            // Never a verdict.
            for verdict in [
                "wrong",
                "bad",
                "broken",
                "poor",
                "violation",
                "must ",
                "should ",
            ] {
                assert!(
                    !note.to_lowercase().contains(verdict),
                    "note {note:?} must not contain verdict word {verdict:?}"
                );
            }
        }
    }

    /// A healthy structure above the guard — both modules clear the cohesion
    /// floor and stay well under the coupling ceiling — yields no note. This
    /// exercises threshold *discrimination*, distinct from the guard test below.
    #[test]
    fn healthy_structure_yields_no_note() {
        let mut g = Graph::new();
        let a: Vec<NodeKey> = ["a0.rs", "a1.rs", "a2.rs"]
            .iter()
            .map(|n| add_file(&mut g, "src/alpha", n))
            .collect();
        let b: Vec<NodeKey> = ["b0.rs", "b1.rs", "b2.rs"]
            .iter()
            .map(|n| add_file(&mut g, "src/beta", n))
            .collect();
        // Each module has internal references (density 2/6 ≈ 0.33 > 0.15).
        imports(&mut g, &a[0], &a[1]);
        imports(&mut g, &a[1], &a[2]);
        imports(&mut g, &b[0], &b[1]);
        imports(&mut g, &b[1], &b[2]);
        // A couple of cross-edges, well under the ceiling (2/18 ≈ 0.11 < 0.5).
        imports(&mut g, &a[0], &b[0]);
        imports(&mut g, &b[2], &a[2]);

        assert!(
            cohesion_coupling_notes(&g).is_empty(),
            "healthy structure is silent"
        );
    }

    /// KTD-C4: a graph below the minimum-size guard fires nothing, even with a
    /// degenerate density (a 2-file module with zero internal edges, and a lone
    /// cross-edge between two 2-file modules).
    #[test]
    fn below_size_guard_is_silent_despite_degenerate_density() {
        let mut g = Graph::new();
        let a0 = add_file(&mut g, "src/alpha", "a0.rs");
        add_file(&mut g, "src/alpha", "a1.rs"); // 2-file module, 0 intra edges
        let b0 = add_file(&mut g, "src/beta", "b0.rs");
        add_file(&mut g, "src/beta", "b1.rs"); // 2-file module
        imports(&mut g, &a0, &b0); // lone cross-edge (density 1/8 but guarded)

        assert!(
            cohesion_coupling_notes(&g).is_empty(),
            "small-N density is suppressed"
        );
    }

    /// KTD7: the same graph produces byte-identical notes across two runs.
    #[test]
    fn notes_are_deterministic_across_runs() {
        let build = || {
            let mut g = Graph::new();
            // A low-cohesion module and (separately) a densely coupled pair, so
            // both note kinds are present and their relative order is exercised.
            add_file(&mut g, "src/lonely", "x.rs");
            add_file(&mut g, "src/lonely", "y.rs");
            add_file(&mut g, "src/lonely", "z.rs");
            let a: Vec<NodeKey> = ["a0.rs", "a1.rs", "a2.rs"]
                .iter()
                .map(|n| add_file(&mut g, "src/alpha", n))
                .collect();
            let b: Vec<NodeKey> = ["b0.rs", "b1.rs", "b2.rs"]
                .iter()
                .map(|n| add_file(&mut g, "src/beta", n))
                .collect();
            imports(&mut g, &a[0], &a[1]);
            imports(&mut g, &a[1], &a[2]);
            imports(&mut g, &b[0], &b[1]);
            imports(&mut g, &b[1], &b[2]);
            for fa in &a {
                for fb in &b {
                    imports(&mut g, fa, fb);
                }
            }
            cohesion_coupling_notes(&g)
        };
        let first = build();
        let second = build();
        assert_eq!(first, second);
        // Two notes present; sorted so "Consider merging" precedes "Consider
        // splitting" deterministically.
        assert_eq!(first.len(), 2, "both notes: {first:?}");
        assert!(first[0].contains("Consider merging"));
        assert!(first[1].contains("Consider splitting"));
    }
}
