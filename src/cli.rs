//! Command-line surface (clap). Declarative only — behavior lives in [`crate::config`].

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Deterministic, offline, LLM-free architectural corpus generator.
/// With no subcommand it runs Tier 0 generation over <REPO>.
#[derive(Parser, Debug)]
#[command(name = "reverse-architect", version, about, long_about = None)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct Cli {
    /// Path to the source tree to analyze — a git repository, or any directory
    /// (Tier 0 generate; omit when using a subcommand).
    #[arg(required = false)]
    pub repo: Option<PathBuf>,

    /// Where to write the corpus (default: a sidecar directory beside the repo).
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,

    /// Opt-in taxonomy profile override (default: automatic taxonomy).
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Link style used inside the corpus.
    #[arg(long, value_enum, default_value = "portable")]
    pub links: LinkStyle,

    /// Most name candidates a heuristic identifier resolution may still turn into
    /// named edges (R23). Above it the graph carries a single "ambiguous, N
    /// candidates" fact and no named edges at all.
    #[arg(long, value_name = "N", default_value_t = crate::config::DEFAULT_AMBIGUITY_CEILING)]
    pub ambiguity_ceiling: usize,

    /// Path to a SCIP index, enabling the `resolved` tier (R7): one the target
    /// project's CI produced, or one `reverse-architect index` built. This run
    /// only reads it and never runs an indexer or a build; `index` is the one
    /// explicit step that does. Omit it for a fully heuristic run — that is not
    /// a degraded mode.
    /// There is no conventional default location on purpose: a stale index must
    /// never be picked up by accident.
    #[arg(long, value_name = "FILE")]
    pub scip_index: Option<PathBuf>,

    /// Language of the authored prose inside cards (`en`, `cs`). Section headings
    /// and the fact vocabulary stay English either way (R8); an unrecognized code
    /// falls back to `en`. Mirrors the `enrich` subcommand's `--lang`.
    #[arg(long, default_value = "en")]
    pub lang: String,

    /// An extra token prefix the secret filter should treat as a credential, on
    /// top of the built-in public ones (`sk-`, `ghp_`, `AKIA`, …). Repeatable.
    ///
    /// For a company token shape no public list knows. Additive: naming your own
    /// never turns the public ones off. The prefix is a literal, not a pattern,
    /// and a match must still be followed by a long token, so a word that merely
    /// starts the same way does not cost you the line it sits on.
    #[arg(long = "secret-prefix", value_name = "PREFIX")]
    pub secret_prefix: Vec<String>,

    /// A language plugin to load: a WebAssembly component built against
    /// `wit-v2/extractor.wit` (contract 0.2) or the frozen `wit/extractor.wit`
    /// (0.1). Repeatable; order is precedence.
    ///
    /// A declared plugin takes the files of every extension it claims, ahead of
    /// the bundled ones, and one whose id matches a bundled plugin replaces it.
    /// Append `=sha256:<hex>` to refuse the file unless its bytes hash to that
    /// digest — checked before any of it runs.
    #[arg(long = "plugin", value_name = "PATH[=sha256:HEX]")]
    pub plugin: Vec<crate::plugin::PluginSpec>,

    /// Also write the corpus to an in-project mirror (`.reverse-architect/` in
    /// the repo) alongside the sidecar. Off by default: sidecar only (R14).
    #[arg(long)]
    pub mirror: bool,

    /// Also write `sites.jsonl` next to the manifest: every place a name was
    /// used, the declarations it could mean, and what each resolution rule
    /// said about each of them. Off by default — the corpus states relations,
    /// not call sites.
    #[arg(long)]
    pub sites: bool,

    /// Read the model verification layer `verify` wrote into the corpus
    /// directory, and mark the "maybe also" claims it speaks about as
    /// model-verified: the candidate the model named first, a claim it named
    /// none of with a demotion mark. Nothing is removed, and the manifest and
    /// the graph's confidence rungs stay as the rules left them. Off by
    /// default; where an index judged a site, the layer is ignored there.
    #[arg(long)]
    pub model_verification: bool,

    /// Read the purpose layer `verify --purpose` wrote into the corpus
    /// directory, and show what the model said a type is for as its own
    /// labelled line under the type card's purpose. The header, the map row
    /// and the manifest keep the purpose read from the code. A sentence whose
    /// type's facts changed since, or that was asked in another `--lang`, is
    /// not shown. Off by default.
    #[arg(long)]
    pub model_purpose: bool,

    /// Also read the bodies of the members — the second layer — and render
    /// it on the C# type cards (`1`, `2` or `3`, how much of it a card
    /// shows). It sits beside the graph, which it adds one fact to: a file
    /// whose bodies could not be read is marked `body_extraction_failed`. The
    /// manifest and coverage name the level and the component that read the
    /// bodies. Off by default: without it the body component is not even
    /// loaded, and the corpus is what it always was.
    #[arg(long, value_name = "LEVEL", value_parser = clap::value_parser!(u8).range(1..=3))]
    pub detail_level: Option<u8>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Subcommands beyond the default Tier 0 generate.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Tier 1: add an LLM-authored interpretation around the verified facts.
    Enrich(EnrichArgs),
    /// Build a SCIP index of a C# repository with scip-dotnet, for
    /// `--scip-index`. Runs the repository's build logic (MSBuild).
    Index(IndexArgs),
    /// Measure each resolution rule against a SCIP index built by `index`,
    /// and propose rights to delete. Writes a report of counts only.
    Measure(MeasureArgs),
    /// Ask a model which candidate each ambiguous usage site means, and write
    /// the answers as a layer beside the corpus. Sends source snippets to the
    /// chosen backend; `--dry-run` shows how many first. With `--purpose`,
    /// asks instead what each type is for, from the facts its card holds.
    Verify(VerifyArgs),
}

