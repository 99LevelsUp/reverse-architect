//! The plugin sandbox and the calls across it (U7).
//!
//! ## What a plugin is given
//!
//! Exactly one source file per call, the run's secret prefixes, and one host
//! query: whether a repository path exists, answered from a snapshot frozen
//! before the walk. Everything the WASI linker adds is linked — a component
//! built by an ordinary Rust toolchain imports it — and every piece of it is
//! answered with nothing:
//!
//! | Interface | Disposition |
//! |---|---|
//! | `wasi:filesystem` | linked, zero preopens — every open fails |
//! | `wasi:sockets` | linked, TCP, UDP and name lookup refused |
//! | `wasi:cli/environment` | empty environment, empty arguments |
//! | `wasi:cli/stdin`, `stdout`, `stderr` | stdin empty, both outputs discarded |
//! | `wasi:cli/exit` | a guest exit is a trap, reported like any other |
//! | `wasi:clocks` | frozen at zero — a plugin cannot put a time into a fact |
//! | `wasi:random` (secure and insecure) | one fixed byte stream, a fixed seed |
//!
//! The randomness is deterministic on purpose: Rust's `HashMap` seeds itself
//! from it, and a plugin that iterates a map must produce the same bytes on
//! every run (R18).
//!
//! ## Bounds
//!
//! Fuel bounds every call, scaled from the input size; a memory ceiling bounds
//! every store. A plugin that exhausts either, or traps, fails that one file:
//! the session discards its instance and makes a fresh one for the next file,
//! so one bad input cannot poison the rest of the run.
//!
//! The engine canonicalizes NaNs and disables relaxed SIMD, the two ways the
//! same component could compute different bits on different CPUs.
//!
//! ## Three contract versions
//!
//! `wit/` holds `ra:plugin@0.1.0` and `wit-v2/` holds `ra:plugin@0.2.0`, both
//! frozen; `wit-v3/` holds `ra:plugin@0.3.0`, which adds the `bodies` pass,
//! declared node kinds for symbols and references by name. Each has its own
//! bindings module below. A component is tried against 0.3, then 0.2, then
//! 0.1, and runs under the first whose `extractor` it exports — so a plugin
//! built against an older contract keeps working, and one built against a
//! newer one is never mistaken for it.
//!
//! The rest of the host speaks 0.2. A 0.1 plugin's output is lifted at this
//! boundary ([`lift`]), where it gains the parts 0.2 adds as absent (no
//! column, no context), which is all 0.1 could say. A 0.3 plugin's output is
//! lowered ([`lower`]): what 0.2 can say passes as 0.2, and what it cannot —
//! a symbol's node kind and block, the references by name, the lookup blocks —
//! travels beside it in [`FileDeclarations`]. The one 0.3 shape that leaves
//! this module as it is is the output of `bodies` ([`FileBodies`] and the
//! records under it), which only the second layer reads.

use super::manifest::{ManifestError, Pass, PluginLanguage, PluginManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{
    Deterministic, HostMonotonicClock, HostWallClock, WasiCtx, WasiCtxBuilder, WasiCtxView,
    WasiView,
};

/// Bindings of the frozen `ra:plugin@0.1.0` contract.
mod v1 {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "language-plugin",
    });
}

/// Bindings of `ra:plugin@0.2.0` — the shape everything past [`Session`]
/// works in.
mod v2 {
    wasmtime::component::bindgen!({
        path: "wit-v2",
        world: "language-plugin",
    });
}

/// Bindings of `ra:plugin@0.3.0`. Only the output of `bodies` leaves this
/// module in these types; everything else is lowered to 0.2 ([`lower`]).
mod v3 {
    wasmtime::component::bindgen!({
        path: "wit-v3",
        world: "language-plugin",
    });
}

pub use v2::ra::plugin::types::{
    Cardinality as WireCardinality, LinkRole as WireLinkRole, StructuralRole as WireStructuralRole,
};
pub use v2::ra::plugin::types::{
    Config as WireConfig, DeclaredType, Edge as WireEdge, Fact as WireFact, FileUsages, KeyedFact,
    Op, Place, Position, Receiver, Relation as WireRelation, RelationKind, Scope as WireScope,
    SiteContext, Span as WireSpan, Symbol as WireSymbol, Usage, UsageKind, UsingDirective, Value,
    Vocabulary as WireVocabulary,
};

/// What `bodies` returns for one file (contract 0.3), under the names the
/// second layer reads it by. Unredacted: the host redacts the texts where it
/// stores them.
pub use v3::ra::plugin::types::{
    AccessMode, AccessPath, Argument as CallArgument, Attribute as BodyAttribute, BaseType,
    BodyKind, BodyNode, CallKind, CompareOp, Condition as BodyCondition, Constant as BodyConstant,
    DeclarationKind, DeclarationMeta, Expr as BodyExpr, FileBodies, FlowTarget, Handler,
    Invocation, Lambda, LiteralContext, MemberAccess, MemberBody, NameOf, NodeKind as BodyNodeKind,
    PathRoot, Position as BodyPosition, Range as BodyRange, StringLiteral, Subscription,
    SubscriptionMode, Tag as BodyTag, TypeShape, TypedName, ValueRef,
};

/// What `declarations` returns for one file, in the host's shape.
///
/// `ops` is everything 0.2 can say. The rest is what only 0.3 can say, and is
/// empty for a 0.1 or 0.2 plugin.
#[derive(Debug, Clone, Default)]
pub struct FileDeclarations {
    pub ops: Vec<Op>,
    /// What a 0.3 symbol says beyond a 0.2 one, for the symbols that say
    /// anything: by the symbol's index in `ops`, in `ops` order.
    pub symbols: Vec<SymbolExtra>,
    /// References by name, for the host to resolve once every file is read.
    pub name_refs: Vec<NameRef>,
    /// The nested blocks names are looked up in; `NameRef::block`,
    /// `SymbolExtra::block` and `LookupBlock::parent` index this list.
    pub blocks: Vec<LookupBlock>,
}

/// What a 0.3 symbol (`ops[op]`, an [`Op::Symbol`]) says that a 0.2 one
/// cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolExtra {
    /// The symbol's index in [`FileDeclarations::ops`].
    pub op: usize,
    /// A node kind from the plugin's own vocabulary, `namespace:name`; `None`
    /// for an ordinary symbol.
    pub node_kind: Option<String>,
    /// The innermost lookup block the declaration is written in.
    pub block: Option<u32>,
}

/// A reference to a declaration by its name alone (contract 0.3, `name-ref`),
/// as the host keeps it until the whole repository has been read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRef {
    /// The key of the node that refers: a symbol the file declares, or the
    /// file itself.
    pub source: String,
    /// The name as the reference spells it.
    pub name: String,
    /// The node kind it can resolve to, `namespace:name`.
    pub target_kind: String,
    /// The edge kind the resolved reference becomes; `None` for a plain
    /// reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    /// The innermost lookup block the reference is written in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<u32>,
    /// The lines it was read from.
    pub span: crate::graph::Span,
    /// Where the name is written, `(line, column)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_at: Option<(u32, u32)>,
}

