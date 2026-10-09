//! Pairing usage sites with an external index (plan A, U9, KTD8, R12).
//!
//! The grammar emits `Calls`; SCIP has no "call" role and emits only
//! occurrences, which [`super::scip`] reads as `References`. The two never meet
//! in merge, so an index on its own could neither confirm a heuristic call nor
//! refute it. This pass makes them meet at the level the plan judges rules at —
//! the usage site, not the edge (KTD7): each site record is paired with the
//! index occurrence written at the same `(file, line, column, name)`, the name
//! being the one the occurrence's moniker ends in and the column where its
//! range starts.
//!
//! ## What a pairing does
//!
//! - **The occurrence's symbol is defined in the repository, at one of the
//!   site's candidates.** That candidate's relation is restated at the index's
//!   precision ([`Resolution::Resolved`], so [`Confidence::ResolvedByIndex`] is
//!   derived), merge makes that claim the edge's winner, and the occurrence
//!   emits no `References` of its own — one relation, not the same one twice.
//!   Every other candidate of the site becomes a guess ("maybe also").
//! - **It is defined in the repository, but at no candidate of the site** —
//!   one a rule with a right to delete removed, one over the ceiling, one of
//!   another role. Every remaining candidate becomes a guess, and nothing is
//!   resurrected: the flowchart asks the index only after exclusion and the
//!   ceiling, and how often an index contradicts a deletion is what
//!   measurement (U11) reads off the record. The occurrence keeps the
//!   `References` it always had.
//! - **It is a symbol from outside the repository** (`System.String`). Every
//!   candidate becomes a guess. So does a range the merged index marks as
//!   conflicting when every target the projects wrote there is from outside
//!   it — two frameworks numbering `Trim`'s overloads differently still agree
//!   the call leaves the repository.
//! - **Nothing could be paired** — no occurrence there, several, a
//!   conflicting range with a repository target among its targets, an
//!   occurrence whose column could not be converted into the plugin's, a
//!   symbol with no single verified definition, an owner the line alone could
//!   not pick, or a document the drift check did not verify. The site refutes
//!   nothing and its candidates keep their rungs.
//!
//! "Becomes a guess" happens at the site, and an edge is the summary of its
//! sites (KTD5): a relation another site still supports keeps that site's
//! rung and rule. Where the refuted site is what the edge stands on, the edge
//! names the index as its rule — `index-elsewhere` or `index-outside` — so
//! the card's "maybe also" says the index is why, not whatever rule rated the
//! candidate before. Only the edges are recomputed: the resolver's record of
//! what each rule said stays as it was — no trace ever names the index — and
//! the pairing is written beside it ([`SiteRecord::index`]) for measurement to
//! compare.
//!
//! ## Where it runs
//!
//! Inside the scip extractor's run, after its drift gate and before it emits —
//! the extractor must know which occurrences a site took before it writes
//! `References` for the rest, and the index is parsed once. The scip extractor
//! has no cache phase, so neither does this: an index is not part of the
//! fingerprint the per-file cache is keyed on, and a pairing cached under it
//! would be served for a different index.
//!
//! ## Pairing without columns
//!
//! A site from a plugin on contract 0.1 has no column. It pairs on `(file,
//! line, name)` only where that line holds exactly one occurrence of the name
//! AND exactly one site of it: two `Append` calls on one line are two
//! occurrences no column can assign, so both sites stay unmapped. In the other
//! direction too — a 0.2 site pairs only when no other site claims the same
//! position.