/// The options that shape the graph, shared by the subcommands that rebuild
/// it (plan A, KTD12). Pass the ones generation was given, or the subcommand
/// works on another graph than the corpus describes.
#[derive(Args, Debug, Clone)]
pub struct GraphArgs {
    /// The corpus directory of a generation run (default: the sidecar beside
    /// the repository, as for generation). Its cache is reused.
    #[arg(long, value_name = "DIR")]
    pub corpus: Option<PathBuf>,

    /// A language plugin to load, as for generation.
    #[arg(long = "plugin", value_name = "PATH[=sha256:HEX]")]
    pub plugin: Vec<crate::plugin::PluginSpec>,

    /// As for generation: most name candidates a resolution may still turn
    /// into named edges.
    #[arg(long, value_name = "N", default_value_t = crate::config::DEFAULT_AMBIGUITY_CEILING)]
    pub ambiguity_ceiling: usize,

    /// As for generation: a SCIP index the graph is built with.
    #[arg(long, value_name = "FILE")]
    pub scip_index: Option<PathBuf>,

    /// As for generation: an extra token prefix the secret filter treats as a
    /// credential. Repeatable.
    #[arg(long = "secret-prefix", value_name = "PREFIX")]
    pub secret_prefix: Vec<String>,
}

impl From<&GraphArgs> for crate::config::GraphOptions {
    fn from(args: &GraphArgs) -> Self {
        crate::config::GraphOptions {
            plugins: args.plugin.clone(),
            ambiguity_ceiling: args.ambiguity_ceiling,
            scip_index: args.scip_index.clone(),
            secret_prefixes: args.secret_prefix.clone(),
        }
    }
}

