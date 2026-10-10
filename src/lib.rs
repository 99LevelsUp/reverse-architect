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
    /// The registry of triggers, `cards/triggers.md` (plan 2b, U7: KTD10) —
    /// for a run with `--detail-level`, `None` otherwise. A field of its own,
    /// like the maps' (KTD9): it pairs with no card, and its one document in
    /// the manifest is `triggers:index`.
    pub triggers: Option<render::RenderedFile>,
    /// What the corpus may claim, and over which files (U7/R6/R20). A separate
    /// artifact rather than a sentence in a view, so a consumer deciding whether
    /// an absence claim is licensed reads a file instead of parsing prose.
    pub coverage: coverage::Coverage,
    /// The second layer (plan 2a, U4): the model of the members' bodies,
    /// paired with the graph's declarations — for a run with
    /// `--detail-level`, `None` otherwise. The run analyzed it and put it on
    /// the C# type cards (U6); the analyses of later steps read it from here.
    pub bodies: Option<extract::bodies::BodyLayer>,
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
        bodies,
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
    // Which kinds of trigger the second layer covers in which language (plan
    // 2b, U7–U8: KTD11) — only for a run with it. Read from what the run's
    // body components declare and from the files they read, before any
    // projection: the view `entry-points` and the registry both carry it.
    let trigger_coverage = bodies
        .as_ref()
        .map(|layer| card::triggers::coverage(&body_components(layer, &extract_options)));
    progress.phase("render views", None);
    let mut corpus =
        render::build_corpus_with(&graph, config.links, profile, trigger_coverage.as_ref())?;
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
    // The second layer's own component and level (plan 2a, U4: KTD9) — only
    // for a run with it, so every other manifest is what it was.
    if let Some(layer) = &bodies {
        manifest.detail_level = Some(layer.level());
        // The run's own plugins are listed already; the component the layer
        // added is not one of them.
        for reader in layer.readers().iter().filter(|r| r.component) {
            manifest.plugins.push(manifest::ManifestPlugin {
                id: reader.id.clone(),
                version: reader.version.clone(),
                sha256: reader.sha256.clone(),
                origin: plugin::Origin::Bundled.word().to_string(),
                languages: reader.languages.clone(),
            });
        }
    }
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
    // The second layer's analysis, once per run and only for a run with it
    // (plan 2a, U6): the keywords of the analyzed types' cards first, so
    // every count read off the cards below sees them. The dynamic-call
    // markers of step 2c are built in it, before the triggers that read them.
    let mut analysis = bodies
        .as_ref()
        .map(|layer| card::layer2::analyze_run(&graph, layer, config.ambiguity_ceiling));
    if let Some(analysis) = &analysis {
        card::add_layer2_keywords(&mut card_set, &insight, analysis);
    }
    // The run's state of XAML coverage (plan 2c, U8: R19, KTD12): from the
    // plugins that claim XAML, the files they failed on and the second
    // layer, before any card renders — the analyzed types' `## Dynamic
    // access` and the XAML documents end in it; coverage carries it.
    let markup_plugins: Vec<String> = extract_options
        .plugins
        .iter()
        .filter(|p| {
            p.manifest()
                .languages
                .iter()
                .any(|l| l.name == coverage::MARKUP_LANGUAGE)
        })
        .map(|p| p.id().to_string())
        .collect();
    let markup = coverage::markup(
        &graph,
        registry.languages(),
        &markup_plugins,
        bodies.as_ref(),
        analysis.as_ref().map(|a| a.dynamic.site_counts()),
    );
    if let (Some(analysis), Some(markup)) = (&mut analysis, &markup) {
        analysis.set_markup_state(markup.state);
    }
    // Built once and shared: the card renderer and the signpost (below) both
    // need a CardIndex, and building it is an O(n) clone-into-a-map over the
    // full card set — not free to repeat at real-repository scale.
    let card_index = card::links::CardIndex::build(&card_set);
    let run = match (&bodies, &analysis) {
        (Some(layer), Some(analysis)) => Some(card::Layer2Run {
            level: layer.level(),
            bodies: layer,
            analysis,
        }),
        _ => None,
    };
    let mut cards =
        card::render_card_files_layered(&card_set, &card_index, lang, config.links, run.as_ref());
    // The documents of XAML files (plan 2c, U8): their unresolved references,
    // and with the second layer the state of XAML coverage and the
    // references from C# — before the manifest and the sizes read the cards.
    card::render::add_markup_documents(
        &mut cards,
        &card_set,
        &graph,
        analysis
            .as_ref()
            .zip(markup.as_ref())
            .map(|(a, m)| (&a.dynamic, m.state)),
        lang,
    );
    let details = card::detail::render_detail_files(&card_set, &card_index, lang, config.links);
    let maps = card::map::render_map_files(&card_set, &card_index, lang, config.links);
    // The registry of triggers (plan 2b, U7): with the second layer only,
    // and then always — the cards and the view `entry-points` link it.
    let triggers = match (&analysis, trigger_coverage) {
        (Some(analysis), Some(coverage)) => Some(card::triggers::render_registry(
            &card::triggers::Registry::of(analysis, coverage),
            &card_index,
            lang,
            config.links,
        )),
        _ => None,
    };
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
    if let Some(file) = &triggers {
        manifest
            .documents
            .push(card::documents::triggers_document(file));
    }
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
    coverage.bodies = bodies.as_ref().map(coverage::bodies);
    coverage.markup = markup;
    progress.phase_done();
    Ok(Generated {
        graph,
        corpus,
        manifest,
        cards,
        details,
        maps,
        triggers,
        coverage,
        bodies,
    })
}