/// A nested block names are looked up in (contract 0.3, `lookup-block`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookupBlock {
    /// The lines it spans, inclusive.
    pub lines: (u32, u32),
    /// The enclosing block; `None` for an outermost one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
}

/// The contract version a plugin runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Contract {
    /// `ra:plugin@0.1.0`: usage sites carry a name, a kind and a line.
    V0_1,
    /// `ra:plugin@0.2.0`: usage sites may carry a column and context.
    V0_2,
    /// `ra:plugin@0.3.0`: the `bodies` pass, declared node kinds for symbols,
    /// references by name.
    V0_3,
}

impl Contract {
    /// Every version this host loads, the one it tries first first.
    pub const SUPPORTED: [Contract; 3] = [Contract::V0_3, Contract::V0_2, Contract::V0_1];

    /// The short word artifacts record (`0.1`, `0.2`): the minor version is
    /// the whole of what a consumer can act on, since only a patch release is
    /// compatible within one.
    pub fn word(self) -> &'static str {
        match self {
            Contract::V0_1 => "0.1",
            Contract::V0_2 => "0.2",
            Contract::V0_3 => "0.3",
        }
    }

    /// The WIT package, as a plugin author writes it.
    pub fn package(self) -> &'static str {
        match self {
            Contract::V0_1 => "ra:plugin@0.1.0",
            Contract::V0_2 => "ra:plugin@0.2.0",
            Contract::V0_3 => "ra:plugin@0.3.0",
        }
    }
}

/// The exported interface a component must carry to run under some contract.
const EXTRACTOR_EXPORT: &str = "ra:plugin/extractor@";

/// Fuel every call gets regardless of input size.
const FUEL_FLOOR: u64 = 500_000_000;
/// Fuel per byte of input. A tree-sitter parse and walk of a 200 KB file costs
/// on the order of 10^9 units; this leaves an order of magnitude of headroom.
const FUEL_PER_BYTE: u64 = 50_000;
/// Linear-memory ceiling per store.
const MEMORY_LIMIT: usize = 512 << 20;

/// Why a plugin could not be loaded or a call did not complete.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{what} is not a WebAssembly component (a core module, or not wasm at all)")]
    NotAComponent { what: String },
    #[error("{what} could not be compiled: {reason}")]
    Compile { what: String, reason: String },
    #[error(
        "{what} cannot run in this host — it imports something the host does not offer: {reason}"
    )]
    Link { what: String, reason: String },
    /// The component exports no `extractor` this host knows. Not a missing
    /// import — the host offers everything such a component might ask for;
    /// what is missing is the interface the host would call (KTD3).
    #[error(
        "{what} implements no plugin contract this host runs: it must export `ra:plugin/extractor` of {wanted}, and it exports {found}"
    )]
    VersionMismatch {
        what: String,
        /// The versions this host loads, in the order it tries them.
        wanted: String,
        /// What the component exports under `ra:plugin/extractor`, or a
        /// statement that it exports none.
        found: String,
    },
    #[error("{what}: {source}")]
    Manifest {
        what: String,
        #[source]
        source: ManifestError,
    },
    /// The host asked for a pass the plugin does not list in its manifest.
    /// The run routes every (extension, pass) pair to a plugin that serves
    /// it, so this is a host defect, never a plugin's.
    #[error("plugin `{plugin}` does not serve the `{pass}` pass, asked for `{file}`")]
    PassNotServed {
        plugin: String,
        pass: &'static str,
        file: String,
    },
    #[error("plugin `{plugin}` ran out of fuel on `{file}`")]
    Fuel { plugin: String, file: String },
    #[error("plugin `{plugin}` trapped on `{file}`: {reason}")]
    Trap {
        plugin: String,
        file: String,
        reason: String,
    },
}

impl PluginError {
    /// A short reason for coverage and progress output.
    pub fn reason(&self) -> String {
        match self {
            PluginError::Fuel { .. } => "fuel exhausted".into(),
            PluginError::Trap { reason, .. } => format!("trapped: {reason}"),
            other => other.to_string(),
        }
    }
}

struct FrozenWall;

impl HostWallClock for FrozenWall {
    fn resolution(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

struct FrozenMonotonic;

impl HostMonotonicClock for FrozenMonotonic {
    fn resolution(&self) -> u64 {
        1
    }
    fn now(&self) -> u64 {
        0
    }
}

/// Per-store state: the locked WASI context and the path snapshot.
pub struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    paths: Arc<HashSet<String>>,
}

impl State {
    fn new(paths: Arc<HashSet<String>>) -> State {
        let mut b = WasiCtxBuilder::new();
        b.allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .socket_addr_check(|_, _| Box::pin(async { false }))
            .wall_clock(FrozenWall)
            .monotonic_clock(FrozenMonotonic)
            .secure_random(Deterministic::new(vec![0x5a, 0xa5, 0x3c, 0xc3]))
            .insecure_random(Deterministic::new(vec![0x11, 0x22, 0x33, 0x44]))
            .insecure_random_seed(0x5241_5241_5241_5241);
        State {
            wasi: b.build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT)
                .trap_on_grow_failure(true)
                .build(),
            paths,
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl v1::ra::plugin::host::Host for State {
    fn path_exists(&mut self, key: String) -> bool {
        self.paths.contains(&key)
    }
}

impl v1::ra::plugin::types::Host for State {}

impl v2::ra::plugin::host::Host for State {
    fn path_exists(&mut self, key: String) -> bool {
        self.paths.contains(&key)
    }
}

impl v2::ra::plugin::types::Host for State {}

impl v3::ra::plugin::host::Host for State {
    fn path_exists(&mut self, key: String) -> bool {
        self.paths.contains(&key)
    }
}

impl v3::ra::plugin::types::Host for State {}

/// The process-wide engine. One per process: compiled code is tied to it, and
/// its configuration is part of what makes a plugin's output deterministic.
fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.cranelift_nan_canonicalization(true);
        config.wasm_relaxed_simd(false);
        Engine::new(&config).expect("the plugin engine configuration is valid")
    })
}

/// A component pre-instantiated under the contract it exports.
#[derive(Clone)]
enum Pre {
    V1(v1::LanguagePluginPre<State>),
    V2(v2::LanguagePluginPre<State>),
    V3(v3::LanguagePluginPre<State>),
}

impl Pre {
    fn contract(&self) -> Contract {
        match self {
            Pre::V1(_) => Contract::V0_1,
            Pre::V2(_) => Contract::V0_2,
            Pre::V3(_) => Contract::V0_3,
        }
    }
}

