//! Tier 1 (T1-KTD1): opt-in LLM narrative enrichment over the Tier 0 fact graph.
//! Lives entirely behind the `enrich` subcommand; the default run never enters here.

/// One accepted narrative for a leaf, with its audit trail (T1-KTD9). Kept in
/// the enrich layer rather than as a graph `Fact`, so the "measured vs.
/// written-by-LLM" boundary stays clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Narrative {
    /// The leaf/view id this narrative belongs to.
    pub view_id: String,
    /// The accepted narrative text.
    pub text: String,
    /// Backend that produced it (LlmBackend::id, a fixed &'static str such as
    /// "claude-cli"). Note: `id()` returns a static str with no model suffix, so
    /// this is the backend family, not "claude-cli:opus"; the model is reported
    /// separately in the run log.
    pub backend_id: String,
    /// Prompt template version (enrich::prompt::PROMPT_VERSION).
    pub prompt_version: String,
    /// Output language code.
    pub lang: String,
    /// Audit trail (T1-KTD9): the leaf facts this narrative was framed around —
    /// the same fact texts that seeded the licensed set. Lets a reader/test see
    /// what the narrative was built from. Carried at `Tier::Narrated`.
    pub backed_by: Vec<String>,
}

pub mod backend;
pub mod bundle;
pub mod claude_cli;
pub mod guardrail;
pub mod openai;
pub mod prompt;
pub mod render;

use crate::enrich::backend::{BackendError, LlmBackend};
use crate::enrich::bundle::{licensed_set, FactBundle};
use crate::enrich::guardrail::{check, Verdict};
use crate::enrich::prompt::{build_prompt, PROMPT_VERSION};
use crate::enrich::render::{render_degraded_leaf, render_enriched_leaf};
use crate::graph::Graph;
use crate::lens;
use crate::render::{Corpus, RenderedFile};
use crate::taxonomy;

/// Resolved configuration for one `enrich` run (the LLM half; the Tier 0 half is
/// carried by `crate::config::Config`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichConfig {
    pub lang: String,
    pub retries: u8,
    pub profile: Option<String>,
    /// Intra-corpus link style for the enriched map (spec C1). Mirrors the Tier 0
    /// `--links` flag.
    pub links: crate::cli::LinkStyle,
}

/// Per-run honesty report (T1-KTD5 / E9): how much of the corpus got narrated.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EnrichReport {
    pub enriched: usize,
    pub degraded: usize,
    /// (view_id, reason) for each degraded leaf.
    pub degradations: Vec<(String, String)>,
}

/// What an enrich run produces: the enriched corpus and the run report.
#[derive(Debug)]
pub struct EnrichResult {
    pub corpus: Corpus,
    pub report: EnrichReport,
    pub narratives: Vec<Narrative>,
}

