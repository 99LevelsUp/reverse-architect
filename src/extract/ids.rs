//! The bundled extractor ids, allocated once per process.
//!
//! [`ExtractorId`] became an owned `Arc<str>` in U3 so a plugin's id can exist
//! at run time (R12). That makes the id a heap value, and a fact carries one —
//! so a naive `"treesitter".into()` at an emission site would allocate once
//! per fact. Over the R17 target that is roughly 1.6 million allocations for
//! seven distinct strings.
//!
//! These statics are the fix: one allocation per id for the life of the
//! process, and every emission site clones from here, which is an atomic
//! refcount bump. `LazyLock` is available because `rust-toolchain.toml` pins
//! 1.95.0 (it stabilized in 1.80), so no `once_cell` dependency is needed.
//!
//! A plugin's id will not appear here — it comes from a manifest and lives in
//! the run's registry instead. This module is the *bundled* set only, which is
//! also why the names are functions rather than public statics: a caller
//! should reach for the registry when it wants "every id this run knows", and
//! only reach here when it means one specific bundled extractor.

use crate::graph::ExtractorId;
use std::sync::LazyLock;

macro_rules! bundled_id {
    ($(#[$doc:meta])* $name:ident, $text:literal) => {
        $(#[$doc])*
        pub fn $name() -> ExtractorId {
            static ID: LazyLock<ExtractorId> = LazyLock::new(|| ExtractorId::from($text));
            ID.clone()
        }
    };
}

bundled_id!(
    /// `filetree` — modules, files and the `Contains` topology.
    filetree,
    "filetree"
);
bundled_id!(
    /// `manifest-deps` — dependency nodes from `Cargo.toml`, `go.mod`,
    /// `package.json` and friends.
    manifest_deps,
    "manifest-deps"
);
bundled_id!(
    /// `metrics` — per-file `Loc`, counted by tokei.
    metrics,
    "metrics"
);
bundled_id!(
    /// `external` — references that leave the repository.
    external,
    "external"
);
bundled_id!(
    /// `scip` — the only source of `Resolution::Resolved` provenance.
    scip,
    "scip"
);
bundled_id!(
    /// `readme` — a directory's README paragraph as its module's doc comment.
    readme,
    "readme"
);
bundled_id!(
    /// `test-pairing` — the per-file test-by-convention coverage fact.
    test_pairing,
    "test-pairing"
);

/// Every bundled id of a host extractor, in no particular order.
///
/// The language plugins are not here: their ids come from their manifests at
/// run time, the same way a third-party plugin's does (R1). What the list below
/// holds is the host's own extractors — a second hand-written list of the ones
/// `default_extractors_with` constructs, which is a liability unless
/// something checks the two agree — see
/// `the_bundled_ids_match_the_extractors_that_actually_run` below. With that
/// test the duplication becomes useful: adding an eighth extractor without
/// giving it an id here fails loudly, instead of quietly forcing its
/// emission sites to build an id per fact.
pub fn all() -> Vec<ExtractorId> {
    vec![
        filetree(),
        manifest_deps(),
        metrics(),
        external(),
        scip(),
        readme(),
        test_pairing(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the module: two calls share one allocation. If this
    /// ever fails the emission paths silently went back to allocating per fact.
    #[test]
    fn an_id_is_allocated_once_and_shared() {
        let a = metrics();
        let b = metrics();
        assert!(
            std::ptr::eq(std::sync::Arc::as_ptr(&a), std::sync::Arc::as_ptr(&b)),
            "two calls to the same bundled id must clone one allocation"
        );
    }

    /// The list here and the extractors that actually run must not drift.
    ///
    /// `trust_ranks()` is built from `default_extractors()`, so its keys are
    /// the ids the run genuinely uses. Nothing else compares the two, and a
    /// mismatch has no other symptom: an extractor missing from here still
    /// works, it just has to allocate its id at every emission site — the one
    /// cost this module exists to remove, and invisible in any output.
    #[test]
    fn the_bundled_ids_match_the_extractors_that_actually_run() {
        let mut from_ids: Vec<String> = all().iter().map(|i| i.to_string()).collect();
        // …plus every bundled language plugin, which writes its own id.
        from_ids.extend(
            crate::plugin::bundled::bundled()
                .iter()
                .map(|p| p.id().to_string()),
        );
        from_ids.sort();
        let mut from_extractors: Vec<String> = crate::extract::trust_ranks()
            .ids()
            .map(|i| i.to_string())
            .collect();
        from_extractors.sort();
        assert_eq!(
            from_ids, from_extractors,
            "extract::ids and the extractor set disagree about the bundled ids"
        );
    }

    /// Ordering is content-based, which is what keeps every corpus byte stable
    /// (see the `ExtractorId` doc). A numeric intern handle would not have this
    /// property, which is why the plan forbids one.
    #[test]
    fn ids_order_by_content_not_by_allocation_sequence() {
        // `test_pairing` is constructed last in `all()` but sorts last by
        // content too, so this cannot pass by accident of creation order.
        let mut sorted = all();
        sorted.sort();
        let names: Vec<&str> = sorted.iter().map(|i| &**i).collect();
        assert_eq!(
            names,
            vec![
                "external",
                "filetree",
                "manifest-deps",
                "metrics",
                "readme",
                "scip",
                "test-pairing",
            ]
        );
    }
}
