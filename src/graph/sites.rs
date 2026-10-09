//! Usage-site records: what the shared resolver decided at every place a name
//! was used (plan A, KTD7).
//!
//! The graph's edges are at the level of RELATIONS — one edge per (owner, kind,
//! target), carrying the ids of its sites (Key Decision on relations). The
//! record of a site is the level below: the candidates its name matched, every
//! applicable rule's verdict on each of them, and what became of each one.
//! Index pairing (U9), measurement (U11) and model verification (U12) work on
//! these records, never on the edges, because a rule is judged by what it said
//! about one candidate at one place — the edge is a summary that has already
//! forgotten which site said what.
//!
//! The records never enter the corpus. A run that asks for them (`--sites`)
//! gets them written to `sites.jsonl` next to the manifest; every run keeps the
//! two counters `coverage.json` reports ([`SiteCounts`]).
//!
//! A run given a SCIP index keeps every record whatever `--sites` says: the
//! index pairing (U9) reads them after the walk, and writes what the index said
//! at each one back into it ([`SiteRecord::index`]).
//!
//! Only the counters are cached. A run that keeps no records never builds one —
//! at the scale of the reference repository a single `ToString` call site
//! holds a thousand candidates, each with its trace, and the records of one
//! generation ran to 6.5 GB on disk that no later run of the same kind ever
//! read — so its per-file usage record holds the file's [`SiteCounts`]
//! instead. A run that keeps records neither reads nor writes the usage
//! records, as a measurement never did: a cached record has no records to
//! serve it, and the counters alone could not stand in for them.

use super::{Confidence, NodeKey, Rule};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// The relation a usage site implies. A closed set of the three core kinds a
/// name can resolve into, rather than an [`super::EdgeKind`]: a declared edge
/// kind would need the corpus's vocabulary to become a name on the wire, and
/// no usage site ever produces one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    Calls,
    Instantiates,
    References,
}

impl SiteKind {
    /// The edge kind the site's relations have.
    pub fn edge_kind(self) -> super::EdgeKind {
        match self {
            SiteKind::Calls => super::EdgeKind::Calls,
            SiteKind::Instantiates => super::EdgeKind::Instantiates,
            SiteKind::References => super::EdgeKind::References,
        }
    }
}

/// One rule's verdict on one candidate (KTD5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The candidate belongs where the site points.
    Back,
    /// The candidate probably does not — but the rule's negative claim is not
    /// proof, so it only moves the relation to "maybe also".
    Dispute,
    /// The rule says the candidate is impossible. Applied as a deletion only
    /// when the rule holds a valid right to delete for this site (KTD11);
    /// otherwise it weighs exactly as [`Verdict::Dispute`]. Recorded as the
    /// rule said it either way, so measurement sees what the rule claimed,
    /// not what the rights table let it do.
    Exclude,
    /// The rule applied to the site and said nothing about this candidate —
    /// an override in a descendant of the declared type, an argument count in
    /// range. Recorded so the trace lists every applicable rule (KTD7).
    Abstain,
}

/// One entry of a candidate's trace: which rule, and what it said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RuleVerdict {
    pub rule: Rule,
    pub verdict: Verdict,
}

/// What became of one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CandidateOutcome {
    /// An edge with this rung, admitted or disputed by this rule.
    Rated { rung: Confidence, rule: Rule },
    /// Removed by this rule, which held a valid right to delete (KTD11).
    Deleted { rule: Rule },
    /// More candidates than the ambiguity ceiling survived exclusion, so the
    /// site became one ambiguity fact and no edge (R23).
    OverCap,
}

/// One candidate of a site, with its full trace (KTD7).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub target: NodeKey,
    /// Every applicable rule's verdict, in catalog order. Empty where no rule
    /// applies: a site without context (plugin contract 0.1, a language whose
    /// plugin reads none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<RuleVerdict>,
    #[serde(flatten)]
    pub outcome: CandidateOutcome,
}

