//! References by name, resolved once every file has been read (contract 0.3,
//! `name-ref`; plan 2c, U5).
//!
//! A plugin reports a declaration that names another by its name alone — a
//! markup file's `{StaticResource K}` — together with the nested lookup blocks
//! of its file and the block each of its declarations is written in. Only the
//! whole repository can say which declaration is meant, so the host looks the
//! name up here, after the declaration pass and beside the type relations
//! ([`super::treesitter`]), and before the usage pass.
//!
//! # Candidates
//!
//! A candidate is a node of a kind the plugin declared for itself whose
//! `symbol_name` is the name the reference spells (never its key: a key that
//! repeats in one file carries an ordinal `~2`, the name does not). Its kind
//! is the reference's `target-kind`, with the widenings of [`BROADER`] and
//! nothing else. A reference names one target kind, but the syntax of XAML's
//! `{StaticResource K}` does not say whether `K` is a style, a template or
//! another resource (the plugin's findings, "A template referenced by
//! `{StaticResource}`", propose a list of target kinds in the contract). Until
//! the contract carries such a list, the table says it for the one plugin
//! that needs it: a `xaml:resource` reference finds a resource, a template or
//! a style of that key, a `xaml:style` reference styles only, and a kind the
//! table does not name only itself. A reference never resolves to the node it
//! comes from.
//!
//! A name the secret filter withheld — on either side — is the filter's
//! marker, which two different secrets share, so it is no candidate and a
//! reference that spells it resolves to nothing: it is admitted as an
//! unresolved reference, whatever the file declares. A file that declares
//! such a name outside every block is a variant by its path alone.
//!
//! # Scopes (plan 2c, KTD5)
//!
//! The candidates are looked for scope by scope, nearest first, and the first
//! scope that holds any is the answer:
//!
//! 1. the reference's own block, then each block around it, one at a time
//!    (candidates of the same file written in exactly that block) —
//!    [`Rule::ScopeBlock`];
//! 2. the reference's file — [`Rule::ScopeFile`];
//! 3. every file its file imports, transitively (`imports` edges, followed
//!    with a visited set, so a cycle ends) — [`Rule::ScopeImports`];
//! 4. every file of the same project: the nearest directory above the file
//!    that holds a tracked `.csproj`, the rule `resolve::project` applies to
//!    usings — [`Rule::ScopeProject`]. A file under no project has no such
//!    scope;
//! 5. the whole repository — [`Rule::NameMatch`].
//!
//! Scopes 1–4 are [`Confidence::RulesBacked`], the last [`Confidence::NameOnly`]
//! (KTD8: no new rung). One candidate in the deciding scope is one edge. More
//! than one are one edge each, rated [`Confidence::Ambiguous`] with the count
//! on every edge and a [`FactKind::CandidateCount`] on the referring node —
//! unless they are configuration variants (below); above the ambiguity
//! ceiling, a [`FactKind::AmbiguousReference`] and no edge, as for a usage
//! site (R23).
//!
//! # Variants (plan 2c, KTD6)
//!
//! Two files are variants of each other when they sit at the same relative
//! path under sibling project roots (two projects whose directories share a
//! parent), or when they declare the same non-empty set of names outside every
//! block (candidate kinds only). Candidates of the deciding scope, each in a
//! file of its own, whose files are pairwise variants and none of which lies
//! in the other's reach (the same file or one it imports, transitively, either
//! way), are variants: each gets an edge of the scope's own rung, not an
//! ambiguity. One pair that is not a variant pair leaves the whole set
//! ambiguous. The rule is structural, with no threshold.
//!
//! # Edges and admissions
//!
//! The edge runs from the reference's `source` (a declaration of the file, or
//! the file itself) and is the reference's `relation` — `extends` is
//! [`EdgeKind::Extends`], an edge kind of the vocabulary is that kind — or,
//! for a plain reference, [`EdgeKind::References`]: the core's relation for a
//! declaration that names another. A relation repeated from one node to one
//! target is one edge, at every site that names it, rated by its strongest
//! site. A name no candidate answers anywhere is not dropped: it is a
//! [`FactKind::UnresolvedReference`] on the file the reference is written in.
//!
//! Everything is ordered by path and position (R20): candidates by file, then
//! the position of their name, then key; references in file order, then in
//! the order the plugin reported them.

