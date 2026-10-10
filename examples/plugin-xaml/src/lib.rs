//! `xaml` — a reverse-architect language plugin for XAML, built as a third
//! party would build it: only against the published contract
//! (`wit-v2/extractor.wit`, `ra:plugin@0.2.0`) and `docs/plugins.md`.
//!
//! - [`parse`] reads a file into a tree of elements and attributes with
//!   their positions, and takes markup extensions apart.
//! - [`xmlns`] says what a prefix means and which type a spelling names.
//! - [`declare`] is pass 1: styles, templates, resources, named template
//!   parts, the `x:Class` root, merged dictionaries and `BasedOn`.
//! - [`scope`] is pass 2: the types the file names, with its one scope.
//!
//! This file only wires them to the contract. Invalid XML fails the file:
//! the exported functions panic, and the release profile's `panic = "abort"`
//! turns that into a trap the host reports for that file alone.

wit_bindgen::generate!({
    path: "../../wit-v2",
    world: "language-plugin",
});

pub mod declare;
pub mod parse;
pub mod scope;
pub mod xmlns;

use exports::ra::plugin::extractor::Guest;
use ra::plugin::types::{Config, FileUsages, Language, Manifest, Op, Vocabulary};

// Constructed only by the export glue, which exists only in the component.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
struct Plugin;

impl Guest for Plugin {
    fn describe() -> Manifest {
        Manifest {
            id: "xaml".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            languages: vec![Language {
                name: "xaml".into(),
                extensions: vec!["xaml".into()],
            }],
            trust_rank: 30,
            depends_on: Vec::new(),
        }
    }

    fn declare() -> Option<Vocabulary> {
        // Every kind used is a core kind or a plain `symbol_kind` value
        // (`style`, `template`, `resource`, `element`, `view`), so nothing
        // needs declaring.
        None
    }

    fn declarations(path: String, source: Vec<u8>, _config: Config) -> Vec<Op> {
        // Text this plugin quotes (a built signature) is redacted by the host
        // with the operator's prefixes, so the plugin does not need the list.
        declare::declarations(&path, &source, &ra::plugin::host::path_exists)
            .unwrap_or_else(|e| panic!("{path}: not valid XML: {e}"))
    }

    fn usages(path: String, source: Vec<u8>) -> FileUsages {
        scope::usages(&source).unwrap_or_else(|e| panic!("{path}: not valid XML: {e}"))
    }
}

// Only the component exports the interface; a native build is for the tests.
#[cfg(target_family = "wasm")]
export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_names_the_xaml_language() {
        let manifest = Plugin::describe();
        assert_eq!(manifest.id, "xaml");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.trust_rank, 30);
        assert!(manifest.depends_on.is_empty());
        assert_eq!(manifest.languages.len(), 1);
        assert_eq!(manifest.languages[0].name, "xaml");
        assert_eq!(manifest.languages[0].extensions, ["xaml"]);
        assert!(Plugin::declare().is_none());
    }
}