use super::ids;
use crate::graph::{
    CandidateOutcome, Confidence, Edge, EdgeKind, Graph, IndexPairing, IndexSummary, NodeKey,
    Provenance, Rule, SiteId, SiteRecord, Tier,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// What the index says about one file that has a document in it.
pub(super) enum IndexedFile {
    /// The drift check verified the document: its reference occurrences.
    Verified(Vec<KeptOccurrence>),
    /// The document drifted or could not be verified; nothing in it pairs.
    Unverified,
}

/// One reference occurrence of a verified document.
pub(super) struct KeptOccurrence {
    /// Its position in the document's occurrence list — how the emission that
    /// follows recognizes an occurrence the pairing folded.
    pub(super) at: usize,
    /// 1-based line where the occurrence's range starts.
    pub(super) line: u32,
    /// 1-based column where it starts, counted in characters as the plugin
    /// counts its sites' columns — converted from the index's own encoding,
    /// and `None` where that was not possible. Such an occurrence still counts
    /// on its line for a site without a column, and pairs with no site that
    /// has one.
    pub(super) column: Option<u32>,
    /// The simple name its moniker ends in.
    pub(super) name: String,
    pub(super) target: OccurrenceTarget,
}

/// What an occurrence's symbol is (KTD8's three groups).
pub(super) enum OccurrenceTarget {
    /// Defined in a verified document of the repository, at this node.
    Repository(NodeKey),
    /// From a package no project of the repository defines anything in —
    /// or a conflicting range all of whose targets are.
    External,
    /// The repository's own, with no single verified definition — or a range
    /// the merged index marks as conflicting with a repository target among
    /// its targets. Refutes nothing.
    Unresolved,
}

/// Pair every kept site record with the index, write each outcome into its
/// record and the run's summary into the ledger, and bring the edges in line.
/// Returns the occurrences that a site's own relation took over, as `(file,
/// position)`, which must emit no `References` of their own.
pub(super) fn pair(
    graph: &mut Graph,
    files: &BTreeMap<String, IndexedFile>,
    sha256: String,
) -> BTreeSet<(String, usize)> {
    let occurrences = OccurrenceIndex::new(files);
    let mut summary = IndexSummary {
        sha256,
        ..IndexSummary::default()
    };
    let mut effects = Effects::default();

    let records = graph.sites_mut().records_mut();
    let positions = site_positions(records, files);
    for record in records.iter_mut() {
        let Some(file) = files.get(&record.file) else {
            continue;
        };
        summary.considered += 1;
        let (pairing, occurrence) = match file {
            IndexedFile::Unverified => (IndexPairing::UnverifiedDocument, None),
            IndexedFile::Verified(_) => pair_one(record, &occurrences, &positions),
        };
        match &pairing {
            IndexPairing::Resolved { .. } => summary.matched += 1,
            IndexPairing::External => summary.external += 1,
            IndexPairing::UnverifiedDocument => summary.unverified_document += 1,
            IndexPairing::Unmapped => summary.unmapped += 1,
        }
        effects.collect(record, &pairing, occurrence);
        record.index = Some(pairing);
    }

    graph.sites_mut().set_index(summary);
    effects.resolve(graph);
    effects.rerate(graph);
    effects.folded
}

/// The kept occurrences by `(file, line, name)`.
struct OccurrenceIndex<'f> {
    by_place: HashMap<(&'f str, u32, &'f str), Vec<&'f KeptOccurrence>>,
}

impl<'f> OccurrenceIndex<'f> {
    fn new(files: &'f BTreeMap<String, IndexedFile>) -> Self {
        let mut by_place: HashMap<(&str, u32, &str), Vec<&KeptOccurrence>> = HashMap::new();
        for (file, indexed) in files {
            let IndexedFile::Verified(kept) = indexed else {
                continue;
            };
            for occurrence in kept {
                by_place
                    .entry((file.as_str(), occurrence.line, occurrence.name.as_str()))
                    .or_default()
                    .push(occurrence);
            }
        }
        OccurrenceIndex { by_place }
    }

    fn at<'a>(&'a self, file: &'a str, line: u32, name: &'a str) -> &'a [&'f KeptOccurrence] {
        // The map is covariant in its key, so it can be asked with keys that
        // borrow less long than the ones it holds.
        let by_place: &'a HashMap<(&'a str, u32, &'a str), Vec<&'f KeptOccurrence>> =
            &self.by_place;
        by_place.get(&(file, line, name)).map_or(&[], Vec::as_slice)
    }
}