/// A live instance, under the same contract as the [`Pre`] it came from.
enum Instance {
    V1(v1::LanguagePlugin),
    V2(v2::LanguagePlugin),
    V3(v3::LanguagePlugin),
}

/// A loaded, checked plugin, ready to hand out sessions.
pub struct Plugin {
    manifest: PluginManifest,
    /// SHA-256 of the component bytes — the identity a run fingerprint and a
    /// declaration's expected hash compare against.
    digest: String,
    pre: Pre,
    vocabulary: Option<WireVocabulary>,
}

impl Plugin {
    /// Load a component from its bytes. `what` names it in errors (a path, or
    /// `bundled:<id>`).
    pub fn load(what: &str, bytes: &[u8]) -> Result<Plugin, PluginError> {
        // A component carries layer 1 in its preamble; a core module carries 0.
        if bytes.len() < 8 || bytes[..4] != *b"\0asm" || bytes[4..8] != [0x0d, 0x00, 0x01, 0x00] {
            return Err(PluginError::NotAComponent { what: what.into() });
        }
        let engine = engine();
        let digest = hex(&Sha256::digest(bytes));
        let component = compiled(what, bytes, &digest)?;
        let link = |e: wasmtime::Error| PluginError::Link {
            what: what.into(),
            reason: first_line(&e),
        };
        // Every contract's host interfaces are offered to every component:
        // they are distinct versioned names, and a component imports the ones
        // of the contract it was built against.
        let mut linker: Linker<State> = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(link)?;
        v1::LanguagePlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s).map_err(link)?;
        v2::LanguagePlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s).map_err(link)?;
        v3::LanguagePlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s).map_err(link)?;
        // Failing here IS a missing import: every import is checked against
        // the linker, and nothing about exports is.
        let instance_pre = linker.instantiate_pre(&component).map_err(link)?;
        // Failing here is not: the imports were all satisfied, so what is
        // missing is the export the host would call. Newest first — a
        // component exporting two is newer than either alone could say.
        let pre = if let Ok(pre) = v3::LanguagePluginPre::new(instance_pre.clone()) {
            Pre::V3(pre)
        } else if let Ok(pre) = v2::LanguagePluginPre::new(instance_pre.clone()) {
            Pre::V2(pre)
        } else if let Ok(pre) = v1::LanguagePluginPre::new(instance_pre) {
            Pre::V1(pre)
        } else {
            return Err(PluginError::VersionMismatch {
                what: what.into(),
                wanted: Contract::SUPPORTED
                    .iter()
                    .map(|c| c.package())
                    .collect::<Vec<_>>()
                    .join(" or "),
                found: exported_contracts(&component),
            });
        };

        // The manifest is not known yet, so the probe session allows every
        // pass; it calls none.
        let mut probe = Session::open(&pre, "(manifest)", Arc::new(HashSet::new()), &Pass::ALL)
            .map_err(|e| e.named(what))?;
        let manifest = probe
            .call(FUEL_FLOOR, "(describe)", |i, s| match i {
                Instance::V1(i) => i
                    .ra_plugin_extractor()
                    .call_describe(s)
                    .map(|m| manifest_of(lift::manifest(m))),
                Instance::V2(i) => i.ra_plugin_extractor().call_describe(s).map(manifest_of),
                Instance::V3(i) => i
                    .ra_plugin_extractor()
                    .call_describe(s)
                    .map(lower::manifest),
            })
            .map_err(|e| e.named(what))?
            .check()
            .map_err(|source| PluginError::Manifest {
                what: what.into(),
                source,
            })?;
        let vocabulary = probe
            .call(FUEL_FLOOR, "(declare)", |i, s| match i {
                Instance::V1(i) => i
                    .ra_plugin_extractor()
                    .call_declare(s)
                    .map(|v| v.map(lift::vocabulary)),
                Instance::V2(i) => i.ra_plugin_extractor().call_declare(s),
                Instance::V3(i) => i
                    .ra_plugin_extractor()
                    .call_declare(s)
                    .map(|v| v.map(lower::vocabulary)),
            })
            .map_err(|e| e.named(what))?;
        drop(probe);
        Ok(Plugin {
            manifest,
            digest,
            pre,
            vocabulary,
        })
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// Hex SHA-256 of the component bytes.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The contract version this plugin runs under — the newest it exports.
    pub fn contract(&self) -> Contract {
        self.pre.contract()
    }

    pub fn vocabulary(&self) -> Option<&WireVocabulary> {
        self.vocabulary.as_ref()
    }

    /// A session for one worker. `paths` is the frozen set `path-exists`
    /// answers from.
    pub fn session(&self, paths: Arc<HashSet<String>>) -> Result<Session<'_>, PluginError> {
        Session::open(&self.pre, &self.manifest.id, paths, &self.manifest.passes)
    }
}

/// An unchecked manifest from what a 0.1 or 0.2 component describes (in
/// 0.2's shape): it serves the first layer's two passes, all those contracts
/// have, and emits nothing.
fn manifest_of(m: v2::ra::plugin::types::Manifest) -> PluginManifest {
    PluginManifest {
        id: m.id,
        version: m.version,
        languages: m
            .languages
            .into_iter()
            .map(|l| PluginLanguage {
                name: l.name,
                extensions: l.extensions,
            })
            .collect(),
        trust_rank: m.trust_rank,
        depends_on: m.depends_on,
        passes: Pass::LAYER_ONE.to_vec(),
        emits: Vec::new(),
    }
}

/// What a component exports under `ra:plugin/extractor`, for a
/// [`PluginError::VersionMismatch`]: the versions, or that there are none.
fn exported_contracts(component: &Component) -> String {
    let versions: Vec<String> = component
        .component_type()
        .exports(engine())
        .filter_map(|(name, _)| name.strip_prefix(EXTRACTOR_EXPORT))
        .map(|version| format!("`{EXTRACTOR_EXPORT}{version}`"))
        .collect();
    if versions.is_empty() {
        "no `ra:plugin/extractor` at all".into()
    } else {
        versions.join(", ")
    }
}

/// One store and one instance, owned by one worker thread.
///
/// After a failed call the instance may be in any state, so it is discarded
/// and the next call gets a fresh one from the same pre-instantiated
/// component.
pub struct Session<'p> {
    pre: &'p Pre,
    plugin: String,
    /// The passes the plugin serves; asking for another is refused before
    /// the instance is touched.
    passes: &'p [Pass],
    paths: Arc<HashSet<String>>,
    live: Option<(Store<State>, Instance)>,
}

