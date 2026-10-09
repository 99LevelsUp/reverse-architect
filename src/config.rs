//! Configuration resolution (U1).
//!
//! Resolves — never writes — the effective run configuration: the repository
//! root to read, the corpus output directory, the link style, and an optional
//! taxonomy profile override. No filesystem enumeration and no I/O beyond the
//! existence checks needed to fail early with a readable error (R14, R15).

use crate::cli::{Cli, LinkStyle};
use std::path::{Path, PathBuf};

/// Conventional in-project mirror directory (R14): when `--mirror` is set, a
/// second, byte-identical copy of the corpus is written here, inside the repo.
/// Recommended for `.gitignore` (the tool never edits `.gitignore` itself).
pub const MIRROR_DIR_NAME: &str = ".reverse-architect";

/// Default ambiguity ceiling (R23): the most name candidates a heuristic
/// identifier resolution may still turn into named edges. Above it the graph
/// carries a single "ambiguous, N candidates" fact and NO named edges.
///
/// Eight, because it is the smallest number that still admits a real overload set
/// whole. A method rarely has more than a handful of overloads, and one name
/// shared by an interface, its implementation and a test double is three — so 8
/// keeps every case where the candidates are genuinely *the same thing*. Names
/// like `Get`, `Run` or `Execute`, which a large codebase declares in dozens of
/// unrelated types, land far above it and are reported as ambiguity instead of
/// drowning both the index and the cards in edges. It also bounds the worst-case
/// edge fan-out per call site to a single digit, which is what keeps R17's
/// 13,270-file repository tractable.
pub const DEFAULT_AMBIGUITY_CEILING: usize = 8;

/// The effective, resolved configuration for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Repository root as supplied by the user (used for I/O; may be relative).
    pub repo_root: PathBuf,
    /// Where the corpus will be written (a real on-disk location; not a path
    /// that ever appears inside the corpus body).
    pub corpus_dir: PathBuf,
    /// How intra-corpus links are rendered.
    pub links: LinkStyle,
    /// Opt-in taxonomy profile override.
    pub profile: Option<String>,
    /// Most candidates a heuristic identifier resolution may still turn into named
    /// edges (R23) — see [`DEFAULT_AMBIGUITY_CEILING`].
    pub ambiguity_ceiling: usize,
    /// Path to an externally produced SCIP index, when the caller named one (R7).
    /// Passed straight through to extraction; `None` means a heuristic run, which
    /// is a complete run and not a degraded one.
    pub scip_index: Option<PathBuf>,
    /// Extra token prefixes the R22 secret filter treats as a credential, on top
    /// of the built-in public ones. Carried verbatim; the filter escapes each one
    /// before compiling, so a prefix is always a literal and never a pattern.
    pub secret_prefixes: Vec<String>,
    /// Language plugins the operator declared, in precedence order (U8).
    pub plugins: Vec<crate::plugin::PluginSpec>,
    /// Language code for the authored prose inside cards (R8). Carried as the raw
    /// user string; the card renderer maps an unrecognized code to English rather
    /// than failing the run, so config resolution has nothing to validate.
    pub lang: String,
    /// Also write `sites.jsonl` — every usage site with its candidates and the
    /// rules' verdicts (plan A, KTD7) — next to the manifest. Off by default:
    /// the corpus is at the level of relations.
    pub sites: bool,
    /// The rights to delete the shared resolver applies (plan A, KTD11).
    /// [`crate::extract::RightsTable::granted`] for every CLI run: no flag
    /// reaches it, since a right is earned by measurement and granted in code.
    /// A library caller — a regression fixture that needs a deletion to happen
    /// — sets another.
    pub rights: crate::extract::RightsTable,
    /// Read the model verification layer `verify` wrote into `corpus_dir` and
    /// mark the "maybe also" claims it speaks about (plan A, U12, KTD12). Off
    /// by default (R15): without it the layer file is never opened, and the
    /// corpus is byte for byte the one a run without a layer writes.
    pub model_verification: bool,
    /// Read the purpose layer `verify --purpose` wrote into `corpus_dir` and
    /// show each type's current sentence as a labelled line under its card's
    /// purpose (plan B, U13: R7, KTD12). Off by default: without it the layer
    /// file is never opened, and the corpus is byte for byte the one a run
    /// without a layer writes.
    pub model_purpose: bool,
    /// Run the resolver's rules at sites over the ambiguity ceiling too
    /// ([`crate::extract::ExtractOptions::trace_over_ceiling`]). Set by `ra
    /// measure` only; off, the graph and every output are what they were.
    pub trace_over_ceiling: bool,
}

