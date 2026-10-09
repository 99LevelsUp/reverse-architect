//! reverse-architect — deterministic, offline, LLM-free architectural corpus
//! generator for source trees.
//!
//! A git repository is the preferred input and still the best one — the index
//! is one canonical file list with blob ids for free (KTD10) — but since U15 it
//! is not the required one: a tree with no commits is indexed through a plain
//! directory source rather than refused.
//!
//! Phase A (walking skeleton): CLI + config (U1), fact-graph IR (U2),
//! filetree + git extractors (U3), taxonomy + lens + renderer (U4),
//! manifest emitter (U5).

pub mod card;
pub mod cli;
pub mod config;
pub mod coverage;
pub mod enrich;
pub mod extract;
pub mod graph;
pub mod index;
pub mod lens;
pub mod manifest;
pub mod measure;
pub mod plugin;
pub mod progress;
pub mod render;
pub mod smell;
pub mod taxonomy;
pub mod verify;

use config::Config;
use extract::{ExtractError, Repo};
use graph::Graph;
use manifest::Manifest;
use render::Corpus;

/// Everything one run produces: the populated graph, the rendered corpus, and
/// the machine manifest — both projections cast from the one graph.
pub struct Generated {
    pub graph: Graph,
    pub corpus: Corpus,
    pub manifest: Manifest,
    /// The entity-centric card corpus (U8): one document per type, per
    /// type-less file, and per module. A separate field rather than extra
    /// entries in `corpus.files`, because the cards are their own projection
    /// (KTD4) and the view corpus must stay exactly what it was.
    pub cards: Vec<render::RenderedFile>,
    /// The details of the cards that have one (plan B, U8–U9), in card order:
    /// since the short card (U9) a type's relation lists, its "maybe also"
    /// claims — with the model's marks, when the layer is applied — and its
    /// API live only here, rendered from the same card set as `cards`. Their
    /// own list beside the cards' (KTD9): only some cards have one, so a
    /// detail pairs with its card by path, never by position (U11).
    pub details: Vec<render::RenderedFile>,
    /// The map of every module that holds a type (plan B, U10), in card
    /// order: one row per type, the level above the cards. Its own list
    /// beside the cards' and the details' (KTD9), paired with its module's
    /// card by path like a detail.
    pub maps: Vec<render::RenderedFile>,
    /// What the corpus may claim, and over which files (U7/R6/R20). A separate
    /// artifact rather than a sentence in a view, so a consumer deciding whether
    /// an absence claim is licensed reads a file instead of parsing prose.
    pub coverage: coverage::Coverage,
}

impl Generated {
    /// The sizes per level as one line of the run's summary (plan B, U12:
    /// KTD13) — `map row 120 / 180 (53 rows), type card …, detail …`, median
    /// and p90 in bytes — so a measurement on a large repository needs no tool
    /// of its own. Read from the coverage artifact rather than counted again,
    /// so the terminal and `coverage.json` cannot disagree. `None` for a run
    /// that counted no card levels.
    pub fn size_summary(&self) -> Option<String> {
        let sizes = self.coverage.cards.as_ref()?.sizes;
        let level = |name: &str, s: coverage::Sizes, one: &str, many: &str| match s.count {
            0 => format!("{name}: none"),
            1 => format!("{name} {} / {} (1 {one})", s.median, s.p90),
            n => format!("{name} {} / {} ({n} {many})", s.median, s.p90),
        };
        Some(format!(
            "sizes in bytes, median / p90 — {}, {}, {}",
            level("map row", sizes.map_row, "row", "rows"),
            level("type card", sizes.type_card, "card", "cards"),
            level("detail", sizes.detail, "detail", "details"),
        ))
    }
}