/// `verify` arguments (plan A, U12, KTD12).
#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// Repository whose ambiguous usage sites to verify.
    pub repo: PathBuf,

    /// Which LLM backend to ask. A cloud backend receives source snippets;
    /// `ollama` keeps them on this machine.
    #[arg(long, value_enum)]
    pub backend: BackendArg,

    /// Backend base URL (ollama / openai-compat) or `claude` binary path (claude-cli).
    #[arg(long)]
    pub base_url: Option<String>,

    /// Model name.
    #[arg(long)]
    pub model: Option<String>,

    /// Name of the env var holding the auth header value (openai-compat), as
    /// for `enrich`.
    #[arg(long, value_name = "ENV_VAR")]
    pub auth_env: Option<String>,

    /// Print how many sites would be sent and an estimate of their tokens,
    /// without calling the backend or writing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Ask what each type is for instead (plan B, U13): one sentence per type
    /// from the facts its card holds — no source code — written as the
    /// purpose layer, which generation shows with `--model-purpose`.
    #[arg(long)]
    pub purpose: bool,

    /// With `--purpose`: the language the sentences are asked in (`en`,
    /// `cs`), as generation's `--lang`. A sentence shows only on cards of the
    /// language it was asked in.
    #[arg(long, default_value = "en")]
    pub lang: String,

    #[command(flatten)]
    pub graph: GraphArgs,
}

/// `measure` arguments (plan A, KTD10).
#[derive(Args, Debug)]
pub struct MeasureArgs {
    /// Repository to measure, checked out at the commit the index was built at.
    pub repo: PathBuf,

    /// The index to measure against. Its record `<stem>.record.json`, which
    /// `reverse-architect index` writes beside it, must be there too.
    #[arg(long, value_name = "FILE")]
    pub scip_index: PathBuf,

    /// Where to write the report (default: `<stem>.measurement.json` beside
    /// the index).
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// The corpus directory of a generation run whose cache to reuse
    /// (default: the sidecar beside the repository, as for generation). Only
    /// the cache is written there.
    #[arg(long, value_name = "DIR")]
    pub corpus: Option<PathBuf>,

    /// A language plugin to load, as for generation. The report names the
    /// component that read the sites, and a right holds only for it.
    #[arg(long = "plugin", value_name = "PATH[=sha256:HEX]")]
    pub plugin: Vec<crate::plugin::PluginSpec>,

    /// As for generation: most name candidates a resolution may still turn
    /// into named edges.
    #[arg(long, value_name = "N", default_value_t = crate::config::DEFAULT_AMBIGUITY_CEILING)]
    pub ambiguity_ceiling: usize,

    /// As for generation; pass the same ones to reuse generation's cache.
    #[arg(long = "secret-prefix", value_name = "PREFIX")]
    pub secret_prefix: Vec<String>,

    /// Fewest sites the index judged that a rule needs to be proposed a right.
    #[arg(long, value_name = "N", default_value_t = crate::measure::DEFAULT_MIN_EVALUATED_SITES)]
    pub min_evaluated_sites: u64,

    /// A rule is proposed a right only while the share of its sites the index
    /// could not judge stays below this percentage.
    #[arg(
        long,
        value_name = "PERCENT",
        default_value_t = crate::measure::DEFAULT_MAX_UNMAPPED_PERCENT,
        value_parser = clap::value_parser!(u32).range(0..=100)
    )]
    pub max_unmapped_percent: u32,
}

/// `index` arguments (plan A, KTD9).
#[derive(Args, Debug)]
pub struct IndexArgs {
    /// Repository to index. Every tracked `.csproj` is indexed separately.
    pub repo: PathBuf,

    /// Where to write the merged index (default: `index.scip` in the sidecar
    /// `<name>.reverse-architect/` beside the repository). The record
    /// `<stem>.record.json` and the per-project logs `<stem>.logs/` go next to it.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Run `dotnet restore` on each project (named by its path) before
    /// indexing it; a project whose restore fails counts as failed. Off by
    /// default: restore reaches the network and runs package build logic of
    /// its own.
    #[arg(long)]
    pub restore: bool,

    /// Keep an index of the projects that succeeded when some fail. The record
    /// marks the index as partial and names the failed projects.
    #[arg(long)]
    pub allow_partial: bool,

