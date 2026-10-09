//! `ra measure`: how far each resolution rule can be trusted, judged against
//! a SCIP index (plan A, U11, R10, KTD10).
//!
//! The rules of the shared resolver (KTD6) back, dispute or exclude the
//! candidates of every usage site; the index pairing (U9) writes beside each
//! site what the compiler bound it to. This pass reads the two side by side
//! and counts, per rule, how often the rule agreed with the index — and above
//! all whether it ever disputed or excluded the very candidate the index
//! confirmed. A rule that never did, over enough sites the index could judge,
//! is proposed for a right to delete (R11, KTD11). The proposal is all this
//! pass produces: granting is a manual change to the rights table in code
//! (`crate::extract::resolve::rights`), committed with the report it rests on.
//!
//! ## What is compared
//!
//! A rule is judged by what it SAID — the per-rule trace each candidate
//! carries (KTD7) — never by what became of the candidate. A rule without a
//! right only disputes, but its trace still says "exclude", and a rule with a
//! right deleted the candidate, but its trace still names it: either way the
//! claim being measured is in the trace. Every site is judged, also one over
//! the ambiguity ceiling, where generation runs no rule until one holds a
//! right to delete there. The index's verdict is [`IndexPairing`]: the
//! candidate it confirmed, or a symbol outside the repository. A site the index could not judge — no document for its file, a
//! drifted document, an occurrence that could not be paired, a target with no
//! single verified definition — refutes and confirms nothing; it is counted as
//! unmapped.
//!
//! The host rules `index-elsewhere` and `index-outside` are the index's own
//! verdicts, not rules of the catalog; they never appear in a trace and never
//! in a report. Only the catalog is measured.
//!
//! ## Per origin
//!
//! A measurement vouches for a rule together with the data the plugin fed it
//! (KTD11), so the report is split by origin — language, plugin component and
//! its SHA-256, plugin contract — exactly the fields a [`DeleteRight`] names.
//! A record is filled from one origin of one report, verbatim.
//!
//! ## What the report may contain
//!
//! Counts, rule ids, revisions and the identities of the index, the commit,
//! the resolver and the component. No file name, no symbol, nothing of the
//! measured code: the report is committed to a public repository. Keys are
//! sorted and there is no timestamp, so the same commit, index and build of RA
//! give the same bytes.

use crate::config::{Config, ConfigError, GraphOptions};
use crate::extract::resolve::rules::{may_delete, resolver_revision, CATALOG};
use crate::extract::{DeleteRight, Repo, RightsTable};
use crate::graph::{IndexPairing, SiteRecord, Verdict};
use crate::index::{ProjectOutcome, Record, RECORD_SCHEMA};
use crate::plugin::PluginSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Version of the [`Report`] format. Raised whenever a field changes meaning or
/// disappears: a committed report is read back by [`check_rights`], and a
/// right must never rest on a report read under another meaning.
pub const REPORT_SCHEMA: u32 = 1;

/// Default for [`Thresholds::min_evaluated_sites`]. Provisional: U14 sets the
/// real value from the first report on the reference repository and records
/// it in `docs/measurements/README.md`.
pub const DEFAULT_MIN_EVALUATED_SITES: u64 = 100;

/// Default for [`Thresholds::max_unmapped_percent`]. Provisional, as
/// [`DEFAULT_MIN_EVALUATED_SITES`].
pub const DEFAULT_MAX_UNMAPPED_PERCENT: u32 = 20;

/// The proposal thresholds (KTD10). The report states the ones it was made
/// with, so a reader can tell a rule that fell short of them from one that
/// failed outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    /// Fewest sites the index judged (`evaluated`) a rule needs before a
    /// right is proposed. A rule with none is never proposed, whatever this
    /// says: removing nothing confirmed proves nothing when nothing was
    /// looked at.
    pub min_evaluated_sites: u64,
    /// The share of a rule's judged sites the index could not judge
    /// (`unmapped / judged`) must stay BELOW this percentage. A rule whose
    /// claims mostly fall where the index is blind is measured on a sample
    /// that may not be the population. A rule with no unmapped site passes
    /// whatever this says, so 0 means "no unmapped site at all", not "never".
    pub max_unmapped_percent: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            min_evaluated_sites: DEFAULT_MIN_EVALUATED_SITES,
            max_unmapped_percent: DEFAULT_MAX_UNMAPPED_PERCENT,
        }
    }
}