impl<'p> Session<'p> {
    fn open(
        pre: &'p Pre,
        plugin: &str,
        paths: Arc<HashSet<String>>,
        passes: &'p [Pass],
    ) -> Result<Session<'p>, PluginError> {
        let mut session = Session {
            pre,
            plugin: plugin.into(),
            passes,
            paths,
            live: None,
        };
        session.ensure()?;
        Ok(session)
    }

    fn ensure(&mut self) -> Result<(), PluginError> {
        if self.live.is_some() {
            return Ok(());
        }
        let mut store = Store::new(engine(), State::new(self.paths.clone()));
        store.limiter(|s| &mut s.limits);
        store
            .set_fuel(FUEL_FLOOR)
            .expect("fuel is enabled on the engine");
        let instance = match self.pre {
            Pre::V1(pre) => pre.instantiate(&mut store).map(Instance::V1),
            Pre::V2(pre) => pre.instantiate(&mut store).map(Instance::V2),
            Pre::V3(pre) => pre.instantiate(&mut store).map(Instance::V3),
        }
        .map_err(|e| PluginError::Trap {
            plugin: self.plugin.clone(),
            file: "(instantiate)".into(),
            reason: first_line(&e),
        })?;
        self.live = Some((store, instance));
        Ok(())
    }

    fn call<T>(
        &mut self,
        fuel: u64,
        file: &str,
        f: impl FnOnce(&Instance, &mut Store<State>) -> wasmtime::Result<T>,
    ) -> Result<T, PluginError> {
        self.ensure()?;
        let (store, instance) = self.live.as_mut().expect("ensured above");
        store.set_fuel(fuel).expect("fuel is enabled on the engine");
        match f(instance, store) {
            Ok(v) => Ok(v),
            Err(e) => {
                // The instance is unusable after a trap; the next call starts
                // from a fresh one.
                self.live = None;
                let out_of_fuel = matches!(
                    e.downcast_ref::<wasmtime::Trap>(),
                    Some(wasmtime::Trap::OutOfFuel)
                );
                Err(if out_of_fuel {
                    PluginError::Fuel {
                        plugin: self.plugin.clone(),
                        file: file.into(),
                    }
                } else {
                    PluginError::Trap {
                        plugin: self.plugin.clone(),
                        file: file.into(),
                        reason: first_line(&e),
                    }
                })
            }
        }
    }

    fn budget(len: usize) -> u64 {
        FUEL_FLOOR.saturating_add(FUEL_PER_BYTE.saturating_mul(len as u64))
    }

    /// Refuse a pass the plugin does not serve, before the instance is
    /// touched — a refusal is not a trap, so the live instance stays.
    /// `bodies` exists only under 0.3, whatever a manifest says.
    fn serving(&self, pass: Pass, file: &str) -> Result<(), PluginError> {
        let exists = pass != Pass::Bodies || matches!(self.pre, Pre::V3(_));
        if exists && self.passes.contains(&pass) {
            Ok(())
        } else {
            Err(PluginError::PassNotServed {
                plugin: self.plugin.clone(),
                pass: pass.word(),
                file: file.into(),
            })
        }
    }

    /// Pass 1 over one file.
    pub fn declarations(
        &mut self,
        path: &str,
        source: &[u8],
        config: &WireConfig,
    ) -> Result<FileDeclarations, PluginError> {
        self.serving(Pass::Declarations, path)?;
        self.call(Self::budget(source.len()), path, |i, s| match i {
            Instance::V1(i) => {
                let config = v1::ra::plugin::types::Config {
                    secret_prefixes: config.secret_prefixes.clone(),
                };
                i.ra_plugin_extractor()
                    .call_declarations(s, path, source, &config)
                    .map(|ops| FileDeclarations {
                        ops: ops.into_iter().map(lift::op).collect(),
                        ..FileDeclarations::default()
                    })
            }
            Instance::V2(i) => i
                .ra_plugin_extractor()
                .call_declarations(s, path, source, config)
                .map(|ops| FileDeclarations {
                    ops,
                    ..FileDeclarations::default()
                }),
            Instance::V3(i) => {
                let config = v3::ra::plugin::types::Config {
                    secret_prefixes: config.secret_prefixes.clone(),
                };
                i.ra_plugin_extractor()
                    .call_declarations(s, path, source, &config)
                    .map(lower::declarations)
            }
        })
    }

    /// Pass 2 over one file: its scopes and its usage sites. One call, one
    /// budget — the plugin parses the file once for both.
    pub fn usages(&mut self, path: &str, source: &[u8]) -> Result<FileUsages, PluginError> {
        self.serving(Pass::Usages, path)?;
        self.call(Self::budget(source.len()), path, |i, s| match i {
            Instance::V1(i) => i
                .ra_plugin_extractor()
                .call_usages(s, path, source)
                .map(lift::usages),
            Instance::V2(i) => i.ra_plugin_extractor().call_usages(s, path, source),
            Instance::V3(i) => i
                .ra_plugin_extractor()
                .call_usages(s, path, source)
                .map(lower::usages),
        })
    }

    /// The bodies of one file's members (contract 0.3), for the second layer.
    /// Its own budget, computed as every other pass's; a trap, exhausted fuel
    /// or memory fails this file and this pass only, and the session recovers
    /// as it does after any failed call. The texts come back unredacted.
    pub fn bodies(&mut self, path: &str, source: &[u8]) -> Result<FileBodies, PluginError> {
        self.serving(Pass::Bodies, path)?;
        self.call(Self::budget(source.len()), path, |i, s| match i {
            Instance::V3(i) => i.ra_plugin_extractor().call_bodies(s, path, source),
            Instance::V1(_) | Instance::V2(_) => {
                unreachable!("`serving` refuses `bodies` below contract 0.3")
            }
        })
    }
}

/// A 0.1 plugin's output in 0.2's shape (KTD3).
///
/// Everything 0.1 says carries over verbatim; what 0.2 adds arrives absent —
/// no declaration name position, no usage column, no context, no scopes — so
/// a 0.1 plugin's sites resolve exactly as they did before 0.2 existed (R6).
mod lift {
    use super::{v1::ra::plugin::types as old, v2::ra::plugin::types as new};

    fn place(p: old::Place) -> new::Place {
        new::Place {
            span: match p {
                old::Place::Whole => new::Span::Whole,
                old::Place::Line(line) => new::Span::Line(line),
                old::Place::Range(range) => new::Span::Range(range),
            },
            name: None,
        }
    }

    fn fact(f: old::Fact) -> new::Fact {
        new::Fact {
            kind: f.kind,
            value: match f.value {
                old::Value::Text(t) => new::Value::Text(t),
                old::Value::Int(i) => new::Value::Int(i),
                old::Value::Flag(b) => new::Value::Flag(b),
            },
            place: place(f.place),
        }
    }

    fn edge(e: old::Edge) -> new::Edge {
        new::Edge {
            source: e.source,
            target: e.target,
            kind: e.kind,
            place: place(e.place),
        }
    }

