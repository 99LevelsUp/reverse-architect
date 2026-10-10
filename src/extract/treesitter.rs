//! The language extractor: declarations and usages from the language plugins.
//!
//! Until U11 this module parsed every file itself with compiled-in tree-sitter
//! grammars. The walkers now live in `plugins/treesitter` and run inside
//! WebAssembly components — one per language family, bundled into the binary
//! and loaded through the same path a third-party plugin takes (R1). What stays
//! here is everything that needs the whole repository at once:
//!
//! - driving both passes over every file, in parallel, applying in file order;
//! - resolving the type relations a declaration names (extends, implements)
//!   once every file has been read, so a base type in another file is found;
//! - resolving the references by name a 0.3 plugin reports (`name-ref`) at the
//!   same point, scope by scope (`super::name_ref`);
//! - the repo-wide `SymbolTable` a usage site resolves against, the ambiguity
//!   ceiling (R23) and the enclosing-declaration lookup — so every plugin gets
//!   the same honesty about ambiguous names (implementation note D1);
//! - the shared resolver (`super::resolve`, plan A U7), which sifts each site's
//!   candidates with one rule catalog for every plugin that reports context.
//!
//! # Symbol identity (KTD3)
//!
//! A symbol node key is `sym:` + `<lang>:<module-path>#<Type>[.<member>(<signature>)]`.
//! The plugin builds it; the rules — the language's own module notion rather
//! than the file path, members as `<Type>.<member>`, callables carrying their
//! normalized parameter list, an ordinal `~2` for a residual duplicate — are
//! documented in `plugins/treesitter/src/walkers.rs` beside the code that
//! applies them.
//!
//! # What reaches the graph
//!
//! Every fact and edge a plugin returns carries the plugin's own id as its
//! extractor, `Tier::Measured` and `Resolution::Heuristic`: tree-sitter without
//! type resolution cannot claim more (R4). Calls, references and constructions
//! are resolved here by NAME against every declaration in the repository:
//!
//! - zero candidates → no edge, no fact — the name is external;
//! - one candidate → one edge, no caveat;
//! - two to the ceiling → one edge per candidate plus a
//!   [`FactKind::CandidateCount`] fact;
//! - above the ceiling → no named edges and one [`FactKind::AmbiguousReference`].
//!
//! The counts are those left after a rule holding a right to delete removed
//! candidates (KTD11); every edge carries the rung and rule the resolver gave
//! its candidate (KTD5).
//!
//! The relation originates at the innermost declaration around the site, or the
//! file for module-level code, and a relation repeated within one file is
//! emitted once, anchored at its first site.

use super::cache::CacheStore;
use super::external::SecretPrefixes;
use super::name_ref::FileNameRefs;
use super::resolve::project::{self, ProjectUsings, ProjectUsingsBuilder, UsingsHandoff};
use super::resolve::rights::{RightsTable, SiteOrigin};
use super::resolve::rules::Site;
use super::resolve::table::{NamePositions, Owner, SymbolTable};
use super::resolve::{resolve_site, Fate};
use super::walk::{walk_files, GraphOp, GraphOps};
use super::{ExtractError, Extractor, Repo, TrackedFile};
use crate::graph::{
    CandidateOutcome, CandidateRecord, Confidence, Edge, EdgeKind, ExtractorId, Fact, FactKind,
    FactValue, Graph, NodeKey, NodeKind, NodeKindId, Provenance, Rule, RuleVerdict, SiteCounts,
    SiteId, SiteKind, SiteRecord, Span, Tier, Vocabulary,
};
use crate::plugin::host::{
    FileUsages, Op, Place, Receiver, RelationKind as WireRelationKind, SiteContext,
    UsageKind as WireUsageKind, Value, WireConfig, WireScope, WireSpan,
};
use crate::plugin::{FileDeclarations, LookupBlock, NameRef, Pass, Plugin, Session};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// The extractor that runs the language plugins over every file.
pub struct Languages {
    /// Extra credential prefixes the operator configured (R22), handed to every
    /// plugin and applied again by the host to what comes back.
    pub secret_prefixes: SecretPrefixes,
    /// The same prefixes as the operator typed them, for the plugins.
    pub secret_prefix_list: Vec<String>,
    /// Most name candidates a heuristic resolution may still turn into named
    /// edges (R23).
    pub ambiguity_ceiling: usize,
    /// The cross-run cache both passes read and write, when the run has one.
    pub(crate) cache: Option<CacheStore>,
    /// The run's plugins in precedence order: the operator's first, then the
    /// bundled ones they did not replace (U8).
    pub plugins: crate::plugin::PluginSet,
    /// The rights to delete the shared resolver applies (plan A, KTD11).
    pub rights: RightsTable,
    /// Keep every usage-site record in the graph's ledger, not only its
    /// counters (plan A, KTD7) — for `--sites`.
    pub keep_sites: bool,
    /// The run has a SCIP index, which the pairing after the walk (plan A,
    /// U9) matches the site records against. Keeps the records like
    /// `keep_sites` does, and also where every declaration writes its name,
    /// which the pairing maps the index's definitions by.
    pub pair_with_index: bool,
    /// Run the rules at sites over the ceiling too, for `ra measure`
    /// ([`super::ExtractOptions::trace_over_ceiling`]). Such a run's usage
    /// records hold verdicts a generation run's do not, so it neither reads
    /// nor writes the usage cache.
    pub trace_over_ceiling: bool,
    /// Where the project-wide usings this run read go, for the external
    /// extractor's decisions by an enum's value (plan B, U5).
    pub(crate) usings_handoff: UsingsHandoff,
    /// Where the declaration-name positions go once the usage pass no longer
    /// needs them, for the second layer's pairing (plan 2a, U4: KTD6) — set
    /// only for a run with `--detail-level`. `None`: they are dropped, as they
    /// always were.
    pub(crate) names_handoff: Option<NamesHandoff>,
}

/// The declaration-name positions of one run, handed from the languages
/// extractor, which collects them in its declaration pass, to the second
/// layer, which pairs every body model with the declaration it belongs to by
/// the position of its name (plan 2a, U4: KTD6). The pattern of
/// [`UsingsHandoff`]: both sides hold a clone of one handoff, the languages
/// extractor puts once, the second layer takes once after every extractor ran.
///
/// On a warm run the positions come back from the declaration records, which
/// store them (`FileOutputWire::names`), so a cached first layer pairs as a
/// fresh one does.
#[derive(Clone, Default)]
pub(crate) struct NamesHandoff(Arc<Mutex<Option<NamePositions>>>);

impl NamesHandoff {
    fn put(&self, names: NamePositions) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(names);
    }

    /// The positions, or none when the languages extractor never ran.
    pub(crate) fn take(&self) -> NamePositions {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_default()
    }
}

/// The id this extractor runs under in progress output and the registry. The
/// facts it writes carry each plugin's own id instead.
pub const LANGUAGES_ID: &str = "languages";

/// The cache phases of the two passes. Renamed from `treesitter-*` in U11: the
/// records now come from plugins, and `CACHE_VERSION` moved with them.
pub const PHASE_DECL: &str = "languages-decl";
pub const PHASE_USAGES: &str = "languages-usages";

impl Languages {
    /// The plugin that makes `pass` over `path` — the first, in precedence
    /// order, that claims its extension and serves the pass.
    fn plugin_for(&self, path: &str, pass: Pass) -> Option<&'static Plugin> {
        let ext = path.rsplit_once('.').map(|(_, e)| e)?;
        self.plugins.for_pass(ext, pass)
    }

    fn config(&self) -> WireConfig {
        WireConfig {
            secret_prefixes: self.secret_prefix_list.clone(),
        }
    }

    /// The rules holding a valid right to delete for the sites of `path`, read
    /// by `plugin` (KTD11).
    fn rights_for(&self, plugin: &Plugin, path: &str) -> Vec<Rule> {
        if self.rights.rights().is_empty() {
            return Vec::new();
        }
        let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
        let Some(language) = plugin.manifest().language_for(ext) else {
            return Vec::new();
        };
        self.rights.active_for(&SiteOrigin {
            language: language.to_string(),
            component: plugin.id().to_string(),
            component_sha256: plugin.digest().to_string(),
            contract: plugin.contract(),
        })
    }

    /// The usings each C# project imports into every file of it
    /// ([`project`]), read from the tracked blobs before the usage pass —
    /// never from what a usage record holds, which a warm run serves from the
    /// cache without the scopes (see the module docs there). A blob that
    /// cannot be read is skipped, as the walks skip it.
    fn project_usings(&self, repo: &Repo, tracked: &[TrackedFile]) -> ProjectUsings {
        let mut builder = ProjectUsingsBuilder::default();
        for tf in tracked {
            if tf.is_symlink || !project::wants(&tf.key) {
                continue;
            }
            // A source file only when this run reads it; a project file
            // always — no plugin claims it.
            let is_source = tf.key.to_ascii_lowercase().ends_with(".cs");
            if is_source && self.plugin_for(&tf.key, Pass::Usages).is_none() {
                continue;
            }
            if let Ok(bytes) = repo.blob_bytes(tf) {
                builder.add(&tf.key, &bytes);
            }
        }
        builder.finish()
    }
}

/// Per-worker sessions, one per plugin, opened on first use.
pub(crate) struct Sessions {
    paths: Arc<HashSet<String>>,
    open: HashMap<&'static str, Session<'static>>,
}

impl Sessions {
    fn new(paths: Arc<HashSet<String>>) -> Sessions {
        Sessions {
            paths,
            open: HashMap::new(),
        }
    }

    pub(crate) fn get(&mut self, plugin: &'static Plugin) -> Option<&mut Session<'static>> {
        if !self.open.contains_key(plugin.id()) {
            match plugin.session(self.paths.clone()) {
                Ok(s) => {
                    self.open.insert(plugin.id(), s);
                }
                Err(e) => {
                    eprintln!("{e}");
                    return None;
                }
            }
        }
        self.open.get_mut(plugin.id())
    }
}

/// Sessions that outlive the rayon split which opened them.
///
/// `map_init` builds its state once per split, and every 512-file batch splits
/// again, so per-split sessions meant instantiating each plugin hundreds of
/// times a pass — each time copying the grammar's parse tables into a fresh
/// linear memory. A split now leases a worker's sessions and hands them back
/// when it ends, so a run instantiates a plugin about once per thread.
///
/// Reuse across files is what a session already did within a split; the pool
/// only stretches it across splits and passes. Which files an instance saw
/// before never reaches the output: every call parses its own source from
/// scratch, and an instance that trapped is discarded (`Session::call`).
pub(crate) struct Pool {
    paths: Arc<HashSet<String>>,
    free: Mutex<Vec<Sessions>>,
}

impl Pool {
    pub(crate) fn new(paths: Arc<HashSet<String>>) -> Pool {
        Pool {
            paths,
            free: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn lease(&self) -> Lease<'_> {
        let reused = self.free.lock().unwrap_or_else(|e| e.into_inner()).pop();
        Lease {
            pool: self,
            sessions: Some(reused.unwrap_or_else(|| Sessions::new(self.paths.clone()))),
        }
    }
}

/// One split's sessions, returned to the pool on drop.
pub(crate) struct Lease<'a> {
    pool: &'a Pool,
    sessions: Option<Sessions>,
}

impl Lease<'_> {
    pub(crate) fn get(&mut self, plugin: &'static Plugin) -> Option<&mut Session<'static>> {
        self.sessions
            .as_mut()
            .expect("present until drop")
            .get(plugin)
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(sessions) = self.sessions.take() {
            self.pool
                .free
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(sessions);
        }
    }
}

impl Extractor for Languages {
    fn id(&self) -> ExtractorId {
        ExtractorId::from(LANGUAGES_ID)
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[
            FactKind::SymbolName,
            FactKind::SymbolKind,
            FactKind::SymbolContainer,
            FactKind::SymbolSignature,
            FactKind::SymbolVisibility,
            FactKind::DocComment,
            // A key-shaped word cut out of a doc comment (KTD11 rule 3).
            FactKind::AnchorSkipped,
            FactKind::EntryPoint,
            FactKind::CandidateCount,
            FactKind::AmbiguousReference,
            FactKind::ExtractionFailed,
            // A reference by name nothing answers (plan 2c, U5).
            FactKind::UnresolvedReference,
        ]
    }

    fn trust_rank(&self) -> u8 {
        30
    }

    fn provenance_ids(&self) -> Vec<(ExtractorId, u8)> {
        self.plugins
            .iter()
            .map(|p| (ExtractorId::from(p.id()), p.manifest().trust_rank))
            .collect()
    }

    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError> {
        let tracked = repo.tracked_files_with_paths()?;
        let pool = Pool::new(Arc::new(PathKeys::snapshot(graph).0));
        let vocabulary = graph.vocabulary().clone();
        let config = self.config();
        // One id allocation per plugin for the whole run: every fact clones it.
        let ids: HashMap<&str, ExtractorId> = self
            .plugins
            .iter()
            .map(|p| (p.id(), ExtractorId::from(p.id())))
            .collect();

        let keep_records = self.keep_sites || self.pair_with_index;
        if keep_records {
            graph.sites_mut().keep_records();
        }

        // ---- Pass 1: declarations ----
        let mut relations: Vec<TypeRelation> = Vec::new();
        let mut names: NamePositions = HashMap::new();
        // The references by name each file reported (contract 0.3), in file
        // order, banked until every file has been read.
        let mut name_refs: Vec<FileNameRefs> = Vec::new();
        progress.total(tracked.len());
        walk_files(
            repo,
            &tracked,
            progress,
            PHASE_DECL,
            self.cache.as_ref(),
            |key| self.plugin_for(key, Pass::Declarations).is_some(),
            |batch| {
                batch
                    .par_iter()
                    .map_init(
                        || pool.lease(),
                        |sessions, (key, src)| {
                            let Some(plugin) = self.plugin_for(key, Pass::Declarations) else {
                                return FileOutput::default();
                            };
                            let Some(session) = sessions.get(plugin) else {
                                return FileOutput::failed_because(
                                    key,
                                    &ids[plugin.id()],
                                    "declarations",
                                    "the plugin could not be started",
                                    &self.secret_prefixes,
                                );
                            };
                            match session.declarations(key, src, &config) {
                                Ok(declarations) => output_from_ops(
                                    key,
                                    &ids[plugin.id()],
                                    declarations,
                                    &vocabulary,
                                    &self.secret_prefixes,
                                ),
                                Err(e) => {
                                    // A 0.3 plugin's own reason is in the
                                    // text: redacted, as coverage stores it.
                                    eprintln!(
                                        "{}",
                                        super::external::redact_secret_block(
                                            &e.to_string(),
                                            &self.secret_prefixes
                                        )
                                        .0
                                    );
                                    FileOutput::failed_because(
                                        key,
                                        &ids[plugin.id()],
                                        "declarations",
                                        &e.reason(),
                                        &self.secret_prefixes,
                                    )
                                }
                            }
                        },
                    )
                    .collect()
            },
            |mut out| {
                names.extend(
                    std::mem::take(&mut out.names)
                        .into_iter()
                        .map(|(file, key, at)| ((file, key), at)),
                );
                if let Some(refs) = out.take_name_refs() {
                    name_refs.push(refs);
                }
                apply_file_output(graph, out, &mut relations)
            },
        )?;
        flush_type_relations(graph, relations);
        // The references by name, against every declaration of the run, the
        // way the type relations above are (plan 2c, U5). A run whose plugins
        // report none changes nothing here.
        super::name_ref::resolve(
            graph,
            &name_refs,
            &names,
            &tracked,
            self.ambiguity_ceiling,
            &|file| {
                self.plugin_for(file, Pass::Declarations)
                    .map(|p| ids[p.id()].clone())
            },
        );
        drop(name_refs);

        // ---- Pass 2: usages ----
        let table = SymbolTable::build(graph, &names);
        let project = Arc::new(self.project_usings(repo, &tracked));
        self.usings_handoff.put(Arc::clone(&project));
        match (&self.names_handoff, self.pair_with_index) {
            (Some(handoff), true) => {
                handoff.put(names.clone());
                graph.sites_mut().keep_declaration_names(names);
            }
            (Some(handoff), false) => handoff.put(names),
            (None, true) => graph.sites_mut().keep_declaration_names(names),
            (None, false) => drop(names),
        }
        progress.phase("languages usages", Some(tracked.len()));
        let mut discard = Vec::new();
        // Not decided by the fingerprint, which a measurement and a run that
        // keeps records leave as generation's so all three share the
        // declaration records. A usage record holds the counters of a run that
        // keeps no records and no records at all (`FileOutputWire`), so it
        // cannot serve a run that needs them; and a measurement's records hold
        // verdicts a generation's do not.
        let usage_cache = if self.trace_over_ceiling || keep_records {
            None
        } else {
            self.cache.as_ref()
        };
        walk_files(
            repo,
            &tracked,
            progress,
            PHASE_USAGES,
            usage_cache,
            |key| self.plugin_for(key, Pass::Usages).is_some(),
            |batch| {
                batch
                    .par_iter()
                    .map_init(
                        || pool.lease(),
                        |sessions, (key, src)| {
                            let mut out = FileOutput::default();
                            let Some(plugin) = self.plugin_for(key, Pass::Usages) else {
                                return out;
                            };
                            let Some(session) = sessions.get(plugin) else {
                                return FileOutput::failed_because(
                                    key,
                                    &ids[plugin.id()],
                                    "usages",
                                    "the plugin could not be started",
                                    &self.secret_prefixes,
                                );
                            };
                            match session.usages(key, src) {
                                Ok(usages) => {
                                    let file = FileSites::from_wire(usages);
                                    flush_usages(
                                        key,
                                        &ids[plugin.id()],
                                        &mut out,
                                        &table,
                                        self.ambiguity_ceiling,
                                        &file,
                                        &self.rights_for(plugin, key),
                                        &project,
                                        keep_records,
                                        self.trace_over_ceiling,
                                    );
                                    out
                                }
                                Err(e) => {
                                    // A 0.3 plugin's own reason is in the
                                    // text: redacted, as coverage stores it.
                                    eprintln!(
                                        "{}",
                                        super::external::redact_secret_block(
                                            &e.to_string(),
                                            &self.secret_prefixes
                                        )
                                        .0
                                    );
                                    FileOutput::failed_because(
                                        key,
                                        &ids[plugin.id()],
                                        "usages",
                                        &e.reason(),
                                        &self.secret_prefixes,
                                    )
                                }
                            }
                        },
                    )
                    .collect()
            },
            |out| apply_usage_output(graph, out, &mut discard),
        )?;
        assert!(discard.is_empty(), "the usage walk names no type relations");
        Ok(())
    }
}

// ---- Wire → graph ----

/// The lines a statement was read from. Where the declaration's name is
/// written (`place.name`, contract 0.2) is not part of a provenance — nothing
/// in the graph has a column — and travels beside it instead, in
/// [`FileOutput::names`], to the symbol table's owner lookup (plan A, KTD7).
fn span_of(place: Place) -> Span {
    match place.span {
        WireSpan::Whole => Span::Whole,
        WireSpan::Line(line) => Span::at(Some(line)),
        WireSpan::Range((start, end)) => Span::range(start, end),
    }
}

fn provenance(file: &str, plugin: &ExtractorId, place: Place) -> Provenance {
    let mut prov = Provenance::new(file, None, plugin.clone(), Tier::Measured);
    prov.span = span_of(place);
    prov
}

/// The declared node kind `word` (`namespace:name`) names in `vocabulary`, or
/// `None` when this corpus declares no such kind. The vocabulary is small and
/// read once per symbol that names a kind, so a scan is enough.
fn node_kind_named(word: &str, vocabulary: &Vocabulary) -> Option<NodeKindId> {
    let (namespace, name) = word.split_once(':')?;
    vocabulary
        .node_declarations()
        .find(|(_, d)| d.namespace == namespace && d.name == name)
        .map(|(id, _)| *id)
}

/// A plugin's declaration ops as the graph changes and pending relations the
/// host applies. A kind this corpus cannot name is dropped with a message
/// rather than guessed at (R7); a text value is redacted again, whatever the
/// plugin did (R22), and a doc comment loses every key-shaped word (KTD11
/// rule 3), each loss recorded as an `AnchorSkipped` on the same node. A C#
/// signature ends before an expression body.
///
/// What only contract 0.3 says rides beside the ops: a symbol of a node kind
/// from the plugin's own vocabulary becomes a node of that kind — a kind the
/// plugin did not declare drops the whole group with a message, as an
/// undeclared fact kind drops its fact — and its block, the references by
/// name and the lookup blocks are kept for the later resolution of those
/// references. A name a reference spells is redacted like any text value, so
/// it still matches the declared name it means.
fn output_from_ops(
    file: &str,
    plugin: &ExtractorId,
    declarations: FileDeclarations,
    vocabulary: &Vocabulary,
    prefixes: &SecretPrefixes,
) -> FileOutput {
    let FileDeclarations {
        ops,
        symbols: extras,
        name_refs,
        blocks,
    } = declarations;
    let mut out = FileOutput {
        name_refs: name_refs
            .into_iter()
            .map(|r| NameRef {
                name: super::external::redact_secret_block(&r.name, prefixes).0,
                ..r
            })
            .collect(),
        blocks,
        ..FileOutput::default()
    };
    let mut extras = extras.into_iter().peekable();
    // The fact, and the skips cutting a doc comment's long tokens left.
    let fact_of = |f: crate::plugin::host::WireFact| -> Option<(Fact, Vec<Fact>)> {
        let kind = crate::graph::fact_kind_from_word(&f.kind, vocabulary).or_else(|| {
            eprintln!(
                "plugin `{plugin}` emitted the undeclared fact kind `{}` in `{file}`",
                f.kind
            );
            None
        })?;
        let value = match f.value {
            Value::Text(t) => FactValue::Text(super::external::redact_secret_block(&t, prefixes).0),
            Value::Int(i) => FactValue::Int(i),
            Value::Flag(b) => FactValue::Bool(b),
        };
        let mut fact = Fact::new(kind, value, provenance(file, plugin, f.place));
        // A C# expression-bodied property (`string Token => "…";`) reaches the
        // host with its body: the walker cuts a signature at a body or at the
        // accessors, and this member has neither. A declaration header holds
        // no `=>` of its own in C#, so the first one starts the body — which
        // may be the very credential a card must not quote (R10).
        if kind == FactKind::SymbolSignature && file.ends_with(".cs") {
            if let FactValue::Text(t) = &mut fact.value {
                if let Some(at) = t.find("=>") {
                    t.truncate(t[..at].trim_end().len());
                }
            }
        }
        let skips = super::external::cut_doc_long_tokens(&mut fact);
        Some((fact, skips))
    };
    let edge_of = |e: crate::plugin::host::WireEdge| -> Option<(NodeKey, NodeKey, Edge)> {
        let kind = crate::graph::edge_kind_from_word(&e.kind, vocabulary).or_else(|| {
            eprintln!(
                "plugin `{plugin}` emitted the undeclared edge kind `{}` in `{file}`",
                e.kind
            );
            None
        })?;
        Some((
            NodeKey::new(e.source),
            NodeKey::new(e.target),
            Edge::new(kind, provenance(file, plugin, e.place)),
        ))
    };
    for (index, op) in ops.into_iter().enumerate() {
        match op {
            Op::Fact(kf) => {
                if let Some((fact, skips)) = fact_of(kf.fact) {
                    let node = NodeKey::new(kf.node);
                    out.fact(node.clone(), fact);
                    for skip in skips {
                        out.fact(node.clone(), skip);
                    }
                }
            }
            Op::Symbol(sym) => {
                let extra = extras.next_if(|e| e.op == index);
                if let Some(word) = extra.as_ref().and_then(|e| e.node_kind.as_deref()) {
                    // Only the plugin's own namespace: a kind is "from its
                    // vocabulary" or it is not declared for this plugin.
                    let own = word
                        .split_once(':')
                        .is_some_and(|(namespace, _)| namespace == &**plugin);
                    match node_kind_named(word, vocabulary).filter(|_| own) {
                        Some(id) => out.node_kinds.push((NodeKey::new(sym.key.clone()), id)),
                        None => {
                            eprintln!(
                                "plugin `{plugin}` emitted the undeclared node kind `{word}` in `{file}`; the symbol `{}` is dropped",
                                sym.key
                            );
                            continue;
                        }
                    }
                }
                if let Some(block) = extra.as_ref().and_then(|e| e.block) {
                    out.symbol_blocks
                        .push((NodeKey::new(sym.key.clone()), block));
                }
                if let Some(at) = sym
                    .facts
                    .iter()
                    .find_map(|f| f.place.name.as_ref().map(|p| (p.line, p.column)))
                {
                    out.names
                        .push((file.to_string(), NodeKey::new(sym.key.clone()), at));
                }
                let group = out.symbol(NodeKey::new(sym.key));
                for f in sym.facts {
                    if let Some((fact, skips)) = fact_of(f) {
                        group.push_fact(fact);
                        for skip in skips {
                            group.push_fact(skip);
                        }
                    }
                }
                for e in sym.edges {
                    if let Some((from, to, edge)) = edge_of(e) {
                        group.push_edge(from, to, edge);
                    }
                }
            }
            Op::Edge(e) => {
                if let Some((from, to, edge)) = edge_of(e) {
                    out.edge(from, to, edge);
                }
            }
            Op::Relation(r) => out.relations.push(TypeRelation {
                from: r.sources.into_iter().map(NodeKey::new).collect(),
                to: r.targets.into_iter().map(NodeKey::new).collect(),
                kind: match r.kind {
                    WireRelationKind::Extends => RelationKind::Extends,
                    WireRelationKind::Implements => RelationKind::Implements,
                    WireRelationKind::ByTargetKind => RelationKind::ByTargetKind,
                },
                prov: provenance(file, plugin, r.place),
            }),
        }
    }
    out
}

/// One type relation a declaration names, buffered until the whole repo has been
/// walked (U3). Nothing is resolved at collection time: both ends are CANDIDATE
/// identities, and the relation becomes an edge in [`flush_type_relations`] only
/// if both exist by then. A base type this repo does not declare therefore yields
/// no edge and no phantom node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TypeRelation {
    /// Candidate identities of the DERIVING type, most specific first.
    from: Vec<NodeKey>,
    /// Candidate identities of the BASE type, most specific first.
    to: Vec<NodeKey>,
    kind: RelationKind,
    /// Anchored at the base type's own line — the bytes the relation was read
    /// from.
    prov: Provenance,
}