/// Errors from the end-to-end generation pipeline.
#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error(transparent)]
    Profile(#[from] taxonomy::profile::UnknownProfile),
    #[error(transparent)]
    Plugins(#[from] plugin::PluginSetError),
    #[error(transparent)]
    Layer(#[from] verify::LayerError),
}

/// Run the full Phase A pipeline: open the repo, extract facts into the graph,
/// and render the corpus. Deterministic and offline.
pub fn generate(config: &Config) -> Result<Generated, GenError> {
    generate_reporting(config, &progress::Silent)
}

/// [`generate`], reporting what it is doing to `progress` as it goes.
///
/// Split rather than added to `generate` so every existing caller — the tests,
/// the enrich path, any library consumer — keeps a signature that mentions no
/// terminal. What is reported is counts and phase names only; the clock and the
/// rendering live in the binary, so the pipeline cannot read a time and cannot
/// leak one into an artifact (R18).
pub fn generate_reporting(
    config: &Config,
    progress: &dyn progress::Progress,
) -> Result<Generated, GenError> {
    let Built {
        repo,
        graph,
        options: extract_options,
        registry,
    } = build_graph(config, progress)?;
    // The one profile flows into BOTH projections, so the corpus and the manifest
    // always agree on the selected view set (KTD-C5).
    let profile = config.profile.as_deref();
    // The model's layer, only when asked (plan A, U12, R15). Read before any
    // projection, but applied to the cards alone: the views, the manifest and
    // the graph's rungs are the rules' (R14, R16).
    let layer = if config.model_verification {
        Some(verify::apply_layer(
            &config.corpus_dir,
            &repo,
            &graph,
            &config.secret_prefixes,
        )?)
    } else {
        None
    };
    progress.phase("render views", None);
    let mut corpus = render::build_corpus(&graph, config.links, profile)?;
    progress.phase_done();
    progress.phase("build manifest", None);
    let mut manifest = manifest::build_manifest(&graph, profile)?;
    manifest.plugins = extract_options
        .plugins
        .with_origins()
        .map(|(p, origin)| manifest::ManifestPlugin {
            id: p.id().to_string(),
            version: p.manifest().version.clone(),
            sha256: p.digest().to_string(),
            origin: origin.word().to_string(),
            languages: p
                .manifest()
                .languages
                .iter()
                .map(|l| l.name.clone())
                .collect(),
        })
        .collect();
    progress.phase_done();
    progress.phase("render cards", None);
    let (mut card_set, insight, card_rows) = match &layer {
        Some((verdicts, _)) => card::build_cards_with_verdicts(&graph, verdicts),
        None => card::build_cards_with_insight(&graph),
    };
    let lang = card::render::Lang::from_code(&config.lang);
    // The model's purpose sentences, only when asked (plan B, U13: R7):
    // matched against the cards just built, before any of them renders, and
    // put on the one field only the card's `## Purpose` reads.
    let purpose_layer = if config.model_purpose {
        Some(verify::purpose::apply_layer(
            &config.corpus_dir,
            &mut card_set,
            lang,
            &config.secret_prefixes,
        )?)
    } else {
        None
    };
    // Built once and shared: the card renderer and the signpost (below) both
    // need a CardIndex, and building it is an O(n) clone-into-a-map over the
    // full card set — not free to repeat at real-repository scale.
    let card_index = card::links::CardIndex::build(&card_set);
    let cards = card::render_card_files(&card_set, &card_index, lang, config.links);
    let details = card::detail::render_detail_files(&card_set, &card_index, lang, config.links);
    let maps = card::map::render_map_files(&card_set, &card_index, lang, config.links);
    // Appends the corpus signpost (U10/R12) to index.md's own RenderedFile —
    // a module list plus a curated "most-referenced types" highlight, built
    // from the same card set rather than walking the graph a second time.
    card::index::append_signpost(&mut corpus, &card_set, &card_index, config.links);
    // The architecture pages read the same aggregation the cards were built
    // from, and sit beside the views at the corpus root.
    corpus.files.extend(card::pages::build_pages(
        &insight,
        &card_set,
        &card_index,
        config.links,
    ));
    // Every page, every card, every map and every detail becomes a manifest
    // document (v8, v10), so a RAG consumer reaches the corpus's explaining
    // content, not only the views.
    for page in corpus
        .files
        .iter()
        .filter(|f| card::pages::PAGE_FILENAMES.contains(&f.path.as_str()))
    {
        let title = page
            .contents
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .unwrap_or(&page.path);
        let question = page
            .contents
            .lines()
            .find_map(|l| l.strip_prefix("**Guiding question:** "))
            .unwrap_or("");
        let id = page.path.trim_end_matches(".md");
        manifest.documents.push(manifest::rendered_document(
            id,
            "page",
            title,
            question,
            &page.path,
            &page.contents,
        ));
    }
    manifest
        .documents
        .extend(card::documents::manifest_documents(
            &card_set, &cards, &maps, &details, lang,
        ));
    card::index::prepend_start_here(
        &mut corpus,
        &[
            (
                "architecture.md",
                "Architecture",
                "module map, layers, dependency cycles, extension points",
            ),
            (
                "hotspots.md",
                "Hotspots",
                "the largest and most connected files and types",
            ),
            (
                "hierarchy.md",
                "Type hierarchy",
                "which types extend or implement which",
            ),
            (
                "integrations.md",
                "Integrations",
                "libraries, databases, URLs and configuration the code reaches",
            ),
        ],
    );
    progress.phase_done();
    // Built last: it reports on the merged graph every other projection read, and
    // the revision comes from the repo handle rather than from inside the graph,
    // so the projection itself stays free of I/O. The card noise is the card
    // layer's own count, handed over rather than re-derived (KTD14).
    progress.phase("coverage", None);
    let mut coverage = coverage::build_coverage(
        &graph,
        repo.head_revision(),
        registry.languages(),
        Some(card_rows),
    );
    coverage.model_verification = layer.map(|(_, counts)| counts);
    coverage.model_purpose = purpose_layer;
    // The card levels are counted over what was rendered (plan B, U12), the
    // way the card noise is counted by the layer that built it.
    coverage.cards = Some(coverage::card_levels(&card_set, &cards, &details, &maps));
    progress.phase_done();
    Ok(Generated {
        graph,
        corpus,
        manifest,
        cards,
        details,
        maps,
        coverage,
    })
}

/// The graph one run builds, with what built it — everything the projections,
/// the measurement (plan A, U11) and the subcommands that rebuild the graph
/// (`enrich`, `verify`) read after extraction.
pub struct Built {
    pub repo: Repo,
    pub graph: Graph,
    /// The extraction options the graph was built with: its plugin set is what
    /// names the component behind each file's usage sites.
    pub options: extract::ExtractOptions,
    pub registry: extract::Registry,
}

/// Open the repository and build its merged fact graph from `config`: the
/// declared plugins, the languages, the extraction options, the cache, every
/// extractor, merge.
///
/// Shared by generation, `measure` (plan A, U11), `enrich` and `verify` (U12):
/// each must work on exactly the graph a generation run with the same options
/// would build — assembling the options twice is how they would drift apart
/// unnoticed. [`config::Config::for_graph`] is how a subcommand gets a config.
pub fn build_graph(config: &Config, progress: &dyn progress::Progress) -> Result<Built, GenError> {
    let repo = Repo::open(&config.repo_root)?;
    let mut graph = Graph::new();
    // The declared plugins are read, verified and loaded before a single file
    // is: a wrong digest or a broken component fails the run here, with its
    // reason, not halfway through the walk. Their kinds are declared next, so
    // the cache below and every extractor read one vocabulary (U19).
    let plugins = plugin::PluginSet::with_declared(&config.plugins)?;
    plugins.declare_vocabularies(&mut graph)?;
    let mut languages = extract::languages::Languages::bundled();
    plugins.register_languages(&mut languages);
    let extract_options = extract::ExtractOptions {
        ambiguity_ceiling: config.ambiguity_ceiling,
        secret_prefixes: config.secret_prefixes.clone(),
        scip_index: config.scip_index.clone(),
        // The bundled languages plus the declared plugins' own. Resolved once
        // here, like `known_ids` below, and threaded (KTD17) — the extractors
        // that classify by language and the coverage projection that reports it
        // must read the same table, or the artifact would name languages the
        // run never actually counted.
        languages,
        plugins,
        rights: config.rights.clone(),
        // The model verification layer is matched against the site records,
        // so a run that reads it keeps them; keeping them changes neither the
        // graph nor the cache's fingerprint.
        site_records: config.sites || config.model_verification,
        trace_over_ceiling: config.trace_over_ceiling,
    };
    // Cross-run caching (U5) is entirely automatic: no CLI flag, no `Config`
    // field (R3) — the cache lives at a fixed, hidden location derived from
    // `corpus_dir` alone. `activate` performs the whole KTD8 decision (trust
    // an unchanged store as-is; wipe and rewrite the header BEFORE returning
    // on any mismatch) and hands back the store to use, or `None`.
    //
    // Any failure anywhere in that sequence — computing the fingerprint,
    // creating the cache directory, reading or writing the header, clearing
    // a stale store — degrades this ONE run to no cache at all rather than
    // failing it (KTD6): a cache-plumbing failure is not a run failure, and
    // `GenError` gains no cache-related variant for it. `compute_fingerprint`
    // reads the same tracked-file list `run_all_with` is about to read again
    // right below, so a failure here that is not purely a cache-plumbing
    // problem (a genuinely broken git index) simply resurfaces there, on the
    // call that was always going to fail anyway — nothing is swallowed that
    // would not already need handling.
    // Resolved ONCE for the whole run and shared by both consumers — the cache,
    // which uses it to canonicalize and vet the ids on a hit, and merge below
    // (KTD17). Building it instantiates every extractor, so the two call sites
    // this replaces were doing that twice per run; under plugins they would
    // have loaded every component twice.
    let registry = extract::Registry::for_run(&extract_options);
    let cache = extract::compute_fingerprint(&repo, &extract_options)
        .ok()
        .and_then(|fingerprint| {
            let cache_root = config.corpus_dir.join(extract::CACHE_DIR_NAME);
            extract::activate(
                cache_root,
                &fingerprint,
                registry.ranks().clone(),
                // The corpus's own declared kinds (U7): a record spells its
                // kinds as names, and only this can turn a name back into one.
                //
                // Everything the plugins declared, a few lines up — the only
                // declaration step this pipeline has, run before the cache is
                // opened so a record naming a plugin's kind can be read back.
                graph.vocabulary().clone(),
            )
        });
    extract::run_all_with_cached(
        &repo,
        &mut graph,
        &extract_options,
        progress,
        cache.as_ref(),
    )?;
    // Merge unifies facts from every extractor before either projection reads
    // the graph (KTD-B5): additive kinds union, scalar conflicts resolve by
    // trust-rank with losers preserved as dissent. `generate` supplies the
    // id→rank map so `graph` never depends on `extract` (no cycle).
    progress.phase("merge", None);
    graph::merge(&mut graph, registry.ranks());
    progress.phase_done();
    Ok(Built {
        repo,
        graph,
        options: extract_options,
        registry,
    })
}
