//! Taxonomy profiles (U13, R18): named, explicit overrides of the automatic
//! taxonomy.
//!
//! A profile is an *override*, not an alternative catalog — it selects among the
//! existing catalog views only (E11: a profile never invents a view). The seed
//! profile `minimal` narrows the corpus to the two most universal backbone
//! views. Adding a named profile is a one-line entry in [`PROFILES`]; the full
//! profile catalog is deferred to the first real need (plan open question).
//!
//! This module is the single source of truth for the name→view-set mapping:
//! `Config::resolve` validates an unknown name against it (failing early with a
//! readable error), and `select_views` applies the very same mapping — so the
//! validated set and the applied set can never drift.

/// The seed profile table: profile name → the ordered view ids it selects.
///
/// Every id here MUST name a real catalog view (`taxonomy::views::catalog`);
/// the `seed_profile_ids_are_real_catalog_views` test enforces it.
const PROFILES: &[(&str, &[&str])] = &[
    // `minimal` — the truthful first picture only: what the repo is (overview)
    // and how it is partitioned (module structure). A strict subset of the
    // automatic backbone.
    ("minimal", &["overview", "module-structure"]),
];

/// An unknown profile name reached the taxonomy layer. Returned by
/// [`view_ids_checked`] so `select_views` (and the projections above it) can
/// surface a readable error instead of panicking — the CLI validates earlier in
/// `Config::resolve`, but the projection functions are public and a library
/// caller can construct a `Config`/profile the CLI never validated.
#[derive(Debug, thiserror::Error)]
#[error("unknown taxonomy profile: {name}")]
pub struct UnknownProfile {
    pub name: String,
}

/// The view ids a named profile selects, as a `Result` — the checked form used
/// by `select_views` so an unknown name is an error, never a panic.
pub fn view_ids_checked(name: &str) -> Result<&'static [&'static str], UnknownProfile> {
    view_ids(name).ok_or_else(|| UnknownProfile {
        name: name.to_string(),
    })
}

/// The view ids a named profile selects, or `None` if the name is unknown.
///
/// `None` is the signal `Config::resolve` turns into a readable
/// `UnknownProfile` error at CLI time.
pub fn view_ids(name: &str) -> Option<&'static [&'static str]> {
    PROFILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ids)| *ids)
}

/// The known profile names, sorted — for a readable "unknown profile" error.
pub fn known() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = PROFILES.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taxonomy::views::catalog;

    #[test]
    fn minimal_selects_overview_and_module_structure() {
        assert_eq!(
            view_ids("minimal"),
            Some(&["overview", "module-structure"][..])
        );
    }

    #[test]
    fn unknown_profile_is_none() {
        assert_eq!(view_ids("does-not-exist"), None);
    }

    #[test]
    fn known_lists_the_seed_profile() {
        assert!(known().contains(&"minimal"));
    }

    /// E11: a profile selects among existing catalog views only — never invents
    /// one. Every id in every profile must resolve to a real catalog view.
    #[test]
    fn seed_profile_ids_are_real_catalog_views() {
        let catalog_ids: Vec<&str> = catalog().iter().map(|v| v.id).collect();
        for (profile, ids) in PROFILES {
            for id in *ids {
                assert!(
                    catalog_ids.contains(id),
                    "profile `{profile}` names `{id}`, which is not a catalog view"
                );
            }
        }
    }
}