/// Which relation a declaration names — or, in C#, that the syntax does not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RelationKind {
    Extends,
    Implements,
    /// A C# base list writes a base class and an implemented interface
    /// identically (`class D : B, IFoo`). Decided at flush time from the resolved
    /// target's own `SymbolKind` fact — never from an `I`-prefix naming
    /// convention, which would be a guess the graph must not make.
    ByTargetKind,
}

// ---- Emission ----

/// Everything one file's declaration walk produces (U1/KTD1).
///
/// The graph changes and the type relations travel together but are not the
/// same kind of thing: the ops are ready to apply, while a relation still has to
/// find its other end, which routinely lives in another file and so cannot be
/// resolved until the whole repository has been walked (KTD4).
///
/// Not serializable for the same reason [`GraphOps`] is not (U7): its ops carry
/// kinds, and a kind needs the corpus's vocabulary to become a name. The cache
/// sees [`FileOutputWire`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FileOutput {
    ops: GraphOps,
    relations: Vec<TypeRelation>,
    /// Where each declaration's name is written, `(file, key, (line, column))`
    /// — what tells two declarations on one line apart (contract 0.2, KTD7).
    /// Declaration pass only.
    names: Vec<(String, NodeKey, (u32, u32))>,
    /// What the resolver decided at each usage site (KTD7) — usage pass only,
    /// and only in a run that keeps the records.
    sites: Vec<SiteRecord>,
    /// The ledger's two counters over this file's sites, in a usage pass that
    /// keeps no records and so builds none ([`SiteCounts`]).
    site_counts: SiteCounts,
    /// Set when the plugin could not read this file (a trap, exhausted fuel).
    /// A failed output applies only its [`FactKind::ExtractionFailed`] fact
    /// and is never cached, so a transient failure is retried on the next run
    /// rather than served as "no facts" forever (U10, R29).
    failed: bool,
    /// The symbols this file declares as nodes of a kind from their plugin's
    /// own vocabulary rather than as [`NodeKind::Symbol`] (contract 0.3).
    /// Declaration pass only.
    node_kinds: Vec<(NodeKey, NodeKindId)>,
    /// The innermost lookup block each symbol is written in, by index into
    /// `blocks` (contract 0.3). Declaration pass only.
    symbol_blocks: Vec<(NodeKey, u32)>,
    /// References by name, for resolution once every file has been read
    /// (contract 0.3). Declaration pass only.
    name_refs: Vec<NameRef>,
    /// The nested blocks `name_refs` and `symbol_blocks` are looked up in.
    blocks: Vec<LookupBlock>,
}

/// [`FileOutput`] as the cache writes it. `relations` needs no twin — a
/// [`TypeRelation`] is keyed by its own closed [`RelationKind`], which no plugin
/// can extend, so nothing in it is a per-corpus name. Neither do `names` and
/// `site_counts`, which speak only the closed [`Rule`] vocabulary and carry no
/// provenance, so a cache hit has no extractor id in it to canonicalize.
///
/// The site records themselves are not on the wire: only a run that keeps no
/// records reads or writes a usage record (see [`crate::graph::sites`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct FileOutputWire {
    ops: crate::extract::wire::WireOps,
    relations: Vec<TypeRelation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    names: Vec<(String, NodeKey, (u32, u32))>,
    #[serde(default, skip_serializing_if = "SiteCounts::is_empty")]
    site_counts: SiteCounts,
    /// The new fields of contract 0.3, each omitted when empty — so a record
    /// of a file without them, every record a 0.1 or 0.2 plugin produces, is
    /// written byte for byte as before and `CACHE_VERSION` did not move. A
    /// node kind is written by its name, as every kind on the wire is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    node_kinds: Vec<(NodeKey, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    symbol_blocks: Vec<(NodeKey, u32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    name_refs: Vec<NameRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    blocks: Vec<LookupBlock>,
}

impl super::walk::WalkOutput for FileOutput {
    type Wire = FileOutputWire;

    fn is_failure(&self) -> bool {
        self.failed
    }

    fn to_wire(&self, vocabulary: &crate::graph::Vocabulary) -> Option<FileOutputWire> {
        // The wire has no place for site records, so an output holding them
        // is not written rather than written without them: served later, such
        // a record would hand a run that keeps records a file with none. No
        // run writes one today — a run that keeps records has no usage cache
        // — and this keeps that a property of the type, not of the caller.
        // Loud in a debug build: `walk::write_records` reports a refused
        // record as an unnameable kind, which would name the wrong cause.
        debug_assert!(
            self.sites.is_empty(),
            "a usage output with site records reached the cache"
        );
        if !self.sites.is_empty() {
            return None;
        }
        Some(FileOutputWire {
            ops: self.ops.to_wire(vocabulary)?,
            relations: self.relations.clone(),
            names: self.names.clone(),
            site_counts: self.site_counts.clone(),
            // Refused like any kind this vocabulary cannot name (RG2).
            node_kinds: self
                .node_kinds
                .iter()
                .map(|(key, id)| Some((key.clone(), vocabulary.node_name(*id)?)))
                .collect::<Option<Vec<_>>>()?,
            symbol_blocks: self.symbol_blocks.clone(),
            name_refs: self.name_refs.clone(),
            blocks: self.blocks.clone(),
        })
    }

    fn from_wire(
        wire: FileOutputWire,
        vocabulary: &crate::graph::Vocabulary,
    ) -> Option<FileOutput> {
        Some(FileOutput {
            ops: wire.ops.into_ops(vocabulary)?,
            relations: wire.relations,
            names: wire.names,
            sites: Vec::new(),
            site_counts: wire.site_counts,
            failed: false,
            // A node kind this corpus no longer declares refuses the whole
            // record, as `into_ops` refuses an unknown fact kind.
            node_kinds: wire
                .node_kinds
                .into_iter()
                .map(|(key, word)| Some((key, node_kind_named(&word, vocabulary)?)))
                .collect::<Option<Vec<_>>>()?,
            symbol_blocks: wire.symbol_blocks,
            name_refs: wire.name_refs,
            blocks: wire.blocks,
        })
    }

    /// Both halves. `relations` is easy to forget — a `TypeRelation` carries its
    /// own `prov`, so skipping it would leave one un-canonicalized id per
    /// pending relation and let a stale record through the guard.
    fn provenances_mut(&mut self) -> Vec<&mut crate::graph::Provenance> {
        // Destructured, not field-accessed, so a third field cannot be added
        // without this failing to compile — see the note in `GraphOps`' impl.
        // `names`, `sites` and `site_counts` carry no provenance (see
        // `FileOutputWire`), and neither do the fields of contract 0.3: a
        // reference by name is the file's own, read back with it.
        let FileOutput {
            ops,
            relations,
            names: _,
            sites: _,
            site_counts: _,
            failed: _,
            node_kinds: _,
            symbol_blocks: _,
            name_refs: _,
            blocks: _,
        } = self;
        let mut out = ops.provenances_mut();
        out.extend(relations.iter_mut().map(|r| &mut r.prov));
        out
    }
}

impl FileOutput {
    /// The output of a file the plugin failed on: nothing but the statement
    /// that it failed, in which pass and why.
    fn failed_because(
        file: &str,
        plugin: &ExtractorId,
        pass: &str,
        reason: &str,
        prefixes: &SecretPrefixes,
    ) -> FileOutput {
        let reason = super::external::redact_secret_block(reason, prefixes).0;
        let mut out = FileOutput {
            failed: true,
            ..FileOutput::default()
        };
        out.fact(
            NodeKey::new(file),
            Fact::new(
                FactKind::ExtractionFailed,
                FactValue::Text(format!("{pass}: {reason}")),
                Provenance::new(file, None, plugin.clone(), Tier::Measured),
            ),
        );
        out
    }

    fn fact(&mut self, key: NodeKey, fact: Fact) {
        self.ops.fact(key, fact);
    }

    fn edge(&mut self, from: NodeKey, to: NodeKey, edge: Edge) {
        self.ops.edge(from, to, edge);
    }

    fn symbol(&mut self, key: NodeKey) -> &mut GraphOp {
        self.ops.symbol(key)
    }

    /// The references by name, their blocks and the symbols' blocks, taken
    /// out of the output — `None` when the file has none of them.
    fn take_name_refs(&mut self) -> Option<FileNameRefs> {
        let refs = FileNameRefs {
            name_refs: std::mem::take(&mut self.name_refs),
            blocks: std::mem::take(&mut self.blocks),
            symbol_blocks: std::mem::take(&mut self.symbol_blocks),
        };
        (refs != FileNameRefs::default()).then_some(refs)
    }
}

/// Apply one file's declaration output, and bank its type relations for the
/// single flush that resolves them all (KTD4). A symbol of a declared node
/// kind (contract 0.3) becomes a node of that kind.
fn apply_file_output(graph: &mut Graph, out: FileOutput, relations: &mut Vec<TypeRelation>) {
    let declared: HashMap<NodeKey, NodeKind> = out
        .node_kinds
        .into_iter()
        .map(|(key, id)| (key, NodeKind::Declared(id)))
        .collect();
    out.ops.apply_with_kinds(graph, &declared);
    relations.extend(out.relations);
}

/// Apply one file's usage output, and bank its site records in the graph's
/// ledger (plan A, KTD7). Called in the walk's own file order, on one thread,
/// so the ledger never depends on how the workers were scheduled.
fn apply_usage_output(graph: &mut Graph, mut out: FileOutput, relations: &mut Vec<TypeRelation>) {
    for record in std::mem::take(&mut out.sites) {
        graph.sites_mut().record(record);
    }
    graph.sites_mut().count(&out.site_counts);
    apply_file_output(graph, out, relations);
}

/// The first of `keys` that is already a node in the graph.
fn first_existing_key<'a>(graph: &Graph, keys: &'a [NodeKey]) -> Option<&'a NodeKey> {
    keys.iter().find(|k| graph.node(k).is_some())
}

/// True if `key` carries a `SymbolKind` fact equal to `kind` — how a C# base list
/// learns whether its entry is an interface, instead of reading an `I` prefix.
fn declares_symbol_kind(graph: &Graph, key: &NodeKey, kind: &str) -> bool {
    graph.node(key).is_some_and(|n| {
        n.facts
            .iter()
            .any(|f| f.kind == FactKind::SymbolKind && f.value == FactValue::Text(kind.to_string()))
    })
}

/// Resolve the buffered type relations into edges (U3).
///
/// Deferred to the end of the RUN, not of a file: a base type routinely lives in
/// another file of the same module — a C# namespace spread over one file per type
/// — so a per-file flush would make an edge depend on the order files are visited
/// in. A relation whose base, or whose deriving type, is not a Symbol node this
/// repo declares is dropped: no phantom target is fabricated (U5 owns external
/// targets), and the declaration's own spelling of the base survives verbatim in
/// the deriving type's `SymbolSignature` regardless (R20).
///
/// Every edge stays `Resolution::Heuristic` — a name match is all this is (KD2)
/// — and carries its rung and rule (plan A, R2): rules-backed by
/// `base-spelling` where the base was found by its spelling, name-only by
/// `name-match` where it was found by its simple name alone.
fn flush_type_relations(graph: &mut Graph, relations: Vec<TypeRelation>) {
    let mut seen: HashSet<(String, EdgeKind, String)> = HashSet::new();
    let unique = unique_type_names(graph);
    for relation in relations {
        let TypeRelation {
            from,
            to,
            kind,
            prov,
        } = relation;
        // The spelled candidates first; failing those, the one type of that
        // name the whole repository declares in the same language family. A
        // base in another file of a TypeScript/JavaScript project, or in
        // another C# namespace, is the ordinary case, and a module-local
        // lookup alone left every such inheritance unlinked.
        let spelled = first_existing_key(graph, &to).cloned();
        let backed = spelled.is_some();
        let to = spelled.or_else(|| unique_base(&unique, &to));
        let (Some(from), Some(to)) = (first_existing_key(graph, &from), to.as_ref()) else {
            continue;
        };
        // A type extending itself is not a relation; it only arises when a
        // spelling resolves back onto the declaring type.
        if from == to {
            continue;
        }
        let kind = match kind {
            RelationKind::Extends => EdgeKind::Extends,
            RelationKind::Implements => EdgeKind::Implements,
            RelationKind::ByTargetKind => {
                if declares_symbol_kind(graph, to, "interface") {
                    EdgeKind::Implements
                } else {
                    EdgeKind::Extends
                }
            }
        };
        // Two `#[cfg]`-gated impls of one trait, or a base named twice, are one
        // relation — the same dedup discipline as the imports `HashSet`.
        if !seen.insert((from.as_str().to_string(), kind, to.as_str().to_string())) {
            continue;
        }
        // Both ways of finding the base end on exactly one target. Found by
        // its spelling — the declaring type's own module, or the qualified
        // path the source writes — the `base-spelling` rule backs it (plan A,
        // KTD6). Found only as the one type of that name anywhere in the
        // repository, nothing backs it: the real base may be an external type
        // that shares the name, so it is a name match. Rated here, at the
        // run-level flush, so the cached per-file `TypeRelation` records never
        // carry a rung of their own.
        let prov = if backed {
            prov.rated(Confidence::RulesBacked, Rule::BaseSpelling)
        } else {
            prov.rated(Confidence::NameOnly, Rule::NameMatch)
        };
        // The declaration's own line is the relation's one site (plan A,
        // KTD7). A base named twice is deduplicated above and keeps the first.
        let site = SiteId::of(&prov);
        graph.add_edge(from, to, Edge::new(kind, prov).at_sites(site));
    }
}

