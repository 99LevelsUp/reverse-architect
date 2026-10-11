//! `xaml` — a reverse-architect language plugin for XAML, built as a third
//! party would build it: only against the published contract
//! (`wit-v3/extractor.wit`, `ra:plugin@0.3.0`) and `docs/plugins.md`.
//!
//! - [`parse`] reads a file into a tree of elements and attributes with
//!   their positions, and takes markup extensions apart.
//! - [`xmlns`] says what a prefix means and which type a spelling names.
//! - [`declare`] is pass 1: styles, templates, resources, named template
//!   parts and the `x:Class` root as nodes of the plugin's own kinds, merged
//!   dictionaries, the file's `*.Resources` blocks and the references by
//!   name (`{StaticResource}`, `{DynamicResource}`, `BasedOn`).
//! - [`scope`] is pass 2: the types the file names, with its one scope.
//! - [`bodies`] is pass 3: the bodies of styles, templates and the `x:Class`
//!   root — bindings, commands, event handlers, setters and literals.
//!
//! This file only wires them to the contract. A file that is not valid XML
//! is refused by every pass with a short reason (`err`), never a trap.

wit_bindgen::generate!({
    path: "../../wit-v3",
    world: "language-plugin",
    additional_derives: [PartialEq],
});

pub mod bodies;
pub mod declare;
pub mod parse;
pub mod scope;
pub mod xmlns;

use exports::ra::plugin::extractor::Guest;
use ra::plugin::types::{
    Config, FileBodies, FileDeclarations, FileUsages, Language, Manifest, NodeKindDecl, Pass,
    StructuralRole, Vocabulary,
};

/// The namespace of this plugin's own kinds; equal to its id.
pub const NAMESPACE: &str = "xaml";

/// The node kinds this plugin declares, each with a card of its own, and
/// their structural roles. In `declare()` a kind is named bare (`style`), the
/// namespace beside it; a symbol's `node-kind` and a reference's
/// `target-kind` spell it `namespace:name` (`xaml:style`, [`declare::STYLE`]).
///
/// Every one is a `definition`: a declaration inside a file. The role places
/// a node in the host's backbone views, not in a hierarchy of XAML: a
/// `container` is a module of the directory tree, whose key must be a
/// `/`-path with a node at every level, and a `leaf` is file-like.
pub const NODE_KINDS: [(&str, StructuralRole); 5] = [
    ("style", StructuralRole::Definition),
    ("template", StructuralRole::Definition),
    ("resource", StructuralRole::Definition),
    ("element", StructuralRole::Definition),
    ("view", StructuralRole::Definition),
];

// Constructed only by the export glue, which exists only in the component.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
struct Plugin;

/// The short reason a pass gives for a file it cannot read.
fn refused(e: parse::Error) -> String {
    format!("not valid XML: {e}")
}

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
            passes: vec![Pass::Declarations, Pass::Usages, Pass::Bodies],
            emits: bodies::EMITS.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn declare() -> Option<Vocabulary> {
        // Every kind gets a card of its own, named like the kind. Both are
        // bare names; the host puts them under the namespace.
        Some(Vocabulary {
            namespace: NAMESPACE.into(),
            card_kinds: NODE_KINDS.iter().map(|(k, _)| k.to_string()).collect(),
            node_kinds: NODE_KINDS
                .iter()
                .map(|&(name, role)| NodeKindDecl {
                    name: name.into(),
                    role,
                    card: Some(name.into()),
                })
                .collect(),
            edge_kinds: Vec::new(),
            fact_kinds: Vec::new(),
        })
    }

    fn declarations(
        path: String,
        source: Vec<u8>,
        _config: Config,
    ) -> Result<FileDeclarations, String> {
        // Text this plugin quotes (a built signature) is redacted by the host
        // with the operator's prefixes, so the plugin does not need the list.
        declare::declarations(&path, &source, &ra::plugin::host::path_exists).map_err(refused)
    }

    fn usages(_path: String, source: Vec<u8>) -> Result<FileUsages, String> {
        scope::usages(&source).map_err(refused)
    }

    fn bodies(_path: String, source: Vec<u8>) -> Result<FileBodies, String> {
        bodies::bodies(&source).map_err(refused)
    }
}

// Only the component exports the interface; a native build is for the tests.
#[cfg(target_family = "wasm")]
export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_names_the_xaml_language_and_its_three_passes() {
        let manifest = Plugin::describe();
        assert_eq!(manifest.id, "xaml");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.trust_rank, 30);
        assert!(manifest.depends_on.is_empty());
        assert_eq!(manifest.languages.len(), 1);
        assert_eq!(manifest.languages[0].name, "xaml");
        assert_eq!(manifest.languages[0].extensions, ["xaml"]);
        assert_eq!(
            manifest.passes,
            [Pass::Declarations, Pass::Usages, Pass::Bodies]
        );
        for name in [
            "member-access",
            "subscription",
            "string-literal",
            "data-source",
            "target-type",
            "trigger",
            "control-key",
            "key-gesture",
            "unparsed-markup",
        ] {
            assert!(
                manifest.emits.iter().any(|e| e == name),
                "{name} in {:?}",
                manifest.emits
            );
        }
    }

    #[test]
    fn the_vocabulary_declares_five_kinds_with_cards_under_the_plugin_namespace() {
        let vocabulary = Plugin::declare().expect("a vocabulary");
        assert_eq!(vocabulary.namespace, "xaml");
        let kinds: Vec<&str> = vocabulary
            .node_kinds
            .iter()
            .map(|k| k.name.as_str())
            .collect();
        // Bare in the vocabulary: the host qualifies them with the namespace,
        // and refuses a name that carries `:` itself.
        assert_eq!(kinds, ["style", "template", "resource", "element", "view"]);
        // …and qualified wherever a symbol or a reference names one.
        let qualified: Vec<String> = kinds.iter().map(|k| format!("{NAMESPACE}:{k}")).collect();
        assert_eq!(
            qualified,
            [
                declare::STYLE,
                declare::TEMPLATE,
                declare::RESOURCE,
                declare::ELEMENT,
                declare::VIEW
            ]
        );
        for kind in &vocabulary.node_kinds {
            let card = kind.card.as_deref().expect("a card policy");
            assert!(vocabulary.card_kinds.iter().any(|c| c == card), "{card}");
            assert!(!kind.name.contains(':') && !card.contains(':'), "{card}");
            // Declarations inside a file: no module of the tree, nothing
            // file-like.
            assert_eq!(kind.role, StructuralRole::Definition, "{}", kind.name);
        }
        assert!(vocabulary.card_kinds.iter().all(|c| !c.contains(':')));
        assert!(vocabulary.edge_kinds.is_empty() && vocabulary.fact_kinds.is_empty());
    }

    #[test]
    fn invalid_xml_is_refused_by_every_pass_with_a_reason() {
        let broken = b"<Grid><Button></Grid>".to_vec();
        // `Plugin::declarations` asks the host import, which a native build
        // does not have; the same function runs here with a stand-in.
        let d = declare::declarations("x.xaml", &broken, &|_| false).map_err(refused);
        let u = Plugin::usages("x.xaml".into(), broken.clone());
        let b = Plugin::bodies("x.xaml".into(), broken);
        for reason in [d.err(), u.err(), b.err()] {
            let reason = reason.expect("an err");
            assert!(reason.starts_with("not valid XML: "), "{reason}");
        }
    }
}
