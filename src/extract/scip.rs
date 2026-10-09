//! The `resolved` RESOLUTION: consume a SCIP index the target project already
//! has (U6).
//!
//! Not a tier. Tier (Measured / Derived / Narrated) says how much a fact is
//! trusted; Resolution (Heuristic / Resolved) says how precise it is, and the two
//! are deliberately independent. What this extractor emits is measured AND
//! resolved; calling it "the resolved tier" would invite the collapse the whole
//! two-axis model exists to prevent.
//!
//! Everything else in the graph is a name heuristic. A SCIP index carries real
//! semantic identity — the compiler resolved it — so the relations read from it
//! are the only ones that may carry [`Resolution::Resolved`], which is in turn
//! what licenses an absence claim (R20).
//!
//! ## What this extractor will not do
//!
//! It never runs a build and never runs an indexer: the main run reads an index
//! and nothing more (plan A, R9). The index comes from outside the run — the
//! target project's own CI, or the separate `reverse-architect index`
//! subcommand (`src/index/`), the one explicit step where RA executes the
//! repository's build logic, and only when the operator asks for it. No part of
//! this module spawns a process; a unit test asserts that over its own source
//! text.
//!
//! Absence of an index is a normal state, not an error: without one the corpus is
//! a complete heuristic corpus. Only an index that exists and cannot be parsed is
//! an error, and then a loud one — a silently empty result would look exactly like
//! "this project has no cross-references".
//!
//! ## The one read that bypasses KTD10
//!
//! Every other extractor reads git blobs ([`Repo::blob_bytes`]) rather than the
//! working tree, so a dirty checkout cannot change the corpus. A SCIP index is a
//! build artifact: it is not tracked, so there is no blob to read and
//! [`std::fs::read`] is the only option. Determinism is preserved differently —
//! the index must be named explicitly on the command line, so a stale file lying
//! around in the repository can never be picked up by accident.
//!
//! ## Why a drift gate exists at all
//!
//! A SCIP index carries no source text and no content hashes, so RA cannot verify
//! by content that the index describes the working tree (measured against
//! `scip-dotnet` 0.2.14 on 19.08.2026). What it can do is check the index's own
//! claims against the tree-sitter facts already in the graph: for every name both
//! sides know, each definition the index places must land inside the declaration
//! range the grammar found. A file that fails gets no `resolved` facts at all.
//!
//! The gate's honest limit: it catches declarations that MOVED, which is what
//! almost every edit does to the lines below it. It cannot catch a declaration
//! appended after the last indexed one, because nothing shifted. `resolved`
//! coverage therefore means "the index and the grammar agree about this file", not
//! "this file is unchanged since indexing".
//!
//! ## Pairing with the usage sites
//!
//! The grammar's `Calls` never meets the index in merge: SCIP has no "call"
//! role, only occurrences, which this extractor reads as `References`. Since
//! plan A, U9 the run's usage sites are paired with those occurrences before
//! anything is emitted ([`super::reconcile`]): every reference occurrence of a
//! verified document is kept with its column, its simple name and what its
//! symbol is — a verified definition in the repository, a symbol from outside
//! it, or neither — including the external ones that used to be dropped. An
//! occurrence the pairing folded into a usage site's own relation emits no
//! `References` of its own.

use super::ids;
use super::reconcile::{self, IndexedFile, KeptOccurrence, OccurrenceTarget};
use super::{ExtractError, Extractor, Repo, TrackedFile};
use crate::graph::{
    Edge, EdgeKind, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, SiteId, Tier,
};
use protobuf::Message;
use scip::types::{descriptor::Suffix, Index, SymbolRole};
use sha2::{Digest, Sha256};
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

/// Above tree-sitter's rank so that a future scalar fact from the index takes
/// precedence in a merge conflict. Nothing today depends on it: the fact kinds
/// this extractor emits are its own, and edge precision is decided by
/// [`Resolution`], not by trust rank (KTD6).
const TRUST_RANK: u8 = 35;

/// The root module node key, where the run-level summary lands.
const ROOT: &str = ".";

/// Reads an externally produced SCIP index into the graph, as the only source of
/// [`Resolution::Resolved`] provenance. Its tier is `Measured`, like any other
/// extractor that reports what it read rather than what it inferred.
pub struct Scip {
    /// Path to the index, exactly as the user named it. `None` — the default —
    /// means this extractor does nothing at all.
    pub index_path: Option<PathBuf>,
    /// Token prefixes this run recognizes with no keyword nearby (R22). The index
    /// names itself in the coverage artifact, and that name gets the same filter
    /// as any other text RA publishes — including a company token shape.
    pub secret_prefixes: super::external::SecretPrefixes,
}

impl Extractor for Scip {
    fn id(&self) -> crate::graph::ExtractorId {
        ids::scip()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[
            FactKind::SymbolMoniker,
            FactKind::ResolvedByIndex,
            FactKind::ResolvedCoverage,
        ]
    }