    /// Per-project time limit in seconds; a project still running is killed
    /// and counted as failed.
    #[arg(long, value_name = "SECONDS", default_value_t = crate::index::DEFAULT_TIMEOUT.as_secs())]
    pub timeout: u64,
}

/// `enrich` arguments (T1-KTD1).
#[derive(Args, Debug)]
pub struct EnrichArgs {
    /// Repository to analyze and enrich.
    pub repo: PathBuf,

    /// Which LLM backend to use.
    #[arg(long, value_enum)]
    pub backend: BackendArg,

    /// Backend base URL (ollama / openai-compat) or `claude` binary path (claude-cli).
    #[arg(long)]
    pub base_url: Option<String>,

    /// Model name.
    #[arg(long)]
    pub model: Option<String>,

    /// Name of the env var holding the auth header value (openai-compat). The
    /// token is read from the environment, never passed on the command line. It
    /// is only sent over `https` or to a loopback host — an `http://` non-loopback
    /// gateway with auth is refused (see `build_backend`).
    #[arg(long, value_name = "ENV_VAR")]
    pub auth_env: Option<String>,

    /// Output narrative language code (T1-KTD10).
    #[arg(long, default_value = "en")]
    pub lang: String,

    /// Intra-corpus link style for the enriched map (spec C1); mirrors the Tier 0
    /// `--links` flag.
    #[arg(long, value_enum, default_value = "portable")]
    pub links: LinkStyle,

    /// Where to write the enriched corpus (default: sidecar `enriched/`).
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,

    /// Guardrail retries before degrading a leaf to its Tier 0 text.
    #[arg(long, default_value_t = 2)]
    pub retries: u8,

    /// Opt-in taxonomy profile override.
    #[arg(long)]
    pub profile: Option<String>,

    /// The graph's options, as generation was given them — or the narrative
    /// describes a different graph than the corpus.
    #[command(flatten)]
    pub graph: GraphArgs,
}

/// CLI spelling of the backend choice.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendArg {
    Ollama,
    #[value(name = "openai-compat")]
    OpenAiCompat,
    #[value(name = "claude-cli")]
    ClaudeCli,
}