use super::external::REDACTION_MARKER;
use super::resolve::table::NamePositions;
use super::TrackedFile;
use crate::graph::{
    Confidence, Edge, EdgeKind, ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind,
    Provenance, Rule, SiteId, Tier,
};
use crate::plugin::{LookupBlock, NameRef};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// One file's references by name and the blocks they are looked up in, as the
/// declaration pass banked them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct FileNameRefs {
    pub(super) name_refs: Vec<NameRef>,
    pub(super) blocks: Vec<LookupBlock>,
    pub(super) symbol_blocks: Vec<(NodeKey, u32)>,
}

/// A declaration a reference may resolve to.
struct Candidate {
    key: NodeKey,
    /// Its node kind, `namespace:name`.
    kind: String,
    file: String,
    /// Where its name is written, `(line, column)`; the column is 0 when only
    /// the line is known.
    at: (u32, u32),
    block: Option<u32>,
}

/// The run-wide facts the scopes are computed from.
struct Scopes<'a> {
    /// Each file's lookup blocks.
    blocks: BTreeMap<String, &'a [LookupBlock]>,
    /// `imports` edges, importer → imported.
    imports: BTreeMap<String, BTreeSet<String>>,
    /// Directories holding a tracked `.csproj`.
    projects: BTreeSet<String>,
    /// Every file's names outside every block, of candidate kinds.
    top_names: BTreeMap<String, BTreeSet<String>>,
}