    fn trust_rank(&self) -> u8 {
        TRUST_RANK
    }

    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        _progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError> {
        let Some(path) = self.index_path.as_ref() else {
            return Ok(());
        };
        let bytes = std::fs::read(path).map_err(|source| ExtractError::ScipRead {
            path: path.clone(),
            source,
        })?;
        // Of the bytes parsed below, not of a second read: the fingerprint names
        // the index this run actually paired against.
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let index = Index::parse_from_bytes(&bytes).map_err(|source| ExtractError::ScipFormat {
            path: path.clone(),
            source,
        })?;

        let tool = tool_label(&index, &self.secret_prefixes);
        let tracked_files: BTreeMap<String, TrackedFile> = repo
            .tracked_files_with_paths()?
            .into_iter()
            .map(|tf| (tf.key.clone(), tf))
            .collect();
        let tracked: BTreeSet<String> = tracked_files.keys().cloned().collect();
        let decls = Decls::build(graph);
        let mut monikers = MonikerCache::default();

        // Pass 1: decide which documents may speak, and build the moniker → node
        // map from the very matches that decided it. A reference can then only
        // resolve to a definition this pass verified.
        // The verified matches travel WITH the document. Pass 2 used to call
        // `definition_matches` again, walking every occurrence of a
        // graph-scale index a second time to recompute what pass 1 had already
        // decided.
        let mut verified: Vec<(&scip::types::Document, String, BTreeMap<String, NodeKey>)> =
            Vec::new();
        let mut definitions = Definitions::default();
        // Documents of tracked files the drift gate did not verify: their usage
        // sites are counted by the pairing, and not paired.
        let mut unverified_files: Vec<String> = Vec::new();
        // Drift reason → count, so the summary can say WHICH disagreement was
        // found. A BTreeMap keeps the sentence in a fixed order (KTD7).
        let mut drifted: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut unverifiable = 0usize;
        let mut unmatched = 0usize;

        for document in &index.documents {
            let Some(file) = repo_key(&document.relative_path, &tracked) else {
                unmatched += 1;
                continue;
            };
            let source = Source::new(repo, tracked_files.get(&file), document);
            match verify(document, &file, &decls, &mut monikers, &|range| {
                source.column(range)
            }) {
                Verdict::Verified(matched) => {
                    let mut nodes = BTreeMap::new();
                    for (moniker, key) in matched {
                        definitions.add(&moniker, key.as_ref());
                        if let Some(key) = key {
                            nodes.insert(moniker, key);
                        }
                    }
                    verified.push((document, file, nodes));
                }
                Verdict::Drifted(reason) => {
                    *drifted.entry(reason).or_default() += 1;
                    unverified_files.push(file);
                }
                Verdict::Unverifiable => {
                    unverifiable += 1;
                    unverified_files.push(file);
                }
            }
        }
        let definitions = definitions.settle();

        // The pairing with the usage sites (plan A, U9), before anything is
        // emitted: it decides which reference occurrences are a site's own
        // relation and so emit no `References` of their own.
        let packages = repository_packages(&index, &mut monikers);
        let mut files: BTreeMap<String, IndexedFile> = unverified_files
            .into_iter()
            .map(|file| (file, IndexedFile::Unverified))
            .collect();
        // Only an occurrence a usage site could pair with is kept: one written
        // on a site's line under a site's name. The rest of a graph-scale index
        // — most of its occurrences — is never looked up, and keeping it would
        // hold a second copy of the index for nothing.
        let places = site_places(graph.sites().records_unordered());
        let no_places = HashSet::new();
        for (document, file, _) in &verified {
            // Read afresh rather than kept from the drift gate: a document's
            // text is needed only while its own occurrences are converted, and
            // holding every verified document's text until the run ends would
            // put the repository's sources in memory beside the index.
            let source = Source::new(repo, tracked_files.get(file), document);
            files.insert(
                file.clone(),
                IndexedFile::Verified(kept_occurrences(
                    document,
                    &definitions,
                    &packages,
                    &mut monikers,
                    places.get(file.as_str()).unwrap_or(&no_places),
                    &|range| source.column(range),
                )),
            );
        }
        drop(places);
        let folded = reconcile::pair(graph, &files, sha256);

        // Pass 2: emit. Facts and edges only ever attach to nodes that already
        // exist — this extractor creates no node of its own, so a symbol the index
        // and tree-sitter both describe stays ONE node (KTD3).
        for (document, file, matched) in &verified {
            graph.add_fact(
                &NodeKey::new(file.clone()),
                Fact::new(
                    FactKind::ResolvedByIndex,
                    FactValue::Text(tool.clone()),
                    resolved_prov(file, None),
                ),
            );

            for (moniker, key) in matched {
                graph.add_fact(
                    key,
                    Fact::new(
                        FactKind::SymbolMoniker,
                        FactValue::Text(moniker.clone()),
                        resolved_prov(file, None),
                    ),
                );
            }

            emit_references(graph, document, file, &decls, &definitions, &folded);
            emit_implementations(graph, document, file, &definitions);
        }

        graph.add_fact(
            &NodeKey::new(ROOT),
            Fact::new(
                FactKind::ResolvedCoverage,
                FactValue::Text(format!(
                    "{tool}: {} documents verified, {} drifted{}, {unverifiable} unverifiable \
                     (no declaration name in common), {unmatched} outside the tracked tree",
                    verified.len(),
                    drifted.values().sum::<usize>(),
                    drift_detail(&drifted),
                )),
                resolved_prov(ROOT, None),
            ),
        );

        Ok(())
    }
}

/// `" (3 declaration moved, 1 declaration renamed in place)"`, or nothing at all
/// when no document drifted — a reader who sees a drift count deserves to know
/// which disagreement produced it, and a reader who sees zero deserves not to
/// read an empty parenthesis.
fn drift_detail(drifted: &BTreeMap<&'static str, usize>) -> String {
    if drifted.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = drifted
        .iter()
        .map(|(reason, count)| format!("{count} {reason}"))
        .collect();
    format!(" ({})", parts.join(", "))
}

/// `"scip-dotnet 0.2.14"`, or a stable placeholder when the index omits its tool
/// info — never an empty string, which would render as a blank coverage claim.
fn tool_label(index: &Index, prefixes: &super::external::SecretPrefixes) -> String {
    let info = index.metadata.tool_info.as_ref();
    let name = sanitize_label(info.map(|i| i.name.as_str()).unwrap_or(""), prefixes);
    let version = sanitize_label(info.map(|i| i.version.as_str()).unwrap_or(""), prefixes);
    match (name.is_empty(), version.is_empty()) {
        (true, _) => "an unnamed indexer".to_string(),
        (false, true) => name,
        (false, false) => format!("{name} {version}"),
    }
}

/// Longest a tool name or version may be before it is cut. Long enough for every
/// real one, short enough that a crafted index cannot pad the summary sentence.
const MAX_LABEL: usize = 60;

/// Make one field of the index's self-description safe to write into an artifact.
///
/// This string is the ONE thing in the corpus that comes from the index rather
/// than from the repository, and it reaches `coverage.json` and a module card.
/// It is therefore held to the same rule as any other text RA publishes, and
/// what comes out is NOT the raw field: control characters become spaces (they
/// would break the one-line summary), runs of whitespace collapse, the length is
/// capped, and the R22 filter runs — a secret pasted into a tool name is a
/// secret in an artifact exactly like one pasted into a doc comment.
fn sanitize_label(raw: &str, prefixes: &super::external::SecretPrefixes) -> String {
    let flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let (flat, redacted) = super::external::redact_secret_block(&flat, prefixes);
    if redacted {
        return "an indexer whose name matched a secret pattern".to_string();
    }
    flat.chars().take(MAX_LABEL).collect()
}

fn resolved_prov(file: &str, line: Option<u32>) -> Provenance {
    Provenance::new(file, line, ids::scip(), Tier::Measured).resolved()
}

/// Map an index document path onto a tracked-file graph key.
///
/// Only the separators are normalized; `project_root` from the index metadata is
/// deliberately never consulted, because it records an absolute path on the
/// machine that ran the indexer. A path that names nothing git tracks is reported,
/// not guessed at — the usual cause is an index produced from a different working
/// directory, and that must diagnose itself (E9).
fn repo_key(relative_path: &str, tracked: &BTreeSet<String>) -> Option<String> {
    let normalized = relative_path.replace('\\', "/");
    tracked.contains(&normalized).then_some(normalized)
}

/// One declaration the grammar found, with the lines it spans.
struct Decl {
    key: NodeKey,
    name: String,
    start: u32,
    end: u32,
    /// Where its name is written, `(line, column)`, when its plugin reports it
    /// (contract 0.2) — what tells two declarations of one name on one line
    /// apart (plan A, KTD8).
    name_at: Option<(u32, u32)>,
}

/// The graph's tree-sitter declarations, indexed per file — the authority this
/// extractor checks the index against, and the thing a reference site is
/// attributed to.
#[derive(Default)]
struct Decls {
    by_file: BTreeMap<String, Vec<Decl>>,
}

