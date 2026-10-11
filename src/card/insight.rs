//! What the graph says once its edges are read at the grain a reader thinks in.
//!
//! The extractors emit relations between the finest things they can see: a
//! method calls a method, a field names a type, a file imports a file. That is
//! the right grain for evidence and the wrong one for understanding. A reader
//! asks "which types does `SessionManager` work with" and "which modules does
//! `src/runtime` depend on" — questions about TYPES and MODULES, not about the
//! 40 individual call sites that answer them.
//!
//! This module computes that answer once per run, from the merged graph, so
//! every card and every architecture page reads the same aggregation:
//!
//! - every symbol is assigned its **unit** — the nearest enclosing type that owns
//!   a card, or, for a free function, the file that declares it; a file is its
//!   own unit;
//! - every unit is assigned its **module** — the directory its file sits in;
//! - every relation between two different units is counted once per edge, by
//!   kind, into a unit-to-unit and a module-to-module table.
//!
//! Nothing here is inferred beyond that lifting. Every counted edge is also
//! counted by the rung it stands on (plan A, R1): an edge the extractor marked
//! ambiguous (a heuristic name match with several candidates) is counted AS
//! ambiguous, so a reader is never shown a dependency that rests only on a guess
//! without being told so, and a name match nothing backed is counted apart from
//! a relation an index or a rule backs.
//!
//! Every table is a `BTreeMap` keyed by identity: the output order depends on
//! the graph's content alone (R18).

use super::keywords::{Keyword, KeywordIndex, KeywordIndexBuilder};
use crate::extract::external::REDACTION_MARKER;
use crate::extract::parent_key;
use crate::extract::treesitter::{symbol_role, Role};
use crate::graph::{
    AnchorText, Confidence, DbAccessKind, EdgeKind, FactKind, FactValue, Graph, Node, NodeKind,
    Span, StructuralRole,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Relation counts between two units (or two modules), by kind.
///
/// The four rung counters partition the same relations a second way, by the
/// [`Confidence`] each edge stands on (plan A, R1, KTD13) — a subset each, not
/// further kinds. An edge with no rung at all (an import, or anything else no
/// name resolution produced) is in none of them; [`Counts::unrated`] counts it,
/// and it reads as it did before rungs existed: certain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub calls: u32,
    pub references: u32,
    pub instantiates: u32,
    pub extends: u32,
    pub implements: u32,
    pub imports: u32,
    pub resolved_by_index: u32,
    pub rules_backed: u32,
    pub name_only: u32,
    pub ambiguous: u32,
}

impl Counts {
    fn add(&mut self, kind: EdgeKind, rung: Option<Confidence>) {
        match kind {
            EdgeKind::Calls => self.calls += 1,
            EdgeKind::References => self.references += 1,
            EdgeKind::Instantiates => self.instantiates += 1,
            EdgeKind::Extends => self.extends += 1,
            EdgeKind::Implements => self.implements += 1,
            EdgeKind::Imports => self.imports += 1,
            _ => return,
        }
        match rung {
            Some(Confidence::ResolvedByIndex) => self.resolved_by_index += 1,
            Some(Confidence::RulesBacked) => self.rules_backed += 1,
            Some(Confidence::NameOnly) => self.name_only += 1,
            Some(Confidence::Ambiguous) => self.ambiguous += 1,
            None => {}
        }
    }

    pub(crate) fn merge(&mut self, other: &Counts) {
        self.calls += other.calls;
        self.references += other.references;
        self.instantiates += other.instantiates;
        self.extends += other.extends;
        self.implements += other.implements;
        self.imports += other.imports;
        self.resolved_by_index += other.resolved_by_index;
        self.rules_backed += other.rules_backed;
        self.name_only += other.name_only;
        self.ambiguous += other.ambiguous;
    }

    /// Every counted relation, ambiguous ones included.
    pub fn total(&self) -> u32 {
        self.calls
            + self.references
            + self.instantiates
            + self.extends
            + self.implements
            + self.imports
    }

    /// The relations that carry no rung — counted by kind, never rated.
    pub fn unrated(&self) -> u32 {
        self.total() - self.resolved_by_index - self.rules_backed - self.name_only - self.ambiguous
    }

    /// The relations something backs: an index resolved them or a rule did —
    /// plus the unrated ones, which keep the reading they had before rungs
    /// existed. A name match nothing backed is NOT here, because its real
    /// target may lie outside the repository (plan A, R2).
    pub fn certain(&self) -> u32 {
        self.resolved_by_index + self.rules_backed + self.unrated()
    }

    /// The relations nothing disputes — the certain ones and the name-only
    /// ones. This is what the main lists hold (plan A, KTD2): only an
    /// ambiguous relation leaves them, so every consumer asking "does this
    /// relation stand" reads this, and only the diagram's line style asks the
    /// stricter [`Counts::certain`].
    pub fn undisputed(&self) -> u32 {
        self.total() - self.ambiguous
    }