/// How intra-corpus links are rendered (R13).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStyle {
    /// Relative Markdown links — portable across tools (default).
    Portable,
    /// `[[wikilink]]` style — for Obsidian vaults.
    Obsidian,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn links_defaults_to_portable() {
        let cli = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert_eq!(cli.links, LinkStyle::Portable);
    }

    #[test]
    fn links_obsidian_parses() {
        let cli =
            Cli::try_parse_from(["reverse-architect", "some/repo", "--links", "obsidian"]).unwrap();
        assert_eq!(cli.links, LinkStyle::Obsidian);
    }

    #[test]
    fn unknown_link_style_is_an_error() {
        let err = Cli::try_parse_from(["reverse-architect", "some/repo", "--links", "wat"]);
        assert!(err.is_err());
    }

    #[test]
    fn bare_invocation_defers_repo_check_to_config() {
        // Parsing no longer fails; the missing-repo error now comes from Config::resolve.
        let cli = Cli::try_parse_from(["reverse-architect"]).unwrap();
        assert!(cli.repo.is_none() && cli.command.is_none());
    }

    #[test]
    fn mirror_defaults_off_and_is_an_opt_in_flag() {
        let default = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert!(!default.mirror, "sidecar only by default");

        let on = Cli::try_parse_from(["reverse-architect", "some/repo", "--mirror"]).unwrap();
        assert!(on.mirror);
    }

    #[test]
    fn sites_defaults_off_and_is_an_opt_in_flag() {
        let default = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert!(!default.sites);
        let on = Cli::try_parse_from(["reverse-architect", "some/repo", "--sites"]).unwrap();
        assert!(on.sites);
    }

    #[test]
    fn ambiguity_ceiling_defaults_to_the_config_constant_and_parses() {
        let default = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert_eq!(
            default.ambiguity_ceiling,
            crate::config::DEFAULT_AMBIGUITY_CEILING
        );

        let raised = Cli::try_parse_from([
            "reverse-architect",
            "some/repo",
            "--ambiguity-ceiling",
            "32",
        ])
        .unwrap();
        assert_eq!(raised.ambiguity_ceiling, 32);
    }

    #[test]
    fn bare_repo_still_parses_as_generate() {
        let cli = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.repo.as_deref(), Some(std::path::Path::new("some/repo")));
    }

    #[test]
    fn enrich_subcommand_parses_backend_and_lang() {
        let cli = Cli::try_parse_from([
            "reverse-architect",
            "enrich",
            "some/repo",
            "--backend",
            "ollama",
            "--base-url",
            "http://localhost:11434",
            "--model",
            "llama3.1",
            "--lang",
            "cs",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Enrich(e) => {
                assert_eq!(e.backend, BackendArg::Ollama);
                assert_eq!(e.lang, "cs");
                assert_eq!(e.model.as_deref(), Some("llama3.1"));
            }
            other => panic!("expected enrich, got {other:?}"),
        }
    }

    #[test]
    fn enrich_lang_defaults_to_en() {
        let cli = Cli::try_parse_from([
            "reverse-architect",
            "enrich",
            "some/repo",
            "--backend",
            "claude-cli",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Enrich(e) => assert_eq!(e.lang, "en"),
            other => panic!("expected enrich, got {other:?}"),
        }
    }

    #[test]
    fn index_subcommand_defaults_to_no_restore_no_partial_and_the_default_timeout() {
        let cli = Cli::try_parse_from(["reverse-architect", "index", "some/repo"]).unwrap();
        match cli.command.unwrap() {
            Command::Index(i) => {
                assert_eq!(i.repo, std::path::Path::new("some/repo"));
                assert_eq!(i.out, None);
                assert!(!i.restore);
                assert!(!i.allow_partial);
                assert_eq!(i.timeout, 1800);
            }
            other => panic!("expected index, got {other:?}"),
        }
    }

    #[test]
    fn measure_subcommand_needs_an_index_and_defaults_its_thresholds() {
        assert!(Cli::try_parse_from(["reverse-architect", "measure", "some/repo"]).is_err());
        let cli = Cli::try_parse_from([
            "reverse-architect",
            "measure",
            "some/repo",
            "--scip-index",
            "x/index.scip",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Measure(m) => {
                assert_eq!(m.scip_index, std::path::Path::new("x/index.scip"));
                assert_eq!(m.out, None);
                assert_eq!(
                    m.min_evaluated_sites,
                    crate::measure::DEFAULT_MIN_EVALUATED_SITES
                );
                assert_eq!(
                    m.max_unmapped_percent,
                    crate::measure::DEFAULT_MAX_UNMAPPED_PERCENT
                );
            }
            other => panic!("expected measure, got {other:?}"),
        }
        assert!(Cli::try_parse_from([
            "reverse-architect",
            "measure",
            "some/repo",
            "--scip-index",
            "x/index.scip",
            "--max-unmapped-percent",
            "101",
        ])
        .is_err());
    }

    #[test]
    fn model_verification_defaults_off_and_is_an_opt_in_flag() {
        let default = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert!(!default.model_verification);
        let on = Cli::try_parse_from(["reverse-architect", "some/repo", "--model-verification"])
            .unwrap();
        assert!(on.model_verification);
    }

    #[test]
    fn model_purpose_defaults_off_and_is_an_opt_in_flag() {
        let default = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert!(!default.model_purpose);
        let on =
            Cli::try_parse_from(["reverse-architect", "some/repo", "--model-purpose"]).unwrap();
        assert!(on.model_purpose);
    }

    #[test]
    fn detail_level_defaults_off_and_takes_one_to_three() {
        let default = Cli::try_parse_from(["reverse-architect", "some/repo"]).unwrap();
        assert_eq!(default.detail_level, None);
        for level in ["1", "2", "3"] {
            let on =
                Cli::try_parse_from(["reverse-architect", "some/repo", "--detail-level", level])
                    .unwrap();
            assert_eq!(on.detail_level, Some(level.parse().unwrap()));
        }
        for wrong in ["0", "4", "x"] {
            assert!(
                Cli::try_parse_from(["reverse-architect", "some/repo", "--detail-level", wrong])
                    .is_err(),
                "{wrong}"
            );
        }
    }

    #[test]
    fn verify_purpose_is_an_opt_in_flag_with_a_language() {
        let sites = Cli::try_parse_from([
            "reverse-architect",
            "verify",
            "some/repo",
            "--backend",
            "ollama",
        ])
        .unwrap();
        match sites.command.unwrap() {
            Command::Verify(v) => assert!(!v.purpose && v.lang == "en"),
            other => panic!("expected verify, got {other:?}"),
        }
        let purpose = Cli::try_parse_from([
            "reverse-architect",
            "verify",
            "some/repo",
            "--backend",
            "ollama",
            "--purpose",
            "--lang",
            "cs",
            "--dry-run",
        ])
        .unwrap();
        match purpose.command.unwrap() {
            Command::Verify(v) => assert!(v.purpose && v.dry_run && v.lang == "cs"),
            other => panic!("expected verify, got {other:?}"),
        }
    }

    #[test]
    fn verify_subcommand_takes_a_backend_and_the_graph_options() {
        assert!(Cli::try_parse_from(["reverse-architect", "verify", "some/repo"]).is_err());
        let cli = Cli::try_parse_from([
            "reverse-architect",
            "verify",
            "some/repo",
            "--backend",
            "claude-cli",
            "--model",
            "haiku",
            "--secret-prefix",
            "kst_",
            "--scip-index",
            "x/index.scip",
            "--corpus",
            "x/corpus",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Verify(v) => {
                assert_eq!(v.backend, BackendArg::ClaudeCli);
                assert!(!v.dry_run);
                assert_eq!(v.graph.secret_prefix, ["kst_"]);
                assert_eq!(
                    v.graph.ambiguity_ceiling,
                    crate::config::DEFAULT_AMBIGUITY_CEILING
                );
                assert_eq!(
                    v.graph.scip_index.as_deref(),
                    Some(std::path::Path::new("x/index.scip"))
                );
                assert_eq!(
                    v.graph.corpus.as_deref(),
                    Some(std::path::Path::new("x/corpus"))
                );
            }
            other => panic!("expected verify, got {other:?}"),
        }
    }

    /// Plan A, KTD12: `enrich` takes the options that shape the graph, as
    /// generation does, so its narrative is about the corpus's graph.
    #[test]
    fn enrich_takes_the_graph_options() {
        let cli = Cli::try_parse_from([
            "reverse-architect",
            "enrich",
            "some/repo",
            "--backend",
            "ollama",
            "--ambiguity-ceiling",
            "3",
            "--secret-prefix",
            "kst_",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Enrich(e) => {
                assert_eq!(e.graph.ambiguity_ceiling, 3);
                assert_eq!(e.graph.secret_prefix, ["kst_"]);
            }
            other => panic!("expected enrich, got {other:?}"),
        }
    }

    #[test]
    fn index_subcommand_parses_its_options() {
        let cli = Cli::try_parse_from([
            "reverse-architect",
            "index",
            "some/repo",
            "--out",
            "x/index.scip",
            "--restore",
            "--allow-partial",
            "--timeout",
            "90",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Index(i) => {
                assert_eq!(i.out.as_deref(), Some(std::path::Path::new("x/index.scip")));
                assert!(i.restore);
                assert!(i.allow_partial);
                assert_eq!(i.timeout, 90);
            }
            other => panic!("expected index, got {other:?}"),
        }
    }
}