/// Errors from the enrich pipeline.
#[derive(Debug, thiserror::Error)]
pub enum EnrichError {
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Profile(#[from] taxonomy::profile::UnknownProfile),
}

/// Enrich the corpus: per leaf, narrate around its facts, guardrail the result,
/// retry, and on repeated rejection degrade to the Tier 0 leaf plus a visible
/// no-interpretation marker (`render_degraded_leaf`). The map starts as the
/// deterministic Tier 0 output and gains only an appended `## Enrichment coverage`
/// section.
pub fn enrich(
    graph: &Graph,
    backend: &dyn LlmBackend,
    config: &EnrichConfig,
) -> Result<EnrichResult, EnrichError> {
    let views = taxonomy::select_views(graph, config.profile.as_deref())?;
    let mut files = Vec::new();
    let mut report = EnrichReport::default();
    let mut narratives = Vec::new();

    // The map starts as deterministic Tier 0 output; the coverage section is
    // appended after the leaf loop (below), once enriched/degraded counts exist.
    files.push(RenderedFile {
        path: "index.md".to_string(),
        contents: crate::render::markdown::render_map(
            &views
                .iter()
                .map(|v| crate::render::markdown::MapEntry {
                    id: v.id.to_string(),
                    title: v.title.to_string(),
                })
                .collect::<Vec<_>>(),
            config.links,
        ),
    });

    for view in &views {
        let lens = lens::compose(view, graph);
        let bundle = FactBundle::from_lens(&lens);
        let licensed = licensed_set(&lens);

        // A run-fatal backend error (unreachable) propagates with `?` and aborts;
        // a per-leaf failure (guardrail rejection or a transient response error)
        // comes back as Degraded and this leaf falls back to its Tier 0 text while
        // the rest continue (spec degradation table).
        match narrate_one(&lens, &bundle, &licensed, backend, config)? {
            LeafOutcome::Narrated(narrative) => {
                files.push(RenderedFile {
                    path: format!("{}.md", view.id),
                    contents: render_enriched_leaf(&lens, &narrative),
                });
                narratives.push(narrative);
                report.enriched += 1;
            }
            LeafOutcome::Degraded(reason) => {
                files.push(RenderedFile {
                    path: format!("{}.md", view.id),
                    contents: render_degraded_leaf(&lens, &reason),
                });
                report.degraded += 1;
                report.degradations.push((view.id.to_string(), reason));
            }
        }
    }

    // Persist coverage into the map (E9: "coverage as a displayed fact") — a
    // durable record of which leaves are narrated vs degraded and why, so a reader
    // of the emitted corpus never has to reconstruct it from the ephemeral stderr
    // report. index.md is files[0] (pushed first, above).
    if let Some(index) = files.first_mut() {
        index.contents.push_str(&format!(
            "\n## Enrichment coverage\n\n{} view(s) narrated, {} degraded.\n",
            report.enriched, report.degraded
        ));
        for (view, reason) in &report.degradations {
            index.contents.push_str(&format!(
                "- `{view}`: {}\n",
                crate::enrich::render::sanitize_reason(reason)
            ));
        }
    }

    Ok(EnrichResult {
        corpus: Corpus { files },
        report,
        narratives,
    })
}

/// The outcome of narrating one leaf: an accepted narrative, or a degradation
/// (guardrail rejection or a transient per-leaf backend error) that falls the
/// leaf back to its Tier 0 text. A run-fatal backend error is NOT represented
/// here — it propagates as `Err(EnrichError)` and aborts the whole run.
enum LeafOutcome {
    Narrated(Narrative),
    Degraded(String),
}

/// Narrate one leaf with bounded retries. Error classification (spec degradation
/// table): `BackendError::Unreachable` is run-fatal (the backend is absent — every
/// leaf would fail identically, so fail fast) and propagates via `?`; a
/// `BackendError::Response` (timeout, 5xx) degrades just this leaf and lets the
/// run continue; guardrail rejection after all retries also degrades this leaf.
fn narrate_one(
    lens: &crate::lens::Lens,
    bundle: &FactBundle,
    licensed: &crate::enrich::bundle::LicensedSet,
    backend: &dyn LlmBackend,
    config: &EnrichConfig,
) -> Result<LeafOutcome, EnrichError> {
    let mut reject_note: Option<String> = None;
    // 1 initial attempt + `retries` retries.
    for _ in 0..=config.retries {
        let prompt = build_prompt(bundle, &config.lang, reject_note.as_deref());
        let candidate = match backend.narrate(&prompt) {
            Ok(c) => c,
            // Backend absent → run-fatal, abort (spec: "backend unreachable → exit").
            Err(e @ BackendError::Unreachable(_)) => return Err(EnrichError::Backend(e)),
            // Transient per-call failure → degrade THIS leaf, continue the run.
            Err(BackendError::Response(msg)) => {
                return Ok(LeafOutcome::Degraded(format!("backend error: {msg}")));
            }
        };
        if candidate.trim().is_empty() {
            // An empty (or whitespace-only) response isn't a structural fabrication,
            // but accepting it would render an empty "## Interpretation" section and
            // count as enriched — dishonest coverage. Treat it like a guardrail
            // rejection: ask the model to retry, then degrade after retries.
            reject_note = Some(
                "Your previous attempt was empty. Write 2–4 short paragraphs \
                 interpreting the facts."
                    .to_string(),
            );
            continue;
        }
        match check(&candidate, licensed) {
            Verdict::Ok => {
                return Ok(LeafOutcome::Narrated(Narrative {
                    view_id: lens.view_id.clone(),
                    text: candidate,
                    backend_id: backend.id().to_string(),
                    prompt_version: PROMPT_VERSION.to_string(),
                    lang: config.lang.clone(),
                    // Audit trail (T1-KTD9): the leaf facts this narrative framed.
                    backed_by: bundle.facts.clone(),
                }));
            }
            Verdict::Rejected { token } => {
                // If the rejected token is a digit-substring of a licensed identifier
                // (a version inside `v1.2`, digits inside `base64`), the model most
                // likely stated a real backticked fact as a bare number. Tell it to
                // quote the identifier verbatim rather than falsely asserting the token
                // is absent from the facts (which would push it to drop a true fact or
                // thrash to degradation).
                reject_note = Some(
                    if licensed.identifiers.iter().any(|id| id.contains(&token)) {
                        format!(
                        "Your previous attempt used `{token}`, which appears only inside a code \
                         identifier in the facts. Quote that whole identifier in backticks exactly \
                         as written — do not state it as a bare number."
                    )
                    } else {
                        format!(
                        "Your previous attempt introduced `{token}`, which is not in the facts. \
                         Rewrite without it."
                    )
                    },
                );
            }
        }
    }
    Ok(LeafOutcome::Degraded(
        "narrative rejected by guardrail after retries".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::backend::FakeBackend;
    use crate::graph::Graph;

    // A graph built over a tiny real fixture repo via the actual extractor
    // pipeline — far less fragile than hand-constructing nodes, and it exercises
    // the same path production uses.
    fn graph_with_facts() -> Graph {
        graph_over_fixture()
    }

    fn graph_over_fixture() -> Graph {
        use git2::{Repository, Signature, Time};
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("core")).unwrap();
        std::fs::write(dir.join("core/lib.rs"), "pub fn f() {}\n").unwrap();

        let repo = Repository::init(dir).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("core/lib.rs")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::new("Ann", "ann@example.test", &Time::new(1_600_000_000, 0)).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        let ra_repo = crate::extract::Repo::open(dir).unwrap();
        let mut g = Graph::new();
        crate::extract::run_all(&ra_repo, &mut g).unwrap();
        crate::graph::merge(&mut g, &crate::extract::trust_ranks());
        g
    }

    fn cfg(retries: u8) -> EnrichConfig {
        EnrichConfig {
            lang: "en".into(),
            retries,
            profile: None,
            links: crate::cli::LinkStyle::Portable,
        }
    }

    #[test]
    fn accepted_narrative_enriches_the_leaf() {
        let graph = graph_with_facts();
        // Fake returns a narrative that only uses licensed tokens.
        let backend = FakeBackend::new(vec!["This repository is small and cohesive.".into(); 8]);
        let result = enrich(&graph, &backend, &cfg(2)).unwrap();
        assert!(result.report.enriched > 0);
        assert_eq!(result.report.degraded, 0);
        assert!(result
            .corpus
            .files
            .iter()
            .any(|f| f.contents.contains("## Interpretation")));
    }

    #[test]
    fn fabrication_after_retries_degrades_to_tier0_leaf() {
        let graph = graph_with_facts();
        // Every attempt fabricates a number → rejected every time → degrade.
        let backend = FakeBackend::new(vec!["This module holds 9999 files.".into(); 20]);
        let result = enrich(&graph, &backend, &cfg(2)).unwrap();
        assert!(result.report.degraded > 0);
        // Degraded leaves still render (Tier 0 + no-interpretation marker), so the corpus is complete.
        assert!(result.corpus.files.iter().all(|f| !f.contents.is_empty()));
        // A degraded leaf has no Interpretation section.
        let degraded_leaf = result
            .corpus
            .files
            .iter()
            .find(|f| f.path == "overview.md")
            .unwrap();
        assert!(!degraded_leaf.contents.contains("## Interpretation"));
    }

    #[test]
    fn empty_narrative_after_retries_degrades_and_does_not_count_as_enriched() {
        // Every attempt returns an empty string → never structurally rejected by
        // check() (no numbers/backticks), but must still be treated as a failed
        // narration, not a zero-length "success".
        let graph = graph_with_facts();
        let backend = FakeBackend::new(vec!["".into(); 20]);
        let result = enrich(&graph, &backend, &cfg(2)).unwrap();
        assert!(result.report.degraded > 0);
        assert_eq!(result.report.enriched, 0);
        assert!(!result
            .corpus
            .files
            .iter()
            .any(|f| f.contents.contains("## Interpretation")));
    }

    #[test]
    fn unreachable_backend_aborts_the_run_readably() {
        // An unreachable backend is run-fatal: every leaf would fail identically,
        // so enrich aborts with a readable error rather than silently degrading
        // the whole corpus (spec: "backend unreachable → exit").
        let graph = graph_with_facts();
        let backend = FakeBackend::failing("connection refused");
        let err = enrich(&graph, &backend, &cfg(2)).unwrap_err();
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn accepted_narrative_records_its_backing_facts() {
        let graph = graph_with_facts();
        let backend = FakeBackend::new(vec!["A small, cohesive repository.".into(); 8]);
        let result = enrich(&graph, &backend, &cfg(2)).unwrap();
        let n = result
            .narratives
            .first()
            .expect("at least one enriched leaf");
        // backed_by names the leaf facts the narrative framed — non-empty and
        // drawn from the graph, never invented.
        assert!(!n.backed_by.is_empty());
    }

    #[test]
    fn transient_backend_error_degrades_the_leaf_and_continues() {
        // A per-call (response) error degrades that leaf and the run continues to
        // completion (spec: "that leaf degrades, others continue").
        let graph = graph_with_facts();
        let backend = FakeBackend::failing_response("upstream timeout");
        let result = enrich(&graph, &backend, &cfg(2)).unwrap();
        assert!(result.report.degraded > 0);
        assert!(result.corpus.files.iter().all(|f| !f.contents.is_empty()));
        assert!(result
            .report
            .degradations
            .iter()
            .any(|(_, r)| r.contains("backend error")));
    }
}