/// What the operator asked for.
#[derive(Debug, Clone)]
pub struct MeasureOptions {
    /// The repository the index was built from.
    pub repo: PathBuf,
    /// The index, with `ra index`'s record beside it (`<stem>.record.json`).
    pub scip_index: PathBuf,
    /// The corpus directory whose cache the graph build reuses; `None` for
    /// the sidecar beside the repository, where generation writes by default.
    /// Nothing but the cache is written there. Only its declaration records
    /// are shared: the usage records of a measurement carry rule verdicts at
    /// sites over the ceiling that generation's do not, so a measurement
    /// reads and writes none ([`crate::extract::ExtractOptions::trace_over_ceiling`]).
    pub corpus_dir: Option<PathBuf>,
    /// The plugins generation is run with — the component the report names is
    /// the one that read the sites.
    pub plugins: Vec<PluginSpec>,
    /// As for generation (R23): it decides which sites end over the ceiling.
    pub ambiguity_ceiling: usize,
    /// As for generation; part of the cache fingerprint, so passing the same
    /// ones lets this run reuse generation's declaration records.
    pub secret_prefixes: Vec<String>,
    pub thresholds: Thresholds,
}

/// The report (KTD10). Serialized by [`to_json`] only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// [`REPORT_SCHEMA`].
    pub schema: u32,
    /// The commit the repository and the index were at.
    pub commit: String,
    /// SHA-256 of the index file, as its record states and the pairing read.
    pub index_sha256: String,
    /// The resolver's revision: every catalog rule with its revision, and the
    /// fingerprint of the rights table the run applied (KTD15).
    pub resolver: String,
    pub thresholds: Thresholds,
    /// One entry per origin that produced a usage site, in origin order.
    pub origins: Vec<OriginReport>,
}

/// The measurement for the sites one plugin component read for one language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginReport {
    /// The language, as the plugin manifest names it (`csharp`).
    pub language: String,
    /// The plugin component's id.
    pub component: String,
    /// That component's SHA-256, hex.
    pub component_sha256: String,
    /// The plugin contract it ran under (`0.2`).
    pub contract: String,
    pub sites: SiteCounts,
    /// Every catalog rule by id — also a rule that judged nothing here, so
    /// every report has the same shape and an absent rule is never mistaken
    /// for an unmeasured one.
    pub rules: BTreeMap<String, RuleReport>,
}

/// What the index said at the origin's sites. The five pairing counters
/// partition `total`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCounts {
    pub total: u64,
    /// Paired with a definition in the repository.
    pub resolved: u64,
    /// Paired with a symbol from outside the repository.
    pub external: u64,
    /// In a document the drift check did not verify.
    pub unverified_document: u64,
    /// In a verified document, but not paired.
    pub unmapped: u64,
    /// In a file the index has no document for.
    pub no_document: u64,
}

/// One rule's counts at one origin. Every counter is a number of SITES.
///
/// A rule "judged" a site when it gave at least one of its candidates a
/// verdict other than abstain. Of those, `unmapped` are sites the index could
/// not judge and `evaluated` the rest; the four outcome counters partition
/// `evaluated`, first match wins:
///
/// 1. `would_remove_confirmed` — disputed or excluded the candidate the index
///    confirmed;
/// 2. `confirmed` — backed the candidate the index confirmed;
/// 3. `refuted` — backed a candidate the index did not confirm: another one,
///    or any one where the index points outside the repository;
/// 4. `disputed_refuted` — only disputed or excluded candidates the index did
///    not confirm.
///
/// `judged == evaluated + unmapped`, and `evaluated == would_remove_confirmed
/// + confirmed + refuted + disputed_refuted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleReport {
    /// The catalog revision the counts are for.
    pub revision: u32,
    /// Whether the catalog lets this rule ever hold a right (KTD6).
    pub may_delete: bool,
    pub judged: u64,
    pub evaluated: u64,
    pub unmapped: u64,
    pub would_remove_confirmed: u64,
    pub confirmed: u64,
    pub refuted: u64,
    pub disputed_refuted: u64,
    pub proposal: Proposal,
}

