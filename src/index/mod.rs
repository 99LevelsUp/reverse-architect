//! `ra index`: build a SCIP index of a C# repository (plan A, KTD9, R9).
//!
//! The one place RA runs another program over the repository it describes. The
//! main run never builds and never indexes — it only reads an index the operator
//! names with `--scip-index` (`src/extract/scip.rs`, which a unit test keeps free
//! of process spawning). This subcommand is the explicit, optional step that
//! produces such an index, so the operator does not have to assemble one by hand.
//!
//! ## What it runs, and why that is a risk
//!
//! `scip-dotnet` loads every project through MSBuild, and MSBuild evaluates the
//! repository's own targets and props — arbitrary code from the repository, run
//! on the operator's machine. That is why this is a separate subcommand rather
//! than a flag on generation, and why the binary prints [`BUILD_LOGIC_WARNING`]
//! before anything starts. `dotnet restore` (network, package scripts) is off by
//! default; the operator turns it on with `--restore`.
//!
//! scip-dotnet is always told `--skip-dotnet-restore`. Its own restore runs
//! `dotnet restore` in the working directory without naming the project, which
//! MSBuild refuses (MSB1011) in a directory holding more than one project file —
//! `X.csproj` beside `X.NetCore.csproj` is common — and scip-dotnet then swallows
//! the failure and indexes the project without its packages while reporting
//! success. So with `--restore` RA restores each project itself, by its path,
//! right before indexing it, and a failed restore fails the project.
//!
//! ## Per project, then merged
//!
//! Each tracked `.csproj` is indexed on its own, with its own directory as the
//! working directory and a time limit after which the process is killed. A
//! solution-wide run would fail as a whole on one broken project; per project, a
//! failure costs that project and the operator may keep the rest
//! (`--allow-partial`). The results are merged into ONE index (KTD9 steps 1-4)
//! by [`merge`], so `--scip-index` keeps taking a single file.
//!
//! ## The seam
//!
//! Everything that touches the machine — finding `dotnet` and `scip-dotnet`,
//! starting a process, polling it, killing it — goes through [`Toolchain`].
//! [`SystemToolchain`] is the real one and what the binary passes; tests pass a
//! fake, so the time-limit loop, the failure policy and the merge are exercised
//! without a .NET SDK. The loop itself ([`run_timed`]) is production code either
//! way: the fake only stands in for the child process.

use crate::extract::{ExtractError, Repo};
use protobuf::Message;
use scip::types::{Diagnostic, Document, Index, Occurrence, Severity, SymbolInformation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Printed by the binary before `ra index` does anything (KTD9 risk row).
pub const BUILD_LOGIC_WARNING: &str = "warning: `reverse-architect index` runs scip-dotnet, \
     which loads every project through MSBuild and so executes the repository's own build \
     logic (targets, props, analyzers) on this machine. Do not run it on a repository you \
     do not trust.";

/// The diagnostic code that marks an occurrence whose target differs between
/// the projects that compiled its document (KTD9 step 3). Every target the
/// projects wrote at the range is kept, each occurrence carrying this code. A
/// reader that pairs occurrences with call sites (U9) reads the range as one
/// occurrence: external when every target there is from outside the
/// repository (two frameworks numbering one method's overloads differently
/// still agree the call leaves it), unmapped when any target is the
/// repository's own. Targets that differ only in the package of an external
/// symbol or of a namespace, or only in a local's number, are not a
/// difference ([`merge`], step 3) and never carry it.
pub const CONFLICT_CODE: &str = "ra-index/conflicting-target";

/// Version of the [`Record`] format. Raised whenever a field changes meaning or
/// disappears, so a reader (U11's measurement) can refuse a record it does not
/// understand instead of misreading it.
///
/// 2: `conflicts` (and [`CONFLICT_CODE`] in the index beside it) no longer
/// counts targets that differ only in package as a conflict. A version-1 index
/// marks those ranges conflicting, which hides from U9 every call the compiler
/// sent outside the repository in a folder built for two frameworks; measuring
/// it would understate refutation, so it is refused and rebuilt.
///
/// 3: locals that differ only in their number (`local 35` against `local
/// 32`) are no longer a conflict, so `conflicts` no longer lists them and the
/// index marks no such range. Nothing U9 reads changes — it pairs no site with
/// a local — but a version-2 record lists ranges a version-3 one does not,
/// and a reader comparing the two would read a different merge.
pub const RECORD_SCHEMA: u32 = 3;

/// Default per-project time limit. Generous on purpose: a large project with a
/// cold MSBuild can take minutes, and a limit that fires on a healthy project
/// produces a partial index nobody asked for.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1800);

/// Time limit for `--version` queries. They print one line; a minute covers the
/// .NET first-run experience on a fresh machine.
const VERSION_TIMEOUT: Duration = Duration::from_secs(120);

/// How often a running child is polled. Short enough that a fast project does
/// not wait noticeably, long enough not to spin.
const POLL: Duration = Duration::from_millis(100);

/// The two programs this step needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Dotnet,
    ScipDotnet,
}

