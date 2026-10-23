//! The languages the second layer's analyses read, one profile each (plan
//! vrstva 3, KTD1).
//!
//! The analyses themselves are not about a language: an entry point, a
//! cluster, a precondition or a state is read from the model of bodies a
//! component reports. What a language decides is which first-layer keys are
//! its types, which files its bodies come from, which kinds of type have
//! functionalities, how a key splits into the container that holds the type
//! and the type's path in it, and which kinds of entry point its idioms
//! name. A language without a profile gets no second layer, and the
//! coverage admits it as it admits any type without one.
//!
//! The first-layer key of a symbol is `sym:<language>:<container>#<path>`
//! (`walkers.rs`), so the shape is shared; only the separator of the
//! container is the language's (`Acme.Views`, `crate::extract`).
//!
//! Limit: the index of types by name is one over every profiled language,
//! so a name two languages declare a type of stays ambiguous rather than
//! being told by the language of the body that wrote it.

use super::entry::EntryKind;
use crate::extract::bodies::BodyLayer;
use crate::graph::NodeKey;

/// What the analyses need to know of one language.
#[derive(Debug, PartialEq, Eq)]
pub struct Profile {
    /// What the first-layer key of the language's symbols starts with.
    keys: &'static str,
    /// The extension of the files its bodies are read from.
    extension: &'static str,
    /// The symbol kinds the analysis reads functionalities of.
    analyzed: &'static [&'static str],
    /// What joins the segments of a key's container.
    separator: &'static str,
    /// The kinds of entry point the language's idioms are read for.
    entries: &'static [EntryKind],
}

/// C#: types in namespaces, the idioms of WPF and MVVM among its entry
/// points (plan 2a, KTD12).
pub const CSHARP: Profile = Profile {
    keys: "sym:csharp:",
    extension: ".cs",
    analyzed: &["class", "struct", "record"],
    separator: ".",
    entries: &[
        EntryKind::Command,
        EntryKind::Key,
        EntryKind::MenuItem,
        EntryKind::Handler,
        EntryKind::PublicMethod,
    ],
};

/// Every profile, the order the analyses meet them in.
const PROFILES: &[&Profile] = &[&CSHARP];

/// The profile of the language `key` is a symbol of, if it has one.
pub fn of_key(key: &NodeKey) -> Option<&'static Profile> {
    of_key_in(PROFILES, key)
}

/// The profile among `profiles` of the language `key` is a symbol of.
fn of_key_in(profiles: &[&'static Profile], key: &NodeKey) -> Option<&'static Profile> {
    profiles
        .iter()
        .copied()
        .find(|p| key.as_str().starts_with(p.keys))
}

impl Profile {
    /// Whether the second layer read a part of the type `ty` from a file of
    /// this language (plan 2a, KTD6).
    pub fn reads(&self, layer: &BodyLayer, ty: &NodeKey) -> bool {
        layer
            .type_shapes_of(ty)
            .iter()
            .any(|(path, _)| path.ends_with(self.extension))
    }

    /// The container and the type path of `key` — `Acme.Views` and
    /// `Outer.Inner` of `sym:csharp:Acme.Views#Outer.Inner` — when `key`
    /// is a key of this language.
    pub fn split<'k>(&self, key: &'k NodeKey) -> Option<(&'k str, &'k str)> {
        key.as_str().strip_prefix(self.keys)?.split_once('#')
    }

    /// The segments of a container, in order (`Acme`, `Views`).
    pub fn container_segments<'c>(&self, container: &'c str) -> impl Iterator<Item = &'c str> {
        container.split(self.separator)
    }

    /// Whether the analysis reads functionalities of a type of the symbol
    /// kind `kind` (`class`).
    pub fn analyzes(&self, kind: &str) -> bool {
        self.analyzed.contains(&kind)
    }

    /// Whether the language's idioms are read for entry points of `kind`.
    pub fn seeds(&self, kind: EntryKind) -> bool {
        self.entries.contains(&kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_finds_the_profile_of_its_language_and_none_without_one() {
        assert_eq!(of_key(&NodeKey::new("sym:csharp:Acme#View")), Some(&CSHARP));
        assert_eq!(of_key(&NodeKey::new("sym:python:pkg#View")), None);
        assert_eq!(of_key(&NodeKey::new("sym:csharpish:Acme#View")), None);
    }

    #[test]
    fn a_key_splits_into_its_container_and_its_type_path() {
        let key = NodeKey::new("sym:csharp:Acme.Views#Outer.Inner");
        assert_eq!(CSHARP.split(&key), Some(("Acme.Views", "Outer.Inner")));
        assert_eq!(
            CSHARP.container_segments("Acme.Views").collect::<Vec<_>>(),
            ["Acme", "Views"]
        );
        assert_eq!(CSHARP.split(&NodeKey::new("sym:rust:crate#T")), None);
    }

    /// A second, narrower language beside C#: each key finds its own
    /// profile, the narrower one analyzes only its own kinds and names only
    /// its own entry points, and C# answers as it does alone.
    #[test]
    fn a_second_profile_reads_only_its_own_types_and_leaves_csharp_as_it_was() {
        const NARROW: Profile = Profile {
            keys: "sym:narrow:",
            extension: ".nw",
            analyzed: &["struct"],
            separator: "::",
            entries: &[EntryKind::PublicMethod],
        };
        let both: &[&Profile] = &[&CSHARP, &NARROW];
        let narrow = NodeKey::new("sym:narrow:crate::io#Reader");
        let csharp = NodeKey::new("sym:csharp:Acme.Views#Editor");
        assert_eq!(of_key_in(both, &narrow), Some(&NARROW));
        assert_eq!(of_key_in(both, &csharp), Some(&CSHARP));
        assert_eq!(of_key_in(both, &csharp), of_key(&csharp));

        assert_eq!(NARROW.split(&narrow), Some(("crate::io", "Reader")));
        assert_eq!(
            NARROW.container_segments("crate::io").collect::<Vec<_>>(),
            ["crate", "io"]
        );
        assert_eq!(NARROW.split(&csharp), None);
        assert!(NARROW.analyzes("struct") && !NARROW.analyzes("class"));
        assert!(NARROW.seeds(EntryKind::PublicMethod));
        for kind in [
            EntryKind::Command,
            EntryKind::Key,
            EntryKind::MenuItem,
            EntryKind::Handler,
        ] {
            assert!(!NARROW.seeds(kind), "{kind:?}");
            assert!(CSHARP.seeds(kind), "{kind:?}");
        }
        assert!(CSHARP.analyzes("class") && CSHARP.analyzes("record"));
    }
}