/// Whether the report proposes a right to delete, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Proposal {
    /// Removed nothing the index confirmed, over enough evaluated sites, with
    /// few enough unmapped ones.
    Proposed,
    /// The catalog says this rule never deletes (KTD6): its negative claim is
    /// no proof, whatever a measurement finds.
    NeverDeletes,
    /// Disputed or excluded at least one candidate the index confirmed.
    RemovedConfirmed,
    /// Fewer evaluated sites than the threshold, or none at all.
    TooFewEvaluated,
    /// Too large a share of the rule's sites was out of the index's sight.
    UnmappedShareTooHigh,
}

/// Why a measurement was refused, or failed.
#[derive(Debug, thiserror::Error)]
pub enum MeasureError {
    #[error(
        "no record beside the index: expected {record} next to {index}. Only an index built by \
         `reverse-architect index` can be measured — its record says which commit it describes \
         and whether every project was indexed. Build one with `reverse-architect index <REPO>`"
    )]
    MissingRecord { index: PathBuf, record: PathBuf },
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the index record {path} is not readable: {detail}")]
    UnreadableRecord { path: PathBuf, detail: String },
    #[error(
        "the index record has format version {found}, this build reads version {expected}; \
         rebuild the index with this build's `reverse-architect index`"
    )]
    RecordSchema { found: u32, expected: u32 },
    #[error(
        "the index file does not match its record: the record was written for SHA-256 \
         {recorded}, the file hashes to {actual}. The index was replaced or changed after \
         `reverse-architect index` wrote it; rebuild it"
    )]
    IndexChanged { recorded: String, actual: String },
    #[error(
        "the index record names no commit: the index was not built from a git repository, so \
         nothing ties it to the code being measured"
    )]
    NoIndexCommit,
    #[error(
        "the repository has no commit to compare the index with: measurement needs the commit \
         the index was built at checked out"
    )]
    NoRepositoryCommit,
    #[error(
        "the index was built at commit {index}, but the repository is at {head}. Check out \
         {index} or rebuild the index at {head}"
    )]
    OtherCommit { index: String, head: String },
    #[error(
        "the index was built from a working tree with uncommitted changes, so it may describe \
         code no commit contains. Commit or discard the changes and rebuild the index"
    )]
    DirtyIndex,
    #[error(
        "the index is partial — {failed} of {total} projects were not indexed ({names}). A \
         rule whose deletions fall in a missing project would look clean; rebuild the index \
         with every project indexed"
    )]
    PartialIndex {
        failed: usize,
        total: usize,
        names: String,
    },
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Extract(#[from] crate::extract::ExtractError),
    #[error(transparent)]
    Generate(#[from] crate::GenError),
}

/// Measure every catalog rule against the index (KTD10).
///
/// Refuses, before any extraction, an index that cannot be trusted to judge
/// the code being measured — see [`verify_index`].
pub fn run(
    opts: &MeasureOptions,
    progress: &dyn crate::progress::Progress,
) -> Result<Report, MeasureError> {
    let record = verify_index(&opts.repo, &opts.scip_index)?;
    // The run a generation with this index would be, minus the projections:
    // every option that shapes the graph comes from the same place.
    let mut config = Config::for_graph(
        opts.repo.clone(),
        opts.corpus_dir.clone(),
        GraphOptions {
            plugins: opts.plugins.clone(),
            ambiguity_ceiling: opts.ambiguity_ceiling,
            scip_index: Some(opts.scip_index.clone()),
            secret_prefixes: opts.secret_prefixes.clone(),
        },
    )?;
    // The rules run at sites over the ceiling too. Generation settles such a
    // site before any rule runs, unless a rule there holds a right to delete
    // — and then the rule deletes there. A report that never judged those
    // sites would vouch for deletions at them all the same.
    config.trace_over_ceiling = true;
    let built = crate::build_graph(&config, progress)?;
    // The pairing hashed the bytes it read. Checked again because the file
    // could have been swapped between the check above and the read.
    let paired = built.graph.sites().index().map(|s| s.sha256.clone());
    if paired.as_deref() != Some(record.index_sha256.as_str()) {
        return Err(MeasureError::IndexChanged {
            recorded: record.index_sha256,
            actual: paired.unwrap_or_default(),
        });
    }

    progress.phase("measure", None);
    let mut origins: BTreeMap<OriginKey, Tally> = BTreeMap::new();
    let mut by_extension: BTreeMap<String, Option<OriginKey>> = BTreeMap::new();
    for site in built.graph.sites().records_sorted() {
        let ext = site.file.rsplit_once('.').map_or("", |(_, e)| e);
        let origin = by_extension
            .entry(ext.to_string())
            .or_insert_with(|| origin_of(&built.options.plugins, ext))
            .clone();
        // A usage site is only ever produced by a plugin, so a file without
        // one has none; skipped rather than counted under a made-up origin.
        if let Some(origin) = origin {
            origins.entry(origin).or_insert_with(Tally::new).add(site);
        }
    }
    progress.phase_done();

    Ok(Report {
        schema: REPORT_SCHEMA,
        commit: record.commit.unwrap_or_default(),
        index_sha256: record.index_sha256,
        resolver: resolver_revision(CATALOG, &config.rights),
        thresholds: opts.thresholds,
        origins: origins
            .into_iter()
            .map(|(key, tally)| tally.into_report(key, &opts.thresholds))
            .collect(),
    })
}

/// Read the record `ra index` wrote beside `index` and refuse an index that
/// cannot judge the code at `repo`'s HEAD (KTD10): one without a record, one
/// whose bytes are not the ones the record describes, one built at another
/// commit or from a working tree with changes, one missing projects.
pub fn verify_index(repo: &Path, index: &Path) -> Result<Record, MeasureError> {
    let record_path = record_path(index);
    let text = match std::fs::read_to_string(&record_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(MeasureError::MissingRecord {
                index: index.to_path_buf(),
                record: record_path,
            })
        }
        Err(source) => {
            return Err(MeasureError::Io {
                path: record_path,
                source,
            })
        }
    };
    let record: Record =
        serde_json::from_str(&text).map_err(|e| MeasureError::UnreadableRecord {
            path: record_path.clone(),
            detail: e.to_string(),
        })?;
    if record.schema != RECORD_SCHEMA {
        return Err(MeasureError::RecordSchema {
            found: record.schema,
            expected: RECORD_SCHEMA,
        });
    }
    let bytes = std::fs::read(index).map_err(|source| MeasureError::Io {
        path: index.to_path_buf(),
        source,
    })?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != record.index_sha256 {
        return Err(MeasureError::IndexChanged {
            recorded: record.index_sha256,
            actual,
        });
    }
    let index_commit = record.commit.clone().ok_or(MeasureError::NoIndexCommit)?;
    let head = Repo::open(repo)?
        .head_revision()
        .ok_or(MeasureError::NoRepositoryCommit)?;
    if index_commit != head {
        return Err(MeasureError::OtherCommit {
            index: index_commit,
            head,
        });
    }
    // `None` is a tree without git, which the missing commit already refused.
    if record.dirty != Some(false) {
        return Err(MeasureError::DirtyIndex);
    }
    let failed: Vec<&str> = record
        .projects
        .iter()
        .filter(|p| p.outcome != ProjectOutcome::Indexed)
        .map(|p| p.path.as_str())
        .collect();
    if record.partial || !failed.is_empty() {
        return Err(MeasureError::PartialIndex {
            failed: failed.len(),
            total: record.projects.len(),
            names: failed.join(", "),
        });
    }
    Ok(record)
}