/// One process to start. Standard output and standard error both go to `log`:
/// scip-dotnet reports per file, and a pipe nobody drains until exit fills up
/// and stalls the child — which the time limit would then misreport as a hang.
/// Output is appended, so a project's `dotnet restore` and its indexing share
/// one log, in the order they ran.
#[derive(Debug, Clone)]
pub struct Call {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub log: PathBuf,
}

/// A started process, as far as the time-limit loop needs to see it.
pub trait Running {
    /// `Some(success)` once the process has exited, `None` while it runs.
    fn try_wait(&mut self) -> std::io::Result<Option<bool>>;
    /// Stop the process and reap it.
    fn kill(&mut self) -> std::io::Result<()>;
}

/// Everything `ra index` asks of the machine.
pub trait Toolchain {
    /// Where `tool` lives, or `None` when it cannot be found.
    fn locate(&self, tool: Tool) -> Option<PathBuf>;
    /// Start `call`; its output is appended to `call.log`.
    fn spawn(&self, call: &Call) -> std::io::Result<Box<dyn Running + '_>>;
}

/// The real toolchain: the environment's `PATH`, `DOTNET_ROOT` and the .NET
/// global-tools directory, and real child processes.
pub struct SystemToolchain;

impl Toolchain for SystemToolchain {
    fn locate(&self, tool: Tool) -> Option<PathBuf> {
        let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
        let on_path = |name: &str| {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|dir| dir.join(exe(name)))
                .find(|candidate| candidate.is_file())
        };
        match tool {
            // `DOTNET_ROOT` first: it is how an SDK installed outside the
            // default location is announced, and it is what scip-dotnet's own
            // MSBuild lookup honours too.
            Tool::Dotnet => std::env::var_os("DOTNET_ROOT")
                .map(|root| PathBuf::from(root).join(exe("dotnet")))
                .filter(|candidate| candidate.is_file())
                .or_else(|| on_path("dotnet")),
            // `dotnet tool install --global` puts tools in `~/.dotnet/tools`,
            // which the installer adds to PATH only for new shells; looking
            // there too saves the operator a confusing "not found" right after
            // installing it.
            Tool::ScipDotnet => on_path("scip-dotnet").or_else(|| {
                let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
                Some(
                    PathBuf::from(home)
                        .join(".dotnet")
                        .join("tools")
                        .join(exe("scip-dotnet")),
                )
                .filter(|candidate| candidate.is_file())
            }),
        }
    }

    fn spawn(&self, call: &Call) -> std::io::Result<Box<dyn Running + '_>> {
        // Append rather than truncate: the restore and the indexing of one
        // project write the same log. The log directory starts empty each run,
        // so nothing of an earlier run is appended to.
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&call.log)?;
        let child = std::process::Command::new(&call.program)
            .args(&call.args)
            .current_dir(&call.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        Ok(Box::new(ChildProcess(child)))
    }
}

/// A real child process.
struct ChildProcess(std::process::Child);

impl Running for ChildProcess {
    fn try_wait(&mut self) -> std::io::Result<Option<bool>> {
        Ok(self.0.try_wait()?.map(|status| status.success()))
    }

    /// Kills the direct child only. scip-dotnet and `dotnet restore` may have
    /// started MSBuild worker nodes; those are not in reach without OS job
    /// objects, and they exit on their own when their work or idle timeout ends.
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()?;
        self.0.wait()?;
        Ok(())
    }
}

/// How a timed run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Succeeded,
    Failed,
    TimedOut,
}

/// Start `call` and wait for it at most `timeout`; past that the process is
/// killed and the run reported as [`Finish::TimedOut`]. Spawn + poll rather than
/// a blocking wait, because std has no wait-with-timeout and a new dependency
/// for one loop is not worth it.
pub fn run_timed(
    toolchain: &dyn Toolchain,
    call: &Call,
    timeout: Duration,
) -> std::io::Result<Finish> {
    let mut child = toolchain.spawn(call)?;
    let started = Instant::now();
    loop {
        if let Some(success) = child.try_wait()? {
            return Ok(if success {
                Finish::Succeeded
            } else {
                Finish::Failed
            });
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            child.kill()?;
            return Ok(Finish::TimedOut);
        }
        std::thread::sleep(POLL.min(timeout - elapsed));
    }
}

/// What the operator asked for.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// The repository to index.
    pub repo: PathBuf,
    /// Where the merged index goes. `None`: `index.scip` in the sidecar
    /// directory beside the repository — outside the tree, so writing it never
    /// makes the next run's working tree dirty.
    pub out: Option<PathBuf>,
    /// Run `dotnet restore` on each project before indexing it (off by
    /// default).
    pub restore: bool,
    /// Keep an index of the projects that succeeded when some failed.
    pub allow_partial: bool,
    /// Per-project time limit.
    pub timeout: Duration,
}