impl Decls {
    fn build(graph: &Graph) -> Decls {
        let mut decls = Decls::default();
        for node in graph.nodes_sorted() {
            if node.kind != NodeKind::Symbol {
                continue;
            }
            // One symbol can be declared in SEVERAL files: a C# partial type, a
            // Rust inherent impl split in two. KTD3 merges those into one node,
            // and `SymbolName` is scalar, so every site but the winning one
            // survives as dissent. Reading the winner alone would leave the
            // other files with no declaration at all — the drift gate would then
            // report each of them as a mismatch and refuse the whole file's
            // resolved facts, and a reference inside them would find no owner.
            for fact in &node.facts {
                if fact.kind != FactKind::SymbolName {
                    continue;
                }
                let sites = std::iter::once((&fact.value, &fact.provenance))
                    .chain(fact.dissent.iter().map(|c| (&c.value, &c.provenance)));
                for (value, prov) in sites {
                    let FactValue::Text(name) = value else {
                        continue;
                    };
                    let Some(start) = prov.span.start() else {
                        continue;
                    };
                    let name_at = graph.sites().declaration_name(&prov.file, &node.key);
                    decls
                        .by_file
                        .entry(prov.file.clone())
                        .or_default()
                        .push(Decl {
                            key: node.key.clone(),
                            name: name.clone(),
                            start,
                            end: prov.span.end().unwrap_or(start),
                            name_at,
                        });
                }
            }
        }
        // Innermost-first on a backwards scan: start ascending, and for equal
        // starts the WIDER range first, so the narrower (nested) one is met later.
        // `key` and `name` only break remaining ties, keeping the order total so
        // the dedup below removes the same claim restated by two extractors.
        for decls in decls.by_file.values_mut() {
            decls.sort_by(|a, b| {
                (a.start, std::cmp::Reverse(a.end), &a.key, &a.name).cmp(&(
                    b.start,
                    std::cmp::Reverse(b.end),
                    &b.key,
                    &b.name,
                ))
            });
            decls.dedup_by(|a, b| {
                (a.start, a.end, &a.key, &a.name) == (b.start, b.end, &b.key, &b.name)
            });
        }
        decls
    }

    fn in_file(&self, file: &str) -> &[Decl] {
        self.by_file.get(file).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The innermost declaration whose range contains `line`.
    ///
    /// Called once per reference occurrence, and a generated C# file carries
    /// thousands of both, so the scan starts at the last declaration that could
    /// possibly contain `line` rather than at the end of the file. Everything
    /// after that point starts below the line and can never match.
    fn owner(&self, file: &str, line: u32) -> Option<&NodeKey> {
        self.enclosing(file, line).next().map(|d| &d.key)
    }

    /// The declaration named `name` that most tightly encloses `line`.
    ///
    /// Innermost, not first: `mod parse { fn parse() }` gives two declarations of
    /// one name at one line, and the outer one is the wrong node to hang the
    /// index's moniker on.
    fn innermost_named(&self, file: &str, name: &str, line: u32) -> Option<&Decl> {
        self.enclosing(file, line).find(|d| d.name == name)
    }

    /// The declaration an index definition of `name` starting at `(line,
    /// column)` names (plan A, KTD8).
    ///
    /// [`Decls::innermost_named`] when one declaration is innermost. Two of
    /// that name spanning the very same lines — two overloads written on one
    /// line — are told apart only by where each writes its name, which a 0.2
    /// plugin reports and the index's definition starts at. Without that
    /// column the definition is [`DefinitionMatch::Tied`]: handing it to
    /// whichever sorts first would pin the moniker, and every occurrence of
    /// it, on a declaration the index may not mean.
    ///
    /// `column` is asked only for a tie: converting it means reading the
    /// document's text ([`Source`]), which almost no definition needs.
    fn definition(
        &self,
        file: &str,
        name: &str,
        line: u32,
        column: impl FnOnce() -> Option<u32>,
    ) -> DefinitionMatch<'_> {
        let mut named = self.enclosing(file, line).filter(|d| d.name == name);
        let Some(first) = named.next() else {
            return DefinitionMatch::None;
        };
        // Equal ranges sort next to each other, so the ties follow at once.
        let tied: Vec<&Decl> = std::iter::once(first)
            .chain(named.take_while(|d| (d.start, d.end) == (first.start, first.end)))
            .collect();
        if let [only] = tied.as_slice() {
            return DefinitionMatch::Node(&only.key);
        }
        column()
            .and_then(|column| tied.iter().find(|d| d.name_at == Some((line, column))))
            .map_or(DefinitionMatch::Tied, |d| DefinitionMatch::Node(&d.key))
    }

    /// Every declaration containing `line`, innermost first.
    ///
    /// The scan starts at the last declaration that could possibly contain the
    /// line rather than at the end of the file — this runs once per reference
    /// occurrence, and a generated C# file carries thousands of both.
    fn enclosing(&self, file: &str, line: u32) -> impl Iterator<Item = &Decl> {
        let decls = self.in_file(file);
        let reachable = decls.partition_point(|d| d.start <= line);
        decls[..reachable]
            .iter()
            .rev()
            .filter(move |d| line <= d.end)
    }
}

/// What a definition occurrence matched among the grammar's declarations.
enum DefinitionMatch<'d> {
    Node(&'d NodeKey),
    /// Several declarations of the name span the same lines and no column
    /// told them apart.
    Tied,
    None,
}

/// A moniker's package — `(manager, name, version)`, with SCIP's `.`
/// placeholder read as empty. `scip-dotnet` writes `. .` for every symbol
/// defined in the indexed sources and a real assembly name and version for
/// one read from metadata (measured, U10).
type Package = (String, String, String);

/// One moniker, parsed.
struct ParsedMoniker {
    /// The declaration name it ends in, for the descriptor kinds a grammar
    /// also declares. `None` for a local, a parameter, or a namespace —
    /// nothing this extractor can attribute to a Symbol node.
    name: Option<String>,
    /// `None` for a local, or a moniker that does not parse.
    package: Option<Package>,
}

/// Parsed monikers, memoized. `scip-dotnet` repeats the same symbol string across
/// hundreds of thousands of occurrences, and parsing each one is measurable.
#[derive(Default)]
struct MonikerCache {
    parsed: BTreeMap<String, ParsedMoniker>,
}

impl MonikerCache {
    fn parsed(&mut self, moniker: &str) -> &ParsedMoniker {
        if !self.parsed.contains_key(moniker) {
            self.parsed.insert(
                moniker.to_string(),
                ParsedMoniker {
                    name: parse_declaration_name(moniker),
                    package: parse_package(moniker),
                },
            );
        }
        &self.parsed[moniker]
    }

    fn declaration_name(&mut self, moniker: &str) -> Option<&str> {
        self.parsed(moniker).name.as_deref()
    }
}

/// The package of a non-local moniker.
fn parse_package(moniker: &str) -> Option<Package> {
    if scip::symbol::is_local_symbol(moniker) {
        return None;
    }
    let symbol = scip::symbol::parse_symbol(moniker).ok()?;
    let package = symbol.package.into_option().unwrap_or_default();
    Some((package.manager, package.name, package.version))
}

/// The packages of the repository's own projects: those of every symbol the
/// index DEFINES, in any document — a symbol defined there was compiled from
/// the repository's sources (KTD8). In `ra index`'s default mode that is the
/// one package `. .`; under `--allow-global-symbol-definitions` it would be one
/// per project, and reading them off the definitions keeps both right.
fn repository_packages(index: &Index, monikers: &mut MonikerCache) -> BTreeSet<Package> {
    let mut packages = BTreeSet::new();
    for document in &index.documents {
        for occurrence in &document.occurrences {
            if is_definition(occurrence.symbol_roles) {
                if let Some(package) = &monikers.parsed(&occurrence.symbol).package {
                    packages.insert(package.clone());
                }
            }
        }
    }
    packages
}