    pub(super) fn op(op: old::Op) -> new::Op {
        match op {
            old::Op::Fact(kf) => new::Op::Fact(new::KeyedFact {
                node: kf.node,
                fact: fact(kf.fact),
            }),
            old::Op::Symbol(sym) => new::Op::Symbol(new::Symbol {
                key: sym.key,
                facts: sym.facts.into_iter().map(fact).collect(),
                edges: sym.edges.into_iter().map(edge).collect(),
            }),
            old::Op::Edge(e) => new::Op::Edge(edge(e)),
            old::Op::Relation(r) => new::Op::Relation(new::Relation {
                sources: r.sources,
                targets: r.targets,
                kind: match r.kind {
                    old::RelationKind::Extends => new::RelationKind::Extends,
                    old::RelationKind::Implements => new::RelationKind::Implements,
                    old::RelationKind::ByTargetKind => new::RelationKind::ByTargetKind,
                },
                place: place(r.place),
            }),
        }
    }

    /// 0.1 reported the line a usage starts on and nothing else; that stays
    /// its line, with no column to pair it with.
    pub(super) fn usages(usages: Vec<old::Usage>) -> new::FileUsages {
        new::FileUsages {
            scopes: Vec::new(),
            usages: usages
                .into_iter()
                .map(|u| new::Usage {
                    name: u.name,
                    kind: match u.kind {
                        old::UsageKind::Call => new::UsageKind::Call,
                        old::UsageKind::CallOrConstruct => new::UsageKind::CallOrConstruct,
                        old::UsageKind::Construct => new::UsageKind::Construct,
                        old::UsageKind::Reference => new::UsageKind::Reference,
                    },
                    line: u.line,
                    column: None,
                    context: None,
                })
                .collect(),
        }
    }

    pub(super) fn manifest(m: old::Manifest) -> new::Manifest {
        new::Manifest {
            id: m.id,
            version: m.version,
            languages: m
                .languages
                .into_iter()
                .map(|l| new::Language {
                    name: l.name,
                    extensions: l.extensions,
                })
                .collect(),
            trust_rank: m.trust_rank,
            depends_on: m.depends_on,
        }
    }

    pub(super) fn vocabulary(v: old::Vocabulary) -> new::Vocabulary {
        new::Vocabulary {
            namespace: v.namespace,
            card_kinds: v.card_kinds,
            node_kinds: v
                .node_kinds
                .into_iter()
                .map(|n| new::NodeKindDecl {
                    name: n.name,
                    role: match n.role {
                        old::StructuralRole::Container => new::StructuralRole::Container,
                        old::StructuralRole::Leaf => new::StructuralRole::Leaf,
                        old::StructuralRole::Definition => new::StructuralRole::Definition,
                        old::StructuralRole::Package => new::StructuralRole::Package,
                        old::StructuralRole::External => new::StructuralRole::External,
                        old::StructuralRole::Opaque => new::StructuralRole::Opaque,
                    },
                    card: n.card,
                })
                .collect(),
            edge_kinds: v
                .edge_kinds
                .into_iter()
                .map(|e| new::EdgeKindDecl {
                    name: e.name,
                    role: match e.role {
                        old::LinkRole::Containment => new::LinkRole::Containment,
                        old::LinkRole::Declaration => new::LinkRole::Declaration,
                        old::LinkRole::Dependency => new::LinkRole::Dependency,
                        old::LinkRole::Opaque => new::LinkRole::Opaque,
                    },
                })
                .collect(),
            fact_kinds: v
                .fact_kinds
                .into_iter()
                .map(|f| new::FactKindDecl {
                    name: f.name,
                    cardinality: match f.cardinality {
                        old::Cardinality::Additive => new::Cardinality::Additive,
                        old::Cardinality::Scalar => new::Cardinality::Scalar,
                    },
                })
                .collect(),
        }
    }
}

/// A 0.3 plugin's output in the shape the rest of the host speaks.
///
/// Everything 0.2 can say passes as 0.2, unchanged in meaning (the records
/// 0.3 kept are 0.2's). What 0.2 cannot say — a symbol's node kind and block,
/// the references by name, the lookup blocks — is taken out of the 0.2 shape
/// and handed beside it in [`FileDeclarations`], so no 0.3 type leaves this
/// module on the declaration and usage path.
mod lower {
    use super::{
        v2::ra::plugin::types as old, v3::ra::plugin::types as new, FileDeclarations, LookupBlock,
        NameRef, SymbolExtra,
    };
    use crate::plugin::manifest::{Pass, PluginLanguage, PluginManifest};

    fn span(s: new::Span) -> old::Span {
        match s {
            new::Span::Whole => old::Span::Whole,
            new::Span::Line(line) => old::Span::Line(line),
            new::Span::Range(range) => old::Span::Range(range),
        }
    }

    fn place(p: new::Place) -> old::Place {
        old::Place {
            span: span(p.span),
            name: p.name.map(|n| old::Position {
                line: n.line,
                column: n.column,
            }),
        }
    }

    fn fact(f: new::Fact) -> old::Fact {
        old::Fact {
            kind: f.kind,
            value: match f.value {
                new::Value::Text(t) => old::Value::Text(t),
                new::Value::Int(i) => old::Value::Int(i),
                new::Value::Flag(b) => old::Value::Flag(b),
            },
            place: place(f.place),
        }
    }

    fn edge(e: new::Edge) -> old::Edge {
        old::Edge {
            source: e.source,
            target: e.target,
            kind: e.kind,
            place: place(e.place),
        }
    }

    /// One op — the `index`-th of its file — in 0.2's shape, and what its
    /// symbol says beyond 0.2, if anything.
    fn op(index: usize, op: new::Op) -> (old::Op, Option<SymbolExtra>) {
        match op {
            new::Op::Fact(kf) => (
                old::Op::Fact(old::KeyedFact {
                    node: kf.node,
                    fact: fact(kf.fact),
                }),
                None,
            ),
            new::Op::Symbol(sym) => {
                let extra =
                    (sym.node_kind.is_some() || sym.block.is_some()).then_some(SymbolExtra {
                        op: index,
                        node_kind: sym.node_kind,
                        block: sym.block,
                    });
                (
                    old::Op::Symbol(old::Symbol {
                        key: sym.key,
                        facts: sym.facts.into_iter().map(fact).collect(),
                        edges: sym.edges.into_iter().map(edge).collect(),
                    }),
                    extra,
                )
            }
            new::Op::Edge(e) => (old::Op::Edge(edge(e)), None),
            new::Op::Relation(r) => (
                old::Op::Relation(old::Relation {
                    sources: r.sources,
                    targets: r.targets,
                    kind: match r.kind {
                        new::RelationKind::Extends => old::RelationKind::Extends,
                        new::RelationKind::Implements => old::RelationKind::Implements,
                        new::RelationKind::ByTargetKind => old::RelationKind::ByTargetKind,
                    },
                    place: place(r.place),
                }),
                None,
            ),
        }
    }