/// What a finished run produced.
#[derive(Debug)]
pub struct IndexOutcome {
    pub index_path: PathBuf,
    pub record_path: PathBuf,
    pub record: Record,
}

/// The record written next to the index: everything a later reader needs to
/// decide whether to trust it (U11 refuses an index from another commit, or a
/// partial one). A stored format — see [`RECORD_SCHEMA`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub schema: u32,
    /// The index file's name, as it sits next to this record.
    pub index: String,
    /// SHA-256 of the index file's bytes, lowercase hex.
    pub index_sha256: String,
    /// The commit the working tree was at, or `None` for a tree without one.
    pub commit: Option<String>,
    /// Whether the working tree differed from that commit (modified or
    /// untracked files) when indexing started; `None` without git. A dirty tree
    /// means the index may describe files no commit contains.
    pub dirty: Option<bool>,
    /// Some projects failed and the operator allowed the rest to stand.
    pub partial: bool,
    /// Whether each project was restored (`dotnet restore`) before it was
    /// indexed. A project whose restore failed is a failed project, so with
    /// `true` every indexed project had its packages.
    pub restore: bool,
    pub tools: ToolVersions,
    /// Every project, in canonical (path) order.
    pub projects: Vec<ProjectRecord>,
    /// Occurrences whose target differs between projects (KTD9 step 3), in
    /// path and range order. The same occurrences carry [`CONFLICT_CODE`] in the
    /// index itself. Targets that differ only in the package of an external
    /// symbol or a namespace agree and are not listed ([`merge`], step 3); the
    /// symbols listed are the ones the projects wrote, verbatim.
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVersions {
    /// `dotnet --version`.
    pub dotnet: Option<String>,
    /// `scip-dotnet --version`. Recorded from the tool, not from the index's
    /// own metadata, which scip-dotnet 0.2.14 fills with `0.1.0-SNAPSHOT`.
    pub scip_dotnet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    /// The `.csproj`, relative to the repository root.
    pub path: String,
    pub outcome: ProjectOutcome,
    /// Why a project failed; `None` for an indexed one.
    pub detail: Option<String>,
    /// The project's scip-dotnet output, relative to the record's directory.
    pub log: String,
    /// Documents the project's index named that are not tracked files of the
    /// repository — generated sources under `obj/`, or files outside the tree.
    /// They are left out of the merged index.
    pub untracked_documents: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectOutcome {
    Indexed,
    Failed,
    TimedOut,
}

/// One range whose occurrences name different targets in different projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Document path, relative to the repository root.
    pub path: String,
    /// The SCIP range, as the indexer wrote it.
    pub range: Vec<i32>,
    /// Per project (canonical order), the symbols it put at this range.
    pub targets: Vec<ConflictTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictTarget {
    pub project: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error(
        "`dotnet` was not found (looked in DOTNET_ROOT and on PATH). `reverse-architect index` \
         needs the .NET SDK: install it from https://dot.net and make sure `dotnet` is on PATH, \
         or set DOTNET_ROOT to its directory"
    )]
    DotnetMissing,
    #[error(
        "`scip-dotnet` was not found (looked on PATH and in ~/.dotnet/tools). Install it with \
         `dotnet tool install --global scip-dotnet` and make sure ~/.dotnet/tools is on PATH"
    )]
    ScipDotnetMissing,
    #[error(transparent)]
    Repo(#[from] ExtractError),
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error("cannot read the working-tree status of {path}: {source}")]
    Status { path: PathBuf, source: git2::Error },
    #[error("{0} has no tracked .csproj file — nothing to index")]
    NoProjects(PathBuf),
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
    #[error("no project could be indexed; per-project logs are in {logs}:\n{failures}")]
    NothingIndexed { logs: PathBuf, failures: String },
    #[error(
        "{failed} of {total} projects could not be indexed, so no index was written; \
         per-project logs are in {logs}. Rerun with --allow-partial to keep an index of the \
         others.\n{failures}"
    )]
    ProjectsFailed {
        failed: usize,
        total: usize,
        logs: PathBuf,
        failures: String,
    },
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> IndexError {
    let context = context.into();
    move |source| IndexError::Io { context, source }
}