/// Where `ra index` writes the record of `index`: `<stem>.record.json` beside it.
pub fn record_path(index: &Path) -> PathBuf {
    let stem = index
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "index".to_string());
    index.with_file_name(format!("{stem}.record.json"))
}

/// The report as committed: pretty JSON with every object's keys sorted, and a
/// final newline.
pub fn to_json(report: &Report) -> String {
    // Through a `Value`, whose maps are sorted (serde_json is built without
    // `preserve_order`), so the key order is the alphabet's rather than the
    // struct's — a reader diffing two reports sees the same layout always.
    let value = serde_json::to_value(report).expect("a report is plain serializable data");
    let mut json = serde_json::to_string_pretty(&value).expect("a JSON value always serializes");
    json.push('\n');
    json
}

/// Check every right of `table` against the reports under `root` (KTD11): the
/// record names a report in `docs/measurements/`, the report is there and
/// hashes to the recorded digest, and in it the rule removed nothing the
/// index confirmed and was proposed. Returns one line per problem; empty when
/// the table is consistent. `tests/measure.rs` runs it over the shipped table
/// and this repository — that test is what makes AE5 hold.
pub fn check_rights(table: &RightsTable, root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for right in table.rights() {
        let rule = right.rule.id();
        let in_measurements = right
            .report
            .strip_prefix("docs/measurements/")
            .is_some_and(|rest| !rest.is_empty() && !rest.split('/').any(|c| c == ".."));
        if !in_measurements {
            problems.push(format!(
                "{rule}: the report `{}` is not a file under docs/measurements/",
                right.report
            ));
            continue;
        }
        match std::fs::read(root.join(&right.report)) {
            Ok(bytes) => {
                if let Err(problem) = verify_right(right, &bytes) {
                    problems.push(format!("{rule}: {problem}"));
                }
            }
            Err(e) => problems.push(format!(
                "{rule}: cannot read the report `{}`: {e}",
                right.report
            )),
        }
    }
    problems
}

