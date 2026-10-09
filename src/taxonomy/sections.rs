//! Grouping the module-structure tree into render sections (U4, R1/R3).
//!
//! Lives next to [`super::views`] rather than inside it for two reasons: the
//! tree derivation and the tree *grouping* are separable concerns, and keeping
//! the grouping here lets `taxonomy` stay a leaf module. It returns its own
//! [`ModuleSection`] instead of `lens::Section`, so the dependency runs one way
//! only — `lens` composes over `taxonomy`, never the reverse.

use super::views::Instance;
use crate::render::split::{fixed_partition, ChunkWeight};

/// The module-node ceiling for one rendered section of the module-structure
/// tree (U4, R3/KTD5).
///
/// Seeded at the diagram's `SPLIT_THRESHOLD`, but deliberately its own constant:
/// the diagram weighs *distinct nodes including files*, this weighs *module rows*
/// only, so the two are not the same quantity and tuning one must not silently
/// move the other.
///
/// **The seed is not calibrated, and the unit it governs is coarser than it
/// looks.** The packing unit is a whole top-level subtree, so this bounds how
/// many subtrees share a section — it does not bound a section's size once a
/// single subtree exceeds it (KTD4). Measured on the reference repository (13 270 files): 2 121
/// module rows land in 36 sections, averaging ~59 rows — well over the threshold,
/// because most sections are one indivisible subtree. The widest sibling sets
/// there (91/90/89 modules) sit four levels deep inside such a subtree, so this
/// split does not reach them at all, and that is deliberate: a later
/// change, not this one, has to cover them —
/// reaching those is the deferred multi-file split, not a threshold tweak.
pub(crate) const MODULE_SECTION_THRESHOLD: usize = 30;

/// One grouped run of module rows: the `###` heading it renders under, and the
/// rows themselves.
///
/// Structurally this mirrors `lens::Section`, and that is deliberate — it is the
/// seam between the two layers. `taxonomy` decides *where* the tree divides;
/// `lens` owns the type the renderer consumes. Naming the boundary costs one
/// small struct and buys a one-directional dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleSection {
    pub heading: Option<String>,
    pub instances: Vec<Instance>,
}

impl ModuleSection {
    /// The whole tree as one undivided, headingless section.
    fn undivided(instances: Vec<Instance>) -> Self {
        ModuleSection {
            heading: None,
            instances,
        }
    }
}

/// The chunk weight for the prose tree: how many module rows a unit holds (R3).
struct ModuleRows(usize);

impl ChunkWeight<Vec<Instance>> for ModuleRows {
    fn peek(&self, unit: &Vec<Instance>) -> usize {
        self.0 + unit.len()
    }
    fn add(&mut self, unit: &Vec<Instance>) {
        self.0 += unit.len();
    }
    fn reset(&mut self) {
        self.0 = 0;
    }
}

/// Cut the flat pre-order tree into its top-level subtrees: a row at indent 0
/// and every row beneath it, in order. These are the indivisible units of the
/// split — keeping a subtree whole is what stops a cut from orphaning children
/// from the parent line that gives them their context (KTD4).
fn top_level_subtrees(instances: Vec<Instance>) -> Vec<Vec<Instance>> {
    let mut units: Vec<Vec<Instance>> = Vec::new();
    for instance in instances {
        // `units.is_empty()` is defensive only: a pre-order walk always opens
        // with a top-level row, so the first row starts the first unit either way.
        if instance.indent_level == 0 || units.is_empty() {
            units.push(vec![instance]);
        } else {
            units
                .last_mut()
                .expect("a unit is open once any row has been seen")
                .push(instance);
        }
    }
    units
}