/// One usage site and what the resolver decided there (KTD7).
///
/// Its stable id is `(file, owner, line, column, name, kind)` — the field order
/// below, which the derived [`Ord`] sorts by.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SiteRecord {
    /// Repo-root-relative, `/`-normalized source path.
    pub file: String,
    /// The node the site's relations originate at: the innermost declaration
    /// around it, or the file for module-level code.
    pub owner: NodeKey,
    /// The 1-based line of the usage (of the used name, when `column` is set).
    pub line: u32,
    /// The 1-based column of the used name, when the plugin reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// The simple name that was resolved.
    pub name: String,
    pub kind: SiteKind,
    /// False when the line alone could not tell which of several
    /// declarations on it encloses the site and no column decided it (KTD7).
    /// `owner` is then the nearest declaration known to enclose the site, or
    /// the file, and the site counts as unmapped rather than assigned.
    #[serde(default = "mapped", skip_serializing_if = "is_mapped")]
    pub owner_mapped: bool,
    /// How many candidates the name matched before any rule ran.
    pub candidate_count: usize,
    /// The candidates, with their traces and outcomes, in the order the symbol
    /// table lists them. Empty for a site over the ceiling that no rule with a
    /// right to delete could have thinned: such a site is settled by its count
    /// alone, with no per-candidate work (KTD5) — except in `ra measure`, which
    /// judges it, every candidate [`CandidateOutcome::OverCap`] with its trace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<CandidateRecord>,
    /// What an external index said at this site (plan A, U9, KTD8). `None`
    /// when the run had no index, and for a site in a file the index has no
    /// document for. The resolver's own `outcome` on each candidate is left as
    /// the resolver decided it: measurement (U11) compares exactly that with
    /// this, so overwriting it with the index's verdict would erase the claim
    /// being measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexPairing>,
}

/// What the index pairing concluded at one usage site (plan A, KTD8).
///
/// Only [`IndexPairing::Resolved`] and [`IndexPairing::External`] are
/// verdicts. The other two say the index was not asked, and a site that was
/// not asked refutes nothing: its candidates keep the rungs the rules gave
/// them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "pairing")]
pub enum IndexPairing {
    /// Paired with one occurrence, whose symbol the index defines in a
    /// verified document of this repository, at this node.
    Resolved { target: NodeKey },
    /// Paired with one occurrence of a symbol from outside the repository —
    /// a package no project of the repository defines anything in.
    External,
    /// The site's document drifted from the index, or had nothing to verify
    /// it by, so nothing in it is paired.
    UnverifiedDocument,
    /// The document was verified, but the site could not be paired with
    /// exactly one occurrence — none there, several (two calls of one name on
    /// a line without columns), a range the merged index marks as
    /// conflicting with a repository target among its targets, an occurrence
    /// whose column could not be converted, an owner the line alone could not
    /// pick — or the
    /// occurrence's symbol has no single verified definition in the
    /// repository.
    Unmapped,
}

fn mapped() -> bool {
    true
}

fn is_mapped(mapped: &bool) -> bool {
    *mapped
}

impl SiteRecord {
    /// Whether the site ended over the ambiguity ceiling: one fact, no edges.
    pub fn over_cap(&self) -> bool {
        (self.candidates.is_empty() && self.candidate_count > 0)
            || self
                .candidates
                .iter()
                .any(|c| c.outcome == CandidateOutcome::OverCap)
    }
}

/// The two counters `coverage.json` reports about usage sites, for any set of
/// them — one file's, or the run's.
///
/// What a run that keeps no records builds and caches in place of them (see
/// the module docs). Both counters are sums, so folding files in any order
/// gives the same totals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCounts {
    /// Candidates a rule with a right to delete removed, per rule.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    deleted_by_rule: BTreeMap<Rule, u64>,
    /// Sites that ended over the ambiguity ceiling.
    #[serde(default, skip_serializing_if = "is_zero")]
    over_cap_sites: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl SiteCounts {
    /// Count one site: whether it ended over the ceiling, and the rule that
    /// removed each of its deleted candidates.
    pub(crate) fn tally(&mut self, over_cap: bool, deleted: impl IntoIterator<Item = Rule>) {
        if over_cap {
            self.over_cap_sites += 1;
        }
        for rule in deleted {
            *self.deleted_by_rule.entry(rule).or_default() += 1;
        }
    }

    /// Fold another set's counts into this one.
    pub(crate) fn add(&mut self, other: &SiteCounts) {
        self.over_cap_sites += other.over_cap_sites;
        for (rule, n) in &other.deleted_by_rule {
            *self.deleted_by_rule.entry(*rule).or_default() += n;
        }
    }

    /// Whether nothing was counted — what a file without usage sites, and
    /// every declaration record, holds.
    pub(crate) fn is_empty(&self) -> bool {
        *self == SiteCounts::default()
    }
}

