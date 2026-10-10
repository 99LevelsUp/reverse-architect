//! The run's plugin set: the operator's declared plugins, then the bundled ones
//! (U8).
//!
//! Order is precedence. A file goes, for each pass, to the first plugin in the
//! set that claims its extension and serves that pass (contract 0.3), so a
//! plugin the operator declared wins over a bundled one for the same extension
//! and pass, and among declared plugins the one named first wins. A plugin
//! built against 0.1 or 0.2 serves both passes of the first layer, so for
//! those the rule is what it always was: the first that claims the extension.
//! A plugin that serves only `bodies` therefore never takes a file's
//! declarations or usages from another. The order comes only from the command
//! line — never from a manifest (U8 1c): a plugin cannot promote itself over
//! another.
//!
//! A declared plugin whose id is a bundled plugin's id **replaces** it. That is
//! how an operator swaps one language's extractor and keeps the rest: declare a
//! better `csharp` and the bundled `csharp` is not loaded into the run at all,
//! so no file can reach it by an extension the replacement forgot.

use super::host::{Plugin, PluginError, WireCardinality, WireLinkRole, WireStructuralRole};
use super::manifest::{Pass, PluginManifest};
use crate::extract::languages::{LanguageEntry, Languages};
use crate::graph::{CardPolicy, Cardinality, Graph, LinkRole, StructuralRole, VocabularyError};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One `--plugin` argument: a component on disk, and optionally the SHA-256
/// its bytes must have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    pub path: PathBuf,
    /// Lowercase hex.
    pub sha256: Option<String>,
}

impl std::str::FromStr for PluginSpec {
    type Err = String;

    /// `path/to/plugin.wasm` or `path/to/plugin.wasm=sha256:<64 hex digits>`.
    fn from_str(s: &str) -> Result<Self, String> {
        let (path, sha256) = match s.rsplit_once("=sha256:") {
            Some((path, hex)) => {
                let hex = hex.to_ascii_lowercase();
                if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!(
                        "`{hex}` is not a SHA-256 digest (64 hex digits expected)"
                    ));
                }
                (path, Some(hex))
            }
            None => (s, None),
        };
        if path.is_empty() {
            return Err("a plugin path is required".into());
        }
        Ok(PluginSpec {
            path: PathBuf::from(path),
            sha256,
        })
    }
}

/// Where a plugin in the set came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Built into this binary.
    Bundled,
    /// Named by the operator, at this path.
    Declared(PathBuf),
}

impl Origin {
    /// `bundled`, or `declared` — the word the manifest records. The path is
    /// deliberately not part of any artifact: it is the operator's machine, not
    /// the corpus (KTD7).
    pub fn word(&self) -> &'static str {
        match self {
            Origin::Bundled => "bundled",
            Origin::Declared(_) => "declared",
        }
    }
}

struct Entry {
    plugin: &'static Plugin,
    origin: Origin,
}

/// The plugins of one run, in precedence order. Cheap to clone.
#[derive(Clone)]
pub struct PluginSet {
    entries: Arc<[Entry]>,
}

/// Why the declared plugins could not be assembled into a set.
#[derive(Debug, thiserror::Error)]
pub enum PluginSetError {
    #[error("cannot read plugin {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "plugin {path} does not have the declared digest: expected sha256:{expected}, the file is sha256:{actual}"
    )]
    Digest {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error(transparent)]
    Load(#[from] PluginError),
    #[error(
        "plugin `{id}` declares its kinds under the namespace `{namespace}`; a plugin declares only under its own id"
    )]
    Namespace { id: String, namespace: String },
    #[error("plugin `{id}` declared a kind the corpus refused: {source}")]
    Vocabulary {
        id: String,
        #[source]
        source: VocabularyError,
    },
    #[error(
        "plugin `{id}` gives node kind `{kind}` the card kind `{card}`, which it never declared"
    )]
    UndeclaredCard {
        id: String,
        kind: String,
        card: String,
    },
    #[error("two declared plugins share the id `{id}`: {first} and {second}")]
    DuplicateId {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
}

impl PluginSet {
    /// Only the plugins built into this binary.
    pub fn bundled() -> PluginSet {
        PluginSet {
            entries: super::bundled::bundled()
                .iter()
                .map(|plugin| Entry {
                    plugin,
                    origin: Origin::Bundled,
                })
                .collect(),
        }
    }