    /// `calls 5 · references 2 · creates 1`, most frequent kind first, ties in a
    /// fixed kind order; `(2 ambiguous)` appended when any were.
    pub fn describe(&self) -> String {
        let mut parts: Vec<(u32, usize, &str)> = [
            (self.extends, "extends"),
            (self.implements, "implements"),
            (self.calls, "calls"),
            (self.instantiates, "creates"),
            (self.references, "references"),
            (self.imports, "imports"),
        ]
        .into_iter()
        .enumerate()
        .filter(|(_, (n, _))| *n > 0)
        .map(|(i, (n, word))| (n, i, word))
        .collect();
        parts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut s = parts
            .iter()
            .map(|(n, _, word)| format!("{word} {n}"))
            .collect::<Vec<_>>()
            .join(" · ");
        if self.ambiguous > 0 {
            s.push_str(&format!(" ({} ambiguous)", self.ambiguous));
        }
        s
    }

    /// The single word that best names this relationship in a diagram edge.
    pub fn dominant_word(&self) -> &'static str {
        if self.extends > 0 {
            return "extends";
        }
        if self.implements > 0 {
            return "implements";
        }
        let options = [
            (self.calls, "calls"),
            (self.instantiates, "creates"),
            (self.references, "uses"),
            (self.imports, "imports"),
        ];
        let mut best = ("uses", 0);
        for (n, word) in options {
            if n > best.1 {
                best = (word, n);
            }
        }
        best.0
    }
}

/// The relation kinds this aggregation reads. Containment and declaration are
/// the tree itself, `DependsOn` is manifest-level and `Uses` reaches outside the
/// repository; each has its own section elsewhere.
fn counted(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Calls
            | EdgeKind::References
            | EdgeKind::Instantiates
            | EdgeKind::Extends
            | EdgeKind::Implements
            | EdgeKind::Imports
    )
}

/// One external the code reaches, and how often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalUse {
    pub identity: String,
    pub label: String,
    pub kind: Option<String>,
    pub uses: u32,
}

/// The run-wide aggregation every card and page reads.
#[derive(Debug, Default)]
pub struct Insight {
    /// Any symbol or file key → its unit key.
    unit_of: HashMap<String, String>,
    /// Symbol key → the file that declares it.
    file_of: HashMap<String, String>,
    /// Unit key → module key.
    module_of: HashMap<String, String>,
    /// File key → its LOC, as the metrics extractor measured it.
    file_loc: HashMap<String, i64>,
    /// Module key → its recursive LOC (Derived by the metrics extractor).
    module_loc: HashMap<String, i64>,
    /// Module key → the LOC of the files directly in it.
    module_own_loc: HashMap<String, i64>,
    /// Unit → unit it reaches → how.
    pub unit_out: BTreeMap<String, BTreeMap<String, Counts>>,
    /// Unit → unit that reaches it → how.
    pub unit_in: BTreeMap<String, BTreeMap<String, Counts>>,
    /// Module → module it depends on → how.
    pub module_out: BTreeMap<String, BTreeMap<String, Counts>>,
    /// Module → module that depends on it → how.
    pub module_in: BTreeMap<String, BTreeMap<String, Counts>>,
    /// Base type → types that extend or implement it.
    pub derived: BTreeMap<String, BTreeSet<String>>,
    /// Unit → the externals it reaches.
    pub unit_externals: BTreeMap<String, BTreeMap<String, u32>>,
    /// Module → the externals its units reach.
    pub module_externals: BTreeMap<String, BTreeMap<String, u32>>,
    /// External key → label and kind.
    externals: HashMap<String, (String, Option<String>)>,
    /// Module key → the units whose file sits directly in it.
    pub module_units: BTreeMap<String, BTreeSet<String>>,
    /// Module key → files directly in it.
    pub module_files: BTreeMap<String, BTreeSet<String>>,
    /// Module key → its direct sub-modules.
    pub module_children: BTreeMap<String, BTreeSet<String>>,
    /// Files the naming or tree convention marks as tests.
    test_files: BTreeSet<String>,
    /// Files at least one declaration was read from.
    declaring_files: BTreeSet<String>,
    /// Unit → one-sentence summary from its own doc comment.
    summaries: HashMap<String, String>,
    /// Unit → readable label.
    labels: HashMap<String, String>,
    /// Unit → declared kind word (`class`, `interface`, …); files have none.
    kinds: HashMap<String, String>,
    /// Type unit → its line span, when the extractor recorded one.
    spans: HashMap<String, (u32, u32)>,
    /// Every unit's words and their document frequencies (plan B, U6).
    keywords: KeywordIndex,
    /// Unit → the database objects it and its members reach, and how.
    db_accesses: HashMap<String, BTreeSet<(DbAccessKind, String)>>,
    /// Unit → the names of the entry points among it and its members.
    entry_points: HashMap<String, BTreeSet<String>>,
}