/// How many site records claim each `(file, line, column, name)` — a position
/// two records claim pairs with neither.
type Positions = HashMap<(String, u32, Option<u32>, String), usize>;

fn site_positions(records: &[SiteRecord], files: &BTreeMap<String, IndexedFile>) -> Positions {
    let mut positions = Positions::new();
    for record in records {
        if files.contains_key(&record.file) {
            *positions
                .entry((
                    record.file.clone(),
                    record.line,
                    record.column,
                    record.name.clone(),
                ))
                .or_default() += 1;
        }
    }
    positions
}

/// Pair one site of a verified document, with the occurrence it paired with.
fn pair_one<'f>(
    record: &SiteRecord,
    occurrences: &OccurrenceIndex<'f>,
    positions: &Positions,
) -> (IndexPairing, Option<&'f KeptOccurrence>) {
    // An owner the line alone could not pick is a relation nobody can state,
    // whatever the index says about its target (KTD7).
    if !record.owner_mapped {
        return (IndexPairing::Unmapped, None);
    }
    let claimed = positions
        .get(&(
            record.file.clone(),
            record.line,
            record.column,
            record.name.clone(),
        ))
        .copied()
        .unwrap_or(0);
    if claimed != 1 {
        return (IndexPairing::Unmapped, None);
    }
    let here = occurrences.at(&record.file, record.line, &record.name);
    let hits: Vec<&KeptOccurrence> = match record.column {
        Some(column) => here
            .iter()
            .copied()
            .filter(|o| o.column == Some(column))
            .collect(),
        None => here.to_vec(),
    };
    let [only] = hits.as_slice() else {
        return (IndexPairing::Unmapped, None);
    };
    let pairing = match &only.target {
        OccurrenceTarget::Repository(key) => IndexPairing::Resolved {
            target: key.clone(),
        },
        OccurrenceTarget::External => IndexPairing::External,
        OccurrenceTarget::Unresolved => IndexPairing::Unmapped,
    };
    (pairing, Some(only))
}

/// What one candidate stands on at one site once the index has spoken: its
/// rung, the rule that gave it, and how many candidates the site kept.
/// Keyed by `(site, owner, kind, target)`.
type Standing = HashMap<(SiteId, NodeKey, EdgeKind, NodeKey), (Confidence, Rule, usize)>;

fn site_id(record: &SiteRecord) -> SiteId {
    SiteId {
        file: record.file.clone(),
        line: record.line,
        column: record.column,
    }
}

/// The candidates a site turned into edges, with their rungs and rules.
fn rated(record: &SiteRecord) -> impl Iterator<Item = (&NodeKey, Confidence, Rule)> {
    record.candidates.iter().filter_map(|c| match c.outcome {
        CandidateOutcome::Rated { rung, rule } => Some((&c.target, rung, rule)),
        _ => None,
    })
}

/// What the pairings ask of the graph, gathered while the records are borrowed
/// and applied once they are not.
#[derive(Default)]
struct Effects {
    /// `(owner, kind, target)` → the sites the index confirmed it at.
    resolved: BTreeMap<(NodeKey, EdgeKind, NodeKey), BTreeSet<SiteId>>,
    /// `owner` → `(kind, target, site)` the index refuted → why: the rule the
    /// lowered claim names. Keyed by owner so the edge walk can ask with the
    /// key it already holds.
    refuted: HashMap<NodeKey, HashMap<(EdgeKind, NodeKey, SiteId), Rule>>,
    folded: BTreeSet<(String, usize)>,
}