/// The verified definitions, moniker → node, with the monikers that cannot be
/// pinned on one node taken out (plan A, KTD8).
///
/// Two ways a moniker loses its node. Two projects that declare a type of the
/// same fully qualified name give ONE symbol with two definitions in different
/// documents, which the moniker cannot tell apart (measured, U10) — a partial
/// type's definitions all land on one node and are not that case. And a
/// definition among tied declarations that no column decided.
#[derive(Default)]
struct Definitions {
    nodes: BTreeMap<String, NodeKey>,
    refused: BTreeSet<String>,
}

impl Definitions {
    /// One verified definition of `moniker`: on `key`, or tied (`None`).
    fn add(&mut self, moniker: &str, key: Option<&NodeKey>) {
        match (key, self.nodes.get(moniker)) {
            (None, _) => {
                self.refused.insert(moniker.to_string());
            }
            (Some(key), Some(held)) if held != key => {
                self.refused.insert(moniker.to_string());
            }
            (Some(_), Some(_)) => {}
            (Some(key), None) => {
                self.nodes.insert(moniker.to_string(), key.clone());
            }
        }
    }

    /// The moniker → node map every emission and the pairing read.
    fn settle(mut self) -> BTreeMap<String, NodeKey> {
        for moniker in &self.refused {
            self.nodes.remove(moniker);
        }
        self.nodes
    }
}

/// Whether the merged index marks an occurrence's range as one two projects
/// disagree about (`ra index`, KTD9).
fn is_conflicting(occurrence: &scip::types::Occurrence) -> bool {
    occurrence
        .diagnostics
        .iter()
        .any(|d| d.code == crate::index::CONFLICT_CODE)
}

/// Whether `package` is from outside the repository: a real package name that
/// no project of the repository defines anything in (KTD8).
fn is_external(package: &Package, packages: &BTreeSet<Package>) -> bool {
    !package.1.is_empty() && !packages.contains(package)
}

/// What each conflicting range of `document` says, by range: `true` when
/// every target the projects wrote there is from outside the repository.
///
/// The merged index keeps every project's occurrence at such a range, so the
/// occurrences there ARE the targets the projects disagree on. When each of
/// them is external — `System/String#Trim().` from net10 against
/// `System/String#Trim(+1).` from .NET Framework, the overload numbering
/// differing between the two frameworks (measured on the reference C# repository, 35 393 of
/// the 55 161 ranges left after U10) — every project agrees the call leaves
/// the repository, which is the one thing pairing reads off an external
/// target. A repository target among them (a polyfill one framework compiles
/// itself) is a real disagreement and answers nothing.
fn conflicting_ranges<'d>(
    document: &'d scip::types::Document,
    packages: &BTreeSet<Package>,
    monikers: &mut MonikerCache,
) -> HashMap<&'d [i32], bool> {
    let mut ranges: HashMap<&[i32], bool> = HashMap::new();
    for occurrence in &document.occurrences {
        if !is_conflicting(occurrence) {
            continue;
        }
        let external = monikers
            .parsed(&occurrence.symbol)
            .package
            .as_ref()
            .is_some_and(|package| is_external(package, packages));
        *ranges.entry(occurrence.range.as_slice()).or_insert(true) &= external;
    }
    ranges
}

/// A verified document's reference occurrences as the pairing reads them
/// (plan A, U9): where each starts, the simple name its moniker ends in, and
/// what its symbol is.
///
/// Every one that names something a usage site could name — the external ones
/// too, which nothing kept before: a call the compiler bound to
/// `System.String` is the verdict that refutes every repository candidate.
/// "Could name" is decided by `places`, the `(line, name)` of every usage
/// site in the document's file: an occurrence no site was written at is never
/// looked up, and all those that share a site's line and name are kept, so
/// "several on one line" still leaves the site unmapped.
///
/// A conflicting range keeps ONE occurrence per name, whatever number of
/// targets the projects wrote there: [`OccurrenceTarget::External`] when every
/// one of them is external ([`conflicting_ranges`]),
/// [`OccurrenceTarget::Unresolved`] otherwise.
///
/// `column` converts a range's start into the plugin's column
/// ([`Source::column`]); an occurrence whose column cannot be converted is
/// kept with none, which no site with a column pairs with.
fn kept_occurrences(
    document: &scip::types::Document,
    definitions: &BTreeMap<String, NodeKey>,
    packages: &BTreeSet<Package>,
    monikers: &mut MonikerCache,
    places: &HashSet<(u32, &str)>,
    column: &dyn Fn(&[i32]) -> Option<u32>,
) -> Vec<KeptOccurrence> {
    if places.is_empty() {
        return Vec::new();
    }
    let conflicts = conflicting_ranges(document, packages, monikers);
    let mut conflicts_kept: HashSet<(&[i32], String)> = HashSet::new();
    let mut kept = Vec::new();
    for (at, occurrence) in document.occurrences.iter().enumerate() {
        if is_definition(occurrence.symbol_roles) {
            continue;
        }
        let Some(line) = start_line(&occurrence.range) else {
            continue;
        };
        let parsed = monikers.parsed(&occurrence.symbol);
        let (Some(name), Some(package)) = (&parsed.name, &parsed.package) else {
            continue;
        };
        if !places.contains(&(line, name.as_str())) {
            continue;
        }
        let target = if let Some(&all_external) = conflicts.get(occurrence.range.as_slice()) {
            if !conflicts_kept.insert((occurrence.range.as_slice(), name.clone())) {
                continue;
            }
            if all_external {
                OccurrenceTarget::External
            } else {
                OccurrenceTarget::Unresolved
            }
        } else if let Some(key) = definitions.get(&occurrence.symbol) {
            OccurrenceTarget::Repository(key.clone())
        } else if is_external(package, packages) {
            OccurrenceTarget::External
        } else {
            // The repository's own package, and no verified definition: it sits
            // in a drifted or unverifiable document, the grammar declares
            // nothing there, or it is one of the refused monikers.
            OccurrenceTarget::Unresolved
        };
        let name = name.clone();
        kept.push(KeptOccurrence {
            at,
            line,
            column: column(&occurrence.range),
            name,
            target,
        });
    }
    kept
}

/// The `(line, name)` of every usage site, by file — where an index
/// occurrence must be written for a site to pair with it.
fn site_places(records: &[crate::graph::SiteRecord]) -> HashMap<&str, HashSet<(u32, &str)>> {
    let mut places: HashMap<&str, HashSet<(u32, &str)>> = HashMap::new();
    for record in records {
        places
            .entry(record.file.as_str())
            .or_default()
            .insert((record.line, record.name.as_str()));
    }
    places
}

/// How a document counts the columns of its ranges (SCIP's
/// `PositionEncoding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnUnit {
    Utf8Bytes,
    Utf16CodeUnits,
    /// UTF-32 code units: Unicode scalar values, which is what the plugin
    /// counts on any well-formed line.
    Scalars,
}