/// What shapes the graph, as every subcommand that rebuilds it takes it —
/// the options a generation run would have been given (plan A, KTD12).
///
/// One struct so `enrich`, `verify` and `measure` cannot each assemble a
/// [`Config`] of their own and drift from generation: a narrative or a model
/// verdict about a graph built without the index, the ceiling or the secret
/// prefixes generation used is about another graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphOptions {
    pub plugins: Vec<crate::plugin::PluginSpec>,
    pub ambiguity_ceiling: usize,
    pub scip_index: Option<PathBuf>,
    pub secret_prefixes: Vec<String>,
}

/// Errors that can arise while resolving configuration — all readable, none a panic.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    // The variant names still say `Repo`; the messages no longer do. Since U15
    // the input is a source tree — a git repository where there is one, any
    // directory where there is not — and telling an operator who pointed at a
    // plain tree that their "repository path" is wrong describes a requirement
    // this build dropped. Renaming the variants would be a public API change
    // that buys nothing: nothing outside the crate matches on them.
    #[error("no source path supplied (usage: reverse-architect <REPO>)")]
    RepoMissing,
    #[error("source path does not exist: {0}")]
    RepoNotFound(PathBuf),
    #[error("source path is not a directory: {0}")]
    RepoNotDir(PathBuf),
    #[error("cannot determine a name for the source at path: {0}")]
    RepoNameUnresolvable(PathBuf),
    #[error("unknown profile: {name} (known profiles: {})", known.join(", "))]
    UnknownProfile {
        name: String,
        known: Vec<&'static str>,
    },
}

impl Config {
    /// Resolve configuration from parsed CLI arguments.
    ///
    /// Validates that the source path exists and is a directory, then resolves
    /// the corpus output directory (either the explicit `--out` or a default
    /// sidecar beside the source tree).
    ///
    /// This is where a non-directory is refused for every CLI run, which is why
    /// `ExtractError::NotASource` never reaches an operator through the binary:
    /// it is `Repo::open`'s own guard for a caller that skipped this one.
    pub fn resolve(cli: &Cli) -> Result<Self, ConfigError> {
        let repo_root = cli.repo.clone().ok_or(ConfigError::RepoMissing)?;

        if !repo_root.exists() {
            return Err(ConfigError::RepoNotFound(repo_root));
        }
        if !repo_root.is_dir() {
            return Err(ConfigError::RepoNotDir(repo_root));
        }

        let corpus_dir = match &cli.out {
            Some(out) => out.clone(),
            None => default_sidecar(&repo_root)?,
        };

        // Validate the taxonomy profile early, against the single source of truth
        // `select_views` later applies (KTD-C5) — so an unknown name fails here
        // with a readable error before any extraction runs, never as a silent
        // fallback to the automatic taxonomy (R18).
        if let Some(name) = &cli.profile {
            if crate::taxonomy::profile::view_ids(name).is_none() {
                return Err(ConfigError::UnknownProfile {
                    name: name.clone(),
                    known: crate::taxonomy::profile::known(),
                });
            }
        }

        Ok(Config {
            repo_root,
            corpus_dir,
            links: cli.links,
            profile: cli.profile.clone(),
            ambiguity_ceiling: cli.ambiguity_ceiling,
            secret_prefixes: cli.secret_prefix.clone(),
            plugins: cli.plugin.clone(),
            scip_index: cli.scip_index.clone(),
            lang: cli.lang.clone(),
            sites: cli.sites,
            rights: crate::extract::RightsTable::granted(),
            model_verification: cli.model_verification,
            model_purpose: cli.model_purpose,
            trace_over_ceiling: false,
        })
    }

    /// The configuration a subcommand rebuilds the graph with: `graph` as
    /// generation would read it, the cache of the corpus at `corpus_dir` (the
    /// sidecar beside `repo_root` by default), and nothing that only shapes a
    /// projection — the subcommand renders none of generation's.
    pub fn for_graph(
        repo_root: PathBuf,
        corpus_dir: Option<PathBuf>,
        graph: GraphOptions,
    ) -> Result<Self, ConfigError> {
        let corpus_dir = match corpus_dir {
            Some(dir) => dir,
            None => default_sidecar(&repo_root)?,
        };
        Ok(Config {
            repo_root,
            corpus_dir,
            links: LinkStyle::Portable,
            profile: None,
            ambiguity_ceiling: graph.ambiguity_ceiling,
            scip_index: graph.scip_index,
            secret_prefixes: graph.secret_prefixes,
            plugins: graph.plugins,
            lang: "en".to_string(),
            sites: false,
            rights: crate::extract::RightsTable::granted(),
            model_verification: false,
            model_purpose: false,
            trace_over_ceiling: false,
        })
    }
}