impl Scopes<'_> {
    /// `block` and every block around it, innermost first. An index outside
    /// the list ends the chain, and so does a parent loop.
    fn chain(&self, file: &str, block: Option<u32>) -> Vec<u32> {
        let list = self.blocks.get(file).copied().unwrap_or_default();
        let mut out = Vec::new();
        let mut next = block;
        while let Some(b) = next {
            let Some(found) = list.get(b as usize) else {
                break;
            };
            if out.contains(&b) || out.len() > list.len() {
                break;
            }
            out.push(b);
            next = found.parent;
        }
        out
    }

    /// `file` and every file it imports, transitively.
    fn reach(&self, file: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::from([file.to_string()]);
        let mut queue = vec![file.to_string()];
        while let Some(at) = queue.pop() {
            for next in self.imports.get(&at).into_iter().flatten() {
                if seen.insert(next.clone()) {
                    queue.push(next.clone());
                }
            }
        }
        seen
    }

    /// The project directory `file` belongs to: the nearest directory at or
    /// above its own that holds a `.csproj`.
    fn project_of(&self, file: &str) -> Option<&str> {
        let mut dir = parent(file);
        loop {
            if let Some(p) = self.projects.get(dir) {
                return Some(p);
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent(dir);
        }
    }

    /// Whether two distinct files behave as variants of each other (KTD6).
    fn variants(&self, a: &str, b: &str) -> bool {
        if a == b {
            return false;
        }
        if let (Some(pa), Some(pb)) = (self.project_of(a), self.project_of(b)) {
            if pa != pb && !pa.is_empty() && !pb.is_empty() && parent(pa) == parent(pb) {
                let under = |file: &'_ str, root: &str| -> Option<String> {
                    file.strip_prefix(root)
                        .and_then(|rest| rest.strip_prefix('/'))
                        .map(str::to_string)
                };
                if under(a, pa).is_some() && under(a, pa) == under(b, pb) {
                    return true;
                }
            }
        }
        match (self.top_names.get(a), self.top_names.get(b)) {
            (Some(x), Some(y)) => !x.is_empty() && x == y,
            _ => false,
        }
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The target kinds a reference may name more broadly than one kind, each
/// with every kind that answers it. A target kind not listed is answered by
/// itself alone. See the module documentation for why the table exists and
/// what would retire it.
const BROADER: &[(&str, &[&str])] = &[(
    RESOURCE_KIND,
    &["xaml:resource", "xaml:template", "xaml:style"],
)];

/// Whether a candidate of the kind `kind` answers a reference to `target`
/// (both `namespace:name`).
fn answers(target: &str, kind: &str) -> bool {
    match BROADER.iter().find(|(t, _)| *t == target) {
        Some((_, kinds)) => kinds.contains(&kind),
        None => target == kind,
    }
}

/// The file a node is read from: the node itself for a file, else the file
/// its `symbol_name` was read from.
fn file_of(graph: &Graph, key: &NodeKey) -> Option<String> {
    let node = graph.node(key)?;
    if node.kind == NodeKind::File {
        return Some(key.as_str().to_string());
    }
    node.facts
        .iter()
        .find(|f| f.kind == FactKind::SymbolName)
        .map(|f| f.provenance.file.clone())
}

fn text_fact(node: &crate::graph::Node, kind: FactKind) -> Option<&str> {
    node.facts
        .iter()
        .find_map(|f| match (&f.value, f.kind == kind) {
            (FactValue::Text(t), true) => Some(t.as_str()),
            _ => None,
        })
}

/// One relation a reference resolved to, before it is an edge.
struct Pending {
    source: NodeKey,
    kind: EdgeKind,
    target: NodeKey,
    prov: Provenance,
    sites: Vec<SiteId>,
}

/// Resolve every reference by name the declaration pass banked, in file
/// order. `plugin_of` names the plugin that read a file's declarations, whose
/// id the edges and facts carry; `ceiling` is the run's ambiguity ceiling.
/// Without a single reference nothing is read and nothing changes.
pub(super) fn resolve(
    graph: &mut Graph,
    files: &[FileNameRefs],
    names: &NamePositions,
    tracked: &[TrackedFile],
    ceiling: usize,
    plugin_of: &dyn Fn(&str) -> Option<ExtractorId>,
) {
    if files.iter().all(|f| f.name_refs.is_empty()) {
        return;
    }
    let symbol_blocks: HashMap<&NodeKey, u32> = files
        .iter()
        .flat_map(|f| f.symbol_blocks.iter().map(|(k, b)| (k, *b)))
        .collect();

    // The file of each banked record, from the nodes it names.
    let mut owned: Vec<(String, &FileNameRefs)> = Vec::new();
    for record in files {
        let keys = record
            .name_refs
            .iter()
            .map(|r| NodeKey::new(r.source.clone()))
            .chain(record.symbol_blocks.iter().map(|(k, _)| k.clone()));
        let file = keys.into_iter().find_map(|k| file_of(graph, &k));
        match file {
            Some(file) => owned.push((file, record)),
            None if !record.name_refs.is_empty() => eprintln!(
                "{} references by name dropped: none of the nodes they come from reached the graph",
                record.name_refs.len()
            ),
            None => {}
        }
    }

    // Every candidate of a kind a reference can name — the kinds any
    // reference targets and every kind of the table — by name. The same kinds
    // make a file's names outside every block, whatever the run's references
    // happen to target, so a view's or a member's name is in no key set and
    // two files are variants or not by what they declare alone.
    let targets: BTreeSet<&str> = owned
        .iter()
        .flat_map(|(_, r)| r.name_refs.iter())
        .map(|r| r.target_kind.as_str())
        .chain(BROADER.iter().flat_map(|(_, kinds)| kinds.iter().copied()))
        .collect();
    let (by_name, top_names) = collect_candidates(
        graph,
        &targets,
        &|file, key| names.get(&(file.to_string(), key.clone())).copied(),
        &symbol_blocks,
    );

    let mut imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in graph.edges_unordered() {
        if e.edge.kind == EdgeKind::Imports {
            imports
                .entry(e.from.as_str().to_string())
                .or_default()
                .insert(e.to.as_str().to_string());
        }
    }
    let projects: BTreeSet<String> = tracked
        .iter()
        .filter(|t| !t.is_symlink && t.key.to_ascii_lowercase().ends_with(".csproj"))
        .map(|t| parent(&t.key).to_string())
        .collect();
    let scopes = Scopes {
        blocks: owned
            .iter()
            .map(|(file, r)| (file.clone(), r.blocks.as_slice()))
            .collect(),
        imports,
        projects,
        top_names,
    };

    let mut reach: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut reach_of = |file: &str| -> BTreeSet<String> {
        reach
            .entry(file.to_string())
            .or_insert_with(|| scopes.reach(file))
            .clone()
    };

    let mut pending: Vec<Pending> = Vec::new();
    let mut pending_at: HashMap<(NodeKey, EdgeKind, NodeKey), usize> = HashMap::new();
    let mut facts: Vec<(NodeKey, Fact)> = Vec::new();
    let mut facts_seen: BTreeSet<(String, FactKind, String)> = BTreeSet::new();

    for (file, record) in &owned {
        let Some(plugin) = plugin_of(file) else {
            continue;
        };
        let file_key = NodeKey::new(file.clone());
        for r in &record.name_refs {
            let edge_kind = match r.relation.as_deref() {
                None => EdgeKind::References,
                Some(word) => match crate::graph::edge_kind_from_word(word, graph.vocabulary()) {
                    Some(kind) => kind,
                    None => {
                        eprintln!(
                            "plugin `{plugin}` named the undeclared relation `{word}` for a reference in `{file}`; the reference is dropped"
                        );
                        continue;
                    }
                },
            };
            let source = NodeKey::new(r.source.clone());
            let source = if graph.node(&source).is_some() {
                source
            } else {
                file_key.clone()
            };
            let mut prov = Provenance::new(file.clone(), None, plugin.clone(), Tier::Measured);
            prov.span = r.span;
            let site = SiteId {
                file: file.clone(),
                line: r.span.start().unwrap_or(0),
                column: r.name_at.map(|(_, c)| c),
            };

            let candidates: Vec<&Candidate> = by_name
                .get(&r.name)
                .into_iter()
                .flatten()
                .filter(|c| answers(&r.target_kind, &c.kind) && c.key != source)
                .collect();
            if candidates.is_empty() {
                let text = format!("{}: no {} declares this name", r.name, r.target_kind);
                if facts_seen.insert((file.clone(), FactKind::UnresolvedReference, text.clone())) {
                    facts.push((
                        file_key.clone(),
                        Fact::new(FactKind::UnresolvedReference, FactValue::Text(text), prov),
                    ));
                }
                continue;
            }

            let (hits, rung, rule) = decide(&scopes, &mut reach_of, file, r.block, &candidates);
            let count = hits.len();
            let varied = count > 1 && are_variants(&scopes, &mut reach_of, &hits);
            let rung = if count > 1 && !varied {
                Confidence::Ambiguous
            } else {
                rung
            };
            if rung == Confidence::Ambiguous {
                let text = format!(
                    "{}: {count} candidate{}",
                    r.name,
                    if count == 1 { "" } else { "s" }
                );
                let kind = if count > ceiling {
                    FactKind::AmbiguousReference
                } else {
                    FactKind::CandidateCount
                };
                if facts_seen.insert((source.as_str().to_string(), kind, text.clone())) {
                    facts.push((
                        source.clone(),
                        Fact::new(kind, FactValue::Text(text), prov.clone()),
                    ));
                }
                if count > ceiling {
                    continue;
                }
            }
            for hit in hits {
                let mut rated = prov.clone().rated(rung, rule);
                if rung == Confidence::Ambiguous {
                    rated = rated.ambiguous(count);
                }
                let key = (source.clone(), edge_kind, hit.key.clone());
                match pending_at.get(&key) {
                    Some(&i) => {
                        let held = &mut pending[i];
                        if held.prov.rung().is_none_or(|h| rung < h) {
                            held.prov = rated;
                        }
                        held.sites.push(site.clone());
                    }
                    None => {
                        pending_at.insert(key, pending.len());
                        pending.push(Pending {
                            source: source.clone(),
                            kind: edge_kind,
                            target: hit.key.clone(),
                            prov: rated,
                            sites: vec![site.clone()],
                        });
                    }
                }
            }
        }
    }

    for (key, fact) in facts {
        graph.add_fact(&key, fact);
    }
    for p in pending {
        graph.add_edge(
            &p.source,
            &p.target,
            Edge::new(p.kind, p.prov).at_sites(p.sites),
        );
    }
}

/// Where a candidate's name is written in its file, when the run knows it.
type NameAt<'f> = &'f dyn Fn(&str, &NodeKey) -> Option<(u32, u32)>;

/// The candidates by name, and every file's names outside every block.
type Collected = (
    BTreeMap<String, Vec<Candidate>>,
    BTreeMap<String, BTreeSet<String>>,
);

/// Every candidate of a kind one of `targets` answers, by name — each list
/// by file, then the position of its name, then key — and every file's names
/// outside every block. `at_of` gives a candidate's name position in its file
/// when the run still knows it (the line of its name fact otherwise, column
/// 0); `symbol_blocks` the block each declaration is written in.
fn collect_candidates(
    graph: &Graph,
    targets: &BTreeSet<&str>,
    at_of: NameAt<'_>,
    symbol_blocks: &HashMap<&NodeKey, u32>,
) -> Collected {
    let mut by_name: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    let mut top_names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut unknown_top: BTreeSet<String> = BTreeSet::new();
    // Every list is sorted below and the names go into sets: no order of the
    // graph's.
    for node in graph.nodes_unordered() {
        let NodeKind::Declared(id) = node.kind else {
            continue;
        };
        let Some(kind) = graph.vocabulary().node_name(id) else {
            continue;
        };
        if !targets.iter().any(|t| answers(t, &kind)) {
            continue;
        }
        let Some(name) = text_fact(node, FactKind::SymbolName) else {
            continue;
        };
        let Some(file) = file_of(graph, &node.key) else {
            continue;
        };
        let line = node
            .facts
            .iter()
            .find(|f| f.kind == FactKind::SymbolName)
            .and_then(|f| f.provenance.span.start())
            .unwrap_or(0);
        let at = at_of(&file, &node.key).unwrap_or((line, 0));
        let block = symbol_blocks.get(&node.key).copied();
        // A name the secret filter withheld is its marker, and two different
        // withheld names share one: it is no name to match, and a file that
        // declares one outside every block has no name set to compare.
        let withheld = name.contains(REDACTION_MARKER);
        if block.is_none() {
            if withheld {
                unknown_top.insert(file.clone());
            } else {
                top_names
                    .entry(file.clone())
                    .or_default()
                    .insert(name.to_string());
            }
        }
        if withheld {
            continue;
        }
        by_name
            .entry(name.to_string())
            .or_default()
            .push(Candidate {
                key: node.key.clone(),
                kind,
                file,
                at,
                block,
            });
    }
    for list in by_name.values_mut() {
        list.sort_by(|a, b| (&a.file, a.at, &a.key).cmp(&(&b.file, b.at, &b.key)));
    }
    for file in &unknown_top {
        top_names.remove(file);
    }
    (by_name, top_names)
}

/// The kind a reference to a resource names (the plugin for XAML's
/// `xaml:resource`, answered by [`BROADER`]).
pub(crate) const RESOURCE_KIND: &str = "xaml:resource";

/// The candidates one name found in the scope that decided, with that
/// scope's rung and rule (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopedHits {
    /// By file, then the position of the name, then key.
    pub(crate) keys: Vec<NodeKey>,
    pub(crate) confidence: Confidence,
    pub(crate) rule: Rule,
}