/// Group the module tree's rows into render sections (U4, R1/R3).
///
/// The split runs along one seam only. Keying the seam on a module's *parent*
/// puts every top-level subtree in the same group, so there is no deeper seam to
/// recurse into and the work is exactly the shared greedy chunker (KTD1/KTD2) —
/// which also avoids the degenerate alternative, where keying on a module's own
/// path component would turn a flat repository into one singleton section per
/// module.
///
/// Below the threshold the tree stays a single undivided, headingless section
/// and renders byte-identically to the unsectioned tree (AE5). A subtree that
/// alone outweighs the threshold becomes its own over-budget section rather than
/// being cut open (KTD4).
///
/// Headings are `Part N`. The plan's `<parent> (part N)` form would prefix every
/// heading with the same repo root, since the seam is always the root; naming
/// the seam would only add a constant. Richer seam headings ("modules a–f") were
/// weighed and deferred in KTD2.
pub(crate) fn module_structure_sections(instances: Vec<Instance>) -> Vec<ModuleSection> {
    if instances.len() <= MODULE_SECTION_THRESHOLD {
        return vec![ModuleSection::undivided(instances)];
    }

    let units = top_level_subtrees(instances);
    let chunks = fixed_partition(units, MODULE_SECTION_THRESHOLD, &mut ModuleRows(0));

    // One chunk means one section — a lone over-budget subtree, or a tree that
    // never needed cutting. A "Part 1" with no part 2 is noise, and a single
    // headingless section is the byte-identical shape (KTD3).
    if chunks.len() <= 1 {
        return vec![ModuleSection::undivided(
            chunks.into_iter().flatten().flatten().collect(),
        )];
    }

    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| ModuleSection {
            heading: Some(format!("Part {}", i + 1)),
            instances: chunk.into_iter().flatten().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taxonomy::views::{
        graph_from_files, graph_from_generated, module_structure_instances,
    };

    /// `Covers AE1.` A flat repository of 150 top-level modules is cut into
    /// chunked sections — a *bounded number* of them, not 150 singletons, which
    /// is what a seam keyed on each module's own path component would produce
    /// (KTD2) — and no module is lost in the cut.
    #[test]
    fn a_wide_flat_repo_is_chunked_into_bounded_sections() {
        let g = graph_from_generated(
            &(0..150)
                .map(|i| format!("m{i:03}/a.rs"))
                .collect::<Vec<_>>(),
        );
        let instances = module_structure_instances(&g);
        assert_eq!(instances.len(), 150, "150 top-level module rows");

        let sections = module_structure_sections(instances);

        // The section COUNT is bounded, not merely each section's size — a size-
        // only assertion would pass for 150 singleton sections.
        assert_eq!(sections.len(), 150_usize.div_ceil(MODULE_SECTION_THRESHOLD));
        for s in &sections {
            assert!(
                s.instances.len() <= MODULE_SECTION_THRESHOLD,
                "section holds {} rows, over the threshold",
                s.instances.len()
            );
            assert!(s.heading.is_some(), "a split section carries a heading");
        }
        let total: usize = sections.iter().map(|s| s.instances.len()).sum();
        assert_eq!(total, 150, "the cut loses nothing");
    }

    /// KTD4 precedence: a subtree that alone outweighs the threshold is emitted
    /// whole as its own over-budget section. Cutting inside it would orphan
    /// children from the parent row that gives them their context, so
    /// indivisibility outranks the size limit.
    #[test]
    fn an_oversized_subtree_becomes_its_own_over_budget_section() {
        let mut paths: Vec<String> = (0..50).map(|i| format!("big/s{i:02}/a.rs")).collect();
        paths.extend((0..40).map(|i| format!("m{i:02}/a.rs")));
        let g = graph_from_generated(&paths);

        let sections = module_structure_sections(module_structure_instances(&g));

        // `big` sorts before `m00`, so the oversized subtree opens the output.
        let first = &sections[0];
        assert_eq!(
            first.instances.len(),
            51,
            "`big` plus its 50 children, whole"
        );
        assert!(
            first.instances[0].text.starts_with("`big`"),
            "the section is rooted at the subtree root: {:?}",
            first.instances[0].text
        );
        assert!(
            first.instances[1..].iter().all(|i| i.indent_level == 1),
            "every other row is a child of that root — none orphaned"
        );
        // The over-budget section is the exception, not the rule.
        for s in &sections[1..] {
            assert!(s.instances.len() <= MODULE_SECTION_THRESHOLD);
        }
    }

    /// `Covers AE5.` A repository under the threshold stays one undivided,
    /// headingless section — the shape that renders byte-identically to the tree
    /// before sectioning existed, so ordinary repos see no change at all.
    #[test]
    fn a_small_repo_stays_one_headingless_section() {
        let g = graph_from_files(&["src/lib.rs", "src/extract/git.rs", "docs/intro.md"]);
        let instances = module_structure_instances(&g);

        let sections = module_structure_sections(instances.clone());

        assert_eq!(sections.len(), 1);
        assert!(
            sections[0].heading.is_none(),
            "no heading for a single section"
        );
        assert_eq!(sections[0].instances, instances, "rows untouched");
    }

    /// A tree over the threshold that is a *single* top-level subtree packs into
    /// one chunk, and that lone chunk renders headingless — a `Part 1` with no
    /// `Part 2` is noise. The merge-back branch is otherwise unreachable from the
    /// multi-subtree fixtures, so it needs its own case.
    #[test]
    fn a_single_oversized_subtree_stays_one_headingless_section() {
        let g = graph_from_generated(
            &(0..40)
                .map(|i| format!("only/s{i:02}/a.rs"))
                .collect::<Vec<_>>(),
        );
        let instances = module_structure_instances(&g);
        assert!(
            instances.len() > MODULE_SECTION_THRESHOLD,
            "the fixture must cross the threshold to reach the packer"
        );

        let sections = module_structure_sections(instances.clone());

        assert_eq!(sections.len(), 1);
        assert!(
            sections[0].heading.is_none(),
            "no `Part 1` without a `Part 2`"
        );
        assert_eq!(
            sections[0].instances, instances,
            "no row dropped or reordered"
        );
    }

    /// The threshold boundary itself: exactly at it stays whole, one row past it
    /// splits. Pins that the guard is `<=`, so a tree of exactly the threshold
    /// never gains headings.
    #[test]
    fn the_threshold_boundary_is_inclusive() {
        let at = graph_from_generated(
            &(0..MODULE_SECTION_THRESHOLD)
                .map(|i| format!("m{i:03}/a.rs"))
                .collect::<Vec<_>>(),
        );
        let at_sections = module_structure_sections(module_structure_instances(&at));
        assert_eq!(at_sections.len(), 1, "exactly at the threshold stays whole");
        assert!(at_sections[0].heading.is_none());

        let over = graph_from_generated(
            &(0..MODULE_SECTION_THRESHOLD + 1)
                .map(|i| format!("m{i:03}/a.rs"))
                .collect::<Vec<_>>(),
        );
        let over_sections = module_structure_sections(module_structure_instances(&over));
        assert_eq!(over_sections.len(), 2, "one row past it splits");
        assert_eq!(
            over_sections[1].instances.len(),
            1,
            "the spill is a single row"
        );
    }

    /// `Covers R7.` Sectioning the same tree twice is byte-identical: the
    /// chunker carries no state across runs and nothing is ordered by anything
    /// but the input.
    #[test]
    fn sectioning_is_deterministic_across_runs() {
        let g = graph_from_generated(
            &(0..150)
                .map(|i| format!("m{i:03}/a.rs"))
                .collect::<Vec<_>>(),
        );

        let first = module_structure_sections(module_structure_instances(&g));
        let second = module_structure_sections(module_structure_instances(&g));

        assert_eq!(first, second);
    }

    /// `Covers R8.` The rows across every section are exactly the tree's rows in
    /// the same order — the split regroups, it never drops, duplicates, or
    /// reorders a module. Uses a mixed fixture (one oversized subtree plus many
    /// small ones) so both the over-budget and the packed paths are exercised.
    #[test]
    fn sections_preserve_every_row_in_order() {
        let mut paths: Vec<String> = (0..50).map(|i| format!("big/s{i:02}/a.rs")).collect();
        paths.extend((0..60).map(|i| format!("m{i:02}/a.rs")));
        let g = graph_from_generated(&paths);
        let instances = module_structure_instances(&g);

        let rejoined: Vec<Instance> = module_structure_sections(instances.clone())
            .into_iter()
            .flat_map(|s| s.instances)
            .collect();

        assert_eq!(rejoined, instances, "the cut preserves every row, in order");
    }

    /// `Covers AE4.` A cut never lands inside a subtree: every section opens at
    /// an indent-0 row, so no section starts with a descendant severed from the
    /// parent line that gives it context. The subtree widths are deliberately
    /// uneven, so the greedy packer has to cut in several different places
    /// rather than at a uniform stride.
    #[test]
    fn every_section_starts_at_a_subtree_root() {
        let mut paths: Vec<String> = Vec::new();
        for (i, width) in [7usize, 3, 11, 2, 9, 5, 13, 4, 8, 6].iter().enumerate() {
            for w in 0..*width {
                paths.push(format!("t{i:02}/c{w:02}/a.rs"));
            }
        }
        let g = graph_from_generated(&paths);

        let sections = module_structure_sections(module_structure_instances(&g));

        assert!(sections.len() > 1, "the fixture is wide enough to split");
        for section in &sections {
            assert_eq!(
                section.instances[0].indent_level, 0,
                "a section opens at a subtree root, never mid-subtree: {:?}",
                section.heading
            );
        }
    }
}