    pub(super) fn declarations(file: new::FileDeclarations) -> FileDeclarations {
        let mut out = FileDeclarations::default();
        for (index, o) in file.ops.into_iter().enumerate() {
            let (o, extra) = op(index, o);
            out.ops.push(o);
            out.symbols.extend(extra);
        }
        out.name_refs = file
            .name_refs
            .into_iter()
            .map(|r| {
                let place = place(r.place);
                NameRef {
                    source: r.source,
                    name: r.name,
                    target_kind: r.target_kind,
                    relation: r.relation,
                    block: r.block,
                    span: match place.span {
                        old::Span::Whole => crate::graph::Span::Whole,
                        old::Span::Line(line) => crate::graph::Span::at(Some(line)),
                        old::Span::Range((start, end)) => crate::graph::Span::range(start, end),
                    },
                    name_at: place.name.map(|n| (n.line, n.column)),
                }
            })
            .collect();
        out.blocks = file
            .blocks
            .into_iter()
            .map(|b| LookupBlock {
                lines: b.lines,
                parent: b.parent,
            })
            .collect();
        out
    }

    pub(super) fn usages(file: new::FileUsages) -> old::FileUsages {
        old::FileUsages {
            scopes: file
                .scopes
                .into_iter()
                .map(|s| old::Scope {
                    namespace: s.namespace,
                    lines: s.lines,
                    parent: s.parent,
                    usings: s
                        .usings
                        .into_iter()
                        .map(|u| old::UsingDirective {
                            target: u.target,
                            alias: u.alias,
                            is_static: u.is_static,
                            is_global: u.is_global,
                        })
                        .collect(),
                })
                .collect(),
            usages: file
                .usages
                .into_iter()
                .map(|u| old::Usage {
                    name: u.name,
                    kind: match u.kind {
                        new::UsageKind::Call => old::UsageKind::Call,
                        new::UsageKind::CallOrConstruct => old::UsageKind::CallOrConstruct,
                        new::UsageKind::Construct => old::UsageKind::Construct,
                        new::UsageKind::Reference => old::UsageKind::Reference,
                    },
                    line: u.line,
                    column: u.column,
                    context: u.context.map(|c| old::SiteContext {
                        scope: c.scope,
                        receiver: match c.receiver {
                            new::Receiver::None => old::Receiver::None,
                            new::Receiver::Name(n) => old::Receiver::Name(n),
                            new::Receiver::Value(v) => old::Receiver::Value(v),
                            new::Receiver::Keyword(k) => old::Receiver::Keyword(k),
                            new::Receiver::Expression => old::Receiver::Expression,
                        },
                        declared_type: c.declared_type.map(|t| old::DeclaredType {
                            name: t.name,
                            keyword: t.keyword,
                        }),
                        argument_count: c.argument_count,
                        qualifier: c.qualifier,
                    }),
                })
                .collect(),
        }
    }

    /// The manifest, unchecked. The passes keep their first mention; a pass
    /// listed twice is served once.
    pub(super) fn manifest(m: new::Manifest) -> PluginManifest {
        let mut passes: Vec<Pass> = Vec::new();
        for pass in m.passes {
            let pass = match pass {
                new::Pass::Declarations => Pass::Declarations,
                new::Pass::Usages => Pass::Usages,
                new::Pass::Bodies => Pass::Bodies,
            };
            if !passes.contains(&pass) {
                passes.push(pass);
            }
        }
        PluginManifest {
            id: m.id,
            version: m.version,
            languages: m
                .languages
                .into_iter()
                .map(|l| PluginLanguage {
                    name: l.name,
                    extensions: l.extensions,
                })
                .collect(),
            trust_rank: m.trust_rank,
            depends_on: m.depends_on,
            passes,
            emits: m.emits,
        }
    }

    pub(super) fn vocabulary(v: new::Vocabulary) -> old::Vocabulary {
        old::Vocabulary {
            namespace: v.namespace,
            card_kinds: v.card_kinds,
            node_kinds: v
                .node_kinds
                .into_iter()
                .map(|n| old::NodeKindDecl {
                    name: n.name,
                    role: match n.role {
                        new::StructuralRole::Container => old::StructuralRole::Container,
                        new::StructuralRole::Leaf => old::StructuralRole::Leaf,
                        new::StructuralRole::Definition => old::StructuralRole::Definition,
                        new::StructuralRole::Package => old::StructuralRole::Package,
                        new::StructuralRole::External => old::StructuralRole::External,
                        new::StructuralRole::Opaque => old::StructuralRole::Opaque,
                    },
                    card: n.card,
                })
                .collect(),
            edge_kinds: v
                .edge_kinds
                .into_iter()
                .map(|e| old::EdgeKindDecl {
                    name: e.name,
                    role: match e.role {
                        new::LinkRole::Containment => old::LinkRole::Containment,
                        new::LinkRole::Declaration => old::LinkRole::Declaration,
                        new::LinkRole::Dependency => old::LinkRole::Dependency,
                        new::LinkRole::Opaque => old::LinkRole::Opaque,
                    },
                })
                .collect(),
            fact_kinds: v
                .fact_kinds
                .into_iter()
                .map(|f| old::FactKindDecl {
                    name: f.name,
                    cardinality: match f.cardinality {
                        new::Cardinality::Additive => old::Cardinality::Additive,
                        new::Cardinality::Scalar => old::Cardinality::Scalar,
                    },
                })
                .collect(),
        }
    }
}

impl PluginError {
    /// Re-attribute an error raised while loading to the artifact being loaded.
    fn named(self, what: &str) -> PluginError {
        match self {
            PluginError::Trap { reason, .. } => PluginError::Trap {
                plugin: what.into(),
                file: "(load)".into(),
                reason,
            },
            PluginError::Fuel { .. } => PluginError::Fuel {
                plugin: what.into(),
                file: "(load)".into(),
            },
            other => other,
        }
    }
}

/// Where compiled components are kept between runs, or `None` when caching
/// is off (`RA_COMPONENT_CACHE=off`).
///
/// Compiling a component is the expensive part of loading it — seconds for
/// the bundled grammars, far more in an unoptimized build — and the result
/// depends only on the component's bytes and on this engine. Keeping it saves
/// that on every run after the first.
fn cache_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    match std::env::var("RA_COMPONENT_CACHE") {
        Ok(v) if v == "off" => return None,
        Ok(v) if !v.is_empty() => return Some(PathBuf::from(v)),
        _ => {}
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    }?;
    Some(base.join("reverse-architect").join("components"))
}

/// This engine's compatibility fingerprint: a compiled artifact from an engine
/// with another configuration or another wasmtime version is never reused.
fn engine_fingerprint() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    engine().precompile_compatibility_hash().hash(&mut h);
    h.finish()
}