impl ColumnUnit {
    /// The unit `document` declares. An undeclared one is UTF-16: every
    /// document `scip-dotnet` 0.2.14 wrote for the reference C# repository leaves the field
    /// unset (10 529 of 10 529, 03.10.2026), and Roslyn's `LinePosition`
    /// counts UTF-16 code units. `None` for a value this build does not know,
    /// whose columns then convert to nothing.
    fn of(document: &scip::types::Document) -> Option<ColumnUnit> {
        use scip::types::PositionEncoding;
        match document.position_encoding.enum_value() {
            Ok(PositionEncoding::UTF8CodeUnitOffsetFromLineStart) => Some(ColumnUnit::Utf8Bytes),
            Ok(PositionEncoding::UTF16CodeUnitOffsetFromLineStart)
            | Ok(PositionEncoding::UnspecifiedPositionEncoding) => Some(ColumnUnit::Utf16CodeUnits),
            Ok(PositionEncoding::UTF32CodeUnitOffsetFromLineStart) => Some(ColumnUnit::Scalars),
            Err(_) => None,
        }
    }
}

/// A document's source text, read from its blob the first time a column has
/// to be converted, for turning the index's columns into the plugin's.
///
/// The plugin counts characters (contract 0.2); SCIP counts in the
/// document's position encoding. Compared raw, the two disagree after every
/// character outside the Basic Multilingual Plane, and two calls of one name
/// on one line can then swap: `/*😀😀😀😀😀😀*/a.F();b.F();` has its calls at
/// characters 13 and 19 and at UTF-16 units 19 and 25, so the second site
/// would pair with the first call's occurrence and claim its target at the
/// index's precision.
struct Source<'r> {
    repo: &'r Repo,
    file: Option<&'r TrackedFile>,
    unit: Option<ColumnUnit>,
    lines: OnceCell<Option<Lines>>,
}

/// A file's bytes and where each of its lines starts.
struct Lines {
    bytes: Vec<u8>,
    /// Byte offset of each line's start. Lines end at `\n`, as tree-sitter's
    /// rows do — the rows the plugin's lines and columns count.
    starts: Vec<usize>,
}

impl<'r> Source<'r> {
    fn new(
        repo: &'r Repo,
        file: Option<&'r TrackedFile>,
        document: &scip::types::Document,
    ) -> Source<'r> {
        Source {
            repo,
            file,
            unit: ColumnUnit::of(document),
            lines: OnceCell::new(),
        }
    }

    /// The 1-based column, counted as the plugin counts it, where `range`
    /// starts. `None` when the range has no start column, or when it cannot
    /// be converted: the file is not readable as source, its line is not
    /// well-formed UTF-8, or the column falls inside a character or past the
    /// line's end. Such an occurrence pairs with no site that has a column.
    fn column(&self, range: &[i32]) -> Option<u32> {
        let line = start_line(range)?;
        let units = start_column(range)? - 1;
        let unit = self.unit?;
        if unit == ColumnUnit::Scalars {
            return Some(units + 1);
        }
        let lines = self.lines.get_or_init(|| self.read()).as_ref()?;
        let at = (line - 1) as usize;
        let start = *lines.starts.get(at)?;
        let end = lines
            .starts
            .get(at + 1)
            .map_or(lines.bytes.len(), |next| next - 1);
        plugin_column(&lines.bytes[start..end], units, unit)
    }

    fn read(&self) -> Option<Lines> {
        let file = self.file?;
        // A symlink's blob is its target's path, not source text; nothing
        // the plugin read lies in it.
        if file.mode == 0o120000 {
            return None;
        }
        let bytes = self.repo.blob_bytes(file).ok()?;
        let starts = std::iter::once(0)
            .chain(
                bytes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'\n')
                    .map(|(i, _)| i + 1),
            )
            .collect();
        Some(Lines { bytes, starts })
    }
}

/// The plugin's 1-based column for a position `units` (0-based) into `line`,
/// counted in `unit`: the characters before it, plus one.
fn plugin_column(line: &[u8], units: u32, unit: ColumnUnit) -> Option<u32> {
    let units = units as usize;
    if unit == ColumnUnit::Utf8Bytes {
        // The plugin counts the bytes that start a character, so a byte
        // offset converts even on a malformed line — but never one inside a
        // character.
        if units > line.len() || line.get(units).is_some_and(|b| b & 0xC0 == 0x80) {
            return None;
        }
        let chars = line[..units].iter().filter(|b| *b & 0xC0 != 0x80).count();
        return Some(chars as u32 + 1);
    }
    let text = std::str::from_utf8(line).ok()?;
    let mut seen = 0usize;
    for (chars, c) in text.chars().enumerate() {
        if seen == units {
            return Some(chars as u32 + 1);
        }
        if seen > units {
            return None;
        }
        seen += match unit {
            ColumnUnit::Utf16CodeUnits => c.len_utf16(),
            _ => 1,
        };
    }
    (seen == units).then(|| text.chars().count() as u32 + 1)
}

/// The simple name a moniker declares, when it declares one a grammar would also
/// see. Parameters, type parameters, locals and namespaces are excluded: no
/// Symbol node exists for them, so pretending otherwise would fabricate a match.
fn parse_declaration_name(moniker: &str) -> Option<String> {
    if scip::symbol::is_local_symbol(moniker) {
        return None;
    }
    let symbol = scip::symbol::parse_symbol(moniker).ok()?;
    let last = symbol.descriptors.last()?;
    match last.suffix.enum_value_or_default() {
        Suffix::Type | Suffix::Method | Suffix::Term | Suffix::Macro => {
            (!last.name.is_empty()).then(|| last.name.clone())
        }
        _ => None,
    }
}

/// Whether an occurrence carries the `Definition` role. A bitset, so it is tested
/// as a mask: an occurrence may carry other roles alongside, and an equality test
/// would silently drop those.
fn is_definition(symbol_roles: i32) -> bool {
    symbol_roles & (SymbolRole::Definition as i32) != 0
}

/// The 1-based start line of an occurrence range. SCIP ranges are 0-based, and
/// both the three- and four-element encodings start with the start line.
fn start_line(range: &[i32]) -> Option<u32> {
    match range.first() {
        Some(line) if *line >= 0 => Some(*line as u32 + 1),
        _ => None,
    }
}

/// The 1-based start column of an occurrence range — its second element in
/// both encodings — in the document's own position encoding, UTF-16 code
/// units for `scip-dotnet`. Never compared with a plugin's column as it is:
/// [`Source::column`] converts it first.
fn start_column(range: &[i32]) -> Option<u32> {
    match range.get(1) {
        Some(column) if range.len() >= 3 && *column >= 0 => Some(*column as u32 + 1),
        _ => None,
    }
}

