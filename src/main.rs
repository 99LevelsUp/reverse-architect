//! Binary entry point. Parse CLI; run Tier 0 generate (default), the Tier 1
//! enrich subcommand, the SCIP `index` subcommand, `measure`, or `verify`.

mod bin_progress;

use bin_progress::TerminalProgress;
use clap::Parser;
use reverse_architect::cli::{
    BackendArg, Cli, Command, EnrichArgs, IndexArgs, MeasureArgs, VerifyArgs,
};
use reverse_architect::config::{Config, MIRROR_DIR_NAME};
use reverse_architect::enrich::backend::{build_backend, BackendChoice, BackendKind};
use reverse_architect::enrich::{enrich, EnrichConfig};
use reverse_architect::progress::Progress;
use reverse_architect::verify::{self, BackendSettings};
use reverse_architect::Generated;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Command::Enrich(args)) => run_enrich(args),
        Some(Command::Index(args)) => run_index(args),
        Some(Command::Measure(args)) => run_measure(args),
        Some(Command::Verify(args)) => run_verify(args),
        None => run_generate(&cli),
    };
    match result {
        Ok(msg) => {
            eprintln!("{msg}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_generate(cli: &Cli) -> Result<String, Box<dyn std::error::Error>> {
    let config = Config::resolve(cli)?;
    // Always on, never a flag. A run over a large repository takes many
    // minutes, and silence is indistinguishable from a hang — which is the
    // state this replaces, measured at ~18 minutes on a 14,396-file repository.
    let progress = TerminalProgress::new();
    let generated = reverse_architect::generate_reporting(&config, &progress)?;
    // Writing is a phase of its own in the run's summary (plan B, U11): the
    // details and the maps add files beside the cards, and what that costs
    // on disk is worth seeing apart from what rendering them cost.
    let written = generated.corpus.files.len()
        + generated.cards.len()
        + generated.details.len()
        + generated.maps.len();
    progress.phase("write corpus", Some(written));
    write_corpus(&config.corpus_dir, &generated, config.sites)?;
    progress.phase_done();
    if cli.mirror {
        let mirror_dir = config.repo_root.join(MIRROR_DIR_NAME);
        progress.phase("write mirror", Some(written));
        write_corpus(&mirror_dir, &generated, config.sites)?;
        progress.phase_done();
        eprintln!(
            "reverse-architect: in-project mirror written to {} — \
             recommend adding `{}/` to .gitignore",
            mirror_dir.display(),
            MIRROR_DIR_NAME
        );
    }
    let mut msg = format!(
        "reverse-architect: corpus written to {}",
        config.corpus_dir.display()
    );
    // The sizes per level (plan B, U12: KTD13), as coverage.json states them.
    if let Some(sizes) = generated.size_summary() {
        msg.push_str("\n  ");
        msg.push_str(&sizes);
    }
    Ok(msg)
}

fn run_enrich(args: &EnrichArgs) -> Result<String, Box<dyn std::error::Error>> {
    // Rebuild the fact graph in-process (T1-KTD2) the way generation builds
    // it — the same plugins, index, ceiling and secret prefixes (R30, plan A
    // KTD12) — so the narrated graph is the one the corpus was built from.
    let config = Config::for_graph(
        args.repo.clone(),
        args.graph.corpus.clone(),
        (&args.graph).into(),
    )?;
    let graph =
        reverse_architect::build_graph(&config, &reverse_architect::progress::Silent)?.graph;

    let backend = build_backend(&backend_choice(
        args.backend,
        &args.base_url,
        &args.model,
        &args.auth_env,
    ))?;

    let cfg = EnrichConfig {
        lang: args.lang.clone(),
        retries: args.retries,
        profile: args.profile.clone(),
        links: args.links,
    };
    let result = enrich(&graph, backend.as_ref(), &cfg)?;

    // Default output: a sidecar `enriched/` directory beside the repo.
    let out_dir = match &args.out {
        Some(dir) => dir.clone(),
        None => args.repo.join("..").join("enriched"),
    };
    std::fs::create_dir_all(&out_dir)?;
    // `run_enrich` never writes a manifest itself, so it never owns any of
    // the line-delimited filenames either — but `--out` can target the same
    // directory a prior `generate` run wrote a manifest into (U10/KTD5), and
    // that directory may hold either manifest form. Chaining in
    // `LINE_DELIMITED_FILENAMES` here (alongside `generated_filenames()`,
    // which already covers `manifest.json`) keeps this cleanup complete
    // rather than only aware of the form that existed before U10.
    let owned: HashSet<String> = result.corpus.files.iter().map(|f| f.path.clone()).collect();
    for name in reverse_architect::taxonomy::generated_filenames()
        .iter()
        .map(String::as_str)
        .chain(reverse_architect::manifest::LINE_DELIMITED_FILENAMES)
    {
        if !owned.contains(name) {
            let path = out_dir.join(name);
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
        }
    }
    write_files(&out_dir, &result.corpus.files)?;

    // Honest run report (T1-KTD5 / E9).
    let mut msg = format!(
        "reverse-architect enrich: {} leaves enriched, {} degraded → {}",
        result.report.enriched,
        result.report.degraded,
        out_dir.display()
    );
    for (view, reason) in &result.report.degradations {
        msg.push_str(&format!("\n  - degraded `{view}`: {reason}"));
    }
    // The corpus is written either way (degraded leaves fall back to Tier 0 text),
    // but a run that narrated NOTHING — whether every leaf degraded or no views
    // were selected at all — is a failure a caller/CI must be able to detect —
    // surface it as a non-zero exit rather than a silent SUCCESS.
    if result.report.enriched == 0 {
        eprintln!("{msg}");
        return Err(format!(
            "enrich narrated 0 leaves ({} degraded) — see report above",
            result.report.degraded
        )
        .into());
    }
    Ok(msg)
}

/// The backend an operator chose on the command line.
fn backend_choice(
    backend: BackendArg,
    base_url: &Option<String>,
    model: &Option<String>,
    auth_env: &Option<String>,
) -> BackendChoice {
    BackendChoice {
        kind: match backend {
            BackendArg::Ollama => BackendKind::Ollama,
            BackendArg::OpenAiCompat => BackendKind::OpenAiCompat,
            BackendArg::ClaudeCli => BackendKind::ClaudeCli,
        },
        base_url: base_url.clone(),
        model: model.clone(),
        auth_env: auth_env.clone(),
    }
}

fn run_verify(args: &VerifyArgs) -> Result<String, Box<dyn std::error::Error>> {
    let config = Config::for_graph(
        args.repo.clone(),
        args.graph.corpus.clone(),
        (&args.graph).into(),
    )?;
    let settings = BackendSettings {
        backend: match args.backend {
            BackendArg::Ollama => "ollama",
            BackendArg::OpenAiCompat => "openai-compat",
            BackendArg::ClaudeCli => "claude-cli",
        }
        .to_string(),
        model: args.model.clone(),
        base_url: args.base_url.clone(),
    };
    // The dry run never builds a backend: it must not need one reachable, and
    // must not need its credentials.
    let backend = if args.dry_run {
        None
    } else {
        Some(build_backend(&backend_choice(
            args.backend,
            &args.base_url,
            &args.model,
            &args.auth_env,
        ))?)
    };
    let progress = TerminalProgress::new();
    if args.purpose {
        return run_verify_purpose(args, &config, &settings, backend.as_deref(), &progress);
    }
    let report = verify::run(&config, &settings, backend.as_deref(), &progress)?;
    if args.dry_run {
        return Ok(format!(
            "reverse-architect verify (dry run): {} ambiguous sites, {} already answered in              the cache; {} would be sent, about {} input tokens (at {} characters a token).              Nothing was sent.",
            report.sites,
            report.cached,
            report.sites - report.cached,
            report.estimated_tokens,
            verify::CHARS_PER_TOKEN
        ));
    }
    Ok(format!(
        "reverse-architect verify: {} ambiguous sites — {} answered from the cache, {} asked          (about {} input tokens); {} name a candidate, {} name none, {} answers were invalid          and stay unverified → {}",
        report.sites,
        report.cached,
        report.asked,
        report.estimated_tokens,
        report.promoted,
        report.none,
        report.invalid,
        report
            .layer
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    ))
}

/// `verify --purpose` (plan B, U13): the same backend, graph options, cache
/// and dry run as the site verdicts, a layer of its own.
fn run_verify_purpose(
    args: &VerifyArgs,
    config: &Config,
    settings: &BackendSettings,
    backend: Option<&dyn reverse_architect::enrich::backend::LlmBackend>,
    progress: &TerminalProgress,
) -> Result<String, Box<dyn std::error::Error>> {
    let lang = reverse_architect::card::render::Lang::from_code(&args.lang);
    let report = verify::purpose::run(config, settings, lang, backend, progress)?;
    if args.dry_run {
        return Ok(format!(
            "reverse-architect verify --purpose (dry run): {} types, {} already answered in \
             the cache; {} would be sent, about {} input tokens (at {} characters a token). \
             Nothing was sent.",
            report.types,
            report.cached,
            report.types - report.cached,
            report.estimated_tokens,
            verify::CHARS_PER_TOKEN
        ));
    }
    Ok(format!(
        "reverse-architect verify --purpose: {} types — {} answered from the cache, {} asked \
         (about {} input tokens); {} sentences written, {} answers failed the checks and are \
         left out → {}",
        report.types,
        report.cached,
        report.asked,
        report.estimated_tokens,
        report.written,
        report.invalid,
        report
            .layer
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    ))
}

fn run_index(args: &IndexArgs) -> Result<String, Box<dyn std::error::Error>> {
    use reverse_architect::index::{self, IndexOptions, ProjectOutcome, SystemToolchain};
    // Before anything runs, and unconditionally: this is the one step that
    // executes code from the repository (KTD9 risk row).
    eprintln!("{}", index::BUILD_LOGIC_WARNING);
    let options = IndexOptions {
        repo: args.repo.clone(),
        out: args.out.clone(),
        restore: args.restore,
        allow_partial: args.allow_partial,
        timeout: std::time::Duration::from_secs(args.timeout),
    };
    let outcome = index::run(&options, &SystemToolchain)?;
    let projects = &outcome.record.projects;
    let indexed = projects
        .iter()
        .filter(|p| p.outcome == ProjectOutcome::Indexed)
        .count();
    let mut msg = format!(
        "reverse-architect index: {indexed} of {} projects indexed → {} (record {})",
        projects.len(),
        outcome.index_path.display(),
        outcome.record_path.display()
    );
    for failed in projects
        .iter()
        .filter(|p| p.outcome != ProjectOutcome::Indexed)
    {
        msg.push_str(&format!(
            "\n  - not indexed `{}`: {} (log {})",
            failed.path,
            failed.detail.as_deref().unwrap_or(""),
            failed.log
        ));
    }
    if !outcome.record.conflicts.is_empty() {
        msg.push_str(&format!(
            "\n  {} occurrence ranges have conflicting targets between projects (see the record)",
            outcome.record.conflicts.len()
        ));
    }
    Ok(msg)
}

fn run_measure(args: &MeasureArgs) -> Result<String, Box<dyn std::error::Error>> {
    use reverse_architect::measure::{self, MeasureOptions, Proposal, Thresholds};
    let options = MeasureOptions {
        repo: args.repo.clone(),
        scip_index: args.scip_index.clone(),
        corpus_dir: args.corpus.clone(),
        plugins: args.plugin.clone(),
        ambiguity_ceiling: args.ambiguity_ceiling,
        secret_prefixes: args.secret_prefix.clone(),
        thresholds: Thresholds {
            min_evaluated_sites: args.min_evaluated_sites,
            max_unmapped_percent: args.max_unmapped_percent,
        },
    };
    // A run over the reference repository takes minutes, like generation.
    let progress = TerminalProgress::new();
    let report = measure::run(&options, &progress)?;
    let out = match &args.out {
        Some(out) => out.clone(),
        None => {
            let stem = args
                .scip_index
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "index".to_string());
            args.scip_index
                .with_file_name(format!("{stem}.measurement.json"))
        }
    };
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, measure::to_json(&report))?;
    let mut msg = format!(
        "reverse-architect measure: report written to {}",
        out.display()
    );
    for origin in &report.origins {
        let proposed: Vec<&str> = origin
            .rules
            .iter()
            .filter(|(_, r)| r.proposal == Proposal::Proposed)
            .map(|(id, _)| id.as_str())
            .collect();
        msg.push_str(&format!(
            "\n  {} ({} {}): {}",
            origin.language,
            origin.component,
            origin.contract,
            if proposed.is_empty() {
                "no right proposed".to_string()
            } else {
                format!("proposes a right to delete for {}", proposed.join(", "))
            }
        ));
    }
    msg.push_str(
        "\n  A right is granted by committing the report to docs/measurements/ and a record \
         to the rights table in code (docs/measurements/README.md).",
    );
    Ok(msg)
}