/// The scopes of a reference by name as a resolved run's graph still holds
/// them, for a reference read after the extraction — a literal in another
/// language's code (plan 2c, U7: R22). The lookup blocks and the names of a
/// file outside every block are the declaration pass's and are gone by then,
/// so only two scopes are left: every file of the same project (the nearest
/// directory above the file holding a `.csproj` File node), then the whole
/// repository; and two candidates are variants only by their paths under
/// sibling project roots. Built once, looked up per name.
pub(crate) struct GraphScopes {
    target: String,
    by_name: BTreeMap<String, Vec<Candidate>>,
    scopes: Scopes<'static>,
}

impl GraphScopes {
    /// The candidates of the kinds that answer `target` (a node kind,
    /// `namespace:name`) over `graph`.
    pub(crate) fn of_graph(graph: &Graph, target: &str) -> GraphScopes {
        let targets = BTreeSet::from([target]);
        let (by_name, _) = collect_candidates(graph, &targets, &|_, _| None, &HashMap::new());
        let projects: BTreeSet<String> = graph
            .nodes_unordered()
            .filter(|n| {
                n.kind == NodeKind::File && n.key.as_str().to_ascii_lowercase().ends_with(".csproj")
            })
            .map(|n| parent(n.key.as_str()).to_string())
            .collect();
        GraphScopes {
            target: target.to_string(),
            by_name,
            scopes: Scopes {
                blocks: BTreeMap::new(),
                imports: BTreeMap::new(),
                projects,
                top_names: BTreeMap::new(),
            },
        }
    }