    /// The declared plugins, in the order given, then every bundled plugin a
    /// declared one did not replace.
    ///
    /// Each file is read once, its digest checked against the declared one
    /// **before** a single byte of it is compiled or run (U8 1), and then
    /// loaded through the same [`Plugin::load`] a bundled plugin takes. A
    /// loaded plugin lives for the rest of the process: a run holds its
    /// plugins until it ends, and a process runs once.
    pub fn with_declared(specs: &[PluginSpec]) -> Result<PluginSet, PluginSetError> {
        if specs.is_empty() {
            return Ok(PluginSet::bundled());
        }
        let mut entries: Vec<Entry> = Vec::new();
        for spec in specs {
            let plugin = load_declared(spec)?;
            if let Some(first) = entries.iter().find(|e| e.plugin.id() == plugin.id()) {
                let Origin::Declared(first) = &first.origin else {
                    unreachable!("only declared plugins are in the list yet")
                };
                return Err(PluginSetError::DuplicateId {
                    id: plugin.id().to_string(),
                    first: first.clone(),
                    second: spec.path.clone(),
                });
            }
            entries.push(Entry {
                plugin: Box::leak(Box::new(plugin)),
                origin: Origin::Declared(spec.path.clone()),
            });
        }
        for plugin in super::bundled::bundled() {
            if !entries.iter().any(|e| e.plugin.id() == plugin.id()) {
                entries.push(Entry {
                    plugin,
                    origin: Origin::Bundled,
                });
            }
        }
        Ok(PluginSet {
            entries: entries.into(),
        })
    }

    /// Every plugin, in precedence order.
    pub fn iter(&self) -> impl Iterator<Item = &'static Plugin> + '_ {
        self.entries.iter().map(|e| e.plugin)
    }

    /// Every plugin with where it came from, in precedence order.
    pub fn with_origins(&self) -> impl Iterator<Item = (&'static Plugin, &Origin)> + '_ {
        self.entries.iter().map(|e| (e.plugin, &e.origin))
    }

    /// The plugin that reads the usage sites of a file with this extension —
    /// the first that claims it and serves `usages`. For a set of 0.1 and 0.2
    /// plugins, the first that claims the extension at all.
    pub fn for_extension(&self, ext: &str) -> Option<&'static Plugin> {
        self.for_pass(ext, Pass::Usages)
    }