fn write_files(
    dir: &Path,
    files: &[reverse_architect::render::RenderedFile],
) -> Result<(), Box<dyn std::error::Error>> {
    use rayon::prelude::*;
    // In parallel: every file is independent, and on a large repository the
    // tens of thousands of cards written one after another were most of the
    // run's tail. What lands on disk does not depend on the order.
    files
        .par_iter()
        .try_for_each(|file| write_if_changed(&dir.join(&file.path), file.contents.as_bytes()))?;
    Ok(())
}

/// Write `contents` to `path` unless it already holds those exact bytes
/// (U12/KTD3) — a byte comparison, not mtime or a hash, so it never rewrites
/// (and never touches the mtime of) a file whose content didn't change. Any
/// failure to read the existing file (missing, unreadable, permissions)
/// degrades to an unconditional write rather than erroring (KTD6/R5).
fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(existing) = std::fs::read(path) {
        if existing == contents {
            return Ok(());
        }
    }
    std::fs::write(path, contents)?;
    Ok(())
}

/// Every file under `dir` (recursively), depth-first — used to find the
/// card files a prior run left on disk. A MISSING directory yields no files
/// rather than erroring (KTD6): the first run into a fresh `corpus_dir` has
/// no `cards/` tree yet. Any other read failure (permissions, a genuinely
/// unreadable subtree) propagates instead of being swallowed — treating it
/// the same as "missing" would make the orphan-cleanup loop believe no cards
/// exist and delete nothing, silently leaving a stale card behind (R1),
/// which is a worse outcome than the run failing loudly.
///
/// Uses the entry's own file type, not `Path::is_dir` — the latter follows
/// symlinks, so a symlink planted under `cards/` would have this walk
/// recurse through it and delete files outside the corpus dir (R4). A
/// symlink is treated as a leaf: `remove_file` deletes the link itself
/// without following it.
fn collect_files_recursively(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let is_real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_real_dir {
            collect_files_recursively(&entry.path(), out)?;
        } else {
            out.push(entry.path());
        }
    }
    Ok(())
}