    /// What `name`, read in `file`, finds: the candidates of `file`'s project
    /// ([`Rule::ScopeProject`], [`Confidence::RulesBacked`]), else those of
    /// the repository ([`Rule::NameMatch`], [`Confidence::NameOnly`]); more
    /// than one that are not variants is [`Confidence::Ambiguous`]. `None`
    /// when nothing declares the name, or when more than `ceiling` are
    /// ambiguous (no edge, as for a reference of the markup itself).
    pub(crate) fn lookup(&self, file: &str, name: &str, ceiling: usize) -> Option<ScopedHits> {
        let candidates: Vec<&Candidate> = self
            .by_name
            .get(name)
            .into_iter()
            .flatten()
            .filter(|c| answers(&self.target, &c.kind))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let mut reach_of = |f: &str| BTreeSet::from([f.to_string()]);
        let (hits, rung, rule) = decide(&self.scopes, &mut reach_of, file, None, &candidates);
        let count = hits.len();
        let varied = count > 1 && are_variants(&self.scopes, &mut reach_of, &hits);
        let confidence = if count > 1 && !varied {
            Confidence::Ambiguous
        } else {
            rung
        };
        if confidence == Confidence::Ambiguous && count > ceiling {
            return None;
        }
        Some(ScopedHits {
            keys: hits.into_iter().map(|c| c.key.clone()).collect(),
            confidence,
            rule,
        })
    }
}