/// The component for `bytes`, from the cache when a compatible compiled copy
/// is there, compiled (and cached) otherwise. A cache that cannot be read or
/// written degrades to compiling, never to failing the load.
fn compiled(what: &str, bytes: &[u8], digest: &str) -> Result<Component, PluginError> {
    let engine = engine();
    let key = format!("{digest}-{:016x}", engine_fingerprint());
    let dir = cache_dir();
    if let Some(dir) = &dir {
        let artifact = dir.join(format!("{key}.cwasm"));
        let seal = dir.join(format!("{key}.sha256"));
        if let (Ok(data), Ok(expected)) = (std::fs::read(&artifact), std::fs::read_to_string(&seal))
        {
            if hex(&Sha256::digest(&data)) == expected.trim() {
                // SAFETY: `deserialize` trusts its input to be a component this
                // engine serialized. These bytes are exactly what `serialize`
                // wrote into the user's own cache directory under a key that
                // binds the source component's hash and this engine's
                // compatibility fingerprint, and the recorded SHA-256 proves
                // they are unchanged since. Anything else is recompiled.
                if let Ok(component) = unsafe { Component::deserialize(engine, &data) } {
                    return Ok(component);
                }
            }
        }
    }
    let component = Component::from_binary(engine, bytes).map_err(|e| PluginError::Compile {
        what: what.into(),
        reason: first_line(&e),
    })?;
    if let (Some(dir), Ok(data)) = (&dir, component.serialize()) {
        let _ = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(format!("{key}.{}.tmp", std::process::id()));
            std::fs::write(&tmp, &data)?;
            std::fs::rename(&tmp, dir.join(format!("{key}.cwasm")))?;
            std::fs::write(
                dir.join(format!("{key}.sha256")),
                hex(&Sha256::digest(&data)),
            )
        })();
    }
    Ok(component)
}