/// Every definition occurrence in `document` that lands inside a declaration the
/// grammar found for the same name — the match that both verifies a document and
/// gives the moniker its node. `None` beside a moniker whose declarations tied
/// (see [`Decls::definition`]).
fn definition_matches(
    document: &scip::types::Document,
    file: &str,
    decls: &Decls,
    monikers: &mut MonikerCache,
    column: &dyn Fn(&[i32]) -> Option<u32>,
) -> Vec<(String, Option<NodeKey>)> {
    let mut out: Vec<(String, Option<NodeKey>)> = Vec::new();
    for occurrence in &document.occurrences {
        if !is_definition(occurrence.symbol_roles) {
            continue;
        }
        let Some(line) = start_line(&occurrence.range) else {
            continue;
        };
        let Some(name) = monikers.declaration_name(&occurrence.symbol) else {
            continue;
        };
        // Same name AND the index's line inside the grammar's range. Matching on
        // the range rather than on an exact line is what keeps an attribute or a
        // multi-line signature — where the grammar's declaration starts above the
        // identifier — from reading as drift.
        match decls.definition(file, name, line, || column(&occurrence.range)) {
            DefinitionMatch::Node(key) => out.push((occurrence.symbol.clone(), Some(key.clone()))),
            DefinitionMatch::Tied => out.push((occurrence.symbol.clone(), None)),
            DefinitionMatch::None => {}
        }
    }
    out
}