/// The candidates of the nearest scope that holds any, with that scope's rung
/// and rule.
fn decide<'c>(
    scopes: &Scopes<'_>,
    reach_of: &mut dyn FnMut(&str) -> BTreeSet<String>,
    file: &str,
    block: Option<u32>,
    candidates: &[&'c Candidate],
) -> (Vec<&'c Candidate>, Confidence, Rule) {
    let pick = |keep: &dyn Fn(&Candidate) -> bool| -> Vec<&'c Candidate> {
        candidates.iter().copied().filter(|c| keep(c)).collect()
    };
    for b in scopes.chain(file, block) {
        let hits = pick(&|c| c.file == file && c.block == Some(b));
        if !hits.is_empty() {
            return (hits, Confidence::RulesBacked, Rule::ScopeBlock);
        }
    }
    let hits = pick(&|c| c.file == file);
    if !hits.is_empty() {
        return (hits, Confidence::RulesBacked, Rule::ScopeFile);
    }
    let reach = reach_of(file);
    let hits = pick(&|c| reach.contains(&c.file));
    if !hits.is_empty() {
        return (hits, Confidence::RulesBacked, Rule::ScopeImports);
    }
    if let Some(project) = scopes.project_of(file) {
        let hits = pick(&|c| scopes.project_of(&c.file) == Some(project));
        if !hits.is_empty() {
            return (hits, Confidence::RulesBacked, Rule::ScopeProject);
        }
    }
    (candidates.to_vec(), Confidence::NameOnly, Rule::NameMatch)
}