/// Language family of a `sym:` key: TypeScript and JavaScript are one, since
/// either may extend a class the other declares.
pub(crate) fn lang_family(key: &str) -> Option<&str> {
    let lang = key.strip_prefix("sym:")?.split(':').next()?;
    Some(if lang == "js" { "ts" } else { lang })
}

/// The last segment of a type path inside a `sym:` key: `a.b#Outer.Inner` →
/// `Inner`, `crate::m#T` → `T`.
fn simple_type_name(key: &str) -> Option<&str> {
    let path = key.split_once('#')?.1;
    let path = path.split('~').next().unwrap_or(path);
    path.rsplit(['.', ':']).next().filter(|n| !n.is_empty())
}

/// `(family, name)` → the type keys declared under it, for every type the graph
/// holds. Built once per flush.
fn unique_type_names(graph: &Graph) -> HashMap<(String, String), Vec<NodeKey>> {
    let mut out: HashMap<(String, String), Vec<NodeKey>> = HashMap::new();
    for node in graph.nodes_sorted() {
        if node.kind != NodeKind::Symbol {
            continue;
        }
        let is_type = node.facts.iter().any(|f| {
            f.kind == FactKind::SymbolKind
                && matches!(&f.value, FactValue::Text(k) if symbol_role(k) == Some(Role::Type))
        });
        if !is_type {
            continue;
        }
        let key = node.key.as_str();
        if let (Some(family), Some(name)) = (lang_family(key), simple_type_name(key)) {
            out.entry((family.to_string(), name.to_string()))
                .or_default()
                .push(node.key.clone());
        }
    }
    out
}

/// The single repository type a base spelling can name when no spelled
/// candidate exists — `None` when there is none, or more than one.
fn unique_base(
    unique: &HashMap<(String, String), Vec<NodeKey>>,
    candidates: &[NodeKey],
) -> Option<NodeKey> {
    let first = candidates.first()?.as_str();
    let family = lang_family(first)?;
    let name = simple_type_name(first)?;
    match unique
        .get(&(family.to_string(), name.to_string()))?
        .as_slice()
    {
        [only] => Some(only.clone()),
        _ => None,
    }
}

// ---- Calls and references: the repo-wide symbol table (U4) ----

/// What a symbol can be the TARGET of. Only these two roles are addressable by a
/// usage site: a property, a field, an enum variant or a synthetic entry point
/// names neither a callable to call nor a type to reference, so none of them ever
/// becomes a `Calls`/`References`/`Instantiates` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Callable,
    Type,
}

/// The role a `SymbolKind` fact value plays, or `None` for a symbol nothing can
/// name — read off the emitted fact rather than re-derived from syntax, so the
/// table can never disagree with the graph it was built from.
pub(crate) fn symbol_role(kind: &str) -> Option<Role> {
    match kind {
        "function" | "method" | "constructor" | "procedure" => Some(Role::Callable),
        "class" | "struct" | "interface" | "trait" | "enum" | "record" | "type" => Some(Role::Type),
        // SQL: what a query names in `FROM`, and a trigger, which nothing
        // names but which earns a card of its own like a type does.
        "table" | "view" | "trigger" => Some(Role::Type),
        _ => None,
    }
}

// ---- Usage sites, as the plugins report them ----

/// What a used identifier implies about the relation it creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageKind {
    /// A syntactic call — `f(…)`, `x.m(…)`.
    Call,
    /// A call the language also uses for construction; types are tried when
    /// no callable matches.
    CallOrConstruct,
    /// An explicit construction — `new T(…)`, a struct or composite literal.
    Instantiate,
    /// A type named in a type position.
    Reference,
}

/// One identifier a file uses, before resolution. In tree order, so the first
/// site of a repeated relation is the one that anchors its edge.
struct UsageSite {
    /// The SIMPLE name to resolve.
    name: String,
    kind: UsageKind,
    /// 1-based line of the usage, which the edge's provenance points at and
    /// which decides the enclosing declaration. The line of the used name
    /// itself when `column` is present (contract 0.2), the line the usage
    /// starts on when it is not.
    line: u32,
    /// 1-based column of the used name, when the plugin reports one — the
    /// part of the site's id (KTD7) that keeps two calls on one line apart.
    column: Option<u32>,
    /// What the syntax says about the site, as the plugin reported it
    /// (contract 0.2, KTD3); `None` from a 0.1 plugin and for a language whose
    /// plugin reads no context — where the shared resolver runs no rule.
    context: Option<SiteContext>,
}

/// Everything one file uses: its sites, and the lexical scopes their context
/// points into (contract 0.2). A 0.1 plugin's file has no scopes.
struct FileSites {
    /// The file's scopes, the file itself first — the namespaces with their
    /// using directives that a site's `context.scope` indexes.
    scopes: Vec<WireScope>,
    sites: Vec<UsageSite>,
}

impl FileSites {
    /// A plugin's usages in the host's shape. Both contract versions arrive
    /// here in 0.2's wire shape: the session lifts a 0.1 plugin's output at
    /// the boundary, with no column, no context and no scopes.
    fn from_wire(file: FileUsages) -> FileSites {
        FileSites {
            scopes: file.scopes,
            sites: file
                .usages
                .into_iter()
                .map(|u| UsageSite {
                    name: u.name,
                    kind: match u.kind {
                        WireUsageKind::Call => UsageKind::Call,
                        WireUsageKind::CallOrConstruct => UsageKind::CallOrConstruct,
                        WireUsageKind::Construct => UsageKind::Instantiate,
                        WireUsageKind::Reference => UsageKind::Reference,
                    },
                    line: u.line,
                    column: u.column,
                    context: u.context,
                })
                .collect(),
        }
    }
}

/// Resolve a file's usage sites into edges, facts and site records (U4/R23,
/// plan A KTD5–KTD7).
///
/// Every edge stays `Resolution::Heuristic`: the callee was matched by NAME, which
/// is all a syntax tree without type information can honestly claim (KD2). The
/// shared resolver ([`resolve_site`]) sifts each site's candidates with the
/// rule catalog and gives every one its rung and rule (R1); this function still
/// owns what the graph receives — the ceiling's fact, the per-file dedup of
/// relations and the ambiguity facts. Zero candidates emit NOTHING — a name
/// this repo does not declare is an external call, and inventing a target node
/// would be a lie (U5 owns external targets) — and neither does a call on a
/// language keyword (`string.IsNullOrEmpty(x)`): such a receiver cannot reach a
/// declaration in the repository, which the grammar, not a rule, says (R5).
///
/// `rights` are the rules holding a valid right to delete for this file's
/// sites (KTD11); `project` what the usings of the file's project import into
/// every file of it; `records` builds a [`SiteRecord`] for every site, where
/// otherwise only the ledger's counters are kept ([`SiteCounts`]); `measuring`
/// runs the rules at sites over the ceiling too ([`resolve_site`]).
#[allow(clippy::too_many_arguments)]
fn flush_usages(
    file: &str,
    plugin: &ExtractorId,
    out: &mut FileOutput,
    table: &SymbolTable,
    ceiling: usize,
    file_sites: &FileSites,
    rights: &[Rule],
    project: &ProjectUsings,
    records: bool,
    measuring: bool,
) {
    let file_key = NodeKey::new(file.to_string());
    let family = table.family_of_file(file);
    let project_wide = project.for_file(file);
    // A relation repeated within one file is ONE relation: three calls to the same
    // method from the same body are not three facts about the architecture. It
    // is one edge that knows all three places (plan A, KTD7) — the relation's
    // site list — and whose provenance is the site with the STRONGEST rung
    // (KTD5: an edge is the summary of its sites, its rung the strongest among
    // them). Equal rungs keep the first site in source order, so the anchor is
    // deterministic and the lower line wins — the discipline
    // `flush_type_relations` and the imports `HashSet` follow.
    //
    // Edges are emitted after the loop, in first-seen order, and so after the
    // ambiguity facts rather than between them; nothing reads a file's ops in
    // order — the graph keys both, and merge and every projection sort them.
    let mut relations: Vec<(NodeKey, EdgeKind, NodeKey, Provenance, Vec<SiteId>)> = Vec::new();
    let mut relation_at: HashMap<(String, EdgeKind, String), usize> = HashMap::new();
    let mut facts_seen: HashSet<(String, EdgeKind, String)> = HashSet::new();

    for site in &file_sites.sites {
        if site
            .context
            .as_ref()
            .is_some_and(|c| matches!(c.receiver, Receiver::Keyword(_)))
        {
            continue;
        }
        let Some((edge_kind, candidates)) = resolve_usage(table, site) else {
            continue;
        };
        // The innermost declaration around the site — normally a member, which is
        // why members are their own nodes (KTD5). Module-level code (a Python
        // script body, a C# top-level statement) is enclosed by no declaration at
        // all, so the FILE carries the relation rather than nothing carrying it.
        // Found before the rules run: `implicit-this` reads the caller's type.
        let lookup = table.owner(file, site.line, site.column);
        let (mapped, owner_mapped) = match lookup {
            Owner::Mapped(key) => (Some(key), true),
            Owner::Unmapped(_) => (None, false),
            Owner::None => (None, true),
        };
        let owner = match lookup {
            Owner::Mapped(key) | Owner::Unmapped(Some(key)) => key.clone(),
            Owner::Unmapped(None) | Owner::None => file_key.clone(),
        };
        let prov = Provenance::new(file, Some(site.line), plugin.clone(), Tier::Measured);
        let rules_site = site.context.as_ref().map(|context| Site {
            kind: site.kind,
            context,
            scopes: &file_sites.scopes,
            project: project_wide,
        });
        let resolved = resolve_site(
            table,
            rules_site.as_ref(),
            mapped,
            family,
            candidates,
            rights,
            ceiling,
            measuring,
        );
        if !records {
            out.site_counts.tally(
                resolved.over_cap,
                resolved.candidates.iter().filter_map(|j| match j.fate {
                    Fate::Deleted(rule) => Some(rule),
                    _ => None,
                }),
            );
        } else {
            out.sites.push(SiteRecord {
                file: file.to_string(),
                owner: owner.clone(),
                line: site.line,
                column: site.column,
                name: site.name.clone(),
                kind: match edge_kind {
                    EdgeKind::Instantiates => SiteKind::Instantiates,
                    EdgeKind::References => SiteKind::References,
                    _ => SiteKind::Calls,
                },
                owner_mapped,
                candidate_count: candidates.len(),
                candidates: resolved
                    .candidates
                    .iter()
                    .map(|j| CandidateRecord {
                        target: j.target.clone(),
                        trace: j
                            .trace
                            .iter()
                            .map(|&(rule, verdict)| RuleVerdict { rule, verdict })
                            .collect(),
                        outcome: match j.fate {
                            Fate::Kept { rung, rule } => CandidateOutcome::Rated { rung, rule },
                            Fate::Deleted(rule) => CandidateOutcome::Deleted { rule },
                            Fate::OverCap => CandidateOutcome::OverCap,
                        },
                    })
                    .collect(),
                // The index pairing's to fill, after the walk (U9).
                index: None,
            });
        }

        if resolved.over_cap {
            // R23: above the ceiling the graph says "ambiguous, N candidates" and
            // nothing else. Named edges stay a privilege of resolved extraction —
            // otherwise one `Get` or `Execute` buries every real relation around it.
            // The fact is NOT an absence claim: the kind is what tells a consumer
            // these edges are missing because the name is ambiguous (R20).
            if facts_seen.insert(fact_key(&owner, edge_kind, &site.name)) {
                out.fact(
                    owner,
                    Fact::new(
                        FactKind::AmbiguousReference,
                        FactValue::Text(candidate_text(&site.name, resolved.count)),
                        prov,
                    ),
                );
            }
            continue;
        }

        // Below/at the ceiling with more than one candidate left, every edge
        // this site produces carries the count (R1) — the reader sees the
        // caveat at the claim itself. The fact below stays on plain `prov`: it
        // carries the count in its own text (`candidate_text`).
        let ambiguous_count = (resolved.count > 1).then_some(resolved.count);
        // Every alternative of this site shares its id, so a consumer can
        // group them back into the one claim they are. The column is the
        // plugin's, when it reports one (contract 0.2); a 0.1 site has none
        // (see `SiteId`).
        let site_id = SiteId {
            file: file.to_string(),
            line: site.line,
            column: site.column,
        };
        for judged in &resolved.candidates {
            let Fate::Kept { rung, rule } = judged.fate else {
                continue;
            };
            // A recursive call is a real call and keeps its self-loop — but only
            // one, however many times the body calls itself.
            let key = (
                owner.as_str().to_string(),
                edge_kind,
                judged.target.as_str().to_string(),
            );
            let rated = || {
                let edge_prov = prov.clone().rated(rung, rule);
                match ambiguous_count {
                    Some(n) => edge_prov.ambiguous(n),
                    None => edge_prov,
                }
            };
            match relation_at.get(&key) {
                Some(&i) => {
                    let relation = &mut relations[i];
                    // Smaller is stronger (`Confidence` lists strongest first);
                    // a tie keeps the earlier site.
                    if relation.3.rung().is_none_or(|held| rung < held) {
                        relation.3 = rated();
                    }
                    relation.4.push(site_id.clone());
                }
                None => {
                    relation_at.insert(key, relations.len());
                    relations.push((
                        owner.clone(),
                        edge_kind,
                        judged.target.clone(),
                        rated(),
                        vec![site_id.clone()],
                    ));
                }
            }
        }
        if ambiguous_count.is_some() && facts_seen.insert(fact_key(&owner, edge_kind, &site.name)) {
            // The edges are there, and so is the caveat: the reader is told the
            // name matched several declarations, so a wrong one among them is
            // visible rather than silently authoritative (KTD6).
            out.fact(
                owner,
                Fact::new(
                    FactKind::CandidateCount,
                    FactValue::Text(candidate_text(&site.name, resolved.count)),
                    prov,
                ),
            );
        }
    }
    for (owner, kind, target, prov, sites) in relations {
        out.edge(owner, target, Edge::new(kind, prov).at_sites(sites));
    }
}

/// The dedup key of one ambiguity fact: one statement per (referring node,
/// relation, name), not one per repeated site.
fn fact_key(owner: &NodeKey, kind: EdgeKind, name: &str) -> (String, EdgeKind, String) {
    (owner.as_str().to_string(), kind, name.to_string())
}

/// The ambiguity fact's text — identifier plus candidate count, and nothing else.
/// The ceiling itself is deliberately absent: it is configuration, and baking it
/// into a fact value would churn every fact whenever a run raises it.
fn candidate_text(name: &str, count: usize) -> String {
    format!(
        "{name}: {count} candidate{}",
        if count == 1 { "" } else { "s" }
    )
}

/// Which relation a usage site implies and which symbols it could mean.
fn resolve_usage<'t>(
    table: &'t SymbolTable,
    site: &UsageSite,
) -> Option<(EdgeKind, &'t [NodeKey])> {
    match site.kind {
        UsageKind::Call => match table.candidates(Role::Callable, &site.name) {
            Some(c) => Some((EdgeKind::Calls, c)),
            // A call syntax naming no callable but a declared TYPE is a
            // construction — but only where the language actually spells
            // construction that way: Python `Foo()` and a Rust tuple struct
            // `Foo(x)` do, while a C# `Foo()` cannot and a Go `T(x)` is a
            // conversion, so neither of those gets the fallback.
            None => None,
        },
        // A call syntax naming no callable but a declared TYPE is a
        // construction — the plugin says where its language spells
        // construction that way (Python `Foo()`, a Rust tuple struct).
        UsageKind::CallOrConstruct => match table.candidates(Role::Callable, &site.name) {
            Some(c) => Some((EdgeKind::Calls, c)),
            None => table
                .candidates(Role::Type, &site.name)
                .map(|c| (EdgeKind::Instantiates, c)),
        },
        UsageKind::Instantiate => table
            .candidates(Role::Type, &site.name)
            .map(|c| (EdgeKind::Instantiates, c)),
        UsageKind::Reference => table
            .candidates(Role::Type, &site.name)
            .map(|c| (EdgeKind::References, c)),
    }
}

/// The file and module keys `filetree` left in the graph, frozen before the
/// declaration walk begins (U0/KTD5).
///
/// Import resolution asks the graph one question — "is there a file or a module
/// at this path?" — and asks it once per candidate, several candidates per
/// import, in every source file of the repository. The answer cannot change
/// while the walk runs: `filetree` is the only extractor that creates
/// [`NodeKind::File`] and [`NodeKind::Module`] nodes and it runs first, and
/// every other key-space carries a prefix (`sym:`, `dep:`, `ext:`, `res:`), so
/// a path-shaped candidate can never match a node a later extractor added.
///
/// Freezing the answer is what lets the declaration walk stop reading the graph
/// at all. A worker that cannot see the graph cannot depend on how much of it
/// has been built when it runs — which is the property the parallel walk rests
/// on, and the reason this exists as a type rather than as a closure over
/// `&Graph`.
pub(crate) struct PathKeys(HashSet<String>);