/// Check one right against the bytes of the report it names (KTD11, AE5).
pub fn verify_right(right: &DeleteRight, report: &[u8]) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(report));
    if actual != right.report_sha256 {
        return Err(format!(
            "the report hashes to {actual}, the record says {}",
            right.report_sha256
        ));
    }
    let report: Report = serde_json::from_slice(report)
        .map_err(|e| format!("the report is not a measurement report: {e}"))?;
    if report.schema != REPORT_SCHEMA {
        return Err(format!(
            "the report has format version {}, this build reads version {REPORT_SCHEMA}",
            report.schema
        ));
    }
    let origin = report
        .origins
        .iter()
        .find(|o| {
            o.language == right.language
                && o.component == right.component
                && o.component_sha256 == right.component_sha256
                && o.contract == right.contract
        })
        .ok_or_else(|| {
            format!(
                "the report measured no sites of {} from component {} ({}) on contract {}",
                right.language, right.component, right.component_sha256, right.contract
            )
        })?;
    let rule = origin
        .rules
        .get(right.rule.id())
        .ok_or("the report does not measure this rule")?;
    if rule.revision != right.revision {
        return Err(format!(
            "the report measured revision {}, the record grants revision {}",
            rule.revision, right.revision
        ));
    }
    if rule.would_remove_confirmed > 0 {
        return Err(format!(
            "in the report the rule disputed or excluded a candidate the index confirmed at {} \
             sites",
            rule.would_remove_confirmed
        ));
    }
    if rule.proposal != Proposal::Proposed {
        return Err(format!(
            "the report does not propose a right for the rule ({:?})",
            rule.proposal
        ));
    }
    Ok(())
}

/// What a right is matched against (KTD11), in report order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OriginKey {
    language: String,
    component: String,
    component_sha256: String,
    contract: String,
}

/// The origin of the sites in files with extension `ext`: the plugin that
/// reads them, as the resolver matches a right against it.
fn origin_of(plugins: &crate::plugin::PluginSet, ext: &str) -> Option<OriginKey> {
    let plugin = plugins.for_extension(ext)?;
    Some(OriginKey {
        language: plugin.manifest().language_for(ext)?.to_string(),
        component: plugin.id().to_string(),
        component_sha256: plugin.digest().to_string(),
        contract: plugin.contract().word().to_string(),
    })
}

/// The counts of one origin, as its sites arrive.
struct Tally {
    sites: SiteCounts,
    /// Indexed like [`CATALOG`].
    rules: Vec<RuleTally>,
}

#[derive(Default, Clone)]
struct RuleTally {
    judged: u64,
    unmapped: u64,
    would_remove_confirmed: u64,
    confirmed: u64,
    refuted: u64,
    disputed_refuted: u64,
}