    /// The plugin that makes `pass` over a file with this extension: the first
    /// in precedence order that claims the extension and serves the pass.
    pub fn for_pass(&self, ext: &str, pass: Pass) -> Option<&'static Plugin> {
        let plugins: Vec<&'static Plugin> = self.iter().collect();
        first_serving(plugins.iter().map(|p| p.manifest()), ext, pass).map(|i| plugins[i])
    }

    /// Name the languages of the declared plugins in the run's registry, so
    /// coverage reports a file a plugin parsed under the plugin's language
    /// rather than as `language: null`.
    ///
    /// An extension a bundled language already names keeps that entry: the
    /// operator chose who PARSES those files, and the bundled entry still says
    /// what they are and how their lines are counted. A new extension gets the
    /// plugin's own name and no line counter — "a language this build cannot
    /// count" rather than a guess. Between two declared plugins the first
    /// declared keeps the name, as it keeps the files.
    ///
    /// Every plugin, bundled ones included, is also recorded with the
    /// contract version it runs under and the extensions it parses, so
    /// coverage can say under which contract each language was read (plan A,
    /// U5).
    ///
    /// Only plugins that serve the first layer take part: coverage is about
    /// the graph, and a plugin that reads only bodies (contract 0.3) adds
    /// nothing to it. Every 0.1 and 0.2 plugin serves the first layer.
    pub fn register_languages(&self, languages: &mut Languages) {
        for plugin in self.iter().filter(|p| p.manifest().serves_layer_one()) {
            languages.record_plugin(
                plugin.id(),
                plugin.contract().word(),
                plugin.manifest().extensions(),
            );
        }
        for (plugin, origin) in self.with_origins() {
            if *origin == Origin::Bundled || !plugin.manifest().serves_layer_one() {
                continue;
            }
            for language in &plugin.manifest().languages {
                for ext in &language.extensions {
                    if languages.for_extension(ext).is_none() {
                        // The manifest check already refused a malformed
                        // extension, and a free key cannot collide.
                        let _ = languages.register(
                            ext,
                            LanguageEntry {
                                name: language.name.clone(),
                                loc: None,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Record the plugins the second layer reads bodies with (plan 2a, U4:
    /// KTD9) — every plugin of the set that serves `bodies` and not the first
    /// layer, and then `component`, the bundled body component the run adds
    /// for `--detail-level`. Called only for a run with the second layer, so a
    /// run without it records exactly what [`PluginSet::register_languages`]
    /// did and nothing more.
    ///
    /// They are recorded with their contract and no extension: an extension
    /// names the plugin that parses its declarations, and that stays the
    /// first layer's, so no language's contracts change. What they add is
    /// their count under their contract (`noise.plugins_by_contract`).
    pub fn register_body_languages(&self, languages: &mut Languages, component: Option<&Plugin>) {
        let body_only = self
            .iter()
            .filter(|p| !p.manifest().serves_layer_one() && p.manifest().serves(Pass::Bodies))
            .map(|p| -> &Plugin { p });
        for plugin in body_only.chain(component) {
            languages.record_plugin(
                plugin.id(),
                plugin.contract().word(),
                std::iter::empty::<&str>(),
            );
        }
    }

    /// Declare every plugin's own kinds in `graph`, before a single file is
    /// read (U19).
    ///
    /// A plugin declares only under its own id as namespace, so no plugin can
    /// define — or redefine — a kind another one emits. A kind it then emits
    /// without having declared it is dropped at the boundary with a message
    /// (`output_from_ops`), never guessed at.
    ///
    /// A plugin declares only when the corpus holds a file its extensions
    /// claim (`corpus_extensions`, the text after the last dot of every
    /// tracked path): a corpus nothing of the plugin reads stays as it was
    /// without the plugin, in particular on the schema versions a declared
    /// vocabulary raises.
    pub fn declare_vocabularies(
        &self,
        graph: &mut Graph,
        corpus_extensions: &std::collections::BTreeSet<String>,
    ) -> Result<(), PluginSetError> {
        for plugin in self.iter() {
            let Some(v) = plugin.vocabulary() else {
                continue;
            };
            if !plugin
                .manifest()
                .extensions()
                .iter()
                .any(|e| corpus_extensions.contains(*e))
            {
                continue;
            }
            let id = plugin.id().to_string();
            if v.namespace != id {
                return Err(PluginSetError::Namespace {
                    id,
                    namespace: v.namespace.clone(),
                });
            }
            let refused = |source| PluginSetError::Vocabulary {
                id: plugin.id().to_string(),
                source,
            };
            let mut cards = std::collections::HashMap::new();
            for card in &v.card_kinds {
                let kind = graph
                    .declare_card_kind(&v.namespace, card)
                    .map_err(refused)?;
                cards.insert(card.as_str(), kind);
            }
            for node in &v.node_kinds {
                let card = match &node.card {
                    None => CardPolicy::None,
                    Some(card) => CardPolicy::Card(*cards.get(card.as_str()).ok_or_else(|| {
                        PluginSetError::UndeclaredCard {
                            id: id.clone(),
                            kind: node.name.clone(),
                            card: card.clone(),
                        }
                    })?),
                };
                let role = match node.role {
                    WireStructuralRole::Container => StructuralRole::Container,
                    WireStructuralRole::Leaf => StructuralRole::Leaf,
                    WireStructuralRole::Definition => StructuralRole::Definition,
                    WireStructuralRole::Package => StructuralRole::Package,
                    WireStructuralRole::External => StructuralRole::External,
                    WireStructuralRole::Opaque => StructuralRole::Opaque,
                };
                graph
                    .declare_node_kind(&v.namespace, &node.name, role, card)
                    .map_err(refused)?;
            }
            for edge in &v.edge_kinds {
                let role = match edge.role {
                    WireLinkRole::Containment => LinkRole::Containment,
                    WireLinkRole::Declaration => LinkRole::Declaration,
                    WireLinkRole::Dependency => LinkRole::Dependency,
                    WireLinkRole::Opaque => LinkRole::Opaque,
                };
                graph
                    .declare_edge_kind(&v.namespace, &edge.name, role)
                    .map_err(refused)?;
            }
            for fact in &v.fact_kinds {
                let cardinality = match fact.cardinality {
                    WireCardinality::Additive => Cardinality::Additive,
                    WireCardinality::Scalar => Cardinality::Scalar,
                };
                graph
                    .declare_fact_kind(&v.namespace, &fact.name, cardinality)
                    .map_err(refused)?;
            }
        }
        Ok(())
    }

    /// `(id, digest)` of every plugin, in precedence order — what decides
    /// whether a stored cache record could have come from this set.
    pub fn identities(&self) -> Vec<(String, String)> {
        self.iter()
            .map(|p| (p.id().to_string(), p.digest().to_string()))
            .collect()
    }
}

impl PartialEq for PluginSet {
    fn eq(&self, other: &PluginSet) -> bool {
        self.identities() == other.identities()
    }
}

impl Eq for PluginSet {}

impl std::fmt::Debug for PluginSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.with_origins().map(|(p, o)| (p.id(), o.word())))
            .finish()
    }
}

/// The index of the first manifest, in precedence order, that serves `pass`
/// over `ext` — the one routing rule every (extension, pass) pair follows.
fn first_serving<'m>(
    manifests: impl IntoIterator<Item = &'m PluginManifest>,
    ext: &str,
    pass: Pass,
) -> Option<usize> {
    manifests.into_iter().position(|m| m.serves_on(ext, pass))
}

fn load_declared(spec: &PluginSpec) -> Result<Plugin, PluginSetError> {
    let bytes = std::fs::read(&spec.path).map_err(|source| PluginSetError::Read {
        path: spec.path.clone(),
        source,
    })?;
    if let Some(expected) = &spec.sha256 {
        let actual = hex(&Sha256::digest(&bytes));
        if &actual != expected {
            return Err(PluginSetError::Digest {
                path: spec.path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(Plugin::load(&display(&spec.path), &bytes)?)
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/probe.wasm")
    }

    fn probe3() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/probe3.wasm")
    }

    fn declared(paths: &[PathBuf]) -> PluginSet {
        let specs: Vec<PluginSpec> = paths
            .iter()
            .map(|path| PluginSpec {
                path: path.clone(),
                sha256: None,
            })
            .collect();
        PluginSet::with_declared(&specs).unwrap()
    }

    fn manifest(id: &str, ext: &str, passes: &[Pass]) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            version: "1".into(),
            languages: vec![super::super::manifest::PluginLanguage {
                name: ext.into(),
                extensions: vec![ext.into()],
            }],
            trust_rank: 30,
            depends_on: Vec::new(),
            passes: passes.to_vec(),
            emits: Vec::new(),
        }
    }

    /// Two components for one extension, one for the first layer and one for
    /// bodies only: each gets its own passes whichever comes first, and the
    /// bodies-only one never takes the first layer's files.
    #[test]
    fn a_pass_goes_to_the_first_plugin_that_serves_it() {
        let bodies = manifest("bodies", "cs", &[Pass::Bodies]);
        let layer_one = manifest("layer-one", "cs", &Pass::LAYER_ONE);
        for order in [[&bodies, &layer_one], [&layer_one, &bodies]] {
            let id = |pass| first_serving(order, "cs", pass).map(|i| order[i].id.as_str());
            assert_eq!(id(Pass::Declarations), Some("layer-one"));
            assert_eq!(id(Pass::Usages), Some("layer-one"));
            assert_eq!(id(Pass::Bodies), Some("bodies"));
        }
        assert_eq!(first_serving([&bodies], "cs", Pass::Declarations), None);
        assert_eq!(first_serving([&layer_one], "cs", Pass::Bodies), None);
    }

    /// For one extension and one pass the first plugin wins, as it always did:
    /// a declared plugin ahead of a bundled one takes the pass from it.
    #[test]
    fn the_first_plugin_serving_the_same_pair_wins() {
        let declared = manifest("declared", "cs", &Pass::ALL);
        let bundled = manifest("bundled", "cs", &Pass::LAYER_ONE);
        let order = [&declared, &bundled];
        for pass in Pass::ALL {
            assert_eq!(first_serving(order, "cs", pass), Some(0), "{pass:?}");
        }
        assert_eq!(
            first_serving([&bundled, &declared], "cs", Pass::Usages),
            Some(0)
        );
        assert_eq!(
            first_serving([&bundled, &declared], "cs", Pass::Bodies),
            Some(1)
        );
    }

    /// The 0.1 probe and the 0.3 probe3 both claim `.probe`. Declared probe
    /// first, the first layer stays the probe's and `bodies` goes to probe3,
    /// the one that serves it; both pass registration, and coverage records
    /// each under its own contract.
    #[test]
    fn two_components_for_one_extension_each_get_their_pass() {
        use crate::plugin::host::Contract;
        let set = declared(&[probe(), probe3()]);
        let id = |pass| set.for_pass("probe", pass).map(|p| p.id());
        assert_eq!(id(Pass::Declarations), Some("probe"));
        assert_eq!(id(Pass::Usages), Some("probe"));
        assert_eq!(id(Pass::Bodies), Some("probe3"));
        assert_eq!(set.for_extension("probe").map(|p| p.id()), Some("probe"));
        assert_eq!(
            set.for_pass("probe3", Pass::Declarations)
                .map(|p| p.contract()),
            Some(Contract::V0_3)
        );

        let mut languages = Languages::bundled();
        set.register_languages(&mut languages);
        assert_eq!(
            languages.for_extension("probe").map(|l| l.name.as_str()),
            Some("probe")
        );
        assert_eq!(
            languages.for_extension("probe3").map(|l| l.name.as_str()),
            Some("probe3")
        );
        assert_eq!(languages.contract_for("a.probe"), Some("0.1"));
        assert_eq!(languages.contract_for("a.probe3"), Some("0.3"));

        // Declared the other way round, probe3 serves all three passes and
        // takes them all — the same precedence a declared plugin has over a
        // bundled one.
        let set = declared(&[probe3(), probe()]);
        for pass in Pass::ALL {
            assert_eq!(set.for_pass("probe", pass).map(|p| p.id()), Some("probe3"));
        }
    }

    #[test]
    fn a_spec_reads_a_path_and_an_optional_digest() {
        let plain: PluginSpec = "a/b.wasm".parse().unwrap();
        assert_eq!(plain.path, PathBuf::from("a/b.wasm"));
        assert_eq!(plain.sha256, None);

        let hex = "AB".repeat(32);
        let pinned: PluginSpec = format!("c:/x.wasm=sha256:{hex}").parse().unwrap();
        assert_eq!(pinned.path, PathBuf::from("c:/x.wasm"));
        assert_eq!(pinned.sha256, Some(hex.to_ascii_lowercase()));

        assert!("x.wasm=sha256:1234".parse::<PluginSpec>().is_err());
        assert!("=sha256:".parse::<PluginSpec>().is_err());
    }

    #[test]
    fn a_declared_plugin_comes_first_and_the_bundled_ones_follow() {
        let set = PluginSet::with_declared(&[PluginSpec {
            path: probe(),
            sha256: None,
        }])
        .unwrap();
        let ids: Vec<&str> = set.iter().map(|p| p.id()).collect();
        assert_eq!(ids[0], "probe");
        assert_eq!(ids.len(), 1 + super::super::bundled::BUNDLED.len());
        assert_eq!(set.for_extension("probe").map(|p| p.id()), Some("probe"));
        assert_eq!(set.for_extension("rs").map(|p| p.id()), Some("rust"));
        let origins: Vec<&str> = set.with_origins().map(|(_, o)| o.word()).collect();
        assert_eq!(origins[0], "declared");
        assert!(origins[1..].iter().all(|o| *o == "bundled"));
    }

    #[test]
    fn a_declared_plugin_with_a_bundled_id_replaces_the_bundled_one() {
        let rust = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/rust.wasm");
        let set = PluginSet::with_declared(&[PluginSpec {
            path: rust,
            sha256: None,
        }])
        .unwrap();
        let rusts: Vec<&Origin> = set
            .with_origins()
            .filter(|(p, _)| p.id() == "rust")
            .map(|(_, o)| o)
            .collect();
        assert_eq!(
            rusts.len(),
            1,
            "the bundled rust must not stay beside its replacement"
        );
        assert_eq!(rusts[0].word(), "declared");
        assert_eq!(set.iter().count(), super::super::bundled::BUNDLED.len());
    }

    #[test]
    fn a_wrong_digest_is_refused_before_the_component_is_loaded() {
        let err = PluginSet::with_declared(&[PluginSpec {
            path: probe(),
            sha256: Some("0".repeat(64)),
        }])
        .map(|_| ())
        .unwrap_err();
        assert!(matches!(err, PluginSetError::Digest { .. }), "{err}");
    }

    #[test]
    fn the_right_digest_is_accepted() {
        let bytes = std::fs::read(probe()).unwrap();
        let set = PluginSet::with_declared(&[PluginSpec {
            path: probe(),
            sha256: Some(hex(&Sha256::digest(&bytes))),
        }]);
        assert!(set.is_ok());
    }

    #[test]
    fn declaring_the_same_plugin_twice_is_refused() {
        let spec = PluginSpec {
            path: probe(),
            sha256: None,
        };
        let err = PluginSet::with_declared(&[spec.clone(), spec])
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, PluginSetError::DuplicateId { .. }), "{err}");
    }

    #[test]
    fn a_missing_file_is_a_readable_error() {
        let err = PluginSet::with_declared(&[PluginSpec {
            path: PathBuf::from("no/such/plugin.wasm"),
            sha256: None,
        }])
        .map(|_| ())
        .unwrap_err();
        assert!(matches!(err, PluginSetError::Read { .. }), "{err}");
    }

    #[test]
    fn the_set_compares_by_what_it_loaded() {
        assert_eq!(PluginSet::bundled(), PluginSet::bundled());
        let with_probe = PluginSet::with_declared(&[PluginSpec {
            path: probe(),
            sha256: None,
        }])
        .unwrap();
        assert_ne!(with_probe, PluginSet::bundled());
    }
}