/// What the drift gate concluded about one document.
///
/// `Drifted` and `Unverifiable` both contribute nothing, but they are not the
/// same statement and the coverage summary must not report them as one number:
/// "the index is stale here" and "there was nothing here to check" say opposite
/// things about how far the index can be trusted elsewhere.
enum Verdict {
    /// The index and the grammar agree about every declaration both of them name.
    /// Each definition with its node, or `None` where the declarations tied.
    Verified(Vec<(String, Option<NodeKey>)>),
    /// They disagree. The reason is a fixed label, for the run summary.
    Drifted(&'static str),
    /// They share no declaration name at all, so nothing could be checked.
    Unverifiable,
}

/// Decide whether a document may contribute `resolved` facts, and return its
/// verified moniker → node matches when it may.
///
/// Two checks, one per direction. Forward: every definition the index places for
/// a name the grammar also knows must land inside one of that name's declaration
/// ranges — this catches a declaration that MOVED. Backward: a line where the
/// grammar declares a name the index has never heard of, and where the index
/// nevertheless defines something else, is a declaration RENAMED in place — which
/// the forward check cannot see, because neither name is known to both sides.
///
/// The backward check's honest cost: a compiler-generated definition that sits
/// alone on a real declaration's own start line, with the declaration's name
/// absent from the whole document, is indistinguishable from a rename and refuses
/// the file. That errs toward withholding `resolved` from a file that deserved
/// it, never toward granting it to one that did not — the direction R20 needs.
fn verify(
    document: &scip::types::Document,
    file: &str,
    decls: &Decls,
    monikers: &mut MonikerCache,
    column: &dyn Fn(&[i32]) -> Option<u32>,
) -> Verdict {
    let grammar_names: BTreeSet<&str> = decls
        .in_file(file)
        .iter()
        .map(|d| d.name.as_str())
        .collect();

    // Owned, because `declaration_name` borrows the cache it memoizes into.
    let mut defs: Vec<(u32, String)> = Vec::new();
    for occurrence in &document.occurrences {
        if !is_definition(occurrence.symbol_roles) {
            continue;
        }
        let Some(line) = start_line(&occurrence.range) else {
            continue;
        };
        let Some(name) = monikers.declaration_name(&occurrence.symbol) else {
            continue;
        };
        defs.push((line, name.to_string()));
    }

    let mut checked = 0usize;
    for (line, name) in &defs {
        if !grammar_names.contains(name.as_str()) {
            // The grammar never declared this name here — a property the grammar
            // does not emit, a generated member. Nothing to disagree about.
            continue;
        }
        checked += 1;
        if decls.innermost_named(file, name, *line).is_none() {
            return Verdict::Drifted("declaration moved");
        }
    }

    // The backward direction. A rename in place shifts nothing, so every other
    // declaration still verifies and the file passes — while the index's answers
    // about the renamed one are answers about a symbol that no longer exists.
    let index_names: BTreeSet<&str> = defs.iter().map(|(_, n)| n.as_str()).collect();
    let defined_lines: BTreeSet<u32> = defs.iter().map(|(line, _)| *line).collect();
    for decl in decls.in_file(file) {
        // Only when the index is silent about this name ANYWHERE in the file. A
        // name the index does define elsewhere is a member the grammar and the
        // indexer merely describe at different lines, not a disagreement.
        if index_names.contains(decl.name.as_str()) {
            continue;
        }
        if defined_lines.contains(&decl.start) {
            return Verdict::Drifted("declaration renamed in place");
        }
    }

    if checked == 0 {
        return Verdict::Unverifiable;
    }
    Verdict::Verified(definition_matches(document, file, decls, monikers, column))
}

/// Reference occurrences become `References` edges from the declaration that
/// encloses the site — or from the file, when module-level code encloses it, the
/// same origin rule `Calls`/`References` already use in the heuristic tier.
///
/// The kind is deliberately `References` and never `Calls`: a SCIP occurrence says
/// "this symbol appears here", not "this symbol is invoked here". Emitting `Calls`
/// from it would claim more precision than the index carries, and the heuristic
/// tier already answers the call question (as its own edge kind, so the two never
/// collide in merge).
///
/// An occurrence the pairing folded into a usage site's own relation (`folded`,
/// by file and position in the document) is skipped: that site's `Calls`
/// already says it at the index's precision, and a `References` beside it
/// would state the one relation twice (plan A, KTD8).
fn emit_references(
    graph: &mut Graph,
    document: &scip::types::Document,
    file: &str,
    decls: &Decls,
    definitions: &BTreeMap<String, NodeKey>,
    folded: &BTreeSet<(String, usize)>,
) {
    for (at, occurrence) in document.occurrences.iter().enumerate() {
        if is_definition(occurrence.symbol_roles) {
            continue;
        }
        if folded.contains(&(file.to_string(), at)) {
            continue;
        }
        let Some(line) = start_line(&occurrence.range) else {
            continue;
        };
        // No phantom targets: a moniker whose definition was never verified — it
        // lives in a drifted document, in another index, or outside the repo — gets
        // no edge at all rather than an invented endpoint.
        let Some(target) = definitions.get(&occurrence.symbol) else {
            continue;
        };
        let from = decls
            .owner(file, line)
            .cloned()
            .unwrap_or_else(|| NodeKey::new(file));
        if from.as_str() == target.as_str() {
            continue;
        }
        // The occurrence is the relation's site (plan A, KTD7); merge unions
        // the sites of every occurrence of one relation. Line only, like the
        // name-resolved sites, so the two spell one place the same way until
        // plugin contract 0.2 brings columns to both.
        let prov = resolved_prov(file, Some(line));
        let site = SiteId::of(&prov);
        graph.add_edge(
            &from,
            target,
            Edge::new(EdgeKind::References, prov).at_sites(site),
        );
    }
}

/// `is_implementation` relationships become `Implements` edges — the only
/// relationship kind here that a name heuristic genuinely cannot see (a C# class
/// and the interface it satisfies share no spelling in the general case).
fn emit_implementations(
    graph: &mut Graph,
    document: &scip::types::Document,
    file: &str,
    definitions: &BTreeMap<String, NodeKey>,
) {
    for information in &document.symbols {
        let Some(from) = definitions.get(&information.symbol) else {
            continue;
        };
        for relationship in &information.relationships {
            if !relationship.is_implementation {
                continue;
            }
            let Some(to) = definitions.get(&relationship.symbol) else {
                continue;
            };
            if from.as_str() == to.as_str() {
                continue;
            }
            let (from, to) = (from.clone(), to.clone());
            graph.add_edge(
                &from,
                &to,
                Edge::new(EdgeKind::Implements, resolved_prov(file, None)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_module_never_spawns_a_process() {
        // R9 as a mechanical check over this file's own source, not a promise in
        // a comment: the main run reads an index, it never produces one. Running
        // scip-dotnet belongs to `ra index` (`src/index/`) and must stay there.
        //
        // Only the production half is scanned — the literals below live in this
        // test module, and scanning the whole file would just find them.
        let source = include_str!("scip.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("this file has a test module");
        for forbidden in ["Command", "process::", "Stdio", "spawn("] {
            assert!(
                !production.contains(forbidden),
                "`{forbidden}` appears in the production code — an indexer must never be run"
            );
        }
    }

    #[test]
    fn a_definition_role_is_read_as_a_bitmask() {
        assert!(is_definition(SymbolRole::Definition as i32));
        // Definition together with WriteAccess — an equality test would miss it.
        assert!(is_definition(
            SymbolRole::Definition as i32 | SymbolRole::WriteAccess as i32
        ));
        assert!(!is_definition(SymbolRole::ReadAccess as i32));
        assert!(!is_definition(0));
    }

    #[test]
    fn scip_ranges_are_zero_based_and_become_one_based_lines() {
        assert_eq!(start_line(&[0, 4, 9]), Some(1));
        assert_eq!(start_line(&[41, 8, 41, 20]), Some(42));
        assert_eq!(start_line(&[]), None);
        assert_eq!(start_line(&[-1, 0, 0]), None);
    }

    #[test]
    fn only_grammar_visible_descriptor_kinds_yield_a_name() {
        assert_eq!(
            parse_declaration_name("scip-dotnet nuget . . Orbit/Billing/Invoice#"),
            Some("Invoice".to_string())
        );
        assert_eq!(
            parse_declaration_name("scip-dotnet nuget . . Orbit/Billing/Invoice#Total()."),
            Some("Total".to_string())
        );
        // A parameter has no Symbol node of its own.
        assert_eq!(
            parse_declaration_name(
                "scip-dotnet nuget . . Orbit/Billing/Invoice#Total().(quantity)"
            ),
            None
        );
        // A namespace is not a declaration this graph carries either.
        assert_eq!(
            parse_declaration_name("scip-dotnet nuget . . Orbit/Billing/"),
            None
        );
        assert_eq!(parse_declaration_name("local 4"), None);
    }

    fn one_line_overloads(name_at: [Option<(u32, u32)>; 2]) -> Decls {
        let decl = |key: &str, name_at| Decl {
            key: NodeKey::new(key),
            name: "Total".to_string(),
            start: 5,
            end: 5,
            name_at,
        };
        let mut decls = Decls::default();
        decls.by_file.insert(
            "src/Invoice.cs".to_string(),
            vec![
                decl("sym:csharp:N#Invoice.Total(int)", name_at[0]),
                decl("sym:csharp:N#Invoice.Total(int,int)", name_at[1]),
            ],
        );
        decls
    }

    /// Two overloads written on one line span the same lines; only where each
    /// writes its name tells the index's two definitions apart (plan A, KTD8).
    #[test]
    fn two_declarations_of_one_name_on_one_line_are_told_apart_by_the_name_column() {
        let decls = one_line_overloads([Some((5, 20)), Some((5, 58))]);
        for (column, key) in [
            (20, "sym:csharp:N#Invoice.Total(int)"),
            (58, "sym:csharp:N#Invoice.Total(int,int)"),
        ] {
            assert!(matches!(
                decls.definition("src/Invoice.cs", "Total", 5, || Some(column)),
                DefinitionMatch::Node(k) if k.as_str() == key
            ));
        }
    }

    /// Without the names' columns — a plugin on contract 0.1 — neither
    /// definition is pinned on a node, so the moniker leaves the map and every
    /// occurrence of it stays unmapped rather than paired with a guess.
    #[test]
    fn without_name_columns_two_declarations_on_one_line_map_to_neither() {
        let decls = one_line_overloads([None, None]);
        assert!(matches!(
            decls.definition("src/Invoice.cs", "Total", 5, || Some(20)),
            DefinitionMatch::Tied
        ));

        let mut definitions = Definitions::default();
        definitions.add("scip-dotnet nuget . . N/Invoice#Total().", None);
        let settled = definitions.settle();
        assert!(settled.is_empty());

        let mut document = scip::types::Document::new();
        let mut reference = scip::types::Occurrence::new();
        reference.symbol = "scip-dotnet nuget . . N/Invoice#Total().".to_string();
        reference.range = vec![11, 16, 21];
        document.occurrences = vec![reference];
        let packages: BTreeSet<Package> = [(String::new(), String::new(), String::new())]
            .into_iter()
            .collect();
        let places: HashSet<(u32, &str)> = [(12, "Total")].into_iter().collect();
        let kept = kept_occurrences(
            &document,
            &settled,
            &packages,
            &mut MonikerCache::default(),
            &places,
            &start_column,
        );
        assert!(matches!(
            kept.as_slice(),
            [KeptOccurrence {
                target: OccurrenceTarget::Unresolved,
                ..
            }]
        ));
    }

    /// The index counts in its document's encoding, the plugin in
    /// characters: six astral characters are twelve UTF-16 units and
    /// twenty-four bytes before a call the plugin puts at column 13.
    #[test]
    fn a_scip_column_converts_into_the_plugins_characters() {
        let line = "/*😀😀😀😀😀😀*/a.F();b.F();".as_bytes();
        // The two `F`s at characters 13 and 19 (1-based).
        assert_eq!(
            plugin_column(line, 18, ColumnUnit::Utf16CodeUnits),
            Some(13)
        );
        assert_eq!(
            plugin_column(line, 24, ColumnUnit::Utf16CodeUnits),
            Some(19)
        );
        assert_eq!(plugin_column(line, 30, ColumnUnit::Utf8Bytes), Some(13));
        assert_eq!(plugin_column(line, 12, ColumnUnit::Scalars), Some(13));
        // Czech is one UTF-16 unit and two bytes per letter.
        let czech = "var s = \"žš\"; s.Trim();".as_bytes();
        assert_eq!(
            plugin_column(czech, 16, ColumnUnit::Utf16CodeUnits),
            Some(17)
        );
        assert_eq!(plugin_column(czech, 18, ColumnUnit::Utf8Bytes), Some(17));
    }

    /// A column inside a character, past the line's end, or on a line that
    /// is not UTF-8 names no character, and converts to nothing.
    #[test]
    fn a_scip_column_that_names_no_character_converts_to_nothing() {
        let line = "/*😀*/F();".as_bytes();
        assert_eq!(plugin_column(line, 3, ColumnUnit::Utf16CodeUnits), None);
        assert_eq!(plugin_column(line, 3, ColumnUnit::Utf8Bytes), None);
        assert_eq!(plugin_column(line, 40, ColumnUnit::Utf16CodeUnits), None);
        assert_eq!(plugin_column(line, 40, ColumnUnit::Utf8Bytes), None);
        assert_eq!(
            plugin_column(b"a\xFFb.F();", 4, ColumnUnit::Utf16CodeUnits),
            None
        );
        // The end of the line is a position, the one after it is not.
        assert_eq!(
            plugin_column(b"F();", 4, ColumnUnit::Utf16CodeUnits),
            Some(5)
        );
    }

    /// `scip-dotnet` declares no encoding; it writes UTF-16 units, as
    /// Roslyn counts them.
    #[test]
    fn an_undeclared_position_encoding_is_utf16() {
        use scip::types::PositionEncoding;
        let mut document = scip::types::Document::new();
        assert_eq!(ColumnUnit::of(&document), Some(ColumnUnit::Utf16CodeUnits));
        document.position_encoding = PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
        assert_eq!(ColumnUnit::of(&document), Some(ColumnUnit::Utf8Bytes));
        document.position_encoding = PositionEncoding::UTF32CodeUnitOffsetFromLineStart.into();
        assert_eq!(ColumnUnit::of(&document), Some(ColumnUnit::Scalars));
    }

    fn reference(symbol: &str, range: Vec<i32>) -> scip::types::Occurrence {
        let mut occurrence = scip::types::Occurrence::new();
        occurrence.symbol = symbol.to_string();
        occurrence.range = range;
        occurrence
    }

    fn repository_package() -> BTreeSet<Package> {
        [("nuget".to_string(), String::new(), String::new())]
            .into_iter()
            .collect()
    }

    /// Only an occurrence written on a usage site's line under its name is
    /// kept for the pairing; every one sharing that place is, so two on one
    /// line still leave a column-less site unmapped.
    #[test]
    fn an_occurrence_at_no_site_place_is_not_kept() {
        let mut document = scip::types::Document::new();
        document.occurrences = vec![
            reference("scip-dotnet nuget . . N/A#Run().", vec![4, 8, 11]),
            reference("scip-dotnet nuget . . N/B#Run().", vec![4, 20, 23]),
            // Same line, another name; another line, the same name.
            reference("scip-dotnet nuget . . N/A#Stop().", vec![4, 30, 34]),
            reference("scip-dotnet nuget . . N/A#Run().", vec![9, 8, 11]),
        ];
        let places: HashSet<(u32, &str)> = [(5, "Run")].into_iter().collect();
        let kept = kept_occurrences(
            &document,
            &BTreeMap::new(),
            &repository_package(),
            &mut MonikerCache::default(),
            &places,
            &start_column,
        );
        let at: Vec<(usize, u32, Option<u32>)> =
            kept.iter().map(|o| (o.at, o.line, o.column)).collect();
        assert_eq!(at, [(0, 5, Some(9)), (1, 5, Some(21))]);
    }

    fn conflicting(symbol: &str) -> scip::types::Occurrence {
        let mut occurrence = reference(symbol, vec![6, 12, 16]);
        let mut diagnostic = scip::types::Diagnostic::new();
        diagnostic.code = crate::index::CONFLICT_CODE.to_string();
        occurrence.diagnostics = vec![diagnostic];
        occurrence
    }

    /// Two frameworks number `String.Trim`'s overloads differently, so the
    /// merged index keeps both targets at the call's range. Both leave the
    /// repository, and that is all the pairing reads: one external
    /// occurrence. A repository target among them stays a disagreement.
    #[test]
    fn a_range_whose_conflicting_targets_are_all_external_is_one_external_occurrence() {
        let places: HashSet<(u32, &str)> = [(7, "Trim")].into_iter().collect();
        let kept_for = |occurrences| {
            let mut document = scip::types::Document::new();
            document.occurrences = occurrences;
            kept_occurrences(
                &document,
                &BTreeMap::new(),
                &repository_package(),
                &mut MonikerCache::default(),
                &places,
                &start_column,
            )
        };

        let external = kept_for(vec![
            conflicting("scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim()."),
            conflicting("scip-dotnet nuget mscorlib 4.0.0.0 System/String#Trim(+1)."),
        ]);
        assert!(
            matches!(
                external.as_slice(),
                [KeptOccurrence {
                    target: OccurrenceTarget::External,
                    ..
                }]
            ),
            "one external occurrence for the range"
        );

        let mixed = kept_for(vec![
            conflicting("scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim()."),
            conflicting("scip-dotnet nuget . . Orbit/Polyfill#Trim()."),
        ]);
        assert!(matches!(
            mixed.as_slice(),
            [KeptOccurrence {
                target: OccurrenceTarget::Unresolved,
                ..
            }]
        ));
    }

    /// Two projects declaring one fully qualified name give one moniker with
    /// two definitions on two nodes (measured, U10): it is pinned on neither.
    /// A partial type's definitions all land on one node and keep it.
    #[test]
    fn a_moniker_defined_on_two_nodes_is_pinned_on_neither() {
        let (a, b) = (NodeKey::new("sym:A"), NodeKey::new("sym:B"));
        let mut definitions = Definitions::default();
        definitions.add("program", Some(&a));
        definitions.add("program", Some(&b));
        definitions.add("partial", Some(&a));
        definitions.add("partial", Some(&a));
        let settled = definitions.settle();
        assert_eq!(settled.get("program"), None);
        assert_eq!(settled.get("partial"), Some(&a));
    }

    /// The package tells a symbol from outside the repository: `. .` is what
    /// `scip-dotnet` writes for the indexed sources, a real assembly for
    /// metadata. A local has none.
    #[test]
    fn a_metadata_symbol_carries_its_assembly_as_its_package() {
        assert_eq!(
            parse_package("scip-dotnet nuget . . B/Greeter#Hello()."),
            Some(("nuget".to_string(), String::new(), String::new()))
        );
        assert_eq!(
            parse_package("scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim()."),
            Some((
                "nuget".to_string(),
                "System.Runtime".to_string(),
                "10.0.0.0".to_string()
            ))
        );
        assert_eq!(parse_package("local 4"), None);
    }

    #[test]
    fn a_document_path_resolves_only_against_a_tracked_file() {
        let tracked: BTreeSet<String> = ["src/Invoice.cs".to_string()].into_iter().collect();
        // Backslashes normalize — that is how a Windows indexer writes them.
        assert_eq!(
            repo_key("src\\Invoice.cs", &tracked),
            Some("src/Invoice.cs".to_string())
        );
        // A path from a different working directory names nothing tracked, and is
        // reported rather than guessed at.
        assert_eq!(repo_key("Project\\src\\Invoice.cs", &tracked), None);
    }

    #[test]
    fn a_tool_label_is_never_blank() {
        let mut index = Index::new();
        assert_eq!(
            tool_label(&index, &crate::extract::external::SecretPrefixes::default()),
            "an unnamed indexer"
        );

        let mut info = scip::types::ToolInfo::new();
        info.name = "scip-dotnet".to_string();
        index.metadata.mut_or_insert_default().tool_info = Some(info.clone()).into();
        assert_eq!(
            tool_label(&index, &crate::extract::external::SecretPrefixes::default()),
            "scip-dotnet"
        );

        info.version = "0.2.14".to_string();
        index.metadata.mut_or_insert_default().tool_info = Some(info).into();
        assert_eq!(
            tool_label(&index, &crate::extract::external::SecretPrefixes::default()),
            "scip-dotnet 0.2.14"
        );
    }
}