impl Insight {
    /// Aggregate the merged graph. One pass over the nodes and one over the
    /// edges; every lookup after that is a hash probe.
    pub fn build(graph: &Graph) -> Insight {
        let vocabulary = graph.vocabulary();
        let nodes: HashMap<&str, &Node> = graph
            .nodes_sorted()
            .into_iter()
            .map(|n| (n.key.as_str(), n))
            .collect();
        let edges = graph.edges_sorted();

        let mut insight = Insight::default();

        // Owners first: `HasMember` gives each member its nearest owner, and a
        // member of a nested type climbs until it meets a type that owns a card.
        let mut owner: HashMap<&str, &str> = HashMap::new();
        for e in &edges {
            match e.edge.kind {
                EdgeKind::HasMember => {
                    owner.insert(e.to.as_str(), e.from.as_str());
                }
                EdgeKind::Declares => {
                    insight
                        .file_of
                        .insert(e.to.as_str().to_string(), e.from.as_str().to_string());
                }
                EdgeKind::Contains => {
                    let child = e.to.as_str();
                    match nodes.get(child).map(|n| n.kind) {
                        Some(NodeKind::File) => {
                            insight
                                .module_files
                                .entry(e.from.as_str().to_string())
                                .or_default()
                                .insert(child.to_string());
                        }
                        Some(NodeKind::Module) => {
                            insight
                                .module_children
                                .entry(e.from.as_str().to_string())
                                .or_default()
                                .insert(child.to_string());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        let is_type = |key: &str| -> bool {
            nodes.get(key).is_some_and(|n| {
                n.kind == NodeKind::Symbol
                    && fact_text(n, FactKind::SymbolKind).and_then(symbol_role) == Some(Role::Type)
            })
        };

        for node in nodes.values() {
            let key = node.key.as_str();
            match node.kind {
                NodeKind::File => {
                    insight.unit_of.insert(key.to_string(), key.to_string());
                    insight.module_of.insert(key.to_string(), parent_key(key));
                    if let Some(loc) = fact_int(node, FactKind::Loc) {
                        insight.file_loc.insert(key.to_string(), loc);
                    }
                    if is_test_path(key) {
                        insight.test_files.insert(key.to_string());
                    }
                    insight.labels.insert(key.to_string(), key.to_string());
                    if let Some(doc) = fact_text(node, FactKind::DocComment) {
                        if let Some(summary) = doc_summary(doc) {
                            insight.summaries.insert(key.to_string(), summary);
                        }
                    }
                }
                NodeKind::Module => {
                    if let Some(loc) = fact_int(node, FactKind::Loc) {
                        insight.module_loc.insert(key.to_string(), loc);
                    }
                    if let Some(doc) = fact_text(node, FactKind::DocComment) {
                        if let Some(summary) = doc_summary(doc) {
                            insight.summaries.insert(key.to_string(), summary);
                        }
                    }
                }
                NodeKind::Symbol => {
                    // Climb to the nearest type that owns a card; bounded by the
                    // number of owners so a malformed cycle cannot hang the run.
                    let mut unit: Option<&str> = is_type(key).then_some(key);
                    let mut cursor = key;
                    let mut steps = 0usize;
                    while unit.is_none() && steps < 64 {
                        match owner.get(cursor) {
                            Some(parent) => {
                                if is_type(parent) {
                                    unit = Some(parent);
                                }
                                cursor = parent;
                            }
                            None => break,
                        }
                        steps += 1;
                    }
                    let unit = match unit {
                        Some(u) => u.to_string(),
                        None => match insight.file_of.get(key) {
                            Some(file) => file.clone(),
                            None => continue,
                        },
                    };
                    insight.unit_of.insert(key.to_string(), unit);
                    if is_type(key) {
                        let label = fact_text(node, FactKind::SymbolName)
                            .unwrap_or(key)
                            .to_string();
                        insight.labels.insert(key.to_string(), label);
                        if let Some(kind) = fact_text(node, FactKind::SymbolKind) {
                            insight.kinds.insert(key.to_string(), kind.to_string());
                        }
                        if let Some(doc) = fact_text(node, FactKind::DocComment) {
                            if let Some(summary) = doc_summary(doc) {
                                insight.summaries.insert(key.to_string(), summary);
                            }
                        }
                        if let Some(prov) = node
                            .facts
                            .iter()
                            .find(|f| f.kind == FactKind::SymbolName)
                            .map(|f| &f.provenance)
                        {
                            if let Span::Range { start, end } = prov.span {
                                insight.spans.insert(key.to_string(), (start, end));
                            }
                        }
                    }
                }
                NodeKind::External => {
                    let label = fact_text(node, FactKind::ExternalName)
                        .unwrap_or(key)
                        .to_string();
                    let kind = fact_text(node, FactKind::ExternalKind).map(str::to_string);
                    insight.externals.insert(key.to_string(), (label, kind));
                }
                NodeKind::Dependency | NodeKind::Declared(_) => {}
            }
        }

        insight.declaring_files = insight.file_of.values().cloned().collect();

        // A module's own size: the files directly in it, not its sub-modules.
        for (module, files) in &insight.module_files {
            let own: i64 = files.iter().filter_map(|f| insight.file_loc.get(f)).sum();
            insight.module_own_loc.insert(module.clone(), own);
        }

        // A module without a README of its own speaks through its index file —
        // `mod.rs`, `lib.rs`, `__init__.py`, `index.ts`, `doc.go` — when that
        // file carries a doc comment.
        const INDEX_FILES: [&str; 10] = [
            "mod.rs",
            "lib.rs",
            "main.rs",
            "__init__.py",
            "index.ts",
            "index.tsx",
            "index.js",
            "doc.go",
            "package-info.java",
            "AssemblyInfo.cs",
        ];
        let mut fallbacks: Vec<(String, String)> = Vec::new();
        for (module, files) in &insight.module_files {
            if insight.summaries.contains_key(module) {
                continue;
            }
            for name in INDEX_FILES {
                let hit = files
                    .iter()
                    .find(|f| f.rsplit('/').next() == Some(name))
                    .and_then(|f| insight.summaries.get(f));
                if let Some(summary) = hit {
                    fallbacks.push((module.clone(), summary.clone()));
                    break;
                }
            }
        }
        for (module, summary) in fallbacks {
            insight.summaries.insert(module, summary);
        }

        // Units' modules: a type lives where its file lives.
        let symbol_units: Vec<(String, String)> = insight
            .unit_of
            .iter()
            .filter(|(k, u)| k == u && !insight.module_of.contains_key(*u))
            .map(|(_, u)| (u.clone(), u.clone()))
            .collect();
        for (unit, _) in symbol_units {
            if let Some(file) = insight.file_of.get(&unit) {
                let module = parent_key(file);
                insight.module_of.insert(unit, module);
            }
        }
        for (unit, module) in &insight.module_of {
            insight
                .module_units
                .entry(module.clone())
                .or_default()
                .insert(unit.clone());
        }

        insight.read_unit_facts(&nodes);

        for e in &edges {
            let (from, to) = (e.from.as_str(), e.to.as_str());
            if e.edge.kind == EdgeKind::Uses {
                let external = nodes.get(to).is_some_and(|t| {
                    crate::graph::node_role(t.kind, vocabulary) == StructuralRole::External
                });
                if external {
                    if let Some(unit) = insight.unit_of.get(from).cloned() {
                        *insight
                            .unit_externals
                            .entry(unit.clone())
                            .or_default()
                            .entry(to.to_string())
                            .or_default() += 1;
                        if let Some(module) = insight.module_of.get(&unit).cloned() {
                            *insight
                                .module_externals
                                .entry(module)
                                .or_default()
                                .entry(to.to_string())
                                .or_default() += 1;
                        }
                    }
                }
                continue;
            }
            if !counted(e.edge.kind) {
                continue;
            }
            let rung = e.edge.provenance.rung();
            // A module-level import (Rust `mod`, a Python package) lands on a
            // Module node: it says something about modules, nothing about units.
            let to_module = nodes.get(to).is_some_and(|n| n.kind == NodeKind::Module);
            let from_unit = insight.unit_of.get(from).cloned();
            let to_unit = if to_module {
                None
            } else {
                insight.unit_of.get(to).cloned()
            };
            // Production code does not call into its own tests. Such an edge is a
            // name match that happened to land on a test helper, and it would
            // make every test module look like a dependency of the code it tests.
            if let (Some(fu), Some(tu)) = (&from_unit, &to_unit) {
                if !insight.is_test_unit(fu) && insight.is_test_unit(tu) {
                    continue;
                }
            }
            if let (Some(fu), Some(tu)) = (&from_unit, &to_unit) {
                if fu != tu {
                    insight
                        .unit_out
                        .entry(fu.clone())
                        .or_default()
                        .entry(tu.clone())
                        .or_default()
                        .add(e.edge.kind, rung);
                    insight
                        .unit_in
                        .entry(tu.clone())
                        .or_default()
                        .entry(fu.clone())
                        .or_default()
                        .add(e.edge.kind, rung);
                }
                // An inheritance resting only on an ambiguous name match is no
                // base and no implementation (plan A, R7): the hierarchy, the
                // extension points and a card's inheritance read this map, and
                // a guess in it would send the reader to the wrong type.
                if matches!(e.edge.kind, EdgeKind::Extends | EdgeKind::Implements)
                    && rung != Some(Confidence::Ambiguous)
                {
                    insight
                        .derived
                        .entry(tu.clone())
                        .or_default()
                        .insert(fu.clone());
                }
            }
            let from_module = from_unit.as_ref().and_then(|u| insight.module_of.get(u));
            let to_module_key = if to_module {
                Some(to.to_string())
            } else {
                to_unit
                    .as_ref()
                    .and_then(|u| insight.module_of.get(u))
                    .cloned()
            };
            if let (Some(fm), Some(tm)) = (from_module.cloned(), to_module_key) {
                if fm != tm {
                    insight
                        .module_out
                        .entry(fm.clone())
                        .or_default()
                        .entry(tm.clone())
                        .or_default()
                        .add(e.edge.kind, rung);
                    insight
                        .module_in
                        .entry(tm)
                        .or_default()
                        .entry(fm)
                        .or_default()
                        .add(e.edge.kind, rung);
                }
            }
        }

        insight
    }

    /// What every unit's own facts and its members' add up to (plan B, U6 and
    /// U7): its keywords with their document frequencies, the database objects
    /// it reaches and how, and its entry points. One pass over the facts of
    /// every node, each lifted to its unit as the relations are; the order the
    /// nodes come in changes nothing — the counts are sums, and every list
    /// read out of them is sorted by its content.
    fn read_unit_facts(&mut self, nodes: &HashMap<&str, &Node>) {
        let mut words = KeywordIndexBuilder::default();
        let mut db: HashMap<String, BTreeSet<(DbAccessKind, String)>> = HashMap::new();
        let mut entries: HashMap<String, BTreeSet<String>> = HashMap::new();
        for node in nodes.values() {
            let key = node.key.as_str();
            let unit = match node.kind {
                NodeKind::File => key,
                NodeKind::Symbol => match self.unit_of.get(key) {
                    Some(unit) => unit.as_str(),
                    None => continue,
                },
                _ => continue,
            };
            // A file's own name is not among its words: it is the card's title
            // already, and it would make every file a document, so a word all
            // types share would never weigh 0.
            if let Some(name) = fact_text(node, FactKind::SymbolName) {
                words.identifier(unit, name);
            }
            if let Some(doc) = fact_text(node, FactKind::DocComment) {
                words.text(unit, &doc_words(doc), 1);
            }
            for fact in &node.facts {
                match (&fact.kind, &fact.value) {
                    (FactKind::Anchor, FactValue::Text(value)) => {
                        if let Some(anchor) = AnchorText::decode(value) {
                            words.text(unit, &anchor.text, anchor.occurrences);
                        }
                    }
                    (FactKind::DbAccess, FactValue::Text(value)) => {
                        if let Some((access, object)) = DbAccessKind::decode(value) {
                            words.db_object(unit, object);
                            db.entry(unit.to_string())
                                .or_default()
                                .insert((access, object.to_string()));
                        }
                    }
                    (FactKind::EntryPoint, FactValue::Bool(true)) => {
                        let name = fact_text(node, FactKind::SymbolName).unwrap_or(key);
                        entries
                            .entry(unit.to_string())
                            .or_default()
                            .insert(name.to_string());
                    }
                    _ => {}
                }
            }
        }
        self.keywords = words.finish();
        self.db_accesses = db;
        self.entry_points = entries;
    }

    /// The keywords of a unit — a type or a file (plan B, U6) — the most
    /// typical for it across the corpus first. See [`super::keywords`].
    pub fn keywords(&self, unit: &str) -> Vec<Keyword> {
        self.keywords.keywords(unit)
    }

    /// [`Self::keywords`] with the words the second layer brings to the unit,
    /// weighed by the first layer's document frequencies (plan 2a, U6: KTD9
    /// point 4) — see [`KeywordIndex::keywords_with`].
    pub fn keywords_with(&self, unit: &str, extra: &super::keywords::UnitWords) -> Vec<Keyword> {
        self.keywords.keywords_with(unit, extra)
    }

    /// Every database object the unit and its members reach, once per way
    /// they reach it, ordered by access and then by object (plan B, U7).
    pub fn db_accesses(&self, unit: &str) -> impl Iterator<Item = (DbAccessKind, &str)> {
        self.db_accesses
            .get(unit)
            .into_iter()
            .flatten()
            .map(|(access, object)| (*access, object.as_str()))
    }

    /// The names of the unit's members (or the unit itself) the extractor
    /// marked as a language-level entry point, in name order.
    pub fn entry_points(&self, unit: &str) -> impl Iterator<Item = &str> {
        self.entry_points
            .get(unit)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }

    pub fn unit_of(&self, key: &str) -> Option<&str> {
        self.unit_of.get(key).map(String::as_str)
    }

    pub fn file_of(&self, key: &str) -> Option<&str> {
        self.file_of.get(key).map(String::as_str)
    }

    pub fn module_of(&self, unit: &str) -> Option<&str> {
        self.module_of.get(unit).map(String::as_str)
    }

    pub fn summary(&self, key: &str) -> Option<&str> {
        self.summaries.get(key).map(String::as_str)
    }

    pub fn label<'a>(&'a self, key: &'a str) -> &'a str {
        self.labels.get(key).map(String::as_str).unwrap_or(key)
    }

    pub fn kind(&self, unit: &str) -> Option<&str> {
        self.kinds.get(unit).map(String::as_str)
    }

    pub fn is_test_file(&self, file: &str) -> bool {
        self.test_files.contains(file)
    }

    /// Whether the extractors read at least one declaration out of `file` —
    /// the line between source code and everything else a repository tracks
    /// (lockfiles, documents, data).
    pub fn declares_code(&self, file: &str) -> bool {
        self.declaring_files.contains(file)
    }

    /// Whether this unit lives in a test file.
    pub fn is_test_unit(&self, unit: &str) -> bool {
        let file = self.file_of(unit).unwrap_or(unit);
        self.is_test_file(file)
    }

    pub fn file_loc(&self, file: &str) -> Option<i64> {
        self.file_loc.get(file).copied()
    }

    pub fn module_loc(&self, module: &str) -> Option<i64> {
        self.module_loc.get(module).copied()
    }

    /// Lines in the files directly inside `module`, sub-modules excluded.
    pub fn module_own_loc(&self, module: &str) -> Option<i64> {
        self.module_own_loc.get(module).copied()
    }

    /// A type's own length in lines, from its declaration range.
    pub fn type_lines(&self, unit: &str) -> Option<u32> {
        self.spans
            .get(unit)
            .map(|(start, end)| end.saturating_sub(*start) + 1)
    }

    pub fn external(&self, key: &str) -> Option<(&str, Option<&str>)> {
        self.externals
            .get(key)
            .map(|(label, kind)| (label.as_str(), kind.as_deref()))
    }

    /// Every type unit, in identity order.
    pub fn type_units(&self) -> impl Iterator<Item = &str> {
        self.kinds.keys().map(String::as_str)
    }

    /// The units reaching `unit`, ranked: most certain relations first, then
    /// most relations, then identity.
    pub fn users(&self, unit: &str) -> Vec<(&str, Counts)> {
        ranked(self.unit_in.get(unit))
    }

    /// The units `unit` reaches, ranked the same way as [`Insight::users`].
    pub fn uses(&self, unit: &str) -> Vec<(&str, Counts)> {
        ranked(self.unit_out.get(unit))
    }

    pub fn module_users(&self, module: &str) -> Vec<(&str, Counts)> {
        ranked(self.module_in.get(module))
    }

    pub fn module_uses(&self, module: &str) -> Vec<(&str, Counts)> {
        ranked(self.module_out.get(module))
    }

    /// Externals reached by `unit`, most used first.
    pub fn externals_of(&self, unit: &str) -> Vec<ExternalUse> {
        self.external_list(self.unit_externals.get(unit))
    }

    pub fn module_externals_of(&self, module: &str) -> Vec<ExternalUse> {
        self.external_list(self.module_externals.get(module))
    }

    fn external_list(&self, map: Option<&BTreeMap<String, u32>>) -> Vec<ExternalUse> {
        let mut out: Vec<ExternalUse> = map
            .into_iter()
            .flatten()
            .map(|(key, uses)| {
                let (label, kind) = self
                    .external(key)
                    .map(|(l, k)| (l.to_string(), k.map(str::to_string)))
                    .unwrap_or_else(|| (key.clone(), None));
                ExternalUse {
                    identity: key.clone(),
                    label,
                    kind,
                    uses: *uses,
                }
            })
            .collect();
        out.sort_by(|a, b| b.uses.cmp(&a.uses).then(a.identity.cmp(&b.identity)));
        out
    }

    /// Every module that holds at least one unit or a counted relation, sorted.
    pub fn modules(&self) -> BTreeSet<&str> {
        let mut set: BTreeSet<&str> = self.module_units.keys().map(String::as_str).collect();
        set.extend(self.module_out.keys().map(String::as_str));
        set.extend(self.module_in.keys().map(String::as_str));
        set
    }

    /// Test units that reach `unit` — the tests that exercise it, read from the
    /// relations rather than from how files are named.
    pub fn tests_of(&self, unit: &str) -> Vec<(&str, Counts)> {
        self.users(unit)
            .into_iter()
            .filter(|(u, _)| self.is_test_unit(u))
            .collect()
    }

    /// The type's bases in this repository: `(base, "extends" | "implements")`
    /// — only those [`Insight::derived_of`] also lists, so an inheritance that
    /// rests on an ambiguous name match is a base on neither side.
    pub fn bases_of(&self, unit: &str) -> Vec<(&str, &'static str)> {
        let mut out: Vec<(&str, &'static str)> = self
            .uses(unit)
            .into_iter()
            .filter(|(base, _)| self.derived.get(*base).is_some_and(|d| d.contains(unit)))
            .filter_map(|(base, counts)| {
                if counts.extends > 0 {
                    Some((base, "extends"))
                } else if counts.implements > 0 {
                    Some((base, "implements"))
                } else {
                    None
                }
            })
            .collect();
        out.sort();
        out
    }

    /// Types that extend or implement `unit`, in identity order.
    pub fn derived_of(&self, unit: &str) -> Vec<&str> {
        self.derived
            .get(unit)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect()
    }
}

fn ranked(map: Option<&BTreeMap<String, Counts>>) -> Vec<(&str, Counts)> {
    let mut out: Vec<(&str, Counts)> = map
        .into_iter()
        .flatten()
        .map(|(k, c)| (k.as_str(), *c))
        .collect();
    out.sort_by(|a, b| {
        b.1.undisputed()
            .cmp(&a.1.undisputed())
            .then(b.1.total().cmp(&a.1.total()))
            .then(a.0.cmp(b.0))
    });
    out
}

/// Sum of a ranked list's counts.
pub fn sum(list: &[(&str, Counts)]) -> Counts {
    let mut total = Counts::default();
    for (_, c) in list {
        total.merge(c);
    }
    total
}

fn fact_text(node: &Node, kind: FactKind) -> Option<&str> {
    node.facts.iter().find_map(|f| match (&f.kind, &f.value) {
        (k, FactValue::Text(v)) if *k == kind => Some(v.as_str()),
        _ => None,
    })
}

fn fact_int(node: &Node, kind: FactKind) -> Option<i64> {
    node.facts.iter().find_map(|f| match (&f.kind, &f.value) {
        (k, FactValue::Int(v)) if *k == kind => Some(*v),
        _ => None,
    })
}

/// Whether a path is a test by the conventions every supported ecosystem uses:
/// a test directory anywhere on the path, or a test affix on the file name.
///
/// A name-and-tree reading, like the test-pairing extractor's, and just as
/// unable to prove a file is NOT a test — which is why nothing here ever says
/// "untested", only which tests were seen reaching a unit.
pub fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let mut parts = lower.split('/').collect::<Vec<_>>();
    let base = parts.pop().unwrap_or("");
    if parts.iter().any(|dir| {
        matches!(
            *dir,
            "test" | "tests" | "__tests__" | "spec" | "specs" | "testing" | "e2e"
        ) || dir.ends_with(".tests")
            || dir.ends_with(".test")
            || dir.ends_with("tests") && dir.contains('.')
    }) {
        return true;
    }
    let stem = base.split('.').next().unwrap_or(base);
    base.contains(".test.")
        || base.contains(".spec.")
        || base.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with("tests") && path.ends_with(".cs")
        || stem.ends_with("test") && (path.ends_with(".cs") || path.ends_with(".java"))
}

/// The first sentence of a doc comment, stripped of markup, or `None` when the
/// comment has no prose at all.
///
/// Documentation markup is removed, not interpreted: C# XML tags drop and their
/// `cref`/`name` target is kept as text, JSDoc/Javadoc block tags (`@param …`)
/// end the summary, and a leading `*` column is trimmed. The sentence ends at
/// the first `. `, the first blank line, or 240 characters at a word boundary.
pub fn doc_summary(doc: &str) -> Option<String> {
    let mut text = String::new();
    for raw in doc.lines() {
        let line = raw.trim().trim_start_matches('*').trim();
        if line.starts_with('@') || line.starts_with(":param") || line.starts_with("Args:") {
            break;
        }
        if line.is_empty() {
            if text.trim().is_empty() {
                continue;
            }
            break;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(line);
    }
    let text = strip_xml(&strip_inline_tags(&text));
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    let mut end = text.len();
    for (i, _) in text.match_indices(". ") {
        // `e.g. ` and `i.e. ` are not sentence ends.
        let before = &text[..i];
        if before.ends_with("e.g") || before.ends_with("i.e") || before.ends_with("etc") {
            continue;
        }
        end = i + 1;
        break;
    }
    let mut sentence = text[..end].to_string();
    if sentence.chars().count() > 240 {
        let cut: String = sentence.chars().take(240).collect();
        let cut = match cut.rfind(' ') {
            Some(space) => cut[..space].to_string(),
            None => cut,
        };
        sentence = format!("{cut}…");
    }
    Some(sentence)
}

/// The prose of a doc comment as keywords read it (plan B, U6): markup removed
/// as [`doc_summary`] removes it, and every line the secret filter replaced
/// left out — its marker would otherwise give the keywords `redacted` and the
/// name of the pattern that fired.
fn doc_words(doc: &str) -> String {
    let kept: Vec<&str> = doc
        .lines()
        .filter(|line| !line.contains(REDACTION_MARKER))
        .collect();
    strip_xml(&strip_inline_tags(&kept.join("\n")))
}

/// JSDoc/TSDoc/Javadoc inline tags: `{@link Foo.bar}` → `Foo.bar`,
/// `{@link Foo | the foo}` → `the foo`, `{@code x}` → `x`.
fn strip_inline_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{@") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let body = inner
            .split_once(char::is_whitespace)
            .map_or("", |(_, b)| b)
            .trim();
        let shown = match body.split_once('|') {
            Some((_, label)) => label.trim(),
            None => body,
        };
        out.push_str(shown);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Remove XML-doc tags, keeping the text between them and the name a
/// self-closing reference points at (`<see cref="Foo"/>` → `Foo`).
fn strip_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find('>') else {
            out.push_str(after);
            return out;
        };
        let tag = &after[1..close];
        let is_tag = tag
            .trim_start_matches('/')
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if !is_tag {
            out.push('<');
            rest = &after[1..];
            continue;
        }
        if let Some(target) = attr(tag, "cref").or_else(|| attr(tag, "name")) {
            if tag.ends_with('/') {
                let simple = target.rsplit(['.', ':']).next().unwrap_or(target);
                out.push_str(simple);
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}=\""))?;
    let value = &tag[at + name.len() + 2..];
    value.find('"').map(|end| &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_the_first_sentence() {
        assert_eq!(
            doc_summary("Central coordinator for sessions. Owns the heartbeat.").as_deref(),
            Some("Central coordinator for sessions.")
        );
    }

    #[test]
    fn xml_doc_tags_drop_and_references_keep_their_name() {
        assert_eq!(
            doc_summary("<summary>\nReads a <see cref=\"Ns.Invoice\"/> from disk.\n</summary>")
                .as_deref(),
            Some("Reads a Invoice from disk.")
        );
    }

    #[test]
    fn inline_link_tags_keep_their_target_or_label() {
        assert_eq!(
            doc_summary("Sent via {@link TransportProvider.send} or {@link Queue | the queue}.")
                .as_deref(),
            Some("Sent via TransportProvider.send or the queue.")
        );
    }

    #[test]
    fn jsdoc_block_tags_end_the_summary() {
        assert_eq!(
            doc_summary("Sends a frame\n@param frame the bytes").as_deref(),
            Some("Sends a frame")
        );
    }

    #[test]
    fn an_abbreviation_does_not_end_the_sentence() {
        assert_eq!(
            doc_summary("Parses e.g. headers. Then bodies.").as_deref(),
            Some("Parses e.g. headers.")
        );
    }

    #[test]
    fn a_comment_of_only_markup_has_no_summary() {
        assert_eq!(doc_summary("<summary>\n</summary>"), None);
        assert_eq!(doc_summary("   \n  "), None);
    }

    #[test]
    fn test_paths_are_recognized_across_ecosystems() {
        for path in [
            "src/__tests__/queue.test.ts",
            "tests/cli.rs",
            "pkg/test_model.py",
            "pkg/model_test.go",
            "Billing.Tests/InvoiceTests.cs",
            "src/foo.spec.ts",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in ["src/runtime/queue.ts", "src/testament.rs", "latest/x.py"] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn counts_describe_the_dominant_kind_first() {
        let mut c = Counts::default();
        c.add(EdgeKind::Calls, Some(Confidence::NameOnly));
        c.add(EdgeKind::Calls, Some(Confidence::Ambiguous));
        c.add(EdgeKind::References, Some(Confidence::RulesBacked));
        assert_eq!(c.describe(), "calls 2 · references 1 (1 ambiguous)");
        assert_eq!(c.dominant_word(), "calls");
    }

    /// Plan A, U2: certainty comes from the rung. Index and rules back a
    /// relation; a name match alone does not, but it is not disputed either;
    /// an edge with no rung reads as it did before rungs existed.
    #[test]
    fn counts_derive_certainty_from_the_rungs() {
        let mut c = Counts::default();
        c.add(EdgeKind::Calls, Some(Confidence::ResolvedByIndex));
        c.add(EdgeKind::Calls, Some(Confidence::RulesBacked));
        c.add(EdgeKind::Calls, Some(Confidence::NameOnly));
        c.add(EdgeKind::Calls, Some(Confidence::Ambiguous));
        c.add(EdgeKind::Imports, None);
        assert_eq!(c.total(), 5);
        assert_eq!(c.unrated(), 1);
        assert_eq!(c.certain(), 3, "index, rules and the unrated import");
        assert_eq!(c.undisputed(), 4, "everything but the ambiguous call");
    }
}