impl Effects {
    fn collect(
        &mut self,
        record: &SiteRecord,
        pairing: &IndexPairing,
        occurrence: Option<&KeptOccurrence>,
    ) {
        let confirmed = match pairing {
            IndexPairing::Resolved { target } => Some(target),
            IndexPairing::External => None,
            IndexPairing::UnverifiedDocument | IndexPairing::Unmapped => return,
        };
        let kind = record.kind.edge_kind();
        let site = site_id(record);
        let confirms_a_candidate =
            confirmed.is_some_and(|target| rated(record).any(|(t, _, _)| t == target));
        let why = match confirmed {
            Some(_) => Rule::IndexElsewhere,
            None => Rule::IndexOutside,
        };
        for (target, _, _) in rated(record) {
            if confirms_a_candidate && Some(target) == confirmed {
                self.resolved
                    .entry((record.owner.clone(), kind, target.clone()))
                    .or_default()
                    .insert(site.clone());
            } else {
                self.refuted
                    .entry(record.owner.clone())
                    .or_default()
                    .insert((kind, target.clone(), site.clone()), why);
            }
        }
        if confirms_a_candidate {
            if let Some(occurrence) = occurrence {
                self.folded.insert((record.file.clone(), occurrence.at));
            }
        }
    }

    /// Restate every confirmed relation at the index's precision. One claim per
    /// relation, anchored at its first confirmed site, carrying every one of
    /// them — the shape a heuristic relation has, so merge folds the two into
    /// one edge with the index's claim the winner and the grammar's in dissent.
    fn resolve(&self, graph: &mut Graph) {
        for ((owner, kind, target), sites) in &self.resolved {
            let Some(first) = sites.first() else {
                continue;
            };
            let provenance =
                Provenance::new(&first.file, Some(first.line), ids::scip(), Tier::Measured)
                    .resolved();
            graph.add_edge(
                owner,
                target,
                Edge::new(*kind, provenance).at_sites(sites.iter().cloned()),
            );
        }
    }