/// Default corpus location: a `<name>.reverse-architect` directory beside the
/// repository (a sidecar outside the project, R14).
///
/// Canonicalization here is deliberate and safe: it only locates a real
/// on-disk directory and never produces a path that is serialized into the
/// corpus or manifest (those stay repo-root-relative — KTD7).
pub(crate) fn default_sidecar(repo_root: &Path) -> Result<PathBuf, ConfigError> {
    let unresolvable = || ConfigError::RepoNameUnresolvable(repo_root.to_path_buf());
    let abs = repo_root.canonicalize().map_err(|_| unresolvable())?;
    let name = abs.file_name().ok_or_else(unresolvable)?;
    let parent = abs.parent().ok_or_else(unresolvable)?;

    let mut sidecar_name = name.to_os_string();
    sidecar_name.push(".reverse-architect");
    Ok(parent.join(sidecar_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cli_for(repo: &Path) -> Cli {
        Cli {
            repo: Some(repo.to_path_buf()),
            out: None,
            profile: None,
            links: LinkStyle::Portable,
            ambiguity_ceiling: DEFAULT_AMBIGUITY_CEILING,
            secret_prefix: Vec::new(),
            plugin: Vec::new(),
            scip_index: None,
            lang: "en".to_string(),
            mirror: false,
            sites: false,
            model_verification: false,
            model_purpose: false,
            command: None,
        }
    }

    #[test]
    fn missing_repo_is_a_readable_error_not_a_panic() {
        let cli = Cli {
            repo: None,
            out: None,
            profile: None,
            links: LinkStyle::Portable,
            ambiguity_ceiling: DEFAULT_AMBIGUITY_CEILING,
            secret_prefix: Vec::new(),
            plugin: Vec::new(),
            scip_index: None,
            lang: "en".to_string(),
            mirror: false,
            sites: false,
            model_verification: false,
            model_purpose: false,
            command: None,
        };
        let err = Config::resolve(&cli).unwrap_err();
        assert!(matches!(err, ConfigError::RepoMissing));
    }

    #[test]
    fn resolves_default_sidecar_beside_repo() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("my-repo");
        std::fs::create_dir(&repo).unwrap();

        let config = Config::resolve(&cli_for(&repo)).unwrap();

        let name = config.corpus_dir.file_name().unwrap().to_string_lossy();
        assert_eq!(name, "my-repo.reverse-architect");
        // Sidecar sits beside the repo, not inside it.
        assert_ne!(config.corpus_dir, repo);
    }

    #[test]
    fn explicit_out_overrides_default_sidecar() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let out = tmp.path().join("elsewhere");

        let mut cli = cli_for(&repo);
        cli.out = Some(out.clone());

        let config = Config::resolve(&cli).unwrap();
        assert_eq!(config.corpus_dir, out);
    }

    #[test]
    fn nonexistent_repo_is_readable_error_not_panic() {
        let cli = cli_for(Path::new("this/path/does/not/exist"));
        let err = Config::resolve(&cli).unwrap_err();
        assert!(matches!(err, ConfigError::RepoNotFound(_)));
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn repo_path_pointing_at_a_file_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a-file");
        std::fs::write(&file, b"not a dir").unwrap();

        let err = Config::resolve(&cli_for(&file)).unwrap_err();
        assert!(matches!(err, ConfigError::RepoNotDir(_)));
    }

    #[test]
    fn link_style_and_profile_are_carried_through() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();

        let mut cli = cli_for(&repo);
        cli.links = LinkStyle::Obsidian;
        cli.profile = Some("minimal".to_string());

        let config = Config::resolve(&cli).unwrap();
        assert_eq!(config.links, LinkStyle::Obsidian);
        assert_eq!(config.profile.as_deref(), Some("minimal"));
    }

    /// R23: the ambiguity ceiling is CONFIGURATION, resolved here from the CLI —
    /// never a literal at the extractor's call site, so a repo whose overload sets
    /// are wider than the default can raise it without a rebuild.
    #[test]
    fn ambiguity_ceiling_defaults_and_is_carried_through() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();

        let mut cli = cli_for(&repo);
        assert_eq!(
            Config::resolve(&cli).unwrap().ambiguity_ceiling,
            DEFAULT_AMBIGUITY_CEILING
        );

        cli.ambiguity_ceiling = 3;
        assert_eq!(Config::resolve(&cli).unwrap().ambiguity_ceiling, 3);
    }

    #[test]
    fn unknown_profile_is_a_readable_error_not_a_silent_fallback() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();

        let mut cli = cli_for(&repo);
        cli.profile = Some("nonesuch".to_string());

        let err = Config::resolve(&cli).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownProfile { .. }));
        let msg = err.to_string();
        assert!(msg.contains("unknown profile: nonesuch"));
        // Lists the known profiles so the error is actionable.
        assert!(msg.contains("minimal"));
    }

    #[test]
    fn known_profile_resolves() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();

        let mut cli = cli_for(&repo);
        cli.profile = Some("minimal".to_string());

        assert_eq!(
            Config::resolve(&cli).unwrap().profile.as_deref(),
            Some("minimal")
        );
    }
}