/// What the index said at a site, as far as measurement cares.
enum IndexVerdict<'s> {
    /// The index confirmed this candidate.
    Confirmed(&'s crate::graph::NodeKey),
    /// The index points outside the repository: every candidate is wrong.
    Outside,
    /// The index could not judge the site.
    Blind,
}

impl Tally {
    fn new() -> Tally {
        Tally {
            sites: SiteCounts::default(),
            rules: vec![RuleTally::default(); CATALOG.len()],
        }
    }

    fn add(&mut self, site: &SiteRecord) {
        self.sites.total += 1;
        let verdict = match &site.index {
            Some(IndexPairing::Resolved { target }) => {
                self.sites.resolved += 1;
                IndexVerdict::Confirmed(target)
            }
            Some(IndexPairing::External) => {
                self.sites.external += 1;
                IndexVerdict::Outside
            }
            Some(IndexPairing::UnverifiedDocument) => {
                self.sites.unverified_document += 1;
                IndexVerdict::Blind
            }
            Some(IndexPairing::Unmapped) => {
                self.sites.unmapped += 1;
                IndexVerdict::Blind
            }
            None => {
                self.sites.no_document += 1;
                IndexVerdict::Blind
            }
        };
        for (spec, tally) in CATALOG.iter().zip(self.rules.iter_mut()) {
            // What the rule said, candidate by candidate — the trace, not the
            // outcome (module docs).
            let said: Vec<(&crate::graph::NodeKey, Verdict)> = site
                .candidates
                .iter()
                .flat_map(|c| {
                    c.trace
                        .iter()
                        .filter(|rv| rv.rule == spec.rule && rv.verdict != Verdict::Abstain)
                        .map(move |rv| (&c.target, rv.verdict))
                })
                .collect();
            if said.is_empty() {
                continue;
            }
            tally.judged += 1;
            // Abstentions are filtered out above, so anything not a backing is
            // a dispute or an exclusion.
            let backs_any = said.iter().any(|(_, v)| *v == Verdict::Back);
            match verdict {
                IndexVerdict::Blind => tally.unmapped += 1,
                IndexVerdict::Confirmed(confirmed) => {
                    let on_confirmed = |back: bool| {
                        said.iter()
                            .any(|(t, v)| *t == confirmed && (*v == Verdict::Back) == back)
                    };
                    if on_confirmed(false) {
                        tally.would_remove_confirmed += 1;
                    } else if on_confirmed(true) {
                        tally.confirmed += 1;
                    } else if backs_any {
                        tally.refuted += 1;
                    } else {
                        tally.disputed_refuted += 1;
                    }
                }
                IndexVerdict::Outside if backs_any => tally.refuted += 1,
                IndexVerdict::Outside => tally.disputed_refuted += 1,
            }
        }
    }

    fn into_report(self, key: OriginKey, thresholds: &Thresholds) -> OriginReport {
        let rules = CATALOG
            .iter()
            .zip(self.rules)
            .map(|(spec, t)| {
                let evaluated = t.judged - t.unmapped;
                let proposal = if !may_delete(spec.rule) {
                    Proposal::NeverDeletes
                } else if t.would_remove_confirmed > 0 {
                    Proposal::RemovedConfirmed
                } else if evaluated == 0 || evaluated < thresholds.min_evaluated_sites {
                    Proposal::TooFewEvaluated
                } else if t.unmapped > 0
                    && t.unmapped * 100 >= u64::from(thresholds.max_unmapped_percent) * t.judged
                {
                    Proposal::UnmappedShareTooHigh
                } else {
                    Proposal::Proposed
                };
                (
                    spec.rule.id().to_string(),
                    RuleReport {
                        revision: spec.revision,
                        may_delete: may_delete(spec.rule),
                        judged: t.judged,
                        evaluated,
                        unmapped: t.unmapped,
                        would_remove_confirmed: t.would_remove_confirmed,
                        confirmed: t.confirmed,
                        refuted: t.refuted,
                        disputed_refuted: t.disputed_refuted,
                        proposal,
                    },
                )
            })
            .collect();
        OriginReport {
            language: key.language,
            component: key.component,
            component_sha256: key.component_sha256,
            contract: key.contract,
            sites: self.sites,
            rules,
        }
    }
}