    /// Re-derive the rung of every name-resolved edge the index refuted at one
    /// of its sites: the strongest rung among its sites, a refuted site counting
    /// as a guess (KTD5). The anchor moves with it, to the strongest site, the
    /// lowest one on a tie.
    fn rerate(&self, graph: &mut Graph) {
        if self.refuted.is_empty() {
            return;
        }
        let refuted_by = |from: &NodeKey, kind: EdgeKind, to: &NodeKey, site: &SiteId| {
            self.refuted
                .get(from)
                .and_then(|map| map.get(&(kind, to.clone(), site.clone())))
                .copied()
        };
        let touched = |from: &NodeKey, to: &NodeKey, edge: &Edge| {
            edge.provenance.resolution == crate::graph::Resolution::Heuristic
                && edge.provenance.rung().is_some()
                && self.refuted.contains_key(from)
                && edge
                    .sites
                    .iter()
                    .any(|site| refuted_by(from, edge.kind, to, site).is_some())
        };

        // Every site of every touched edge, and what each candidate stands on
        // there — read from the records, the only place that still knows.
        let mut sites: HashSet<SiteId> = HashSet::new();
        for e in graph.edges_unordered() {
            if touched(e.from, e.to, e.edge) {
                sites.extend(e.edge.sites.iter().cloned());
            }
        }
        let mut standing: Standing = HashMap::new();
        for record in graph.sites().records_sorted() {
            let site = site_id(record);
            if !sites.contains(&site) {
                continue;
            }
            let kind = record.kind.edge_kind();
            let count = rated(record).count();
            for (target, rung, rule) in rated(record) {
                let (rung, rule) = match refuted_by(&record.owner, kind, target, &site) {
                    Some(why) => (Confidence::Ambiguous, why),
                    None => (rung, rule),
                };
                let key = (site.clone(), record.owner.clone(), kind, target.clone());
                let held = standing.entry(key).or_insert((rung, rule, count));
                if rung < held.0 {
                    *held = (rung, rule, count);
                }
            }
        }

        graph.rewrite_edges(|from, to, edge| {
            if !touched(from, to, edge) {
                return None;
            }
            let (site, (rung, rule, count)) = edge
                .sites
                .iter()
                .filter_map(|site| {
                    standing
                        .get(&(site.clone(), from.clone(), edge.kind, to.clone()))
                        .map(|held| (site, *held))
                })
                .min_by(|a, b| (a.1 .0, a.0).cmp(&(b.1 .0, b.0)))?;
            let mut provenance = Provenance::new(
                &site.file,
                Some(site.line),
                edge.provenance.extractor.clone(),
                edge.provenance.tier,
            )
            .rated(rung, rule);
            if count > 1 {
                provenance = provenance.ambiguous(count);
            }
            Some(Edge {
                kind: edge.kind,
                provenance,
                dissent: edge.dissent.clone(),
                sites: edge.sites.clone(),
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{CandidateRecord, SiteKind};

    fn record(line: u32, column: Option<u32>, name: &str, targets: &[&str]) -> SiteRecord {
        SiteRecord {
            file: "src/Report.cs".to_string(),
            owner: NodeKey::new("sym:csharp:N#Report.Write()"),
            line,
            column,
            name: name.to_string(),
            kind: SiteKind::Calls,
            owner_mapped: true,
            candidate_count: targets.len(),
            candidates: targets
                .iter()
                .map(|t| CandidateRecord {
                    target: NodeKey::new(*t),
                    trace: Vec::new(),
                    outcome: CandidateOutcome::Rated {
                        rung: Confidence::Ambiguous,
                        rule: Rule::NameMatch,
                    },
                })
                .collect(),
            index: None,
        }
    }

    fn occurrence(at: usize, line: u32, column: u32, name: &str, target: &str) -> KeptOccurrence {
        KeptOccurrence {
            at,
            line,
            column: Some(column),
            name: name.to_string(),
            target: OccurrenceTarget::Repository(NodeKey::new(target)),
        }
    }

    fn pairings(records: &[SiteRecord], kept: Vec<KeptOccurrence>) -> Vec<IndexPairing> {
        let files: BTreeMap<String, IndexedFile> =
            [("src/Report.cs".to_string(), IndexedFile::Verified(kept))]
                .into_iter()
                .collect();
        let occurrences = OccurrenceIndex::new(&files);
        let positions = site_positions(records, &files);
        records
            .iter()
            .map(|r| pair_one(r, &occurrences, &positions).0)
            .collect()
    }

    /// A 0.1 plugin reports no column, so two `Append` calls on one line are
    /// two records of one position — and two occurrences no column can assign.
    /// Neither pairs.
    #[test]
    fn two_calls_of_one_name_on_one_line_without_columns_stay_unmapped() {
        let targets = ["sym:A.Append()", "sym:B.Append()"];
        let records = [
            record(4, None, "Append", &targets),
            record(4, None, "Append", &targets),
        ];
        let kept = vec![
            occurrence(0, 4, 5, "Append", "sym:A.Append()"),
            occurrence(1, 4, 18, "Append", "sym:B.Append()"),
        ];

        assert_eq!(
            pairings(&records, kept),
            [IndexPairing::Unmapped, IndexPairing::Unmapped]
        );
    }

    /// The same two calls with columns pair one by one, each with the
    /// occurrence written where its own name starts.
    #[test]
    fn two_calls_of_one_name_on_one_line_with_columns_pair_one_by_one() {
        let targets = ["sym:A.Append()", "sym:B.Append()"];
        let records = [
            record(4, Some(5), "Append", &targets),
            record(4, Some(18), "Append", &targets),
        ];
        let kept = vec![
            occurrence(0, 4, 5, "Append", "sym:A.Append()"),
            occurrence(1, 4, 18, "Append", "sym:B.Append()"),
        ];

        assert_eq!(
            pairings(&records, kept),
            [
                IndexPairing::Resolved {
                    target: NodeKey::new("sym:A.Append()")
                },
                IndexPairing::Resolved {
                    target: NodeKey::new("sym:B.Append()")
                },
            ]
        );
    }

    /// Without a column a site still pairs where its line holds one occurrence
    /// of the name and no other site of it.
    #[test]
    fn a_lone_call_without_a_column_pairs_by_its_line() {
        let records = [record(7, None, "Run", &["sym:A.Run()", "sym:B.Run()"])];
        let kept = vec![occurrence(3, 7, 12, "Run", "sym:A.Run()")];

        assert_eq!(
            pairings(&records, kept),
            [IndexPairing::Resolved {
                target: NodeKey::new("sym:A.Run()")
            }]
        );
    }
}