/// The run's usage-site ledger: the two counters coverage reports, always, and
/// the full records when the run asked for them.
///
/// Records are kept only on request because at the scale of the reference
/// repository they run to millions; the counters are what every run needs,
/// and they are folded in as each file's sites arrive — from its records, or,
/// in a run that keeps none, from the file's [`SiteCounts`].
#[derive(Debug, Default)]
pub struct SiteLedger {
    keep: bool,
    records: Vec<SiteRecord>,
    counts: SiteCounts,
    /// Where each declaration's name is written, `(file, key) → (line,
    /// column)`, both 1-based — kept only for a run with an index, which maps
    /// the index's definitions onto nodes by it (KTD8). Nothing else in the
    /// graph carries a column, and the language walk drops these once its
    /// symbol table is built.
    declaration_names: HashMap<(String, NodeKey), (u32, u32)>,
    /// What the index pairing found, when the run had an index.
    index: Option<IndexSummary>,
}

/// The index pairing's run-level result (plan A, U9): which index, and how far
/// the usage sites paired with it. `coverage.json` reports it.
///
/// The five site counters partition `considered`:
/// `considered == matched + external + unverified_document + unmapped`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexSummary {
    /// SHA-256 of the index file's bytes, lowercase hex — which index the
    /// pairing read, since the same path can hold another index tomorrow.
    pub sha256: String,
    /// Usage sites in files the index has a document for.
    pub considered: u64,
    /// …paired with a definition in the repository.
    pub matched: u64,
    /// …paired with a symbol from outside the repository.
    pub external: u64,
    /// …in a document the drift check did not verify, so not paired.
    pub unverified_document: u64,
    /// …in a verified document, and not paired (see [`IndexPairing::Unmapped`]).
    pub unmapped: u64,
}

impl SiteLedger {
    /// Keep every record from now on, not only the counters.
    pub fn keep_records(&mut self) {
        self.keep = true;
    }

    /// Bank one site: count it, and keep it when asked to.
    pub fn record(&mut self, record: SiteRecord) {
        self.counts.tally(
            record.over_cap(),
            record.candidates.iter().filter_map(|c| match c.outcome {
                CandidateOutcome::Deleted { rule } => Some(rule),
                _ => None,
            }),
        );
        if self.keep {
            self.records.push(record);
        }
    }

    /// Bank the counts of sites whose records were not built.
    pub(crate) fn count(&mut self, counts: &SiteCounts) {
        self.counts.add(counts);
    }

    /// The kept records in their stable-id order — the order the walk banked
    /// them in depends on nothing, but sorting here makes that a property of
    /// the type rather than of every caller.
    pub fn records_sorted(&self) -> Vec<&SiteRecord> {
        let mut out: Vec<&SiteRecord> = self.records.iter().collect();
        out.sort();
        out
    }

    /// The kept records in the order they were banked — for a pass that only
    /// looks records up and so must not pay for [`Self::records_sorted`]'s
    /// sort of millions of them.
    pub(crate) fn records_unordered(&self) -> &[SiteRecord] {
        &self.records
    }

    /// Candidates a rule with a right to delete removed, per rule.
    pub fn deleted_by_rule(&self) -> &BTreeMap<Rule, u64> {
        &self.counts.deleted_by_rule
    }

    /// Sites that ended over the ambiguity ceiling.
    pub fn over_cap_sites(&self) -> u64 {
        self.counts.over_cap_sites
    }

    /// The kept records in the order they were banked, for the index pairing
    /// to write its outcome into.
    pub(crate) fn records_mut(&mut self) -> &mut [SiteRecord] {
        &mut self.records
    }

    /// Keep where every declaration's name is written, for the index pairing.
    pub(crate) fn keep_declaration_names(&mut self, names: HashMap<(String, NodeKey), (u32, u32)>) {
        self.declaration_names = names;
    }

    /// Where the declaration `key` writes its name in `file`, when its plugin
    /// reports one (contract 0.2) and the run kept them.
    pub(crate) fn declaration_name(&self, file: &str, key: &NodeKey) -> Option<(u32, u32)> {
        // A borrowed lookup would need the key type to borrow as a pair of
        // references; one short clone per declaration is cheaper to read.
        self.declaration_names
            .get(&(file.to_string(), key.clone()))
            .copied()
    }

    /// Record what the index pairing found.
    pub(crate) fn set_index(&mut self, summary: IndexSummary) {
        self.index = Some(summary);
    }

    /// What the index pairing found, when the run had an index.
    pub fn index(&self) -> Option<&IndexSummary> {
        self.index.as_ref()
    }
}