/// Whether several candidates are configuration variants: each in a file of
/// its own, the files pairwise variants, none in the other's reach.
fn are_variants(
    scopes: &Scopes<'_>,
    reach_of: &mut dyn FnMut(&str) -> BTreeSet<String>,
    hits: &[&Candidate],
) -> bool {
    for (i, a) in hits.iter().enumerate() {
        for b in &hits[i + 1..] {
            if !scopes.variants(&a.file, &b.file)
                || reach_of(&a.file).contains(&b.file)
                || reach_of(&b.file).contains(&a.file)
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(projects: &[&str], top: &[(&str, &[&str])]) -> Scopes<'static> {
        Scopes {
            blocks: BTreeMap::new(),
            imports: BTreeMap::new(),
            projects: projects.iter().map(|p| p.to_string()).collect(),
            top_names: top
                .iter()
                .map(|(f, names)| (f.to_string(), names.iter().map(|n| n.to_string()).collect()))
                .collect(),
        }
    }

    #[test]
    fn a_resource_reference_is_answered_more_broadly_than_a_style_one() {
        for kind in ["xaml:resource", "xaml:template", "xaml:style"] {
            assert!(answers("xaml:resource", kind), "{kind}");
        }
        assert!(!answers("xaml:resource", "xaml:element"));
        assert!(!answers("xaml:resource", "xaml:view"));
        assert!(answers("xaml:style", "xaml:style"));
        assert!(!answers("xaml:style", "xaml:resource"));
        assert!(!answers("xaml:style", "xaml:template"));
        // A kind the table does not name answers itself only.
        assert!(answers("probe3:widget", "probe3:widget"));
        assert!(!answers("probe3:widget", "xaml:resource"));
    }

    #[test]
    fn a_file_belongs_to_the_nearest_directory_holding_a_project() {
        let s = scopes(&["App", "App/Sub", ""], &[]);
        assert_eq!(s.project_of("App/Views/A.xaml"), Some("App"));
        assert_eq!(s.project_of("App/Sub/B.xaml"), Some("App/Sub"));
        assert_eq!(s.project_of("Other/C.xaml"), Some(""));
        assert_eq!(scopes(&["App"], &[]).project_of("Other/C.xaml"), None);
    }

    #[test]
    fn the_same_path_under_sibling_project_roots_is_a_variant() {
        let s = scopes(&["Skins/Blue", "Skins/Red", "Skins/Red/Inner"], &[]);
        assert!(s.variants("Skins/Blue/Brushes.xaml", "Skins/Red/Brushes.xaml"));
        assert!(!s.variants("Skins/Blue/Brushes.xaml", "Skins/Red/Other.xaml"));
        // Not siblings: one root lies under the other's.
        assert!(!s.variants("Skins/Red/Brushes.xaml", "Skins/Red/Inner/Brushes.xaml"));
        // One project is no pair of roots.
        assert!(!s.variants("Skins/Blue/a/B.xaml", "Skins/Blue/b/B.xaml"));
    }

    #[test]
    fn the_same_set_of_names_outside_every_block_is_a_variant() {
        let s = scopes(
            &[],
            &[
                ("Light.xaml", &["B", "Fg"]),
                ("Dark.xaml", &["Fg", "B"]),
                ("Other.xaml", &["B"]),
                ("Empty.xaml", &[]),
                ("Empty2.xaml", &[]),
            ],
        );
        assert!(s.variants("Light.xaml", "Dark.xaml"));
        assert!(!s.variants("Light.xaml", "Other.xaml"));
        assert!(!s.variants("Empty.xaml", "Empty2.xaml"));
        assert!(!s.variants("Light.xaml", "Light.xaml"));
    }

    #[test]
    fn a_block_chain_ends_at_a_bad_index_and_at_a_loop() {
        let blocks = [
            LookupBlock {
                lines: (1, 9),
                parent: None,
            },
            LookupBlock {
                lines: (2, 8),
                parent: Some(0),
            },
            LookupBlock {
                lines: (3, 4),
                parent: Some(3),
            },
            LookupBlock {
                lines: (5, 6),
                parent: Some(2),
            },
        ];
        let mut s = scopes(&[], &[]);
        s.blocks.insert("f".into(), &blocks);
        assert_eq!(s.chain("f", Some(1)), [1, 0]);
        assert_eq!(s.chain("f", Some(3)), [3, 2]);
        assert_eq!(s.chain("f", Some(7)), Vec::<u32>::new());
        assert_eq!(s.chain("f", None), Vec::<u32>::new());
    }

    #[test]
    fn reach_follows_imports_through_a_cycle() {
        let mut s = scopes(&[], &[]);
        s.imports
            .insert("a".into(), BTreeSet::from(["b".to_string()]));
        s.imports.insert(
            "b".into(),
            BTreeSet::from(["a".to_string(), "c".to_string()]),
        );
        assert_eq!(
            s.reach("a"),
            BTreeSet::from(["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }
}