fn first_line(e: &wasmtime::Error) -> String {
    let text = format!("{e:#}");
    text.lines().next().unwrap_or("").to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE: &[u8] = include_bytes!("../../plugins/prebuilt/probe.wasm");

    fn probe() -> Plugin {
        Plugin::load("probe", PROBE).expect("the probe component loads")
    }

    fn signature_of(ops: &[Op]) -> String {
        for op in ops {
            if let Op::Symbol(sym) = op {
                for fact in &sym.facts {
                    if fact.kind == "symbol_signature" {
                        if let Value::Text(t) = &fact.value {
                            return t.clone();
                        }
                    }
                }
            }
        }
        panic!("the probe always returns a signature: {ops:?}")
    }

    fn run(plugin: &Plugin, path: &str) -> Result<String, PluginError> {
        let paths: HashSet<String> = ["src/a.rs".to_string()].into_iter().collect();
        let mut s = plugin.session(Arc::new(paths))?;
        let config = WireConfig {
            secret_prefixes: vec!["acme_".into()],
        };
        s.declarations(path, b"", &config)
            .map(|declared| signature_of(&declared.ops))
    }

    #[test]
    fn the_manifest_comes_from_the_component_and_is_checked() {
        let p = probe();
        assert_eq!(p.id(), "probe");
        assert_eq!(p.manifest().language_for("probe"), Some("probe"));
        assert_eq!(p.digest().len(), 64);
        assert!(p.vocabulary().is_none());
    }

    #[test]
    fn a_core_module_or_garbage_is_not_a_component() {
        assert!(matches!(
            Plugin::load("x", b"\0asm\x01\0\0\0rest"),
            Err(PluginError::NotAComponent { .. })
        ));
        assert!(matches!(
            Plugin::load("x", b"not wasm"),
            Err(PluginError::NotAComponent { .. })
        ));
    }

    #[test]
    fn a_file_open_fails_with_zero_preopens() {
        assert_eq!(run(&probe(), "fs.probe").unwrap(), "fs:refused");
    }

    #[test]
    fn a_socket_connect_fails() {
        assert_eq!(run(&probe(), "net.probe").unwrap(), "net:refused");
    }

    #[test]
    fn the_environment_and_arguments_are_empty() {
        assert_eq!(run(&probe(), "env.probe").unwrap(), "env:0 args:0");
    }

    #[test]
    fn the_clock_is_frozen() {
        assert_eq!(run(&probe(), "clock.probe").unwrap(), "clock:0");
    }

    #[test]
    fn secure_randomness_is_the_same_on_every_run() {
        let p = probe();
        let a = run(&p, "random.probe").unwrap();
        let b = run(&p, "random.probe").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, "random:unavailable");
    }

    #[test]
    fn a_guest_hash_map_iterates_in_the_same_order_every_run() {
        let p = probe();
        let first = run(&p, "hash.probe").unwrap();
        for _ in 0..5 {
            assert_eq!(run(&p, "hash.probe").unwrap(), first);
        }
    }

    #[test]
    fn writing_to_stdout_does_not_fail_the_call() {
        assert_eq!(run(&probe(), "stdout.probe").unwrap(), "stdout:written");
    }

    #[test]
    fn the_path_query_answers_from_the_snapshot() {
        assert_eq!(
            run(&probe(), "path.probe").unwrap(),
            "exists:true missing:false"
        );
    }

    #[test]
    fn the_run_configuration_reaches_the_guest() {
        assert_eq!(run(&probe(), "plain.probe").unwrap(), "prefixes:1");
    }

    #[test]
    fn a_trap_fails_one_file_and_the_session_recovers() {
        let p = probe();
        let paths = Arc::new(HashSet::new());
        let mut s = p.session(paths).unwrap();
        let config = WireConfig {
            secret_prefixes: Vec::new(),
        };
        let err = s.declarations("trap.probe", b"", &config).unwrap_err();
        assert!(matches!(err, PluginError::Trap { .. }), "{err}");
        // The same session serves the next file from a fresh instance.
        let declared = s.declarations("plain.probe", b"", &config).unwrap();
        assert_eq!(signature_of(&declared.ops), "prefixes:0");
    }

    #[test]
    fn a_runaway_guest_is_stopped_by_fuel() {
        let err = run(&probe(), "loop.probe").unwrap_err();
        assert!(matches!(err, PluginError::Fuel { .. }), "{err}");
    }

    #[test]
    fn a_guest_cannot_grow_past_the_memory_ceiling() {
        let err = run(&probe(), "memory.probe").unwrap_err();
        assert!(matches!(err, PluginError::Trap { .. }), "{err}");
    }

    #[test]
    fn usages_come_back_as_spellings_for_the_host_to_resolve() {
        let p = probe();
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();
        let usages = s.usages("a.probe", b"").unwrap().usages;
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].name, "Thing");
        assert!(matches!(usages[0].kind, UsageKind::Call));
    }

    /// The probe is built against the frozen 0.1 contract and keeps running
    /// under it: its sites arrive in 0.2's shape with nothing 0.2 added —
    /// no column, no context, no scopes — which is all 0.1 can say (R6).
    #[test]
    fn a_plugin_built_against_0_1_runs_under_0_1_with_no_context() {
        let p = probe();
        assert_eq!(p.contract(), Contract::V0_1);
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();
        let file = s.usages("a.probe", b"").unwrap();
        assert!(file.scopes.is_empty());
        assert_eq!(file.usages[0].line, 1);
        assert_eq!(file.usages[0].column, None);
        assert!(file.usages[0].context.is_none());
    }

    /// A component exporting no `extractor` at all — `(component)`, whose
    /// whole encoding is the eight-byte preamble — imports nothing, so every
    /// import is satisfied: what it lacks is the export. The error says that,
    /// names both versions the host would have run, and does not call it a
    /// missing import, which is what it was reported as before 0.2.
    #[test]
    fn a_component_exporting_neither_contract_is_a_version_mismatch() {
        let empty_component = b"\0asm\x0d\x00\x01\x00";
        let err = match Plugin::load("x.wasm", empty_component) {
            Err(e) => e,
            Ok(_) => panic!("a component without an extractor must not load"),
        };
        assert!(
            matches!(&err, PluginError::VersionMismatch { found, .. } if found.contains("no `ra:plugin/extractor`")),
            "{err}"
        );
        let message = err.to_string();
        assert!(message.contains("ra:plugin@0.3.0"), "{message}");
        assert!(message.contains("ra:plugin@0.2.0"), "{message}");
        assert!(message.contains("ra:plugin@0.1.0"), "{message}");
        assert!(!message.contains("import"), "{message}");
    }

    // ---- Contract 0.3 ----

    const PROBE3: &[u8] = include_bytes!("../../plugins/prebuilt/probe3.wasm");

    fn probe3() -> Plugin {
        Plugin::load("probe3", PROBE3).expect("the probe3 component loads")
    }

    /// A 0.3 component runs under 0.3 and says which passes it serves and
    /// what it emits; a 0.1 and a 0.2 component run under their own contract
    /// as before, serving the first layer's two passes and emitting nothing.
    #[test]
    fn each_component_runs_under_the_newest_contract_it_exports() {
        let p = probe3();
        assert_eq!(p.contract(), Contract::V0_3);
        assert_eq!(p.manifest().passes, Pass::ALL);
        assert_eq!(p.manifest().emits, ["probe3-tag"]);
        assert_eq!(p.manifest().language_for("probe3"), Some("probe3"));
        let vocabulary = p.vocabulary().expect("probe3 declares a vocabulary");
        assert_eq!(vocabulary.node_kinds[0].card.as_deref(), Some("widget"));

        let old = probe();
        assert_eq!(old.contract(), Contract::V0_1);
        assert_eq!(old.manifest().passes, Pass::LAYER_ONE);
        assert!(old.manifest().emits.is_empty());

        let csharp = super::super::bundled::load_one("csharp").unwrap();
        assert_eq!(csharp.contract(), Contract::V0_2);
        assert_eq!(csharp.manifest().passes, Pass::LAYER_ONE);
        assert!(csharp.manifest().emits.is_empty());
    }

    #[test]
    fn bodies_of_an_empty_file_are_an_empty_model() {
        let p = probe3();
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();
        let bodies = s.bodies("empty.probe3", b"").unwrap();
        assert!(bodies.types.is_empty());
        assert!(bodies.members.is_empty());
        assert!(bodies.typed_members.is_empty());
        assert!(bodies.metas.is_empty());
        let bodies = s.bodies("a.probe3", b"x").unwrap();
        assert_eq!(bodies.members.len(), 1);
        assert!(matches!(
            bodies.members[0].nodes[0].kind,
            BodyNodeKind::Body
        ));
        assert_eq!(bodies.members[0].nodes[0].tags[0].name, "probe3-tag");
    }

    /// A runaway `bodies` costs that file and that pass: the next file's
    /// bodies and the first layer's passes run in the same session.
    #[test]
    fn exhausted_fuel_in_bodies_fails_one_file_and_the_session_recovers() {
        let p = probe3();
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();
        let err = s.bodies("loop.probe3", b"x").unwrap_err();
        assert!(matches!(err, PluginError::Fuel { .. }), "{err}");
        assert_eq!(s.bodies("next.probe3", b"x").unwrap().members.len(), 1);
        let config = WireConfig {
            secret_prefixes: Vec::new(),
        };
        let declared = s.declarations("next.probe3", b"x", &config).unwrap();
        assert_eq!(signature_of(&declared.ops), "prefixes:0");
        assert_eq!(
            s.usages("next.probe3", b"x").unwrap().usages[0].name,
            "Thing"
        );
    }

    /// A pass the plugin does not serve is refused before the instance is
    /// touched; it is not a trap, and the session goes on.
    #[test]
    fn a_pass_the_plugin_does_not_serve_is_refused() {
        let p = probe();
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();
        let err = s.bodies("a.probe", b"x").unwrap_err();
        assert!(
            matches!(&err, PluginError::PassNotServed { pass: "bodies", .. }),
            "{err}"
        );
        assert_eq!(s.usages("a.probe", b"").unwrap().usages.len(), 1);
    }

    /// What 0.2 cannot say arrives beside the ops: the symbol's node kind and
    /// block by its index in the ops, the references by name, the blocks.
    #[test]
    fn a_0_3_declaration_is_lowered_with_what_0_2_cannot_say_beside_it() {
        let p = probe3();
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();
        let config = WireConfig {
            secret_prefixes: Vec::new(),
        };
        let file = s.declarations("widget.probe3", b"", &config).unwrap();
        assert!(
            matches!(&file.ops[0], Op::Symbol(sym) if sym.key == "sym:probe3:widget.probe3#Main")
        );
        assert_eq!(
            file.symbols,
            [SymbolExtra {
                op: 0,
                node_kind: Some("probe3:widget".into()),
                block: Some(0),
            }]
        );
        assert_eq!(
            file.name_refs,
            [NameRef {
                source: "sym:probe3:widget.probe3#Main".into(),
                name: "Other".into(),
                target_kind: "probe3:widget".into(),
                relation: None,
                block: Some(0),
                span: crate::graph::Span::at(Some(2)),
                name_at: Some((2, 5)),
            }]
        );
        assert_eq!(
            file.blocks,
            [LookupBlock {
                lines: (1, 3),
                parent: None,
            }]
        );
        // An ordinary symbol says nothing beyond 0.2.
        let plain = s.declarations("plain.probe3", b"", &config).unwrap();
        assert!(plain.symbols.is_empty() && plain.name_refs.is_empty() && plain.blocks.is_empty());
    }
}