/// `sites`: also write the usage-site records (`--sites`, plan A KTD7). A run
/// without it removes a `sites.jsonl` an earlier run left, so the directory
/// never pairs a fresh corpus with site records of another state.
fn write_corpus(
    dir: &Path,
    generated: &Generated,
    sites: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;

    // U10/KTD3: same O(1) count `graph.node_count() + graph.edge_count()`
    // would give — `build_manifest` already walked the graph into these two
    // Vecs, so reading their length costs nothing further.
    let scale = generated.manifest.nodes.len() + generated.manifest.edges.len();
    let line_delimited = reverse_architect::manifest::should_write_line_delimited(scale);

    // U10/KTD5: the four line-delimited filenames, paired with the writer
    // that produces each one's content. One table drives the owned-set
    // insert, the orphan-cleanup candidates, and the writes below, so the
    // four names can never drift out of sync with each other.
    type ManifestJsonlWriter = fn(&reverse_architect::manifest::Manifest) -> String;
    let jsonl_files: [(&str, ManifestJsonlWriter); 4] = [
        (
            reverse_architect::manifest::STRINGS_FILENAME,
            reverse_architect::manifest::to_strings_json,
        ),
        (
            reverse_architect::manifest::NODES_FILENAME,
            reverse_architect::manifest::to_nodes_jsonl,
        ),
        (
            reverse_architect::manifest::EDGES_FILENAME,
            reverse_architect::manifest::to_edges_jsonl,
        ),
        (
            reverse_architect::manifest::DOCUMENTS_FILENAME,
            reverse_architect::manifest::to_documents_jsonl,
        ),
    ];

    // U12/KTD1: the set of top-level names the orphan-cleanup loop below must
    // never delete. Exactly one manifest form is owned per run — `manifest.json`
    // below the threshold, or the four line-delimited files above it (U10/KTD5)
    // — never both, so a run that crosses the threshold in either direction
    // cleans up the other form's files below rather than leaving them behind.
    // `coverage.json` is not in `generated_filenames()`'s cleanup namespace —
    // it can never go stale that way — so it has no reason to be in this set.
    let mut new_owned_top: HashSet<String> = generated
        .corpus
        .files
        .iter()
        .map(|f| f.path.clone())
        .collect();
    new_owned_top.extend(
        reverse_architect::manifest::owned_filenames(line_delimited)
            .into_iter()
            .map(str::to_string),
    );

    // The card corpus is its own projection (U8/KTD4) and lives under `cards/`,
    // so it never contends with a view file for a path. Compared as `PathBuf`
    // (not `String`) against what `collect_files_recursively` returns, so
    // Windows' `\` vs. the corpus's `/`-joined paths never need reconciling.
    // The details and the maps (plan B, U11: KTD9) live under `cards/` too,
    // in trees of their own: left out of this set, every run would delete
    // each one before writing it again, and an unchanged rerun would rewrite
    // every detail and map instead of none (U12/KTD3).
    let new_owned_cards: HashSet<PathBuf> = generated
        .cards
        .iter()
        .chain(&generated.details)
        .chain(&generated.maps)
        .map(|f| dir.join(&f.path))
        .collect();

    // U12/KTD2: delete orphans BEFORE writing. The delete and write sets are
    // always disjoint, so this ordering costs nothing and keeps the window in
    // which a stale (possibly under-redacted) file sits next to a fresh,
    // still-valid corpus as short as possible. The line-delimited filenames
    // are candidates here even though `generated_filenames()` doesn't know
    // about them (U10/KTD5) — that keeps `taxonomy` free of a dependency on
    // `manifest`'s naming, which already depends on `taxonomy` the other way.
    for name in reverse_architect::taxonomy::generated_filenames()
        .iter()
        .map(String::as_str)
        .chain(reverse_architect::manifest::LINE_DELIMITED_FILENAMES)
    {
        if !new_owned_top.contains(name) {
            let path = dir.join(name);
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
        }
    }
    let mut existing_cards = Vec::new();
    collect_files_recursively(&dir.join("cards"), &mut existing_cards)?;
    for existing in existing_cards {
        if !new_owned_cards.contains(&existing) {
            std::fs::remove_file(&existing)?;
        }
    }

    write_files(dir, &generated.corpus.files)?;
    write_files(dir, &generated.cards)?;
    write_files(dir, &generated.details)?;
    write_files(dir, &generated.maps)?;
    if line_delimited {
        for (name, render) in jsonl_files {
            write_if_changed(&dir.join(name), render(&generated.manifest).as_bytes())?;
        }
    } else {
        let manifest_json = reverse_architect::manifest::to_json(&generated.manifest);
        write_if_changed(&dir.join("manifest.json"), manifest_json.as_bytes())?;
    }
    let coverage_json = reverse_architect::coverage::to_json(&generated.coverage);
    write_if_changed(&dir.join("coverage.json"), coverage_json.as_bytes())?;
    let sites_path = dir.join(reverse_architect::manifest::SITES_FILENAME);
    if sites {
        let records = reverse_architect::manifest::to_sites_jsonl(&generated.graph);
        write_if_changed(&sites_path, records.as_bytes())?;
    } else if sites_path.is_file() {
        std::fs::remove_file(&sites_path)?;
    }
    Ok(())
}