/// The run's body components as the line of trigger coverage reads them
/// (plan 2b, U7: KTD11): the languages each claims, what its `describe`
/// says it emits, and whether it read a file. The bundled body component is
/// no plugin of the run, so its declaration is read from the bundle.
fn body_components(
    layer: &extract::bodies::BodyLayer,
    options: &extract::ExtractOptions,
) -> Vec<card::triggers::BodyComponent> {
    let read: std::collections::BTreeSet<&str> =
        layer.files().map(|(_, f)| f.plugin.as_str()).collect();
    layer
        .readers()
        .iter()
        .map(|r| {
            let plugin = options.plugins.iter().find(|p| p.id() == r.id).or_else(|| {
                (r.component && plugin::bundled::csharp_body().id() == r.id)
                    .then(plugin::bundled::csharp_body)
            });
            card::triggers::BodyComponent {
                languages: r.languages.clone(),
                emits: plugin
                    .map(|p| p.manifest().emits.clone())
                    .unwrap_or_default(),
                read: read.contains(r.id.as_str()),
            }
        })
        .collect()
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
    /// The second layer, for a run with `--detail-level` (plan 2a, U4).
    pub bodies: Option<extract::bodies::BodyLayer>,
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
    // The second layer (plan 2a, U4: KTD7): only a run with a detail level
    // loads the body component, and only such a run records it among the
    // languages — after the first layer's plugins, so no extension changes
    // hands. It stays out of `plugins`, which the cache fingerprint names.
    let body_pass = config
        .detail_level
        .map(|level| extract::bodies::BodyPass::new(level, &plugins));
    let mut languages = extract::languages::Languages::bundled();
    plugins.register_languages(&mut languages);
    if let Some(pass) = &body_pass {
        plugins.register_body_languages(&mut languages, pass.component());
    }
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
    // The declaration-name positions, kept past the usage pass only for the
    // second layer's pairing.
    let names = body_pass.as_ref().map(|_| extract::NamesHandoff::default());
    extract::run_all_handing_names(
        &repo,
        &mut graph,
        &extract_options,
        progress,
        cache.as_ref(),
        names.as_ref(),
    )?;
    // After every first-layer extractor, before the merge sorts the facts
    // its failures add (KTD8).
    let bodies = match (&body_pass, &names) {
        (Some(pass), Some(names)) => Some(extract::bodies::run(
            pass,
            &repo,
            &mut graph,
            &names.take(),
            &extract::external::SecretPrefixes::new(&config.secret_prefixes),
            cache.as_ref(),
            progress,
        )?),
        _ => None,
    };
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
        bodies,
    })
}