/// Index every tracked `.csproj` of `opts.repo` and write the merged index and
/// its [`Record`].
///
/// Nothing is written when a project fails and the operator did not allow a
/// partial index: an index missing projects under the usual name is exactly the
/// file someone would later use without noticing.
pub fn run(opts: &IndexOptions, toolchain: &dyn Toolchain) -> Result<IndexOutcome, IndexError> {
    let dotnet = toolchain
        .locate(Tool::Dotnet)
        .ok_or(IndexError::DotnetMissing)?;
    let scip_dotnet = toolchain
        .locate(Tool::ScipDotnet)
        .ok_or(IndexError::ScipDotnetMissing)?;

    let repo = Repo::open(&opts.repo)?;
    // Real paths, `/`-separated: a SCIP document path names the file as it is
    // on disk, never RA's percent-escaped graph key.
    let tracked: BTreeSet<String> = repo
        .tracked_files_with_paths()?
        .iter()
        .map(|tf| tf.path.to_string_lossy().replace('\\', "/"))
        .collect();
    // A BTreeSet iterates sorted, so this is already the canonical project
    // order of KTD9 step 1.
    let projects: Vec<&String> = tracked
        .iter()
        .filter(|p| p.to_ascii_lowercase().ends_with(".csproj"))
        .collect();
    if projects.is_empty() {
        return Err(IndexError::NoProjects(opts.repo.clone()));
    }
    // Taken before anything runs: the build writes `obj/` and `bin/`, and in a
    // repository that does not ignore them that would read as a dirty tree the
    // operator never had.
    let commit = repo.head_revision();
    let dirty = worktree_dirty(&opts.repo)?;

    let repo_abs = std::path::absolute(&opts.repo)
        .map_err(io_err(format!("cannot resolve {}", opts.repo.display())))?;
    let index_path = match &opts.out {
        Some(out) => out.clone(),
        None => crate::config::default_sidecar(&opts.repo)?.join("index.scip"),
    };
    let out_dir = index_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = index_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "index".to_string());
    let logs_name = format!("{stem}.logs");
    let logs = out_dir.join(&logs_name);
    // Logs of an earlier run would sit next to this run's record and name
    // projects it never ran; start from an empty directory.
    if logs.exists() {
        std::fs::remove_dir_all(&logs)
            .map_err(io_err(format!("cannot clear {}", logs.display())))?;
    }
    std::fs::create_dir_all(&logs).map_err(io_err(format!("cannot create {}", logs.display())))?;

    let tools = ToolVersions {
        dotnet: tool_version(
            toolchain,
            &dotnet,
            &repo_abs,
            &logs.join("dotnet-version.log"),
        ),
        scip_dotnet: tool_version(
            toolchain,
            &scip_dotnet,
            &repo_abs,
            &logs.join("scip-dotnet-version.log"),
        ),
    };

    let mut indexed: Vec<ProjectIndex> = Vec::new();
    let mut records: Vec<ProjectRecord> = Vec::new();
    for (n, project) in projects.iter().enumerate() {
        eprintln!(
            "reverse-architect index: [{}/{}] {project}",
            n + 1,
            projects.len()
        );
        let root = project
            .rsplit_once('/')
            .map_or("", |(dir, _)| dir)
            .to_string();
        let file_stem = Path::new(project.as_str())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let log_file = format!("{:03}-{file_stem}.log", n + 1);
        let part = logs.join(format!("{:03}.scip", n + 1));
        let project_dir = repo_abs.join(&root);
        let csproj = repo_abs.join(project.as_str()).into_os_string();
        let log = logs.join(&log_file);
        // The project is named explicitly: a bare `dotnet restore` in a
        // directory with two project files fails with MSB1011 (module docs).
        let restore = opts.restore.then(|| Call {
            program: dotnet.clone(),
            args: vec![
                "restore".into(),
                csproj.clone(),
                "/p:EnableWindowsTargeting=true".into(),
            ],
            cwd: project_dir.clone(),
            log: log.clone(),
        });
        let index = Call {
            program: scip_dotnet.clone(),
            args: vec![
                "index".into(),
                csproj,
                "--output".into(),
                std::path::absolute(&part)
                    .map_err(io_err(format!("cannot resolve {}", part.display())))?
                    .into_os_string(),
                "--working-directory".into(),
                project_dir.clone().into_os_string(),
                "--skip-dotnet-restore".into(),
            ],
            cwd: project_dir,
            log,
        };
        // A failed restore stops the project: indexing it anyway is exactly the
        // package-less index this step exists to avoid.
        let failure = restore
            .as_ref()
            .and_then(|call| step_failure("dotnet restore", toolchain, call, opts.timeout))
            .or_else(|| step_failure("scip-dotnet", toolchain, &index, opts.timeout))
            .or_else(|| match read_index(&part) {
                Ok(index) => {
                    indexed.push(ProjectIndex {
                        project: project.to_string(),
                        root,
                        index,
                    });
                    None
                }
                Err(reason) => Some((ProjectOutcome::Failed, reason)),
            });
        // The per-project index lives only until it is merged.
        let _ = std::fs::remove_file(&part);
        let (outcome, detail) = match failure {
            None => (ProjectOutcome::Indexed, None),
            Some((outcome, detail)) => (outcome, Some(detail)),
        };
        records.push(ProjectRecord {
            path: project.to_string(),
            outcome,
            detail,
            log: format!("{logs_name}/{log_file}"),
            untracked_documents: Vec::new(),
        });
    }

    let failed: Vec<&ProjectRecord> = records
        .iter()
        .filter(|r| r.outcome != ProjectOutcome::Indexed)
        .collect();
    let failures = failed
        .iter()
        .map(|r| format!("  - {}: {}", r.path, r.detail.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");
    if indexed.is_empty() {
        return Err(IndexError::NothingIndexed { logs, failures });
    }
    if !failed.is_empty() && !opts.allow_partial {
        return Err(IndexError::ProjectsFailed {
            failed: failed.len(),
            total: records.len(),
            logs,
            failures,
        });
    }
    let partial = !failed.is_empty();

    let project_root = url::Url::from_directory_path(&repo_abs)
        .map(|u| u.to_string())
        .unwrap_or_default();
    let merged = merge(indexed, &tracked, &project_root);
    for record in &mut records {
        if let Some(untracked) = merged.untracked.get(&record.path) {
            record.untracked_documents = untracked.clone();
        }
    }

    let bytes = merged.index.write_to_bytes().map_err(|e| IndexError::Io {
        context: "cannot encode the merged index".to_string(),
        source: std::io::Error::other(e),
    })?;
    let index_sha256 = format!("{:x}", Sha256::digest(&bytes));
    write_replacing(&index_path, &bytes)?;

    let record = Record {
        schema: RECORD_SCHEMA,
        index: index_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        index_sha256,
        commit,
        dirty,
        partial,
        restore: opts.restore,
        tools,
        projects: records,
        conflicts: merged.conflicts,
    };
    let record_path = out_dir.join(format!("{stem}.record.json"));
    let mut json = serde_json::to_string_pretty(&record).map_err(|e| IndexError::Io {
        context: "cannot encode the record".to_string(),
        source: std::io::Error::other(e),
    })?;
    json.push('\n');
    write_replacing(&record_path, json.as_bytes())?;

    Ok(IndexOutcome {
        index_path,
        record_path,
        record,
    })
}

/// Run one step of a project (`dotnet restore` or scip-dotnet) under the time
/// limit; `None` when it succeeded, otherwise how the project failed and why.
fn step_failure(
    what: &str,
    toolchain: &dyn Toolchain,
    call: &Call,
    timeout: Duration,
) -> Option<(ProjectOutcome, String)> {
    match run_timed(toolchain, call, timeout) {
        Ok(Finish::Succeeded) => None,
        Err(e) => Some((
            ProjectOutcome::Failed,
            format!("could not start {what}: {e}"),
        )),
        Ok(Finish::TimedOut) => Some((
            ProjectOutcome::TimedOut,
            format!("{what} killed after the {} s time limit", timeout.as_secs()),
        )),
        Ok(Finish::Failed) => Some((
            ProjectOutcome::Failed,
            format!("{what} exited with an error"),
        )),
    }
}

/// Write through a temporary file and rename it into place, so a reader never
/// sees half an index and an interrupted run leaves the previous file intact.
fn write_replacing(path: &Path, bytes: &[u8]) -> Result<(), IndexError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(io_err(format!("cannot create {}", parent.display())))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(io_err(format!("cannot write {}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(io_err(format!("cannot write {}", path.display())))
}

fn read_index(path: &Path) -> Result<Index, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("scip-dotnet reported success but its index is unreadable: {e}"))?;
    Index::parse_from_bytes(&bytes)
        .map_err(|e| format!("scip-dotnet reported success but wrote no valid index: {e}"))
}

/// First line of `<tool> --version`, or `None` when it cannot be had. A missing
/// version is recorded as missing rather than failing the run: it describes the
/// index, it does not decide it.
fn tool_version(
    toolchain: &dyn Toolchain,
    program: &Path,
    cwd: &Path,
    log: &Path,
) -> Option<String> {
    let call = Call {
        program: program.to_path_buf(),
        args: vec!["--version".into()],
        cwd: cwd.to_path_buf(),
        log: log.to_path_buf(),
    };
    match run_timed(toolchain, &call, VERSION_TIMEOUT) {
        Ok(Finish::Succeeded) => std::fs::read_to_string(log)
            .ok()?
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Whether the working tree differs from HEAD — modified, staged or untracked
/// files, ignored ones excepted. `None` for a tree that is not a git repository.
fn worktree_dirty(root: &Path) -> Result<Option<bool>, IndexError> {
    let git = match git2::Repository::open(root) {
        Ok(git) => git,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(source) => {
            return Err(IndexError::Status {
                path: root.to_path_buf(),
                source,
            })
        }
    };
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = git
        .statuses(Some(&mut options))
        .map_err(|source| IndexError::Status {
            path: root.to_path_buf(),
            source,
        })?;
    Ok(Some(!statuses.is_empty()))
}

/// One project's index, ready to merge.
#[derive(Debug, Clone)]
pub struct ProjectIndex {
    /// The `.csproj`, relative to the repository root (`/`-separated). Decides
    /// the canonical order.
    pub project: String,
    /// The directory the project was indexed from, relative to the repository
    /// root (`""` for the root itself). The index's document paths are relative
    /// to it.
    pub root: String,
    pub index: Index,
}

/// The result of [`merge`].
#[derive(Debug)]
pub struct Merged {
    pub index: Index,
    /// See [`Record::conflicts`].
    pub conflicts: Vec<Conflict>,
    /// Per project, the documents left out because they are not tracked files.
    pub untracked: BTreeMap<String, Vec<String>>,
}

/// Merge per-project indexes into one (KTD9 steps 1-4).
///
/// 1. Projects are taken in canonical order — sorted by `.csproj` path — so
///    whichever "first" a step below picks is the same in every run.
/// 2. Each document path is recomputed from the project's indexing root to a
///    path relative to the repository root (`\` folded to `/`, `.` and `..`
///    resolved lexically) and kept only when it names a `tracked` file. The
///    consumer ignores `project_root`, so a path left relative to a project
///    directory would silently match nothing.
/// 3. A document several projects produced (a shared or linked file) becomes
///    one. Identical occurrences and symbol entries collapse. Where two
///    projects put DIFFERENT targets on the same range, every occurrence at that
///    range is kept and carries a [`CONFLICT_CODE`] diagnostic, and the range is
///    listed in [`Merged::conflicts`]: nothing the compiler said is dropped, and
///    a reader can tell the range apart without the record. A range only one
///    project indexed (other preprocessor symbols) is not a conflict.
///
///    Targets that differ ONLY in the package part of the symbol (scheme,
///    manager, package name, version) with identical descriptors are not
///    different targets when they are external (package other than `. .`) or a
///    namespace. A folder holding `X.csproj` for .NET Framework beside
///    `X.NetCore.csproj` for net10 binds the same call to `mscorlib 4.0.0.0` in
///    one and to `System.Runtime 10.0.0.0` in the other; both say "this
///    external symbol", which is what U9 reads off it. On the reference C# repository (149
///    projects) these were 321 850 of the 377 011 ranges the merge used to
///    report (classified from the record of 03.10.2026). Locals that differ
///    only in their number (`local 35` against `local 32`) are not different
///    targets either: each compilation numbers a document's locals on its
///    own, and U9 pairs no site with a local (16 977 of the 55 161 ranges
///    left). Such a range is merged under ONE symbol per descriptor, and one
///    for all its locals — the lexicographically smallest of the strings the
///    projects wrote, so the choice depends on the symbols, never on project
///    order — and the now identical occurrences collapse. A local renamed
///    this way can share its number with another variable elsewhere in the
///    document; nothing reads a local's identity across ranges. A namespace canonicalized this way may
///    read `. .` where one project said `mscorlib`; U9 attributes nothing to a
///    namespace, so the choice is moot there. A repository symbol against an
///    external one of the same descriptor (a polyfill), or descriptors that
///    differ at all (an overload ordinal `Trim()` against `Trim(+1)` included),
///    stay a conflict. Definitions follow the same rule, being occurrences at
///    a range like any other; in scip-dotnet's default mode a definition is
///    `. .`, so only a namespace one could differ in package alone. Both
///    packages' entries stay in `external_symbols`, unified by full symbol.
/// 4. Documents are sorted by path, occurrences by range then symbol, symbol
///    entries by symbol — the output depends on the inputs only, never on the
///    order projects finished in or were passed in.
///
/// `project_root` becomes the merged index's `project_root`: the paths are now
/// relative to the repository root, and SCIP says that field names it.
pub fn merge(
    mut projects: Vec<ProjectIndex>,
    tracked: &BTreeSet<String>,
    project_root: &str,
) -> Merged {
    projects.sort_by(|a, b| a.project.cmp(&b.project));

    let mut by_path: BTreeMap<String, Vec<(usize, Document)>> = BTreeMap::new();
    let mut untracked: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut external: BTreeMap<String, SymbolInformation> = BTreeMap::new();
    for (i, project) in projects.iter_mut().enumerate() {
        for document in std::mem::take(&mut project.index.documents) {
            match repo_path(&project.root, &document.relative_path) {
                Some(path) if tracked.contains(&path) => {
                    by_path.entry(path).or_default().push((i, document))
                }
                other => untracked
                    .entry(project.project.clone())
                    .or_default()
                    .push(other.unwrap_or_else(|| document.relative_path.replace('\\', "/"))),
            }
        }
        for symbol in std::mem::take(&mut project.index.external_symbols) {
            unify_symbol(&mut external, symbol);
        }
    }
    for paths in untracked.values_mut() {
        paths.sort();
        paths.dedup();
    }

    let mut conflicts = Vec::new();
    let documents = by_path
        .into_iter()
        .map(|(path, documents)| merge_document(path, documents, &projects, &mut conflicts))
        .collect();

    let mut index = Index::new();
    if let Some(first) = projects.first() {
        let source = first.index.metadata.get_or_default();
        let metadata = index.metadata.mut_or_insert_default();
        metadata.version = source.version;
        metadata.text_document_encoding = source.text_document_encoding;
        let mut tool = scip::types::ToolInfo::new();
        tool.name = source.tool_info.name.clone();
        tool.version = source.tool_info.version.clone();
        // Arguments are left out: they are absolute paths of one machine and
        // one project, and would make the merged bytes depend on both.
        metadata.tool_info = Some(tool).into();
        metadata.project_root = project_root.to_string();
    }
    index.documents = documents;
    index.external_symbols = external.into_values().collect();

    Merged {
        index,
        conflicts,
        untracked,
    }
}

/// `relative` (as the indexer wrote it, relative to `root`) as a path relative
/// to the repository root, or `None` when it climbs out of the repository.
fn repo_path(root: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for segment in root.split(['/', '\\']).chain(relative.split(['/', '\\'])) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// Add `symbol` to `into`, unifying with an entry for the same symbol: the
/// first one (canonical project order) keeps its fields, and relationships are
/// the union — a project that saw an extra `implements` must not lose it.
fn unify_symbol(into: &mut BTreeMap<String, SymbolInformation>, symbol: SymbolInformation) {
    match into.get_mut(&symbol.symbol) {
        None => {
            into.insert(symbol.symbol.clone(), symbol);
        }
        Some(existing) => {
            existing.relationships.extend(symbol.relationships);
            existing.relationships.sort_by(|a, b| {
                a.symbol
                    .cmp(&b.symbol)
                    .then_with(|| bytes(a).cmp(&bytes(b)))
            });
            existing.relationships.dedup();
        }
    }
}

fn bytes(message: &impl Message) -> Vec<u8> {
    message.write_to_bytes().unwrap_or_default()
}

/// Merge one path's documents (KTD9 step 3). `documents` arrive in canonical
/// project order; `projects` names each index.
fn merge_document(
    path: String,
    documents: Vec<(usize, Document)>,
    projects: &[ProjectIndex],
    conflicts: &mut Vec<Conflict>,
) -> Document {
    let mut merged = Document::new();
    merged.relative_path = path.clone();
    if let Some((_, first)) = documents.first() {
        merged.position_encoding = first.position_encoding;
    }
    merged.language = documents
        .iter()
        .map(|(_, d)| d.language.as_str())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_string();
    merged.text = documents
        .iter()
        .map(|(_, d)| d.text.as_str())
        .find(|t| !t.is_empty())
        .unwrap_or_default()
        .to_string();

    // range → project → that project's occurrences at the range.
    let mut by_range: BTreeMap<Vec<i32>, BTreeMap<usize, Vec<Occurrence>>> = BTreeMap::new();
    let mut symbols: BTreeMap<String, SymbolInformation> = BTreeMap::new();
    for (project, document) in documents {
        for occurrence in document.occurrences {
            by_range
                .entry(occurrence.range.clone())
                .or_default()
                .entry(project)
                .or_default()
                .push(occurrence);
        }
        for symbol in document.symbols {
            unify_symbol(&mut symbols, symbol);
        }
    }

    let mut occurrences = Vec::new();
    for (range, mut per_project) in by_range {
        let targets: Vec<ConflictTarget> = per_project
            .iter()
            .map(|(&project, occurrences)| {
                let symbols: BTreeSet<&str> =
                    occurrences.iter().map(|o| o.symbol.as_str()).collect();
                ConflictTarget {
                    project: projects[project].project.clone(),
                    symbols: symbols.into_iter().map(str::to_string).collect(),
                }
            })
            .collect();
        // Equal strings settle almost every range; only one the projects
        // disagree on is worth parsing symbols for.
        let mut conflicting = targets.windows(2).any(|w| w[0].symbols != w[1].symbols);
        if conflicting {
            if let Some(canonical) = package_agnostic_agreement(&targets) {
                for occurrence in per_project.values_mut().flatten() {
                    if let Some(symbol) = canonical.get(&occurrence.symbol) {
                        occurrence.symbol = symbol.clone();
                    }
                }
                conflicting = false;
            }
        }
        let diagnostic = conflicting.then(|| conflict_diagnostic(&targets));
        for occurrence in per_project.into_values().flatten() {
            let mut occurrence = occurrence;
            if let Some(diagnostic) = &diagnostic {
                occurrence.diagnostics.push(diagnostic.clone());
            }
            occurrences.push(occurrence);
        }
        if conflicting {
            conflicts.push(Conflict {
                path: path.clone(),
                range,
                targets,
            });
        }
    }
    occurrences.sort_by(|a, b| {
        a.range
            .cmp(&b.range)
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.symbol_roles.cmp(&b.symbol_roles))
            .then_with(|| bytes(a).cmp(&bytes(b)))
    });
    occurrences.dedup();
    merged.occurrences = occurrences;
    merged.symbols = symbols.into_values().collect();
    merged
}

/// What a target means for agreement between projects (KTD9 step 3): the
/// descriptors alone, or the whole symbol string.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TargetClass<'a> {
    /// The whole symbol must match: a symbol of the repository's own sources
    /// (package `. .`), or a string that does not parse.
    Exact(&'a str),
    /// A local (`local 35`). Every local is one class: each compilation
    /// numbers a document's locals on its own, so two projects over one file
    /// give one variable different numbers, and nothing a local's number says
    /// is read downstream — U9 pairs no usage site with a local.
    Local,
    /// The descriptors, each as its encoded message; the scheme and package
    /// (manager, name, version) do not count.
    Descriptors(Vec<Vec<u8>>),
}

/// How `symbol` takes part in agreement. Locals agree with each other
/// whatever their numbers ([`TargetClass::Local`]). A target outside the
/// repository (package other than `. .`) and any namespace are compared by
/// descriptor only: a folder holding `X.csproj` for .NET Framework and `X.NetCore.csproj`
/// for net10 compiles the same call against `mscorlib 4.0.0.0` in one and
/// `System.Runtime 10.0.0.0` in the other, and a namespace is the same
/// namespace whichever assembly scip-dotnet attributes it to (`. .` in one
/// build, `mscorlib 4.0.0.0` in the other). A repository symbol is NOT
/// compared by descriptor: the same descriptor in the repository and in a
/// framework assembly (a polyfill the older build compiles itself) are two
/// different targets.
fn target_class(symbol: &str) -> TargetClass<'_> {
    if scip::symbol::is_local_symbol(symbol) {
        return TargetClass::Local;
    }
    let Ok(parsed) = scip::symbol::parse_symbol(symbol) else {
        return TargetClass::Exact(symbol);
    };
    let namespace = parsed
        .descriptors
        .last()
        .is_some_and(|d| d.suffix.enum_value() == Ok(scip::types::descriptor::Suffix::Namespace));
    // `parse_symbol` reads SCIP's `.` placeholder as empty, so `. .` is an
    // empty name and version.
    let external = parsed
        .package
        .as_ref()
        .is_some_and(|p| !p.name.is_empty() || !p.version.is_empty());
    if namespace || external {
        TargetClass::Descriptors(parsed.descriptors.iter().map(bytes).collect())
    } else {
        TargetClass::Exact(symbol)
    }
}

/// When `targets` differ only in symbols of one [`TargetClass`] — every project
/// names the same classes at the range — the projects agree after all, and the
/// map says which symbol string each one is replaced by: per class, the
/// lexicographically smallest of the symbols the projects used. Smallest
/// rather than first project's, so the choice is a property of the symbols
/// alone. `None` when the projects really disagree.
fn package_agnostic_agreement(targets: &[ConflictTarget]) -> Option<BTreeMap<String, String>> {
    let classes: Vec<BTreeSet<TargetClass<'_>>> = targets
        .iter()
        .map(|t| t.symbols.iter().map(|s| target_class(s)).collect())
        .collect();
    if classes.windows(2).any(|w| w[0] != w[1]) {
        return None;
    }
    let mut smallest: BTreeMap<TargetClass<'_>, &str> = BTreeMap::new();
    for symbol in targets.iter().flat_map(|t| &t.symbols) {
        let held = smallest.entry(target_class(symbol)).or_insert(symbol);
        if symbol.as_str() < *held {
            *held = symbol;
        }
    }
    Some(
        targets
            .iter()
            .flat_map(|t| &t.symbols)
            .filter_map(|symbol| {
                let canonical = smallest[&target_class(symbol)];
                (canonical != symbol).then(|| (symbol.clone(), canonical.to_string()))
            })
            .collect(),
    )
}

/// The diagnostic every occurrence at a conflicting range carries. Its message
/// names each project's targets, so the index alone says what disagreed.
fn conflict_diagnostic(targets: &[ConflictTarget]) -> Diagnostic {
    let mut diagnostic = Diagnostic::new();
    diagnostic.severity = Severity::Warning.into();
    diagnostic.code = CONFLICT_CODE.to_string();
    diagnostic.source = "reverse-architect index".to_string();
    diagnostic.message = format!(
        "projects disagree on this occurrence's target: {}",
        targets
            .iter()
            .map(|t| format!("{} -> {}", t.project, t.symbols.join(", ")))
            .collect::<Vec<_>>()
            .join("; ")
    );
    diagnostic
}