impl PathKeys {
    /// Freeze the file and module keys currently in `graph`.
    ///
    /// Owned strings, not borrows: the caller goes on to take `&mut Graph` for
    /// the walk, and a snapshot borrowing the graph would forbid that.
    pub(crate) fn snapshot(graph: &Graph) -> PathKeys {
        PathKeys(
            graph
                .nodes_sorted()
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::File | NodeKind::Module))
                .map(|n| n.key.as_str().to_string())
                .collect(),
        )
    }

    /// The frozen keys, for a pass that hands them to its plugins' sessions
    /// itself (the second layer, plan 2a U4).
    pub(crate) fn into_set(self) -> HashSet<String> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry's bundled languages and the bundled plugins must agree:
    /// coverage names a language by the registry, and a plugin parses it. A
    /// language one side has and the other lacks would be reported and never
    /// parsed, or parsed and never reported.
    #[test]
    fn bundled_languages_match_the_plugins_that_actually_parse() {
        let registry = crate::extract::languages::Languages::bundled();
        // C and C++ are named by the registry and read by the host lexer
        // alone (plan B, U4) — the one stated exception, and coverage says so
        // about their files rather than claiming a grammar found nothing.
        let lexed_only: std::collections::BTreeSet<&str> =
            registry.lexed_only_extensions().collect();
        let claimed: std::collections::BTreeSet<&str> = registry
            .extensions()
            .filter(|e| !lexed_only.contains(e))
            .collect();
        let parsed: std::collections::BTreeSet<&str> = crate::plugin::bundled::bundled()
            .iter()
            .flat_map(|p| p.manifest().extensions())
            .collect();
        assert_eq!(
            claimed, parsed,
            "coverage would name a language no plugin parses, or stay silent about one it does"
        );
        // …and the reporting name each plugin gives an extension is the
        // registry's name for it.
        for plugin in crate::plugin::bundled::bundled() {
            for ext in plugin.manifest().extensions() {
                assert_eq!(
                    plugin.manifest().language_for(ext),
                    registry.name_for(&format!("a.{ext}")),
                    "`.{ext}`: plugin and registry disagree on the language name"
                );
            }
        }
    }

    /// The COMMITTED `csharp.wasm` runs under contract 0.2 and reads context:
    /// a call on `string` says its receiver is a keyword (AE1). The host's
    /// tests run the component under `plugins/prebuilt/`, not the crate's
    /// source, so a walker change that was never rebuilt into the component
    /// would otherwise pass every native test and ship without effect — this
    /// fails instead (plan A, Risks: a stale component).
    #[test]
    fn the_committed_csharp_component_reads_context() {
        use crate::plugin::host::{Contract, Receiver};
        let plugin = named("csharp");
        assert_eq!(plugin.contract(), Contract::V0_2);
        let mut session = plugin
            .session(Arc::new(HashSet::new()))
            .expect("a bundled plugin opens a session");
        let file = session
            .usages(
                "a.cs",
                b"class C {\n    bool M(string s) {\n        return string.IsNullOrEmpty(s);\n    }\n}\n",
            )
            .expect("the component reads a test fixture");
        let site = file
            .usages
            .iter()
            .find(|u| u.name == "IsNullOrEmpty")
            .expect("the call is a site");
        let context = site
            .context
            .as_ref()
            .expect("a 0.2 C# site carries context");
        assert!(
            matches!(&context.receiver, Receiver::Keyword(k) if k == "string"),
            "{:?}",
            context.receiver
        );
        assert_eq!((site.line, site.column), (3, Some(23)));
        // The file's own scope, which the site points into.
        assert_eq!(file.scopes.len(), 1);
        assert_eq!(context.scope, 0);
    }

    // ---- The walkers, reached through the bundled plugins ----
    //
    // Until U11 these tests called the in-process walkers directly. The
    // walkers now run inside components, so the same names and signatures
    // below route each call through the bundled plugin that claims the file —
    // which is a stronger test than before: every assertion here now also
    // checks the boundary, the wire conversion and the host-side apply.

    /// Which grammar a TS-family test meant. The component picks it from the
    /// file extension; the argument survives so the tests read as they did.
    #[derive(Clone, Copy)]
    enum Grammar {
        TypeScript,
        Tsx,
        JavaScript,
    }

    /// The languages a test can name, standing in for the walker enum that
    /// moved into the plugin crate.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Language {
        Plugin(&'static str),
    }

    impl Language {
        fn from_path(path: &str) -> Option<Language> {
            let ext = path.rsplit_once('.').map(|(_, e)| e)?;
            crate::plugin::bundled::for_extension(ext).map(|p| Language::Plugin(p.id()))
        }
    }

    fn plugin_for(path: &str) -> &'static Plugin {
        let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        crate::plugin::bundled::for_extension(ext)
            .unwrap_or_else(|| panic!("no bundled plugin claims `{path}`"))
    }

    fn declare_into(
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        prefixes: &SecretPrefixes,
    ) {
        declare_with(plugin_for(file), file, src, out, paths, prefixes)
    }

    /// The plugin a walker test named, whatever the path says — a fixture
    /// called `x` still goes to the walker the test meant.
    fn named(id: &str) -> &'static Plugin {
        crate::plugin::bundled::bundled()
            .iter()
            .find(|p| p.id() == id)
            .unwrap_or_else(|| panic!("no bundled plugin `{id}`"))
    }

    fn declare_with(
        plugin: &'static Plugin,
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        prefixes: &SecretPrefixes,
    ) {
        let mut session = plugin
            .session(Arc::new(paths.0.clone()))
            .expect("a bundled plugin opens a session");
        let config = WireConfig {
            secret_prefixes: prefixes.extra().to_vec(),
        };
        let declarations = session
            .declarations(file, src, &config)
            .expect("a bundled plugin reads a test fixture");
        let id = ExtractorId::from(plugin.id());
        let produced = output_from_ops(file, &id, declarations, &Vocabulary::default(), prefixes);
        out.ops.0.extend(produced.ops.0);
        out.relations.extend(produced.relations);
        out.names.extend(produced.names);
    }

    fn extract_rust(
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        prefixes: &SecretPrefixes,
    ) {
        declare_with(named("rust"), file, src, out, paths, prefixes)
    }

    fn extract_python(
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        prefixes: &SecretPrefixes,
    ) {
        declare_with(named("python"), file, src, out, paths, prefixes)
    }

    fn extract_ts(
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        _grammar: Grammar,
        prefixes: &SecretPrefixes,
    ) {
        declare_into(file, src, out, paths, prefixes)
    }

    fn extract_ecma(
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        _grammar: Grammar,
        _tag: &str,
        prefixes: &SecretPrefixes,
    ) {
        declare_into(file, src, out, paths, prefixes)
    }

    fn extract_csharp(file: &str, src: &[u8], out: &mut FileOutput, prefixes: &SecretPrefixes) {
        declare_with(
            named("csharp"),
            file,
            src,
            out,
            &PathKeys(HashSet::new()),
            prefixes,
        )
    }

    fn extract_go(file: &str, src: &[u8], out: &mut FileOutput, prefixes: &SecretPrefixes) {
        declare_with(
            named("go"),
            file,
            src,
            out,
            &PathKeys(HashSet::new()),
            prefixes,
        )
    }

    fn extract_declarations(
        _lang: Language,
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        paths: &PathKeys,
        prefixes: &SecretPrefixes,
    ) {
        declare_into(file, src, out, paths, prefixes)
    }

    fn extract_usages(
        _lang: Language,
        file: &str,
        src: &[u8],
        out: &mut FileOutput,
        table: &SymbolTable,
        ceiling: usize,
        rights: &[Rule],
    ) {
        let plugin = plugin_for(file);
        let mut session = plugin
            .session(Arc::new(HashSet::new()))
            .expect("a bundled plugin opens a session");
        let file_sites = FileSites::from_wire(
            session
                .usages(file, src)
                .expect("a bundled plugin reads a test fixture"),
        );
        let id = ExtractorId::from(plugin.id());
        flush_usages(
            file,
            &id,
            out,
            table,
            ceiling,
            &file_sites,
            rights,
            &ProjectUsings::default(),
            true,
            false,
        );
    }

    use crate::graph::{FactValue, Graph, NodeKind};

    /// `FileOutput::provenances_mut` must reach its `relations`, not only its
    /// `ops`.
    ///
    /// This test lives here rather than beside its siblings in `walk.rs` for a
    /// concrete reason: `FileOutput` and `TypeRelation` are private to this
    /// module, so `walk.rs` can only build a `GraphOps` — and a `GraphOps` has
    /// no relations. Deleting the `relations` line from `provenances_mut`
    /// therefore compiled and left the whole suite green, which is exactly the
    /// hole a code review found.
    ///
    /// Nothing downstream can catch it either. An un-canonicalized id is
    /// CONTENT-EQUAL to the canonical one and `ExtractorId` orders by content,
    /// so the corpus renders and hashes identically. Only reachability, checked
    /// here, can see it.
    #[test]
    fn file_output_provenances_reach_its_type_relations() {
        use crate::extract::walk::WalkOutput;
        use crate::graph::{FactKind, Provenance, Tier};

        let prov =
            |extractor: &str| Provenance::new("a.rs", Some(1), extractor.into(), Tier::Measured);

        let mut out = FileOutput::default();
        out.fact(
            NodeKey::new("a.rs"),
            Fact::new(FactKind::Loc, FactValue::Int(1), prov("treesitter")),
        );
        // The relation's provenance is the ONLY one naming this id, so a
        // `provenances_mut` that skips `relations` cannot report it.
        out.relations.push(TypeRelation {
            from: vec![NodeKey::new("sym:a")],
            to: vec![NodeKey::new("sym:b")],
            kind: RelationKind::Extends,
            prov: prov("only-on-the-relation"),
        });

        let reached: Vec<String> = out
            .provenances_mut()
            .into_iter()
            .map(|p| p.extractor.to_string())
            .collect();

        assert_eq!(
            reached.len(),
            2,
            "both the op's provenance and the relation's must be reachable, got {reached:?}"
        );
        assert!(
            reached.iter().any(|id| id == "only-on-the-relation"),
            "the TypeRelation's own provenance was not reached, so a cached \
             record's relation ids are never canonicalized or vetted: {reached:?}"
        );
    }

    /// A `languages-decl` record of a file without name references, blocks or
    /// declared node kinds — everything a 0.1 or 0.2 plugin can say — is
    /// written byte for byte as before contract 0.3 existed, so every warm
    /// cache stays valid and `CACHE_VERSION` did not have to move. The
    /// literal was captured from the binary before 0.3 was added.
    #[test]
    fn a_declaration_record_without_name_refs_keeps_its_bytes() {
        use crate::extract::walk::WalkOutput;
        use crate::graph::{FactKind, Provenance, Tier};

        let prov = || Provenance::new("a.cs", Some(3), "csharp".into(), Tier::Measured);
        let mut out = FileOutput::default();
        out.fact(
            NodeKey::new("a.cs"),
            Fact::new(FactKind::Loc, FactValue::Int(7), prov()),
        );
        out.symbol(NodeKey::new("sym:csharp:N#A"))
            .push_fact(Fact::new(
                FactKind::SymbolName,
                FactValue::Text("A".into()),
                prov(),
            ));
        out.relations.push(TypeRelation {
            from: vec![NodeKey::new("sym:csharp:N#A")],
            to: vec![NodeKey::new("sym:csharp:N#B")],
            kind: RelationKind::ByTargetKind,
            prov: prov(),
        });
        out.names
            .push(("a.cs".to_string(), NodeKey::new("sym:csharp:N#A"), (3, 14)));
        let wire = out.to_wire(&Vocabulary::default()).unwrap();
        assert_eq!(
            serde_json::to_string(&wire).unwrap(),
            concat!(
                r#"{"ops":[{"Fact":{"key":"a.cs","fact":{"kind":"loc","value":{"int":7},"#,
                r#""provenance":{"file":"a.cs","span":{"line":3},"extractor":"csharp","#,
                r#""tier":"measured","resolution":"heuristic","ambiguous_candidates":null},"#,
                r#""dissent":[]}}},{"Symbol":{"key":"sym:csharp:N#A","facts":[{"kind":"symbol_name","#,
                r#""value":{"text":"A"},"provenance":{"file":"a.cs","span":{"line":3},"#,
                r#""extractor":"csharp","tier":"measured","resolution":"heuristic","#,
                r#""ambiguous_candidates":null},"dissent":[]}],"edges":[]}}],"#,
                r#""relations":[{"from":["sym:csharp:N#A"],"to":["sym:csharp:N#B"],"#,
                r#""kind":"ByTargetKind","prov":{"file":"a.cs","span":{"line":3},"#,
                r#""extractor":"csharp","tier":"measured","resolution":"heuristic","#,
                r#""ambiguous_candidates":null}}],"names":[["a.cs","sym:csharp:N#A",[3,14]]]}"#,
            )
        );
    }

    /// The fields of contract 0.3 go through the cache and come back: a node
    /// kind by its name, the references and blocks as written. A record whose
    /// node kind this corpus no longer declares is refused whole, like one
    /// naming an unknown fact kind.
    #[test]
    fn a_declaration_record_with_name_refs_round_trips_through_the_cache_form() {
        use crate::extract::walk::WalkOutput;
        use crate::graph::{CardPolicy, NodeDeclaration, StructuralRole};

        let mut vocabulary = Vocabulary::default();
        let widget = vocabulary
            .declare_node(NodeDeclaration {
                namespace: "probe3".into(),
                name: "widget".into(),
                role: StructuralRole::Definition,
                card: CardPolicy::None,
            })
            .unwrap();
        let key = NodeKey::new("sym:probe3:w.probe3#Main");
        let mut out = FileOutput::default();
        out.symbol(key.clone());
        out.node_kinds.push((key.clone(), widget));
        out.symbol_blocks.push((key.clone(), 0));
        out.name_refs.push(NameRef {
            source: key.as_str().into(),
            name: "Other".into(),
            target_kind: "probe3:widget".into(),
            relation: Some("extends".into()),
            block: Some(0),
            span: Span::at(Some(2)),
            name_at: Some((2, 5)),
        });
        out.blocks.push(LookupBlock {
            lines: (1, 3),
            parent: None,
        });

        let json = serde_json::to_string(&out.to_wire(&vocabulary).unwrap()).unwrap();
        assert!(
            json.contains(r#""node_kinds":[["sym:probe3:w.probe3#Main","probe3:widget"]]"#),
            "{json}"
        );
        let wire: FileOutputWire = serde_json::from_str(&json).unwrap();
        assert_eq!(FileOutput::from_wire(wire.clone(), &vocabulary), Some(out));
        assert_eq!(FileOutput::from_wire(wire, &Vocabulary::default()), None);
    }

    /// Sorted keys of all Symbol nodes in the graph.
    fn symbol_keys(graph: &Graph) -> Vec<String> {
        let mut keys: Vec<String> = graph
            .nodes_sorted()
            .iter()
            .filter(|n| n.kind == NodeKind::Symbol)
            .map(|n| n.key.as_str().to_string())
            .collect();
        keys.sort();
        keys
    }

    /// The facts on a node, as (kind, value) pairs.
    fn facts_of<'a>(graph: &'a Graph, key: &str) -> Vec<(FactKind, &'a FactValue)> {
        graph
            .node(&NodeKey::new(key))
            .map(|n| n.facts.iter().map(|f| (f.kind, &f.value)).collect())
            .unwrap_or_default()
    }

    fn graph_with_file(path: &str) -> Graph {
        let mut g = Graph::new();
        g.ensure_node(NodeKey::new(path), NodeKind::File).unwrap();
        g
    }

    // The declaration walk the way the pipeline calls it (U0): freeze the
    // file/module keys the graph already holds, then walk. Going through the
    // snapshot is not ceremony — it is what lets a test that seeded its files
    // with `graph_with_file` see its own imports resolve, exactly as a file in a
    // real repository does after `filetree` has run.

    /// Walk one file and apply what it produced — the whole per-file pipeline,
    /// the way the driver runs it: snapshot, parse into a `FileOutput`, then
    /// `apply_file_output`. A test that called only the walk would assert
    /// against a graph nothing had been written to.
    fn decl_apply(
        graph: &mut Graph,
        walk: impl FnOnce(&mut FileOutput, &PathKeys),
    ) -> Vec<TypeRelation> {
        let paths = PathKeys::snapshot(graph);
        let mut out = FileOutput::default();
        walk(&mut out, &paths);
        let mut relations = Vec::new();
        apply_file_output(graph, out, &mut relations);
        relations
    }

    fn decl_rust(
        file: &str,
        src: &[u8],
        graph: &mut Graph,
        prefixes: &SecretPrefixes,
    ) -> Vec<TypeRelation> {
        decl_apply(graph, |out, paths| {
            extract_rust(file, src, out, paths, prefixes)
        })
    }

    fn decl_ts(
        file: &str,
        src: &[u8],
        graph: &mut Graph,
        language: Grammar,
        prefixes: &SecretPrefixes,
    ) -> Vec<TypeRelation> {
        decl_apply(graph, |out, paths| {
            extract_ts(file, src, out, paths, language, prefixes)
        })
    }

    fn decl_python(
        file: &str,
        src: &[u8],
        graph: &mut Graph,
        prefixes: &SecretPrefixes,
    ) -> Vec<TypeRelation> {
        decl_apply(graph, |out, paths| {
            extract_python(file, src, out, paths, prefixes)
        })
    }

    fn decl_csharp(
        file: &str,
        src: &[u8],
        graph: &mut Graph,
        prefixes: &SecretPrefixes,
    ) -> Vec<TypeRelation> {
        decl_apply(graph, |out, _| extract_csharp(file, src, out, prefixes))
    }

    fn decl_go(
        file: &str,
        src: &[u8],
        graph: &mut Graph,
        prefixes: &SecretPrefixes,
    ) -> Vec<TypeRelation> {
        decl_apply(graph, |out, _| extract_go(file, src, out, prefixes))
    }

    #[test]
    fn functions_and_types_become_symbols() {
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"fn main() {}\nstruct Foo;\nfn helper() {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            symbol_keys(&g),
            vec![
                "sym:rust:crate::a#Foo".to_string(),
                "sym:rust:crate::a#helper()".to_string(),
                "sym:rust:crate::a#main()".to_string(),
            ]
        );
    }

    #[test]
    fn main_at_file_scope_is_an_entry_point() {
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"fn main() {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let facts = facts_of(&g, "sym:rust:crate::a#main()");
        assert!(facts
            .iter()
            .any(|(k, v)| *k == FactKind::EntryPoint && **v == FactValue::Bool(true)));
    }

    #[test]
    fn a_method_named_main_is_not_an_entry_point() {
        // `main` only counts at file scope, not as a method on a type.
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"struct S;\nimpl S { fn main() {} }\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let facts = facts_of(&g, "sym:rust:crate::a#S.main()");
        assert!(!facts.iter().any(|(k, _)| *k == FactKind::EntryPoint));
    }

    #[test]
    fn same_name_methods_across_types_get_distinct_keys() {
        // The KTD-B1 case a bare `sym:<path>#name` would silently corrupt: two
        // `new` methods, identical arity, on different types in one file.
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"struct A;\nstruct B;\nimpl A { fn new() {} }\nimpl B { fn new() {} }\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(keys.contains(&"sym:rust:crate::a#A.new()".to_string()));
        assert!(keys.contains(&"sym:rust:crate::a#B.new()".to_string()));
        // Both methods survive as distinct nodes, not deduped into one.
        assert_eq!(keys.iter().filter(|k| k.ends_with(".new()")).count(), 2);
    }

    #[test]
    fn method_carries_its_container_as_a_fact() {
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"struct A;\nimpl A { fn run(&self) {} }\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let facts = facts_of(&g, "sym:rust:crate::a#A.run(&self)");
        assert!(facts.iter().any(
            |(k, v)| *k == FactKind::SymbolContainer && **v == FactValue::Text("A".to_string())
        ));
    }

    #[test]
    fn file_node_and_its_symbol_do_not_collide() {
        // Regression against the ensure_node kind-guard: a File node and a
        // Symbol node derived from the same file must not share a key.
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"fn f() {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            g.node(&NodeKey::new("src/a.rs")).unwrap().kind,
            NodeKind::File
        );
        assert_eq!(
            g.node(&NodeKey::new("sym:rust:crate::a#f()")).unwrap().kind,
            NodeKind::Symbol
        );
    }

    #[test]
    fn broken_source_yields_no_panic() {
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"fn ( { struct } impl",
            &mut g,
            &SecretPrefixes::default(),
        );
        // Whatever partial facts result, extraction must not panic.
    }

    // ---- TypeScript ----

    #[test]
    fn typescript_functions_classes_and_methods_become_symbols() {
        let mut g = graph_with_file("src/a.ts");
        decl_ts(
            "src/a.ts",
            b"export function greet() {}\nclass Cfg { run() {} }\ninterface I {}\ntype T = number;\nenum E { A }\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(keys.contains(&"sym:ts:src/a.ts#greet()".to_string()));
        assert!(keys.contains(&"sym:ts:src/a.ts#Cfg".to_string()));
        assert!(keys.contains(&"sym:ts:src/a.ts#Cfg.run()".to_string()));
        assert!(keys.contains(&"sym:ts:src/a.ts#I".to_string()));
        assert!(keys.contains(&"sym:ts:src/a.ts#T".to_string()));
        assert!(keys.contains(&"sym:ts:src/a.ts#E".to_string()));
        // The method carries its class as container.
        let facts = facts_of(&g, "sym:ts:src/a.ts#Cfg.run()");
        assert!(facts
            .iter()
            .any(|(k, v)| *k == FactKind::SymbolContainer && **v == FactValue::Text("Cfg".into())));
    }

    #[test]
    fn typescript_has_no_language_level_entry_point() {
        // Honest gap (plan): TS/JS entry is the manifest `main`/`bin` (U8), not
        // a language construct. Even a function named `main` is not an entry.
        let mut g = graph_with_file("src/a.ts");
        decl_ts(
            "src/a.ts",
            b"function main() {}\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert!(g
            .nodes_sorted()
            .iter()
            .filter(|n| n.kind == NodeKind::Symbol)
            .all(|n| !n.facts.iter().any(|f| f.kind == FactKind::EntryPoint)));
    }

    #[test]
    fn tsx_parses_and_emits_symbols() {
        let mut g = graph_with_file("src/a.tsx");
        decl_ts(
            "src/a.tsx",
            b"export function App() { return <div/>; }\n",
            &mut g,
            Grammar::Tsx,
            &SecretPrefixes::default(),
        );
        assert!(symbol_keys(&g).contains(&"sym:ts:src/a.tsx#App()".to_string()));
    }

    // ---- Python ----

    #[test]
    fn python_functions_classes_and_methods_become_symbols() {
        let mut g = graph_with_file("pkg/a.py");
        decl_python(
            "pkg/a.py",
            b"def top():\n    pass\nclass K:\n    def m(self):\n        pass\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(keys.contains(&"sym:python:pkg.a#top()".to_string()));
        assert!(keys.contains(&"sym:python:pkg.a#K".to_string()));
        assert!(keys.contains(&"sym:python:pkg.a#K.m(self)".to_string()));
    }

    #[test]
    fn python_dunder_main_guard_is_an_entry_point() {
        let mut g = graph_with_file("pkg/a.py");
        decl_python(
            "pkg/a.py",
            b"def run():\n    pass\nif __name__ == '__main__':\n    run()\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let facts = facts_of(&g, "sym:python:pkg.a#__main__");
        assert!(facts
            .iter()
            .any(|(k, v)| *k == FactKind::EntryPoint && **v == FactValue::Bool(true)));
    }

    // ---- C# ----

    #[test]
    fn csharp_types_and_methods_become_symbols_with_main_entry() {
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"namespace N {\n  public class C {\n    public static void Main(string[] a) {}\n    public int Run() { return 0; }\n  }\n  struct S {}\n  record R(int X);\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        // Namespace is transparent to the container (like a Rust `mod`).
        assert!(keys.contains(&"sym:csharp:N#C".to_string()));
        assert!(keys.contains(&"sym:csharp:N#C.Main(string[])".to_string()));
        assert!(keys.contains(&"sym:csharp:N#C.Run()".to_string()));
        assert!(keys.contains(&"sym:csharp:N#S".to_string()));
        assert!(keys.contains(&"sym:csharp:N#R".to_string()));
        // Main is the entry point.
        let facts = facts_of(&g, "sym:csharp:N#C.Main(string[])");
        assert!(facts
            .iter()
            .any(|(k, v)| *k == FactKind::EntryPoint && **v == FactValue::Bool(true)));
    }

    #[test]
    fn csharp_file_scoped_namespace_still_finds_types() {
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"namespace N;\npublic class C { public void M() {} }\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(keys.contains(&"sym:csharp:N#C".to_string()));
        assert!(keys.contains(&"sym:csharp:N#C.M()".to_string()));
    }

    #[test]
    fn csharp_top_level_program_gets_a_synthetic_entry_point() {
        // A modern C# 9 Program.cs with only top-level statements and no `Main`.
        let mut g = graph_with_file("Program.cs");
        decl_csharp(
            "Program.cs",
            b"Console.WriteLine(\"hi\");\nint x = 1;\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let facts = facts_of(&g, "sym:csharp:Program#__top_level__");
        assert!(
            facts
                .iter()
                .any(|(k, v)| *k == FactKind::EntryPoint && **v == FactValue::Bool(true)),
            "top-level statements must yield a synthetic entry point"
        );
    }

    #[test]
    fn csharp_instance_method_named_main_is_not_an_entry_point() {
        // A non-static method named `Main` is not the program entry point.
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"class C { public void Main() {} }\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let facts = facts_of(&g, "sym:csharp:A#C.Main()");
        assert!(!facts.iter().any(|(k, _)| *k == FactKind::EntryPoint));
        // And no synthetic top-level entry either (this file has a class body).
        assert!(g
            .node(&NodeKey::new("sym:csharp:A#__top_level__"))
            .is_none());
    }

    // ---- Go ----

    #[test]
    fn go_functions_types_and_methods_become_symbols_with_main_entry() {
        let mut g = graph_with_file("main.go");
        decl_go(
            "main.go",
            b"package main\nfunc main() {}\nfunc Helper() {}\ntype T struct {}\nfunc (t T) Method() {}\ntype I interface {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(keys.contains(&"sym:go:.#main()".to_string()));
        assert!(keys.contains(&"sym:go:.#Helper()".to_string()));
        assert!(keys.contains(&"sym:go:.#T".to_string()));
        assert!(keys.contains(&"sym:go:.#T.Method()".to_string()));
        assert!(keys.contains(&"sym:go:.#I".to_string()));
        // Top-level func main is the entry point.
        let facts = facts_of(&g, "sym:go:.#main()");
        assert!(facts
            .iter()
            .any(|(k, v)| *k == FactKind::EntryPoint && **v == FactValue::Bool(true)));
        // The method carries its receiver type as container.
        let m = facts_of(&g, "sym:go:.#T.Method()");
        assert!(m
            .iter()
            .any(|(k, v)| *k == FactKind::SymbolContainer && **v == FactValue::Text("T".into())));
    }

    // ---- degradation ----

    #[test]
    fn broken_sources_in_each_new_language_do_not_panic() {
        // Each broken file goes to its own language's plugin: a trap inside the
        // component would surface here as a failed call, and a walker that
        // panicked on malformed input would trap. Both must not happen.
        let mut g = graph_with_files(&["x.ts", "x.py", "x.cs", "x.go"]);
        decl_ts(
            "x.ts",
            b"export function ( { class",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        decl_python(
            "x.py",
            b"def (:\n  class ??",
            &mut g,
            &SecretPrefixes::default(),
        );
        decl_csharp(
            "x.cs",
            b"namespace { class ( void",
            &mut g,
            &SecretPrefixes::default(),
        );
        decl_go(
            "x.go",
            b"package func ( type struct {",
            &mut g,
            &SecretPrefixes::default(),
        );
        // No assertion beyond "did not panic" — the `expect` in the plugin
        // route is the assertion that no component trapped.
    }

    // ---- U2: members, signatures, visibility, doc comments ----

    /// Sorted `type --HasMember--> member` pairs.
    fn member_edges(graph: &Graph) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = graph
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == EdgeKind::HasMember)
            .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
            .collect();
        pairs.sort();
        pairs
    }

    /// The text value of one fact kind on a node, or `None` when the node does not
    /// carry that kind at all — the distinction the "no empty fact" rule needs.
    fn text_fact(graph: &Graph, key: &str, kind: FactKind) -> Option<String> {
        graph
            .node(&NodeKey::new(key))?
            .facts
            .iter()
            .find(|f| f.kind == kind)
            .and_then(|f| match &f.value {
                FactValue::Text(t) => Some(t.clone()),
                _ => None,
            })
    }

    /// The `(line, end_line)` provenance range of one fact kind on a node.
    fn fact_range(graph: &Graph, key: &str, kind: FactKind) -> Option<(Option<u32>, Option<u32>)> {
        graph
            .node(&NodeKey::new(key))?
            .facts
            .iter()
            .find(|f| f.kind == kind)
            .map(|f| (f.provenance.span.start(), f.provenance.span.range_end()))
    }

    #[test]
    fn a_class_with_two_methods_and_a_property_yields_three_members() {
        // KTD5: members are their own nodes joined by HasMember, because a call or
        // a reference leaves the METHOD, not the type.
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"namespace N;\npublic class C {\n  public int Add(int a, int b) { return a + b; }\n  public void Reset() {\n    Count = 0;\n  }\n  public int Count { get; set; }\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );

        assert_eq!(
            member_edges(&g),
            vec![
                (
                    "sym:csharp:N#C".to_string(),
                    "sym:csharp:N#C.Add(int,int)".to_string()
                ),
                (
                    "sym:csharp:N#C".to_string(),
                    "sym:csharp:N#C.Count".to_string()
                ),
                (
                    "sym:csharp:N#C".to_string(),
                    "sym:csharp:N#C.Reset()".to_string()
                ),
            ]
        );

        // Each member reports its own declaration line range (R1). `Reset` spans
        // three lines, so an end that merely echoed the start would be wrong.
        assert_eq!(
            fact_range(&g, "sym:csharp:N#C.Reset()", FactKind::SymbolKind),
            Some((Some(4), Some(6)))
        );
        // …and `Count` fits on one, which is a `Span::Line` and therefore has no
        // range end to report. A one-line declaration and a point anchored at
        // that line are the same place, and the model gives one place one
        // spelling (see `Span`).
        assert_eq!(
            fact_range(&g, "sym:csharp:N#C.Count", FactKind::SymbolKind),
            Some((Some(7), None))
        );
        // Kinds are distinguished, so a card can list methods apart from state.
        assert_eq!(
            text_fact(&g, "sym:csharp:N#C.Count", FactKind::SymbolKind).as_deref(),
            Some("property")
        );
        // The signature is the declaration header, never the body.
        assert_eq!(
            text_fact(&g, "sym:csharp:N#C.Add(int,int)", FactKind::SymbolSignature).as_deref(),
            Some("public int Add(int a, int b)")
        );
        assert_eq!(
            text_fact(&g, "sym:csharp:N#C.Count", FactKind::SymbolSignature).as_deref(),
            Some("public int Count")
        );
        // And the visibility the source states, verbatim.
        assert_eq!(
            text_fact(
                &g,
                "sym:csharp:N#C.Add(int,int)",
                FactKind::SymbolVisibility
            )
            .as_deref(),
            Some("public")
        );
    }

    #[test]
    fn overloaded_methods_get_distinct_identities() {
        // KD8/KTD3: the corpus unit is the type, so its overloads must stay two
        // members. Before signatures entered the identity these collided and the
        // second one took a byte-offset suffix, which also moved on reformatting.
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"namespace N;\npublic class C {\n  public void Run(int a) {}\n  public void Run(string a) {}\n  public void Run(int a, string b) {}\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(
            keys.contains(&"sym:csharp:N#C.Run(int)".to_string()),
            "{keys:?}"
        );
        assert!(
            keys.contains(&"sym:csharp:N#C.Run(string)".to_string()),
            "{keys:?}"
        );
        assert!(
            keys.contains(&"sym:csharp:N#C.Run(int,string)".to_string()),
            "{keys:?}"
        );
        // Three distinct nodes, not one merged node and not an ordinal fallback.
        assert_eq!(keys.iter().filter(|k| k.contains("#C.Run")).count(), 3);
        assert!(keys.iter().all(|k| !k.contains('~')), "{keys:?}");
        // Every overload is a member of the one type.
        assert_eq!(member_edges(&g).len(), 3);
    }

    #[test]
    fn overload_identities_survive_reformatting() {
        // KTD3 requires identity stable against reformatting; the Phase-B
        // `@<byte-offset>` disambiguator would have moved with every blank line.
        let tight = b"namespace N;\npublic class C {\n  public void Run(int a) {}\n  public void Run(string a) {}\n}\n";
        let loose = b"namespace N;\n\n\npublic class C\n{\n\n    public void Run( int   a ) {}\n\n    public void Run(\n        string a\n    ) {}\n}\n";
        let keys_of = |src: &[u8]| {
            let mut g = graph_with_file("A.cs");
            decl_csharp("A.cs", src, &mut g, &SecretPrefixes::default());
            symbol_keys(&g)
        };
        assert_eq!(keys_of(tight), keys_of(loose));
    }

    #[test]
    fn a_types_identity_survives_moving_it_to_another_file() {
        // KTD3: identity is rooted at the language's MODULE path — a C# namespace
        // — not at the file, so moving the type does not rename its card.
        let in_a = {
            let mut g = graph_with_file("Old.cs");
            decl_csharp(
                "Old.cs",
                b"namespace N.Sub;\npublic class C { public void M() {} }\n",
                &mut g,
                &SecretPrefixes::default(),
            );
            symbol_keys(&g)
        };
        let in_b = {
            let mut g = graph_with_file("New/Renamed.cs");
            decl_csharp(
                "New/Renamed.cs",
                b"namespace N.Sub;\npublic class C { public void M() {} }\n",
                &mut g,
                &SecretPrefixes::default(),
            );
            symbol_keys(&g)
        };
        assert_eq!(in_a, in_b);
        assert!(
            in_a.contains(&"sym:csharp:N.Sub#C.M()".to_string()),
            "{in_a:?}"
        );
    }

    #[test]
    fn a_nested_type_gets_its_own_node_and_reports_its_enclosing_type() {
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"namespace N;\npublic class Outer {\n  public class Inner {\n    public void M() {}\n  }\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        let keys = symbol_keys(&g);
        assert!(
            keys.contains(&"sym:csharp:N#Outer.Inner".to_string()),
            "{keys:?}"
        );
        // Its own member hangs off the nested type, not off the outer one.
        assert!(
            keys.contains(&"sym:csharp:N#Outer.Inner.M()".to_string()),
            "{keys:?}"
        );
        // The container fact names the IMMEDIATE enclosing type, not the chain.
        assert_eq!(
            text_fact(&g, "sym:csharp:N#Outer.Inner", FactKind::SymbolContainer).as_deref(),
            Some("Outer")
        );
        assert_eq!(
            text_fact(
                &g,
                "sym:csharp:N#Outer.Inner.M()",
                FactKind::SymbolContainer
            )
            .as_deref(),
            Some("Inner")
        );
        assert_eq!(
            member_edges(&g),
            vec![
                (
                    "sym:csharp:N#Outer".to_string(),
                    "sym:csharp:N#Outer.Inner".to_string()
                ),
                (
                    "sym:csharp:N#Outer.Inner".to_string(),
                    "sym:csharp:N#Outer.Inner.M()".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_file_that_declares_nothing_yields_no_symbol_node() {
        // No empty symbol, no placeholder — the File node filetree made stands
        // alone.
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            b"// only a comment\nconst X: u32 = 1;\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(g.nodes_sorted().iter().all(|n| n.kind != NodeKind::Symbol));
        assert_eq!(g.node_count(), 1);
    }

    fn decl_js(file: &str, src: &[u8], graph: &mut Graph) -> Vec<TypeRelation> {
        decl_apply(graph, |out, paths| {
            extract_ecma(
                file,
                src,
                out,
                paths,
                Grammar::JavaScript,
                "js",
                &SecretPrefixes::default(),
            )
        })
    }

    #[test]
    fn javascript_classes_fields_arrows_and_require_are_extracted() {
        let mut g = graph_with_files(&["src/a.js", "src/b.js"]);
        let rels = decl_js(
            "src/a.js",
            b"const b = require('./b');\n\n/** Keeps a queue. */\nclass Queue extends Base {\n  size = 0;\n  push(x) { return x; }\n}\n\n/** Adds two. */\nexport const add = (a, b) => a + b;\nconst one = x => x;\nconst plain = 42;\n",
            &mut g,
        );
        assert!(g.node(&NodeKey::new("sym:js:src/a.js#Queue")).is_some());
        assert!(g
            .node(&NodeKey::new("sym:js:src/a.js#Queue.size"))
            .is_some());
        assert!(g
            .node(&NodeKey::new("sym:js:src/a.js#Queue.push(x)"))
            .is_some());
        assert!(g.node(&NodeKey::new("sym:js:src/a.js#add(a,b)")).is_some());
        assert!(g.node(&NodeKey::new("sym:js:src/a.js#one(x)")).is_some());
        assert!(
            g.node(&NodeKey::new("sym:js:src/a.js#plain")).is_none(),
            "a const that holds no function is not a declaration"
        );
        assert_eq!(
            text_fact(&g, "sym:js:src/a.js#add(a,b)", FactKind::SymbolSignature).as_deref(),
            Some("const add = (a, b) =>"),
            "the signature stops where the arrow body begins; `export` is the visibility, as for a TS `export function`"
        );
        assert_eq!(
            text_fact(&g, "sym:js:src/a.js#add(a,b)", FactKind::DocComment).as_deref(),
            Some("Adds two.")
        );
        assert_eq!(
            text_fact(&g, "sym:js:src/a.js#add(a,b)", FactKind::SymbolVisibility).as_deref(),
            Some("export")
        );
        assert!(
            has_import_edge(&g, "src/a.js", "src/b.js"),
            "a CommonJS require of a relative path is an import"
        );
        assert!(
            rels.iter()
                .any(|r| r.to.iter().any(|k| k.as_str().ends_with("#Base"))),
            "a bare JavaScript class_heritage names its base"
        );
    }

    #[test]
    fn a_typescript_arrow_function_is_a_declaration() {
        let mut g = graph_with_file("src/a.ts");
        decl_ts(
            "src/a.ts",
            b"export const parse = (text: string): number => text.length;\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            text_fact(&g, "sym:ts:src/a.ts#parse(string)", FactKind::SymbolKind).as_deref(),
            Some("function")
        );
    }

    #[test]
    fn a_doc_comment_is_carried_verbatim_with_its_diacritics() {
        // R10: quoted in whatever language it is written in — never translated,
        // never reflowed, never stripped of diacritics.
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            "namespace N;\npublic class C {\n  /// Vypočítá součet dvou čísel.\n  /// Záporné hodnoty vrací chybu.\n  public int Add(int a, int b) { return a + b; }\n}\n".as_bytes(),
            &mut g,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            text_fact(&g, "sym:csharp:N#C.Add(int,int)", FactKind::DocComment).as_deref(),
            Some("Vypočítá součet dvou čísel.\nZáporné hodnoty vrací chybu.")
        );
        // The doc fact is anchored to the COMMENT's lines, not the declaration's.
        assert_eq!(
            fact_range(&g, "sym:csharp:N#C.Add(int,int)", FactKind::DocComment),
            Some((Some(3), Some(4)))
        );
    }

    #[test]
    fn a_member_without_a_doc_comment_carries_no_doc_fact() {
        let mut g = graph_with_file("A.cs");
        decl_csharp(
            "A.cs",
            b"namespace N;\npublic class C {\n  // an ordinary comment, not a doc comment\n  public void Bare() {}\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(
            text_fact(&g, "sym:csharp:N#C.Bare()", FactKind::DocComment).is_none(),
            "an absent doc comment must be no fact at all, never an empty one"
        );
        // The node itself exists — only the doc fact is missing.
        assert!(g.node(&NodeKey::new("sym:csharp:N#C.Bare()")).is_some());
    }

    #[test]
    fn rust_doc_comments_attach_outward_and_the_file_doc_lands_on_the_file() {
        let mut g = graph_with_file("src/a.rs");
        decl_rust(
            "src/a.rs",
            "//! Modul počítá slevy.\n\n/// Přehled struktury.\n#[derive(Debug)]\npub struct Cfg {\n    pub rate: u32,\n}\n".as_bytes(),
            &mut g,
            &SecretPrefixes::default(),
        );
        // `//!` documents the module, so it lands on the File node — attaching it
        // to the struct below would misattribute it.
        assert_eq!(
            text_fact(&g, "src/a.rs", FactKind::DocComment).as_deref(),
            Some("Modul počítá slevy.")
        );
        // `///` documents the item below, across the intervening attribute.
        assert_eq!(
            text_fact(&g, "sym:rust:crate::a#Cfg", FactKind::DocComment).as_deref(),
            Some("Přehled struktury.")
        );
        assert_eq!(
            text_fact(&g, "sym:rust:crate::a#Cfg", FactKind::SymbolVisibility).as_deref(),
            Some("pub")
        );
        // A struct's fields are its members (KTD5).
        assert_eq!(
            member_edges(&g),
            vec![(
                "sym:rust:crate::a#Cfg".to_string(),
                "sym:rust:crate::a#Cfg.rate".to_string()
            )]
        );
    }

    #[test]
    fn typescript_and_python_members_carry_docs_signatures_and_owners() {
        let mut g = graph_with_file("src/a.ts");
        decl_ts(
            "src/a.ts",
            "/** Konfigurace běhu. */\nexport class Cfg {\n  private retries: number = 3;\n  public run(a: number, b?: string): void {}\n}\n".as_bytes(),
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            text_fact(&g, "sym:ts:src/a.ts#Cfg", FactKind::DocComment).as_deref(),
            Some("Konfigurace běhu.")
        );
        // `export` is the visibility TypeScript states at module scope.
        assert_eq!(
            text_fact(&g, "sym:ts:src/a.ts#Cfg", FactKind::SymbolVisibility).as_deref(),
            Some("export")
        );
        assert_eq!(
            text_fact(
                &g,
                "sym:ts:src/a.ts#Cfg.retries",
                FactKind::SymbolVisibility
            )
            .as_deref(),
            Some("private")
        );
        assert_eq!(
            member_edges(&g),
            vec![
                (
                    "sym:ts:src/a.ts#Cfg".to_string(),
                    "sym:ts:src/a.ts#Cfg.retries".to_string()
                ),
                (
                    "sym:ts:src/a.ts#Cfg".to_string(),
                    "sym:ts:src/a.ts#Cfg.run(number,string)".to_string()
                ),
            ]
        );

        let mut g = graph_with_file("pkg/a.py");
        decl_python(
            "pkg/a.py",
            "class K:\n    \"\"\"Dokumentace třídy.\"\"\"\n    def m(self, x):\n        \"\"\"Dokumentace metody.\"\"\"\n        pass\n".as_bytes(),
            &mut g,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            text_fact(&g, "sym:python:pkg.a#K", FactKind::DocComment).as_deref(),
            Some("Dokumentace třídy.")
        );
        assert_eq!(
            text_fact(&g, "sym:python:pkg.a#K.m(self,x)", FactKind::DocComment).as_deref(),
            Some("Dokumentace metody.")
        );
        // Python states no access modifier — the underscore convention is
        // interpretation, so no visibility fact is invented.
        assert!(text_fact(
            &g,
            "sym:python:pkg.a#K.m(self,x)",
            FactKind::SymbolVisibility
        )
        .is_none());
        assert_eq!(
            member_edges(&g),
            vec![(
                "sym:python:pkg.a#K".to_string(),
                "sym:python:pkg.a#K.m(self,x)".to_string()
            )]
        );
    }

    #[test]
    fn go_struct_fields_and_methods_are_members_of_their_type() {
        let mut g = graph_with_file("cmd/main.go");
        decl_go(
            "cmd/main.go",
            b"package main\nfunc (t T) Method(a int) bool { return true }\ntype T struct {\n\tName string\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        // The method is declared ABOVE its type and still finds its owner.
        assert_eq!(
            member_edges(&g),
            vec![
                (
                    "sym:go:cmd#T".to_string(),
                    "sym:go:cmd#T.Method(int)".to_string()
                ),
                ("sym:go:cmd#T".to_string(), "sym:go:cmd#T.Name".to_string()),
            ]
        );
        // Go states no access modifier; capitalization is a convention, not a fact.
        assert!(text_fact(&g, "sym:go:cmd#T.Method(int)", FactKind::SymbolVisibility).is_none());
    }

    // ---- Imports ----

    /// A graph pre-seeded with File nodes (as filetree would create them).
    fn graph_with_files(paths: &[&str]) -> Graph {
        let mut g = Graph::new();
        for p in paths {
            g.ensure_node(NodeKey::new(*p), NodeKind::File).unwrap();
        }
        g
    }

    fn has_import_edge(g: &Graph, from: &str, to: &str) -> bool {
        g.edges_sorted().iter().any(|e| {
            e.from.as_str() == from && e.to.as_str() == to && e.edge.kind == EdgeKind::Imports
        })
    }

    // ---- The snapshot import resolution reads (U0) ----

    #[test]
    fn an_import_resolves_against_the_snapshot_not_against_the_live_graph() {
        // The whole point of U0: the walk answers "does this file exist?" from a
        // frozen set, so it cannot depend on how much of the graph is built when
        // it runs. Demonstrated by disagreeing with the graph on purpose — the
        // target file IS a node, and the snapshot handed to the walk says it is
        // not. No edge. A resolver still reading the graph would emit one.
        let mut g = graph_with_files(&["src/a.ts", "src/b.ts"]);
        let empty = PathKeys(HashSet::new());
        let mut out = FileOutput::default();
        extract_ts(
            "src/a.ts",
            b"import { thing } from './b';\n",
            &mut out,
            &empty,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        apply_file_output(&mut g, out, &mut Vec::new());
        assert!(
            !has_import_edge(&g, "src/a.ts", "src/b.ts"),
            "the resolver consulted the graph instead of the snapshot it was given"
        );
    }

    #[test]
    fn the_snapshot_holds_files_and_modules_and_nothing_else() {
        // Restricted to what `filetree` makes, because that is the only thing a
        // path-shaped candidate can name (KTD5). Every other key-space carries a
        // prefix, so nothing is lost by the restriction — and a later extractor
        // adding nodes cannot change an import's answer.
        let mut g = Graph::new();
        g.ensure_node(NodeKey::new("src"), NodeKind::Module)
            .unwrap();
        g.ensure_node(NodeKey::new("src/a.rs"), NodeKind::File)
            .unwrap();
        g.ensure_node(NodeKey::new("sym:rust:crate::a#Foo"), NodeKind::Symbol)
            .unwrap();
        g.ensure_node(NodeKey::new("dep:cargo:serde"), NodeKind::Dependency)
            .unwrap();
        let keys = PathKeys::snapshot(&g);
        // This set is what every plugin's `path-exists` answers from.
        assert!(keys.0.contains("src/a.rs"));
        assert!(keys.0.contains("src"));
        assert!(!keys.0.contains("sym:rust:crate::a#Foo"));
        assert!(!keys.0.contains("dep:cargo:serde"));
        assert_eq!(keys.0.len(), 2);
    }

    #[test]
    fn typescript_internal_import_creates_an_imports_edge() {
        let mut g = graph_with_files(&["src/a.ts", "src/b.ts"]);
        decl_ts(
            "src/a.ts",
            b"import { thing } from './b';\nimport React from 'react';\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert!(has_import_edge(&g, "src/a.ts", "src/b.ts"));
        // The external npm package must NOT become a phantom node or an edge.
        assert!(g.node(&NodeKey::new("react")).is_none());
        assert_eq!(
            g.edges_sorted()
                .iter()
                .filter(|e| e.edge.kind == EdgeKind::Imports)
                .count(),
            1
        );
    }

    #[test]
    fn python_relative_import_creates_an_imports_edge() {
        let mut g = graph_with_files(&["pkg/a.py", "pkg/b.py"]);
        decl_python(
            "pkg/a.py",
            b"from .b import thing\nimport os\nfrom other import x\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(has_import_edge(&g, "pkg/a.py", "pkg/b.py"));
        // Absolute/stdlib imports create no phantom node and no edge.
        assert!(g.node(&NodeKey::new("os")).is_none());
        assert!(g.node(&NodeKey::new("other")).is_none());
        assert_eq!(
            g.edges_sorted()
                .iter()
                .filter(|e| e.edge.kind == EdgeKind::Imports)
                .count(),
            1
        );
    }

    #[test]
    fn rust_crate_use_creates_an_imports_edge_but_std_does_not() {
        let mut g = graph_with_files(&["src/a.rs", "src/b.rs"]);
        decl_rust(
            "src/a.rs",
            b"use crate::b::Thing;\nuse std::collections::HashSet;\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(has_import_edge(&g, "src/a.rs", "src/b.rs"));
        assert!(g.node(&NodeKey::new("std")).is_none());
    }

    #[test]
    fn rust_crate_use_anchors_at_the_files_own_crate_src_in_a_nested_layout() {
        // `crates/foo/src/a.rs` with `use crate::b::Thing;` must resolve to
        // `crates/foo/src/b.rs`, NOT `crates/b.rs` (anchoring at the first path
        // component `crates/`, the bug this fixes).
        let mut g = graph_with_files(&["crates/foo/src/a.rs", "crates/foo/src/b.rs"]);
        decl_rust(
            "crates/foo/src/a.rs",
            b"use crate::b::Thing;\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(has_import_edge(
            &g,
            "crates/foo/src/a.rs",
            "crates/foo/src/b.rs"
        ));
    }

    #[test]
    fn rust_crate_use_with_no_src_ancestor_skips_the_edge() {
        // A file under no `src/` directory: the crate root can't be determined
        // confidently, so the edge is skipped (fail closed), never mis-resolved.
        let mut g = graph_with_files(&["a.rs", "b.rs"]);
        decl_rust(
            "a.rs",
            b"use crate::b::Thing;\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(g
            .edges_sorted()
            .iter()
            .all(|e| e.edge.kind != EdgeKind::Imports));
    }

    #[test]
    fn rust_self_use_resolves_against_the_files_own_module_dir() {
        // `self::sub` in `src/a.rs` targets a submodule of `a`, which lives in
        // `src/a/` — so it resolves to `src/a/sub.rs`, not a sibling `src/sub`.
        let mut g = graph_with_files(&["src/a.rs", "src/a/sub.rs"]);
        decl_rust(
            "src/a.rs",
            b"use self::sub::Thing;\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(has_import_edge(&g, "src/a.rs", "src/a/sub.rs"));
    }

    #[test]
    fn rust_self_use_inside_an_inline_mod_fails_closed() {
        // `self::sibling` inside `mod inner { ... }` is relative to `inner`, not
        // to `a`'s module dir — the resolver does not track the inline anchor, so
        // it must emit NO edge rather than a wrong-but-existing one to
        // `src/a/sibling.rs` (which belongs to `a`, not to `a::inner`).
        let mut g = graph_with_files(&["src/a.rs", "src/a/sibling.rs"]);
        decl_rust(
            "src/a.rs",
            b"mod inner {\n    use self::sibling::Thing;\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        assert!(!has_import_edge(&g, "src/a.rs", "src/a/sibling.rs"));
    }

    #[test]
    fn repeated_import_of_the_same_module_is_a_single_edge() {
        let mut g = graph_with_files(&["src/a.ts", "src/b.ts"]);
        decl_ts(
            "src/a.ts",
            b"import { one } from './b';\nimport { two } from './b';\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert_eq!(
            g.edges_sorted()
                .iter()
                .filter(|e| e.edge.kind == EdgeKind::Imports)
                .count(),
            1
        );
    }

    #[test]
    fn a_file_importing_itself_creates_no_self_loop() {
        let mut g = graph_with_files(&["web/index.ts"]);
        decl_ts(
            "web/index.ts",
            b"import { x } from './index';\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert!(g
            .edges_sorted()
            .iter()
            .all(|e| e.edge.kind != EdgeKind::Imports));
    }

    #[test]
    fn import_to_an_untracked_file_is_skipped_no_phantom() {
        // './missing' is not in the graph → no edge, no fabricated node.
        let mut g = graph_with_files(&["src/a.ts"]);
        decl_ts(
            "src/a.ts",
            b"import { x } from './missing';\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        assert!(g.node(&NodeKey::new("src/missing")).is_none());
        assert!(g.node(&NodeKey::new("src/missing.ts")).is_none());
        assert!(g
            .edges_sorted()
            .iter()
            .all(|e| e.edge.kind != EdgeKind::Imports));
    }

    // ---- U3: type relations (extends / implements) ----

    /// Sorted `from -> to` pairs of one relation kind.
    fn relation_edges(graph: &Graph, kind: EdgeKind) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = graph
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == kind)
            .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
            .collect();
        pairs.sort();
        pairs
    }

    /// Every `Extends`/`Implements` pair, whatever the kind.
    fn all_relations(graph: &Graph) -> Vec<(String, String)> {
        let mut all = relation_edges(graph, EdgeKind::Extends);
        all.extend(relation_edges(graph, EdgeKind::Implements));
        all.sort();
        all
    }

    /// A generic `impl<T> Trait<T> for Type` names the same trait as a bare one:
    /// generic ARGUMENTS never belong to a type's identity.
    #[test]
    fn rust_generic_trait_impl_resolves_to_the_bare_trait() {
        let mut g = graph_with_file("src/a.rs");
        let rels = decl_rust(
            "src/a.rs",
            b"pub trait Shape<T> {}\npub struct Sq;\nimpl<T> Shape<T> for Sq {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(
            relation_edges(&g, EdgeKind::Implements),
            vec![(
                "sym:rust:crate::a#Sq".to_string(),
                "sym:rust:crate::a#Shape".to_string()
            )]
        );
    }

    /// `impl Trait for &S` implements the trait for a REFERENCE, which is not a
    /// type this repo declares. A relation is skipped rather than pinned on `S`.
    #[test]
    fn rust_impl_for_a_reference_type_names_no_declared_type() {
        let mut g = graph_with_file("src/a.rs");
        let rels = decl_rust(
            "src/a.rs",
            b"pub trait Shape {}\npub struct Sq;\nimpl Shape for &Sq {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert!(all_relations(&g).is_empty());
    }

    /// Two `#[cfg]`-gated impls of one trait are one relation, not two — the same
    /// dedup discipline the imports walk applies per target.
    #[test]
    fn two_cfg_gated_impls_of_one_trait_are_one_edge() {
        let mut g = graph_with_file("src/a.rs");
        let rels = decl_rust(
            "src/a.rs",
            b"pub trait Shape {}\npub struct Sq;\n#[cfg(unix)]\nimpl Shape for Sq {}\n#[cfg(windows)]\nimpl Shape for Sq {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(relation_edges(&g, EdgeKind::Implements).len(), 1);
    }

    /// A C# base spelled with its namespace resolves against THAT namespace, not
    /// against the deriving type's — the second candidate `base_candidates` offers.
    #[test]
    fn csharp_namespace_qualified_base_resolves_across_namespaces() {
        let mut g = graph_with_file("A.cs");
        let rels = decl_csharp(
            "A.cs",
            b"namespace A.B {\n  public class Base {}\n}\nnamespace C {\n  public class D : A.B.Base {}\n}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(
            relation_edges(&g, EdgeKind::Extends),
            vec![(
                "sym:csharp:C#D".to_string(),
                "sym:csharp:A.B#Base".to_string()
            )]
        );
    }

    /// A nested type named `Outer.Inner` is a type path inside the declaring
    /// module, not a namespace-qualified name — the FIRST candidate, so the more
    /// local reading wins.
    #[test]
    fn csharp_nested_type_base_resolves_within_the_declaring_module() {
        let mut g = graph_with_file("A.cs");
        let rels = decl_csharp(
            "A.cs",
            b"namespace N;\npublic class Outer {\n  public class Inner {}\n}\npublic class D : Outer.Inner {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(
            relation_edges(&g, EdgeKind::Extends),
            vec![(
                "sym:csharp:N#D".to_string(),
                "sym:csharp:N#Outer.Inner".to_string()
            )]
        );
    }

    /// A generic base (`class D : Base<int>`) is the same base as a bare one.
    #[test]
    fn csharp_generic_base_drops_its_type_arguments() {
        let mut g = graph_with_file("A.cs");
        let rels = decl_csharp(
            "A.cs",
            b"namespace N;\npublic class Base<T> {}\npublic class D : Base<int> {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(
            relation_edges(&g, EdgeKind::Extends),
            vec![(
                "sym:csharp:N#D".to_string(),
                "sym:csharp:N#Base".to_string()
            )]
        );
    }

    /// A struct can only implement interfaces, and the kind still comes from the
    /// target's `SymbolKind`, never from the `I` prefix in its name.
    #[test]
    fn csharp_struct_base_list_entry_is_an_implements() {
        let mut g = graph_with_file("A.cs");
        let rels = decl_csharp(
            "A.cs",
            b"namespace N;\npublic interface Marker {}\npublic struct S : Marker {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        // `Marker` carries no `I` prefix and is still recognized as an interface.
        assert_eq!(
            relation_edges(&g, EdgeKind::Implements),
            vec![(
                "sym:csharp:N#S".to_string(),
                "sym:csharp:N#Marker".to_string()
            )]
        );
        assert!(relation_edges(&g, EdgeKind::Extends).is_empty());
    }

    /// A TS mixin (`extends mix(Base)`) names a CALL, not a type, so nothing is
    /// resolved — least of all the class passed to the factory.
    #[test]
    fn typescript_mixin_base_is_not_a_relation() {
        let mut g = graph_with_file("a.ts");
        let rels = decl_ts(
            "a.ts",
            b"export class Base {}\nfunction mix(b: unknown) { return b; }\nexport class D extends mix(Base) {}\n",
            &mut g,
            Grammar::TypeScript,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert!(all_relations(&g).is_empty());
    }

    /// TSX runs the same walker, so heritage is extracted there too.
    #[test]
    fn tsx_heritage_is_extracted_like_typescript() {
        let mut g = graph_with_file("a.tsx");
        let rels = decl_ts(
            "a.tsx",
            b"export class Base {}\nexport class D extends Base {}\n",
            &mut g,
            Grammar::Tsx,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(
            relation_edges(&g, EdgeKind::Extends),
            vec![(
                "sym:ts:a.tsx#D".to_string(),
                "sym:ts:a.tsx#Base".to_string()
            )]
        );
    }

    /// `class D(Base, metaclass=Meta)`: the keyword argument is not a base class.
    #[test]
    fn python_metaclass_keyword_is_not_a_base_class() {
        let mut g = graph_with_file("a.py");
        let rels = decl_python(
            "a.py",
            b"class Meta(type):\n    pass\n\n\nclass Base:\n    pass\n\n\nclass D(Base, metaclass=Meta):\n    pass\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert_eq!(
            relation_edges(&g, EdgeKind::Extends)
                .into_iter()
                .filter(|(from, _)| from == "sym:python:a#D")
                .collect::<Vec<_>>(),
            vec![(
                "sym:python:a#D".to_string(),
                "sym:python:a#Base".to_string()
            )]
        );
    }

    /// A base type declared in a module the spelling cannot name goes unresolved —
    /// and the deriving type still carries `: Base` verbatim in its signature, so
    /// the reader is never left unable to tell "unresolved" from "no base" (R20).
    #[test]
    fn an_unresolved_base_is_still_visible_in_the_signature() {
        let mut g = graph_with_file("A.cs");
        let rels = decl_csharp(
            "A.cs",
            b"namespace N;\npublic class D : Elsewhere.Base {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert!(all_relations(&g).is_empty());
        assert_eq!(
            text_fact(&g, "sym:csharp:N#D", FactKind::SymbolSignature).as_deref(),
            Some("public class D : Elsewhere.Base")
        );
    }

    /// A relation whose deriving type is declared in ANOTHER module — a Rust
    /// `impl` of a foreign type — has no local owner node and is dropped rather
    /// than hung off a fabricated one.
    #[test]
    fn impl_of_a_type_from_another_module_fabricates_nothing() {
        let mut g = graph_with_file("src/a.rs");
        let rels = decl_rust(
            "src/a.rs",
            b"pub trait Shape {}\nimpl Shape for Elsewhere {}\n",
            &mut g,
            &SecretPrefixes::default(),
        );
        flush_type_relations(&mut g, rels);
        assert!(all_relations(&g).is_empty());
        assert!(g
            .node(&NodeKey::new("sym:rust:crate::a#Elsewhere"))
            .is_none());
    }

    // ---- U4: calls, references and admitted ambiguity ----

    /// The whole extractor over a fixture repo, in `run`'s own order: pass 1 over
    /// every file, the type relations flushed, then pass 2 against the repo-wide
    /// symbol table. `ceiling` is the configured R23 ambiguity ceiling.
    fn usage_graph(files: &[(&str, &str)], ceiling: usize) -> Graph {
        usage_graph_with(files, ceiling, &[])
    }

    /// [`usage_graph`], with `rights` holding a valid right to delete for
    /// every site (KTD11), and every site record kept in the ledger.
    fn usage_graph_with(files: &[(&str, &str)], ceiling: usize, rights: &[Rule]) -> Graph {
        let paths: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
        let mut g = graph_with_files(&paths);
        g.sites_mut().keep_records();
        let mut relations: Vec<TypeRelation> = Vec::new();
        // One snapshot for the whole walk, taken before it starts — the shape
        // `run` uses, and the one a per-file snapshot would quietly diverge from
        // once the walk starts adding nodes.
        let keys = PathKeys::snapshot(&g);
        let mut names: NamePositions = HashMap::new();
        for (path, src) in files {
            let lang = Language::from_path(path).expect("fixture language has a grammar");
            let mut out = FileOutput::default();
            extract_declarations(
                lang,
                path,
                src.as_bytes(),
                &mut out,
                &keys,
                &SecretPrefixes::default(),
            );
            names.extend(
                std::mem::take(&mut out.names)
                    .into_iter()
                    .map(|(file, key, at)| ((file, key), at)),
            );
            apply_file_output(&mut g, out, &mut relations);
        }
        flush_type_relations(&mut g, relations);
        let table = SymbolTable::build(&g, &names);
        for (path, src) in files {
            let lang = Language::from_path(path).expect("fixture language has a grammar");
            let mut out = FileOutput::default();
            extract_usages(
                lang,
                path,
                src.as_bytes(),
                &mut out,
                &table,
                ceiling,
                rights,
            );
            apply_usage_output(&mut g, out, &mut Vec::new());
        }
        g
    }

    /// Sorted `(node key, fact text)` pairs of one ambiguity fact kind.
    fn ambiguity_facts(graph: &Graph, kind: FactKind) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for node in graph.nodes_sorted() {
            for fact in node.facts.iter().filter(|f| f.kind == kind) {
                if let FactValue::Text(text) = &fact.value {
                    out.push((node.key.as_str().to_string(), text.clone()));
                }
            }
        }
        out.sort();
        out
    }

    /// One candidate, one edge — and NO caveat. An unambiguous match that still
    /// carried a candidate-count fact would bury the ambiguous cases in noise.
    #[test]
    fn a_call_to_an_unambiguous_method_yields_one_edge_and_no_candidate_fact() {
        let g = usage_graph(
            &[
                (
                    "A.cs",
                    "namespace N;\npublic class Caller {\n  public void Go() {\n    Only();\n  }\n}\n",
                ),
                (
                    "B.cs",
                    "namespace N;\npublic class Target {\n  public void Only() {}\n}\n",
                ),
            ],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::Calls),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "sym:csharp:N#Target.Only()".to_string()
            )]
        );
        assert!(ambiguity_facts(&g, FactKind::CandidateCount).is_empty());
        assert!(ambiguity_facts(&g, FactKind::AmbiguousReference).is_empty());
    }

    /// The R23/KTD6 core below the ceiling: every candidate gets its edge, and the
    /// count is stated so a reader knows at most one of them is the real callee.
    #[test]
    fn a_call_to_a_name_in_three_types_yields_three_edges_and_a_candidate_fact() {
        let g = usage_graph(
            &[
                (
                    "A.cs",
                    "namespace N;\npublic class Caller {\n  public void Go() {\n    Run();\n  }\n}\n",
                ),
                (
                    "B.cs",
                    "namespace N;\npublic class One { public void Run() {} }\n\
                     public class Two { public void Run() {} }\n\
                     public class Three { public void Run() {} }\n",
                ),
            ],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::Calls),
            vec![
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#One.Run()".to_string()
                ),
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#Three.Run()".to_string()
                ),
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#Two.Run()".to_string()
                ),
            ]
        );
        assert_eq!(
            ambiguity_facts(&g, FactKind::CandidateCount),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "Run: 3 candidates".to_string()
            )]
        );
        // Below the ceiling the edges ARE emitted, so the no-edge fact stays absent.
        assert!(ambiguity_facts(&g, FactKind::AmbiguousReference).is_empty());
    }

    /// Zero candidates → nothing. A name this repo does not declare is an external
    /// call, and inventing a target node would be a fabrication (U5 owns externals).
    #[test]
    fn a_call_to_a_method_not_in_the_repo_yields_no_edge() {
        let g = usage_graph(
            &[(
                "A.cs",
                "namespace N;\npublic class Caller {\n  public void Go() {\n    Console.WriteLine();\n  }\n}\n",
            )],
            8,
        );

        assert!(relation_edges(&g, EdgeKind::Calls).is_empty());
        assert!(ambiguity_facts(&g, FactKind::CandidateCount).is_empty());
        assert!(ambiguity_facts(&g, FactKind::AmbiguousReference).is_empty());
        // And no phantom node stands in for the external name.
        assert!(g.node(&NodeKey::new("sym:csharp:N#Console")).is_none());
    }

    /// Construction names a TYPE, so it is `Instantiates` and never `Calls` — a
    /// consumer asking "who builds this type" must not have to filter call noise.
    #[test]
    fn new_type_yields_instantiates_not_calls() {
        let g = usage_graph(
            &[
                (
                    "A.cs",
                    "namespace N;\npublic class Caller {\n  public void Go() {\n    var t = new Target();\n  }\n}\n",
                ),
                ("B.cs", "namespace N;\npublic class Target { }\n"),
            ],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::Instantiates),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "sym:csharp:N#Target".to_string()
            )]
        );
        assert!(relation_edges(&g, EdgeKind::Calls).is_empty());
        // The constructed type is not ALSO reported as a plain reference: the
        // `new` already said it.
        assert!(relation_edges(&g, EdgeKind::References).is_empty());
    }

    /// Recursion is a real call and keeps its self-loop — but exactly one, however
    /// many times the body calls itself.
    #[test]
    fn a_recursive_self_call_does_not_create_a_duplicate_edge() {
        let g = usage_graph(
            &[(
                "A.cs",
                "namespace N;\npublic class R {\n  public void Loop() {\n    Loop();\n    Loop();\n  }\n}\n",
            )],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::Calls),
            vec![(
                "sym:csharp:N#R.Loop()".to_string(),
                "sym:csharp:N#R.Loop()".to_string()
            )]
        );
    }

    /// A type named in a type position — here a parameter — is a `References`.
    #[test]
    fn a_type_named_in_a_parameter_is_a_reference() {
        let g = usage_graph(
            &[
                (
                    "A.cs",
                    "namespace N;\npublic class Caller {\n  public void Go(Target t) {}\n}\n",
                ),
                ("B.cs", "namespace N;\npublic class Target { }\n"),
            ],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::References),
            vec![(
                "sym:csharp:N#Caller.Go(Target)".to_string(),
                "sym:csharp:N#Target".to_string()
            )]
        );
    }

    /// Module-level code is enclosed by no declaration, so the FILE carries the
    /// relation — the alternative is losing it, which R20 forbids.
    #[test]
    fn a_module_level_call_is_carried_by_the_file() {
        let g = usage_graph(&[("m.py", "def helper():\n    pass\n\nhelper()\n")], 8);

        assert_eq!(
            relation_edges(&g, EdgeKind::Calls),
            vec![("m.py".to_string(), "sym:python:m#helper()".to_string())]
        );
    }

    /// Python spells construction as a plain call, so a call naming no callable but
    /// a declared class is an `Instantiates`, not a dropped relation.
    #[test]
    fn a_python_call_on_a_declared_class_is_an_instantiation() {
        let g = usage_graph(
            &[(
                "p.py",
                "class Thing:\n    pass\n\ndef make():\n    return Thing()\n",
            )],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::Instantiates),
            vec![(
                "sym:python:p#make()".to_string(),
                "sym:python:p#Thing".to_string()
            )]
        );
        assert!(relation_edges(&g, EdgeKind::Calls).is_empty());
    }

    /// A Rust struct literal is the language's only construction syntax.
    #[test]
    fn a_rust_struct_literal_is_an_instantiation() {
        let g = usage_graph(
            &[(
                "src/a.rs",
                "pub struct Cfg { pub n: u8 }\npub fn build() -> Cfg { Cfg { n: 1 } }\n",
            )],
            8,
        );

        assert_eq!(
            relation_edges(&g, EdgeKind::Instantiates),
            vec![(
                "sym:rust:crate::a#build()".to_string(),
                "sym:rust:crate::a#Cfg".to_string()
            )]
        );
    }

    /// A source declaring `Get` in `count` types, plus one caller. The shape R23
    /// exists for: a name a large codebase spreads over dozens of unrelated types.
    fn many_same_named_methods(count: usize) -> String {
        let mut src =
            String::from("namespace N;\npublic class Caller { public void Go() { Get(); } }\n");
        for i in 0..count {
            src.push_str(&format!("public class T{i} {{ public void Get() {{}} }}\n"));
        }
        src
    }

    /// Above the ceiling the graph names NO edges and says "ambiguous, N
    /// candidates" instead (R23) — and the distinct fact kind is what keeps that
    /// from reading as "nothing there" (R20).
    #[test]
    fn a_call_above_the_ceiling_yields_no_edges_only_the_ambiguity_fact() {
        let src = many_same_named_methods(12);
        let g = usage_graph(&[("A.cs", &src)], 8);

        assert!(
            relation_edges(&g, EdgeKind::Calls).is_empty(),
            "named edges stay a privilege of resolved extraction above the ceiling"
        );
        assert_eq!(
            ambiguity_facts(&g, FactKind::AmbiguousReference),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "Get: 12 candidates".to_string()
            )]
        );
        // The below-ceiling fact is the wrong statement here and must not appear.
        assert!(ambiguity_facts(&g, FactKind::CandidateCount).is_empty());
    }

    /// Below (or at) the ceiling every candidate DOES get a named edge (R1) —
    /// but each one must carry its own `ambiguous_candidates` count on its
    /// `Provenance` (KTD2), not just the companion `CandidateCount` fact:
    /// this is what lets the card render the caveat right at the row instead
    /// of only in a separate `## Notes` line the reader has to cross-reference.
    #[test]
    fn a_call_below_the_ceiling_tags_every_named_edge_with_its_candidate_count() {
        let src = many_same_named_methods(2);
        let g = usage_graph(&[("A.cs", &src)], 8);

        let mut calls: Vec<(String, Option<usize>)> = g
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == EdgeKind::Calls)
            .map(|e| {
                (
                    e.to.as_str().to_string(),
                    e.edge.provenance.ambiguous_candidates,
                )
            })
            .collect();
        calls.sort();
        assert_eq!(
            calls,
            vec![
                ("sym:csharp:N#T0.Get()".to_string(), Some(2)),
                ("sym:csharp:N#T1.Get()".to_string(), Some(2)),
            ],
            "every one of the below-ceiling candidates must carry the same count"
        );
        assert_eq!(
            ambiguity_facts(&g, FactKind::CandidateCount),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "Get: 2 candidates".to_string()
            )]
        );
        // The above-ceiling fact is the wrong statement here and must not appear.
        assert!(ambiguity_facts(&g, FactKind::AmbiguousReference).is_empty());
    }

    /// An unambiguous call (exactly one candidate) must NOT carry a count —
    /// `None` stays the marker for "nothing to disclose," the baseline every
    /// ambiguous case above is contrasted against.
    #[test]
    fn an_unambiguous_call_carries_no_candidate_count() {
        let src = many_same_named_methods(1);
        let g = usage_graph(&[("A.cs", &src)], 8);

        let calls: Vec<Option<usize>> = g
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == EdgeKind::Calls)
            .map(|e| e.edge.provenance.ambiguous_candidates)
            .collect();
        assert_eq!(calls, vec![None]);
        assert!(ambiguity_facts(&g, FactKind::CandidateCount).is_empty());
        assert!(ambiguity_facts(&g, FactKind::AmbiguousReference).is_empty());
    }

    /// The R17 scale guard: with a name in hundreds of types, the edge count is
    /// bounded by the CEILING and not by the candidate count. Raising the ceiling
    /// over the same fixture proves it is the ceiling doing the bounding.
    #[test]
    fn edge_count_stays_bounded_by_the_ceiling_not_by_the_candidate_count() {
        let src = many_same_named_methods(300);

        let bounded = usage_graph(&[("A.cs", &src)], 8);
        let calls = relation_edges(&bounded, EdgeKind::Calls);
        assert!(
            calls.len() <= 8,
            "300 candidates must not become 300 edges, got {}",
            calls.len()
        );
        assert_eq!(
            ambiguity_facts(&bounded, FactKind::AmbiguousReference),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "Get: 300 candidates".to_string()
            )],
            "the count is still stated in full — only the edges are withheld"
        );

        // Same fixture, ceiling above the candidate count: now every candidate is
        // named, which is what shows the ceiling was the binding constraint.
        let unbounded = usage_graph(&[("A.cs", &src)], 500);
        assert_eq!(relation_edges(&unbounded, EdgeKind::Calls).len(), 300);
        assert!(ambiguity_facts(&unbounded, FactKind::AmbiguousReference).is_empty());
        assert_eq!(
            ambiguity_facts(&unbounded, FactKind::CandidateCount),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "Get: 300 candidates".to_string()
            )]
        );
    }

    /// U1: a `FileOutput` is the whole per-file declaration walk buffer — its
    /// graph ops AND its still-unresolved type relations — so the on-disk
    /// cache a later unit adds must round-trip both, not just the ops.
    #[test]
    fn file_output_with_relations_round_trips_through_json() {
        let mut out = FileOutput::default();
        out.fact(
            NodeKey::new("src/a.rs"),
            Fact::new(
                FactKind::Loc,
                FactValue::Int(10),
                Provenance::new("src/a.rs", None, "metrics".into(), Tier::Measured),
            ),
        );
        out.relations.push(TypeRelation {
            from: vec![NodeKey::new("sym:rust:crate::a#D")],
            to: vec![NodeKey::new("sym:rust:crate::a#B")],
            kind: RelationKind::Extends,
            prov: Provenance::new("src/a.rs", Some(3), "treesitter".into(), Tier::Measured),
        });

        // Through the wire twin — the only form that reaches a disk (U7).
        use crate::extract::walk::WalkOutput;
        let vocabulary = crate::graph::Vocabulary::default();
        let json = serde_json::to_string(
            &out.to_wire(&vocabulary)
                .expect("every kind in this fixture is one the vocabulary can name"),
        )
        .unwrap();
        let wire: FileOutputWire = serde_json::from_str(&json).unwrap();
        let round_tripped = FileOutput::from_wire(wire, &vocabulary)
            .expect("every kind here is a core kind, which any vocabulary can name");
        assert_eq!(out, round_tripped);
        assert_eq!(round_tripped.relations.len(), 1);
    }

    // ---- Plan A, U7: the shared resolver over real C# context ----
    //
    // These go through the committed C# component, so every site carries the
    // context contract 0.2 delivers; the expected rungs follow from the rule
    // catalog (KTD6) and the per-candidate flowchart (KTD5), never from the
    // code under test.

    /// `(from, to, rung, rule)` of every edge of `kind`, sorted.
    fn rated(graph: &Graph, kind: EdgeKind) -> Vec<(String, String, Confidence, Rule)> {
        let mut out: Vec<(String, String, Confidence, Rule)> = graph
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == kind)
            .map(|e| {
                (
                    e.from.as_str().to_string(),
                    e.to.as_str().to_string(),
                    e.edge
                        .provenance
                        .rung()
                        .expect("a name-resolved edge is rated"),
                    e.edge.provenance.rule.expect("a rated edge names its rule"),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// The trace one candidate of the site named `name` carries.
    fn trace_of(graph: &Graph, name: &str, target: &str) -> Vec<(Rule, crate::graph::Verdict)> {
        let records = graph.sites().records_sorted();
        let candidate = records
            .iter()
            .filter(|r| r.name == name)
            .flat_map(|r| r.candidates.iter())
            .find(|c| c.target.as_str() == target)
            .unwrap_or_else(|| panic!("no candidate `{target}` at a site named `{name}`"));
        candidate
            .trace
            .iter()
            .map(|v| (v.rule, v.verdict))
            .collect()
    }

    fn cs(name: &str, src: &str) -> (String, String) {
        (name.to_string(), format!("namespace N;\n{src}"))
    }

    fn graph_of(files: &[(String, String)], rights: &[Rule]) -> Graph {
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        usage_graph_with(&files, 8, rights)
    }

    /// AE1 (R5): a call on a language keyword cannot reach the repository —
    /// no edge, no caveat, no site record, whatever the repo declares under
    /// the name.
    #[test]
    fn a_call_on_a_keyword_receiver_yields_nothing() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class C {\n  public bool M(string s) {\n    return string.IsNullOrEmpty(s);\n  }\n}\n",
                ),
                cs(
                    "B.cs",
                    "public class OctetString {\n  public static bool IsNullOrEmpty(OctetString s) { return true; }\n}\n",
                ),
            ],
            &[],
        );
        assert!(relation_edges(&g, EdgeKind::Calls).is_empty());
        assert!(ambiguity_facts(&g, FactKind::CandidateCount).is_empty());
        assert!(g
            .sites()
            .records_sorted()
            .iter()
            .all(|r| r.name != "IsNullOrEmpty"));
    }

    const STRING_BUILDER_CALLER: &str = "public class Report {\n  public void Write(StringBuilder result) {\n    result.Append(\"x\");\n  }\n}\n";
    const KAL_KAP_ITEM: &str = "public class LedgerItem {\n  public void Append(string s) {}\n}\n";

    /// AE2 (R3, R8): the receiver is declared as `StringBuilder`, a type the
    /// repository does not declare, so `declared-receiver` disputes the one
    /// repo candidate. Without a right it stays, ambiguous — in "maybe also".
    #[test]
    fn a_receiver_of_an_external_declared_type_disputes_the_repo_candidate() {
        let g = graph_of(
            &[cs("A.cs", STRING_BUILDER_CALLER), cs("B.cs", KAL_KAP_ITEM)],
            &[],
        );
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![(
                "sym:csharp:N#Report.Write(StringBuilder)".to_string(),
                "sym:csharp:N#LedgerItem.Append(string)".to_string(),
                Confidence::Ambiguous,
                Rule::DeclaredReceiver,
            )]
        );
        assert!(
            trace_of(&g, "Append", "sym:csharp:N#LedgerItem.Append(string)")
                .contains(&(Rule::DeclaredReceiver, crate::graph::Verdict::Exclude))
        );
        assert!(g.sites().deleted_by_rule().is_empty());
    }

    /// AE2 with the right to delete: the edge does not exist at all, and the
    /// ledger counts the deletion under the rule that made it.
    #[test]
    fn a_rule_holding_a_right_deletes_the_candidate_and_the_ledger_counts_it() {
        let g = graph_of(
            &[cs("A.cs", STRING_BUILDER_CALLER), cs("B.cs", KAL_KAP_ITEM)],
            &[Rule::DeclaredReceiver],
        );
        assert!(relation_edges(&g, EdgeKind::Calls).is_empty());
        assert_eq!(
            g.sites().deleted_by_rule().get(&Rule::DeclaredReceiver),
            Some(&1)
        );
    }

    /// A rule without the right — here `implicit-this` is granted, which
    /// never excludes — deletes nothing: the exclusion stays a dispute.
    #[test]
    fn an_exclusion_without_its_own_right_is_only_a_dispute() {
        let g = graph_of(
            &[cs("A.cs", STRING_BUILDER_CALLER), cs("B.cs", KAL_KAP_ITEM)],
            &[Rule::ImplicitThis],
        );
        assert_eq!(rated(&g, EdgeKind::Calls).len(), 1);
        assert_eq!(rated(&g, EdgeKind::Calls)[0].2, Confidence::Ambiguous);
        assert!(g.sites().deleted_by_rule().is_empty());
    }

    const FOO_AND_BAR: &str = "public class Foo {\n  public void Run() {}\n}\npublic class Bar {\n  public void Run() {}\n}\n";

    /// A receiver declared as a repository type backs that type's member —
    /// the only one backed and undisputed, so rules-backed — and disputes the
    /// same name on an unrelated type.
    #[test]
    fn a_declared_repo_type_backs_its_member_and_disputes_another() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {\n  public void Go(Foo f) {\n    f.Run();\n  }\n}\n",
                ),
                cs("B.cs", FOO_AND_BAR),
            ],
            &[],
        );
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![
                (
                    "sym:csharp:N#Caller.Go(Foo)".to_string(),
                    "sym:csharp:N#Bar.Run()".to_string(),
                    Confidence::Ambiguous,
                    Rule::DeclaredReceiver,
                ),
                (
                    "sym:csharp:N#Caller.Go(Foo)".to_string(),
                    "sym:csharp:N#Foo.Run()".to_string(),
                    Confidence::RulesBacked,
                    Rule::DeclaredReceiver,
                ),
            ]
        );
    }

    const INTERFACE_AND_IMPL: &str = "public interface IFoo {\n  void Run();\n}\npublic class Foo : IFoo {\n  public void Run() {}\n}\npublic class Bar {\n  public void Run() {}\n}\n";
    const INTERFACE_CALLER: &str =
        "public class Caller {\n  public void Go(IFoo f) {\n    f.Run();\n  }\n}\n";

    /// A receiver declared as an interface backs the interface's member; the
    /// implementation in a descendant is reachable by dispatch, so the rule
    /// says nothing about it — it is not disputed — and even with the right to
    /// delete it stays. The unrelated `Bar.Run` is deleted.
    #[test]
    fn an_implementation_of_the_declared_interface_is_never_disputed_or_deleted() {
        for rights in [&[][..], &[Rule::DeclaredReceiver][..]] {
            let g = graph_of(
                &[cs("A.cs", INTERFACE_CALLER), cs("B.cs", INTERFACE_AND_IMPL)],
                rights,
            );
            let calls = rated(&g, EdgeKind::Calls);
            assert!(
                calls.contains(&(
                    "sym:csharp:N#Caller.Go(IFoo)".to_string(),
                    "sym:csharp:N#IFoo.Run()".to_string(),
                    Confidence::RulesBacked,
                    Rule::DeclaredReceiver,
                )),
                "{calls:?}"
            );
            // Undecided, with other candidates left: ambiguous by the name.
            assert!(
                calls.contains(&(
                    "sym:csharp:N#Caller.Go(IFoo)".to_string(),
                    "sym:csharp:N#Foo.Run()".to_string(),
                    Confidence::Ambiguous,
                    Rule::NameMatch,
                )),
                "{calls:?}"
            );
            assert!(trace_of(&g, "Run", "sym:csharp:N#Foo.Run()")
                .contains(&(Rule::DeclaredReceiver, crate::graph::Verdict::Abstain)));
            let bar_kept = calls.iter().any(|c| c.1 == "sym:csharp:N#Bar.Run()");
            assert_eq!(bar_kept, rights.is_empty(), "{calls:?}");
        }
    }

    /// A repository extension method on `this IEnumerable<T>` called on a
    /// `List<int>`: neither type is the repository's, so nothing shows they
    /// are unrelated — the rule abstains and the one candidate is name-only.
    #[test]
    fn an_extension_method_on_an_external_interface_is_not_disputed() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {\n  public void Go() {\n    List<int> xs = new List<int>();\n    xs.Shuffle();\n  }\n}\n",
                ),
                cs(
                    "B.cs",
                    "public static class Ext {\n  public static void Shuffle<T>(this IEnumerable<T> xs) {}\n}\n",
                ),
            ],
            &[Rule::DeclaredReceiver],
        );
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "sym:csharp:N#Ext.Shuffle(IEnumerable<T>)".to_string(),
                Confidence::NameOnly,
                Rule::NameMatch,
            )]
        );
        assert_eq!(
            trace_of(&g, "Shuffle", "sym:csharp:N#Ext.Shuffle(IEnumerable<T>)"),
            vec![
                (Rule::RoleMismatch, crate::graph::Verdict::Abstain),
                (Rule::DeclaredReceiver, crate::graph::Verdict::Abstain),
                (Rule::Arity, crate::graph::Verdict::Abstain),
            ]
        );
    }

    /// AE4 (R8, R11): a type whose namespace the file cannot see is disputed
    /// by `namespace-visibility`, which never holds a right — the one
    /// candidate stays, ambiguous. With the using directive it is name-only.
    /// (The files sit under `src/` so no path stem reads like a namespace.)
    #[test]
    fn a_type_in_a_namespace_the_file_does_not_import_is_disputed_not_deleted() {
        let target = (
            "src/Widget.cs".to_string(),
            "namespace B;\npublic class Widget {}\n".to_string(),
        );
        let hidden = (
            "src/Maker.cs".to_string(),
            "namespace A;\npublic class Maker {\n  public object Make() {\n    return new Widget();\n  }\n}\n"
                .to_string(),
        );
        let g = graph_of(&[hidden, target.clone()], &[Rule::NamespaceVisibility]);
        assert_eq!(
            rated(&g, EdgeKind::Instantiates),
            vec![(
                "sym:csharp:A#Maker.Make()".to_string(),
                "sym:csharp:B#Widget".to_string(),
                Confidence::Ambiguous,
                Rule::NamespaceVisibility,
            )]
        );

        let imported = (
            "src/Maker.cs".to_string(),
            "using B;\nnamespace A;\npublic class Maker {\n  public object Make() {\n    return new Widget();\n  }\n}\n"
                .to_string(),
        );
        let g = graph_of(&[imported, target], &[]);
        assert_eq!(rated(&g, EdgeKind::Instantiates)[0].2, Confidence::NameOnly);
    }

    /// A bare call the plugin says has no receiver: `implicit-this` backs the
    /// caller's own member, the only one backed, while the same name on
    /// another type is left undecided — ambiguous, since two remain.
    #[test]
    fn a_bare_call_inside_a_type_is_backed_by_implicit_this() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Worker {\n  public void Go() {\n    Helper();\n  }\n  private void Helper() {}\n}\n",
                ),
                cs("B.cs", "public class Other {\n  public void Helper() {}\n}\n"),
            ],
            &[],
        );
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![
                (
                    "sym:csharp:N#Worker.Go()".to_string(),
                    "sym:csharp:N#Other.Helper()".to_string(),
                    Confidence::Ambiguous,
                    Rule::NameMatch,
                ),
                (
                    "sym:csharp:N#Worker.Go()".to_string(),
                    "sym:csharp:N#Worker.Helper()".to_string(),
                    Confidence::RulesBacked,
                    Rule::ImplicitThis,
                ),
            ]
        );
    }

    /// `Type.Member()` on a repository type: `static-receiver` backs the
    /// type's static member and disputes the same name elsewhere.
    #[test]
    fn a_call_on_a_type_name_is_judged_by_static_receiver() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {\n  public void Go() {\n    Util.Make();\n  }\n}\n",
                ),
                cs(
                    "B.cs",
                    "public static class Util {\n  public static void Make() {}\n}\npublic class Other {\n  public void Make() {}\n}\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#Other.Make()".to_string(),
                    Confidence::Ambiguous,
                    Rule::StaticReceiver,
                ),
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#Util.Make()".to_string(),
                    Confidence::RulesBacked,
                    Rule::StaticReceiver,
                ),
            ]
        );
    }

    /// `Util.Make()` where `Util` is a local the file declares without a
    /// type: a call on that value, which `static-receiver` must not read as
    /// the type `Util` — nothing backs the repository's `Util.Make`.
    #[test]
    fn a_value_named_like_a_type_is_not_judged_by_static_receiver() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {
  public void Go() {
    var Util = Load();
    Util.Make();
  }
}
",
                ),
                cs(
                    "B.cs",
                    "public static class Util {
  public static void Make() {}
}
public class Other {
  public void Make() {}
}
",
                ),
            ],
            &[],
        );
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#Other.Make()".to_string(),
                    Confidence::Ambiguous,
                    Rule::NameMatch,
                ),
                (
                    "sym:csharp:N#Caller.Go()".to_string(),
                    "sym:csharp:N#Util.Make()".to_string(),
                    Confidence::Ambiguous,
                    Rule::NameMatch,
                ),
            ]
        );
    }

    /// Two overloads on the declared type are both backed, so the backing
    /// picks no one target: both are ambiguous until another rule tells them
    /// apart (KTD5), and their edges name the shared name, not the rule that
    /// backed them both.
    #[test]
    fn two_backed_overloads_are_both_ambiguous() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {\n  public void Go(Foo f) {\n    f.Run(1);\n  }\n}\n",
                ),
                cs(
                    "B.cs",
                    "public class Foo {\n  public void Run(int a) {}\n  public void Run(string a) {}\n}\n",
                ),
            ],
            &[],
        );
        let calls = rated(&g, EdgeKind::Calls);
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls
            .iter()
            .all(|c| c.2 == Confidence::Ambiguous && c.3 == Rule::NameMatch));
    }

    /// A candidate the declared type backs and the argument count disputes
    /// carries both verdicts in its trace, and the dispute decides: it is
    /// ambiguous, named by `arity`. The overload that fits is then the only
    /// one backed and undisputed — rules-backed.
    #[test]
    fn the_trace_keeps_every_rule_and_a_dispute_beats_a_back() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {\n  public void Go(Foo f) {\n    f.Run();\n  }\n}\n",
                ),
                cs(
                    "B.cs",
                    "public class Foo {\n  public void Run() {}\n  public void Run(int a) {}\n}\n",
                ),
            ],
            &[],
        );
        let trace = trace_of(&g, "Run", "sym:csharp:N#Foo.Run(int)");
        assert!(trace.contains(&(Rule::DeclaredReceiver, crate::graph::Verdict::Back)));
        assert!(trace.contains(&(Rule::Arity, crate::graph::Verdict::Dispute)));
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![
                (
                    "sym:csharp:N#Caller.Go(Foo)".to_string(),
                    "sym:csharp:N#Foo.Run()".to_string(),
                    Confidence::RulesBacked,
                    Rule::DeclaredReceiver,
                ),
                (
                    "sym:csharp:N#Caller.Go(Foo)".to_string(),
                    "sym:csharp:N#Foo.Run(int)".to_string(),
                    Confidence::Ambiguous,
                    Rule::Arity,
                ),
            ]
        );
    }

    /// An edge is the summary of its sites (KTD5): one relation read at an
    /// ambiguous site and at a backed one carries the backed rung, anchored at
    /// the backed site, and lists both.
    #[test]
    fn an_edge_takes_the_strongest_rung_among_its_sites() {
        let g = graph_of(
            &[
                cs(
                    "A.cs",
                    "public class Caller {\n  public Foo Make() { return null; }\n  public void Go(Foo f) {\n    Make().Run();\n    f.Run();\n  }\n}\n",
                ),
                cs("B.cs", FOO_AND_BAR),
            ],
            &[],
        );
        let edge = g
            .edges_sorted()
            .into_iter()
            .find(|e| e.edge.kind == EdgeKind::Calls && e.to.as_str() == "sym:csharp:N#Foo.Run()")
            .expect("the relation exists");
        assert_eq!(edge.edge.provenance.rung(), Some(Confidence::RulesBacked));
        assert_eq!(edge.edge.provenance.span.start(), Some(6));
        let lines: Vec<u32> = edge.edge.sites.iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![5, 6]);
    }

    /// Over the ceiling with no right that could thin the candidates, the site
    /// is settled by its count alone: the one fact as before, and a record
    /// with no per-candidate work in it (KTD5).
    #[test]
    fn a_site_over_the_ceiling_without_rights_is_settled_without_judging_candidates() {
        let src = many_same_named_methods(12);
        let g = usage_graph(&[("A.cs", &src)], 8);
        assert_eq!(
            ambiguity_facts(&g, FactKind::AmbiguousReference),
            vec![(
                "sym:csharp:N#Caller.Go()".to_string(),
                "Get: 12 candidates".to_string()
            )]
        );
        let records = g.sites().records_sorted();
        let site = records.iter().find(|r| r.name == "Get").unwrap();
        assert_eq!(site.candidate_count, 12);
        assert!(site.candidates.is_empty());
        assert_eq!(g.sites().over_cap_sites(), 1);
    }

    /// With a right in force the candidates ARE judged before the ceiling, and
    /// the ceiling is compared with what is left: twelve `Get`s on unrelated
    /// types, called on a receiver declared as one of them, leave one.
    #[test]
    fn with_a_right_the_ceiling_counts_candidates_after_exclusion() {
        let mut src = String::from(
            "namespace N;\npublic class Caller { public void Go(T0 t) { t.Get(); } }\n",
        );
        for i in 0..12 {
            src.push_str(&format!("public class T{i} {{ public void Get() {{}} }}\n"));
        }
        let g = usage_graph_with(&[("A.cs", &src)], 8, &[Rule::DeclaredReceiver]);
        assert!(ambiguity_facts(&g, FactKind::AmbiguousReference).is_empty());
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![(
                "sym:csharp:N#Caller.Go(T0)".to_string(),
                "sym:csharp:N#T0.Get()".to_string(),
                Confidence::RulesBacked,
                Rule::DeclaredReceiver,
            )]
        );
        assert_eq!(
            g.sites().deleted_by_rule().get(&Rule::DeclaredReceiver),
            Some(&11)
        );
        assert_eq!(g.sites().over_cap_sites(), 0);
    }

    /// A language whose plugin reads no context runs no rule: one candidate is
    /// name-only, by the name match (R6).
    #[test]
    fn a_python_single_candidate_is_name_only() {
        let g = usage_graph(&[("m.py", "def helper():\n    pass\n\nhelper()\n")], 8);
        assert_eq!(
            rated(&g, EdgeKind::Calls),
            vec![(
                "m.py".to_string(),
                "sym:python:m#helper()".to_string(),
                Confidence::NameOnly,
                Rule::NameMatch,
            )]
        );
        let records = g.sites().records_sorted();
        assert!(records[0].candidates[0].trace.is_empty());
    }

    /// Two members on one line: the column of the call and of each member's
    /// name decide which encloses it, rather than whichever sorts last.
    #[test]
    fn the_column_decides_the_owner_between_declarations_on_one_line() {
        let g = usage_graph(
            &[(
                "A.cs",
                "namespace N;\npublic class C { public void A() { X(); } public void B() { Y(); } public void X() {} public void Y() {} }\n",
            )],
            8,
        );
        let calls = relation_edges(&g, EdgeKind::Calls);
        assert_eq!(
            calls,
            vec![
                (
                    "sym:csharp:N#C.A()".to_string(),
                    "sym:csharp:N#C.X()".to_string()
                ),
                (
                    "sym:csharp:N#C.B()".to_string(),
                    "sym:csharp:N#C.Y()".to_string()
                ),
            ]
        );
        assert!(g.sites().records_sorted().iter().all(|r| r.owner_mapped));
    }

    /// The base found by its spelling is backed by `base-spelling`; one found
    /// only as the single type of its name elsewhere is a name match.
    #[test]
    fn a_spelled_base_is_rules_backed_and_a_unique_name_is_name_only() {
        let g = usage_graph(
            &[
                (
                    "A.cs",
                    "namespace N;\npublic class Base {}\npublic class D : Base {}\npublic class E : Far {}\n",
                ),
                ("B.cs", "namespace M;\npublic class Far {}\n"),
            ],
            8,
        );
        assert_eq!(
            rated(&g, EdgeKind::Extends),
            vec![
                (
                    "sym:csharp:N#D".to_string(),
                    "sym:csharp:N#Base".to_string(),
                    Confidence::RulesBacked,
                    Rule::BaseSpelling,
                ),
                (
                    "sym:csharp:N#E".to_string(),
                    "sym:csharp:M#Far".to_string(),
                    Confidence::NameOnly,
                    Rule::NameMatch,
                ),
            ]
        );
    }

    /// Without columns (a plugin that reads no context, here TypeScript) a
    /// type and its members on one line cannot be told apart by the line: the
    /// site is unmapped, never handed to whichever declaration sorts last, and
    /// its relation hangs off the nearest declaration known to enclose it —
    /// here none, so the file (plan A, KTD7).
    #[test]
    fn without_columns_declarations_sharing_a_line_leave_the_site_unmapped() {
        let g = usage_graph(
            &[(
                "web/a.ts",
                "export class A { m() { foo(); } n() { foo(); } }\nexport function foo() {}\n",
            )],
            8,
        );
        let calls = relation_edges(&g, EdgeKind::Calls);
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].0, "web/a.ts");
        assert!(calls[0].1.ends_with("#foo()"), "{calls:?}");
        let records = g.sites().records_sorted();
        assert!(!records.is_empty());
        assert!(
            records
                .iter()
                .all(|r| !r.owner_mapped && r.owner.as_str() == "web/a.ts"),
            "{records:?}"
        );
    }
}
