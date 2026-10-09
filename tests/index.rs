//! U10 integration tests: `reverse-architect index` (plan A, KTD9).
//!
//! No .NET SDK is needed. The step reaches the machine only through
//! [`Toolchain`], and these tests hand it a fake one: it "finds" the tools on
//! demand, its `dotnet restore` succeeds or fails per project, and its
//! scip-dotnet writes a prepared index, fails, or never ends.
//! The time-limit loop, the failure policy, the merge and the record are the
//! production code either way. The one test that goes through the binary uses
//! the real lookup with an emptied environment.

mod common;

use common::{init_repo_with_files, scip_document, scip_occurrence, scip_sym};
use protobuf::Message;
use reverse_architect::index::{
    merge, run, Call, IndexError, IndexOptions, Merged, ProjectIndex, ProjectOutcome, Record,
    Running, Tool, Toolchain, CONFLICT_CODE,
};
use scip::types::{Document, Index, Occurrence};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// What the fake scip-dotnet does for one project.
#[derive(Clone)]
enum Behavior {
    /// Exit 0 having written this index to `--output`.
    Write(Index),
    /// Exit non-zero.
    Fail,
    /// Never exit.
    Hang,
}

struct FakeToolchain {
    dotnet: bool,
    /// Keyed by the `.csproj` file name.
    projects: BTreeMap<String, Behavior>,
    /// `.csproj` file names whose `dotnet restore` exits non-zero.
    failing_restores: BTreeSet<String>,
    calls: Mutex<Vec<Call>>,
    kills: AtomicUsize,
}

impl FakeToolchain {
    fn new(projects: &[(&str, Behavior)]) -> Self {
        FakeToolchain {
            dotnet: true,
            projects: projects
                .iter()
                .map(|(name, b)| (name.to_string(), b.clone()))
                .collect(),
            failing_restores: BTreeSet::new(),
            calls: Mutex::new(Vec::new()),
            kills: AtomicUsize::new(0),
        }
    }

    /// Every call, as `(program, args)`, in the order they were started.
    fn all_calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c.program.to_string_lossy().into_owned(),
                    c.args
                        .iter()
                        .map(|a| a.to_string_lossy().into_owned())
                        .collect(),
                )
            })
            .collect()
    }

    fn index_calls(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.args.first().is_some_and(|a| a == "index"))
            .map(|c| {
                c.args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }
}

struct Exited(bool);
impl Running for Exited {
    fn try_wait(&mut self) -> std::io::Result<Option<bool>> {
        Ok(Some(self.0))
    }
    fn kill(&mut self) -> std::io::Result<()> {
        panic!("an exited process is never killed")
    }
}

struct Hanging<'a>(&'a AtomicUsize);
impl Running for Hanging<'_> {
    fn try_wait(&mut self) -> std::io::Result<Option<bool>> {
        Ok(None)
    }
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl Toolchain for FakeToolchain {
    fn locate(&self, tool: Tool) -> Option<PathBuf> {
        match tool {
            Tool::Dotnet => self.dotnet.then(|| PathBuf::from("fake/dotnet")),
            Tool::ScipDotnet => Some(PathBuf::from("fake/scip-dotnet")),
        }
    }

    fn spawn(&self, call: &Call) -> std::io::Result<Box<dyn Running + '_>> {
        self.calls.lock().unwrap().push(call.clone());
        let args: Vec<String> = call
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if args.first().map(String::as_str) == Some("--version") {
            std::fs::write(&call.log, "9.9.9\n")?;
            return Ok(Box::new(Exited(true)));
        }
        let project = Path::new(&args[1])
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if args[0] == "restore" {
            std::fs::write(
                &call.log,
                format!(
                    "fake dotnet restore: {project}
"
                ),
            )?;
            return Ok(Box::new(Exited(!self.failing_restores.contains(&project))));
        }
        let output = &args[args.iter().position(|a| a == "--output").unwrap() + 1];
        std::fs::write(&call.log, format!("fake scip-dotnet: {project}\n"))?;
        match self.projects.get(&project).expect("a prepared project") {
            Behavior::Write(index) => {
                std::fs::write(output, index.write_to_bytes().unwrap())?;
                Ok(Box::new(Exited(true)))
            }
            Behavior::Fail => Ok(Box::new(Exited(false))),
            Behavior::Hang => Ok(Box::new(Hanging(&self.kills))),
        }
    }
}

fn index_of(documents: Vec<Document>) -> Index {
    let mut index = Index::new();
    let mut tool = scip::types::ToolInfo::new();
    tool.name = "scip-dotnet".to_string();
    tool.version = "0.1.0-SNAPSHOT".to_string();
    index.metadata.mut_or_insert_default().tool_info = Some(tool).into();
    index.documents = documents;
    index
}

fn reference(symbol: &str, line: i32, column: i32) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol.to_string();
    o.range = vec![line, column, column + 5];
    o
}

/// A repository with two projects and a file both of them compile.
fn two_project_repo(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("src/A/A.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\" />\n"),
            ("src/A/Caller.cs", "class Caller {}\n"),
            ("lib/B/B.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\" />\n"),
            ("lib/B/Greeter.cs", "class Greeter {}\n"),
            ("src/Shared/Util.cs", "class Util {}\n"),
        ],
    );
}

/// Project A's index as scip-dotnet writes it from `src/A`: its own file, the
/// linked shared file reached through `..`, and a generated file under `obj/`.
fn index_a() -> Index {
    index_of(vec![
        scip_document(
            "Caller.cs",
            vec![scip_occurrence(&scip_sym("Caller#"), 1, true)],
        ),
        scip_document(
            "..\\Shared\\Util.cs",
            vec![scip_occurrence(&scip_sym("Util#"), 1, true)],
        ),
        scip_document("obj\\Debug\\net10.0\\A.AssemblyInfo.cs", vec![]),
    ])
}

/// Project B's index from `lib/B`.
fn index_b() -> Index {
    index_of(vec![scip_document(
        "Greeter.cs",
        vec![scip_occurrence(&scip_sym("Greeter#"), 1, true)],
    )])
}

fn options(repo: &Path, out: &Path) -> IndexOptions {
    IndexOptions {
        repo: repo.to_path_buf(),
        out: Some(out.join("index.scip")),
        restore: false,
        allow_partial: false,
        timeout: Duration::from_secs(60),
    }
}

fn read_record(out: &Path) -> Record {
    serde_json::from_str(&std::fs::read_to_string(out.join("index.record.json")).unwrap()).unwrap()
}

fn read_index(path: &Path) -> Index {
    Index::parse_from_bytes(&std::fs::read(path).unwrap()).unwrap()
}

fn paths(index: &Index) -> Vec<&str> {
    index
        .documents
        .iter()
        .map(|d| d.relative_path.as_str())
        .collect()
}

#[test]
fn without_dotnet_the_binary_fails_with_guidance_after_the_build_logic_warning() {
    let tmp = tempfile::tempdir().unwrap();
    two_project_repo(tmp.path());
    let empty = tmp.path().join("no-home");
    std::fs::create_dir_all(&empty).unwrap();

    assert_cmd::Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg("index")
        .arg(tmp.path())
        .env("PATH", "")
        .env_remove("DOTNET_ROOT")
        .env("USERPROFILE", &empty)
        .env("HOME", &empty)
        .assert()
        .failure()
        .stderr(predicates::str::contains("build logic"))
        .stderr(predicates::str::contains("`dotnet` was not found"))
        .stderr(predicates::str::contains("https://dot.net"));
}

#[test]
fn two_projects_merge_into_one_index_with_repository_relative_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Write(index_b())),
    ]);

    let outcome = run(&options(&repo, &out), &fake).unwrap();

    let index = read_index(&outcome.index_path);
    // Recomputed from each project's root, verified against tracked files,
    // sorted by path; the generated `obj/` file is not a tracked file.
    assert_eq!(
        paths(&index),
        ["lib/B/Greeter.cs", "src/A/Caller.cs", "src/Shared/Util.cs"]
    );
    let record = &outcome.record;
    assert_eq!(
        record
            .projects
            .iter()
            .map(|p| (p.path.as_str(), p.outcome))
            .collect::<Vec<_>>(),
        [
            ("lib/B/B.csproj", ProjectOutcome::Indexed),
            ("src/A/A.csproj", ProjectOutcome::Indexed),
        ]
    );
    assert_eq!(
        record.projects[1].untracked_documents,
        ["src/A/obj/Debug/net10.0/A.AssemblyInfo.cs"]
    );
    assert!(!record.partial);
    assert_eq!(record.tools.scip_dotnet.as_deref(), Some("9.9.9"));
    assert_eq!(
        *record,
        read_record(&out),
        "the record on disk is the one returned"
    );
}

#[test]
fn the_record_carries_the_sha256_of_the_index_file_and_the_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Write(index_b())),
    ]);

    let outcome = run(&options(&repo, &out), &fake).unwrap();

    let bytes = std::fs::read(&outcome.index_path).unwrap();
    let expected = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(outcome.record.index_sha256, expected);
    let head = git2::Repository::open(&repo)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    assert_eq!(outcome.record.commit.as_deref(), Some(head.as_str()));
    assert_eq!(outcome.record.dirty, Some(false));
}

#[test]
fn the_record_says_the_working_tree_was_dirty() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    // An edit the commit does not contain — and that the build would compile.
    std::fs::write(repo.join("src/A/Caller.cs"), "class Caller { int x; }\n").unwrap();
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Write(index_b())),
    ]);

    let outcome = run(&options(&repo, &out), &fake).unwrap();

    assert_eq!(outcome.record.dirty, Some(true));
}

#[test]
fn scip_dotnet_never_restores_and_without_restore_nothing_else_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    two_project_repo(&repo);
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Write(index_b())),
    ]);

    let outcome = run(&options(&repo, &tmp.path().join("out")), &fake).unwrap();

    let calls = fake.index_calls();
    assert_eq!(calls.len(), 2, "one scip-dotnet run per project");
    assert!(calls
        .iter()
        .all(|args| args.iter().any(|a| a == "--skip-dotnet-restore")));
    assert!(
        fake.all_calls()
            .iter()
            .all(|(_, args)| args.first().map(String::as_str) != Some("restore")),
        "no restore unless the operator asks for it"
    );
    assert!(!outcome.record.restore);
}

/// scip-dotnet's own restore runs `dotnet restore` in the working directory
/// without naming the project, which MSBuild refuses (MSB1011) in a directory
/// holding two `.csproj` files — and scip-dotnet then indexes without packages
/// and reports success. So RA restores each project itself, by its path.
#[test]
fn with_restore_ra_restores_each_project_by_path_before_indexing_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    // Two project files in one directory: the layout MSB1011 trips on.
    init_repo_with_files(
        &repo,
        "Alice",
        &[
            (
                "src/A/A.csproj",
                "<Project Sdk=\"Microsoft.NET.Sdk\" />
",
            ),
            (
                "src/A/A.NetCore.csproj",
                "<Project Sdk=\"Microsoft.NET.Sdk\" />
",
            ),
            (
                "src/A/Caller.cs",
                "class Caller {}
",
            ),
            (
                "lib/B/B.csproj",
                "<Project Sdk=\"Microsoft.NET.Sdk\" />
",
            ),
            (
                "lib/B/Greeter.cs",
                "class Greeter {}
",
            ),
            (
                "src/Shared/Util.cs",
                "class Util {}
",
            ),
        ],
    );
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("A.NetCore.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Write(index_b())),
    ]);
    let mut opts = options(&repo, &tmp.path().join("out"));
    opts.restore = true;

    let outcome = run(&opts, &fake).unwrap();

    let repo_abs = std::path::absolute(&repo).unwrap();
    // Per project call: the program, the verb and the project it names.
    let project_calls: Vec<(String, String, String)> = fake
        .all_calls()
        .into_iter()
        .filter(|(_, args)| args[0] != "--version")
        .map(|(program, args)| (program, args[0].clone(), args[1].clone()))
        .collect();
    let mut expected = Vec::new();
    for project in ["lib/B/B.csproj", "src/A/A.NetCore.csproj", "src/A/A.csproj"] {
        let csproj = repo_abs.join(project).to_string_lossy().into_owned();
        expected.push((
            "fake/dotnet".to_string(),
            "restore".to_string(),
            csproj.clone(),
        ));
        expected.push(("fake/scip-dotnet".to_string(), "index".to_string(), csproj));
    }
    assert_eq!(
        project_calls, expected,
        "each project is restored by its own path, right before it is indexed"
    );
    assert!(fake
        .all_calls()
        .iter()
        .filter(|(_, args)| args[0] == "restore")
        .all(|(_, args)| args[2..] == ["/p:EnableWindowsTargeting=true"]));
    assert!(fake
        .index_calls()
        .iter()
        .all(|args| args.iter().any(|a| a == "--skip-dotnet-restore")));
    // The restore shares the project's log and working directory.
    let calls = fake.calls.lock().unwrap();
    let project_only: Vec<&Call> = calls
        .iter()
        .filter(|c| c.args.first().is_some_and(|a| a != "--version"))
        .collect();
    for pair in project_only.chunks(2) {
        assert_eq!(pair[0].log, pair[1].log);
        assert_eq!(pair[0].cwd, pair[1].cwd);
    }
    assert!(outcome.record.restore);
}

#[test]
fn a_failed_restore_fails_the_project_without_indexing_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    let mut fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Write(index_b())),
    ]);
    fake.failing_restores.insert("B.csproj".to_string());
    let mut opts = options(&repo, &out);
    opts.restore = true;
    opts.allow_partial = true;

    let outcome = run(&opts, &fake).unwrap();

    let b = &outcome.record.projects[0];
    assert_eq!(
        (b.path.as_str(), b.outcome),
        ("lib/B/B.csproj", ProjectOutcome::Failed)
    );
    assert!(
        b.detail.as_deref().is_some_and(|d| d.contains("restore")),
        "{:?}",
        b.detail
    );
    assert!(
        fake.index_calls()
            .iter()
            .all(|args| !args[1].ends_with("B.csproj")),
        "a project whose restore failed is not indexed without its packages"
    );
    assert_eq!(outcome.record.projects[1].outcome, ProjectOutcome::Indexed);
    assert_eq!(
        paths(&read_index(&outcome.index_path)),
        ["src/A/Caller.cs", "src/Shared/Util.cs"]
    );
}

#[test]
fn a_failed_project_fails_the_run_and_writes_no_index() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Fail),
    ]);

    let err = run(&options(&repo, &out), &fake).unwrap_err();

    assert!(
        matches!(
            err,
            IndexError::ProjectsFailed {
                failed: 1,
                total: 2,
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("lib/B/B.csproj"), "{err}");
    assert!(!out.join("index.scip").exists());
    assert!(!out.join("index.record.json").exists());
}

#[test]
fn with_allow_partial_a_failed_project_is_named_in_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Fail),
    ]);
    let mut opts = options(&repo, &out);
    opts.allow_partial = true;

    let outcome = run(&opts, &fake).unwrap();

    assert!(outcome.record.partial);
    let b = &outcome.record.projects[0];
    assert_eq!(b.path, "lib/B/B.csproj");
    assert_eq!(b.outcome, ProjectOutcome::Failed);
    assert!(
        out.join(&b.log).is_file(),
        "the failed project's log is kept"
    );
    assert_eq!(
        paths(&read_index(&outcome.index_path)),
        ["src/A/Caller.cs", "src/Shared/Util.cs"]
    );
}

#[test]
fn a_project_past_the_time_limit_is_killed_and_counted_as_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let out = tmp.path().join("out");
    two_project_repo(&repo);
    let fake = FakeToolchain::new(&[
        ("A.csproj", Behavior::Write(index_a())),
        ("B.csproj", Behavior::Hang),
    ]);
    let mut opts = options(&repo, &out);
    opts.allow_partial = true;
    opts.timeout = Duration::from_millis(200);

    let outcome = run(&opts, &fake).unwrap();

    assert_eq!(fake.kills.load(Ordering::SeqCst), 1);
    let b = &outcome.record.projects[0];
    assert_eq!(
        (b.path.as_str(), b.outcome),
        ("lib/B/B.csproj", ProjectOutcome::TimedOut)
    );
    assert_eq!(
        outcome.record.projects[1].outcome,
        ProjectOutcome::Indexed,
        "the next project still runs"
    );
}

// ---- merge (KTD9 steps 1-4) ----

fn tracked(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn project(path: &str, root: &str, index: Index) -> ProjectIndex {
    ProjectIndex {
        project: path.to_string(),
        root: root.to_string(),
        index,
    }
}

const ROOT_URI: &str = "file:///repo/";

#[test]
fn merging_is_independent_of_project_order_and_writes_a_shared_document_once() {
    let shared = || {
        scip_document(
            "../Shared/Util.cs",
            vec![
                scip_occurrence(&scip_sym("Util#"), 1, true),
                reference(&scip_sym("Greeter#Hello()."), 4, 8),
            ],
        )
    };
    let a = project(
        "src/A/A.csproj",
        "src/A",
        index_of(vec![scip_document("Caller.cs", vec![]), shared()]),
    );
    let b = project(
        "src/B/B.csproj",
        "src/B",
        index_of(vec![scip_document("Greeter.cs", vec![]), shared()]),
    );
    let files = tracked(&["src/A/Caller.cs", "src/B/Greeter.cs", "src/Shared/Util.cs"]);

    let forward = merge(vec![a.clone(), b.clone()], &files, ROOT_URI);
    let backward = merge(vec![b, a], &files, ROOT_URI);

    assert_eq!(
        forward.index.write_to_bytes().unwrap(),
        backward.index.write_to_bytes().unwrap()
    );
    assert_eq!(
        paths(&forward.index),
        ["src/A/Caller.cs", "src/B/Greeter.cs", "src/Shared/Util.cs"]
    );
    let util = &forward.index.documents[2];
    assert_eq!(util.occurrences.len(), 2, "identical occurrences unified");
    assert!(forward.conflicts.is_empty());
    assert!(util.occurrences.iter().all(|o| o.diagnostics.is_empty()));
    assert_eq!(forward.index.metadata.project_root, ROOT_URI);
}

#[test]
fn a_shared_occurrence_with_different_targets_is_marked_conflicting_in_both_orders() {
    let shared = |target: &str| {
        scip_document(
            "Shared.cs",
            vec![
                scip_occurrence(&scip_sym("Shared#"), 1, true),
                reference(&scip_sym(target), 6, 12),
            ],
        )
    };
    let a = project("A.csproj", "", index_of(vec![shared("Left#Run().")]));
    let b = project("B.csproj", "", index_of(vec![shared("Right#Run().")]));
    let files = tracked(&["Shared.cs"]);

    let forward = merge(vec![a.clone(), b.clone()], &files, ROOT_URI);
    let backward = merge(vec![b, a], &files, ROOT_URI);

    assert_eq!(
        forward.index.write_to_bytes().unwrap(),
        backward.index.write_to_bytes().unwrap()
    );
    assert_eq!(forward.conflicts, backward.conflicts);

    let document = &forward.index.documents[0];
    let at = |line: i32| -> Vec<&Occurrence> {
        document
            .occurrences
            .iter()
            .filter(|o| o.range[0] == line)
            .collect()
    };
    // Both targets survive, each marked; the agreeing definition is untouched.
    let conflicting = at(6);
    assert_eq!(
        conflicting
            .iter()
            .map(|o| o.symbol.as_str())
            .collect::<Vec<_>>(),
        [scip_sym("Left#Run()."), scip_sym("Right#Run().")]
    );
    assert!(conflicting
        .iter()
        .all(|o| o.diagnostics.len() == 1 && o.diagnostics[0].code == CONFLICT_CODE));
    assert!(at(0).iter().all(|o| o.diagnostics.is_empty()));

    assert_eq!(forward.conflicts.len(), 1);
    let conflict = &forward.conflicts[0];
    assert_eq!(conflict.path, "Shared.cs");
    assert_eq!(conflict.range, vec![6, 12, 17]);
    assert_eq!(
        conflict
            .targets
            .iter()
            .map(|t| (t.project.as_str(), t.symbols.clone()))
            .collect::<Vec<_>>(),
        [
            ("A.csproj", vec![scip_sym("Left#Run().")]),
            ("B.csproj", vec![scip_sym("Right#Run().")]),
        ]
    );
}

/// `Shared.cs` compiled by two projects that agree on its definition and put
/// `a_target` / `b_target` on one reference at line 6, merged in both project
/// orders. `a` is a .NET Framework build and `b` a net10 one of the same
/// sources, the pair a source folder of the reference C# repository holds as `X.csproj` and
/// `X.NetCore.csproj`. Named here so the Framework project sorts FIRST: a
/// merge that kept the first project's symbol would then keep `a_target`.
fn merge_one_shared_reference(a_target: &str, b_target: &str) -> (Merged, Merged) {
    let shared = |target: &str| {
        scip_document(
            "Shared.cs",
            vec![
                scip_occurrence(&scip_sym("Shared#"), 1, true),
                reference(target, 6, 12),
            ],
        )
    };
    let a = project("X/Fx.csproj", "X", index_of(vec![shared(a_target)]));
    let b = project("X/Net.csproj", "X", index_of(vec![shared(b_target)]));
    let files = tracked(&["X/Shared.cs"]);
    let forward = merge(vec![a.clone(), b.clone()], &files, ROOT_URI);
    let backward = merge(vec![b, a], &files, ROOT_URI);
    assert_eq!(
        forward.index.write_to_bytes().unwrap(),
        backward.index.write_to_bytes().unwrap(),
        "the merged bytes do not depend on project order"
    );
    assert_eq!(forward.conflicts, backward.conflicts);
    (forward, backward)
}

/// The occurrences the merged `Shared.cs` holds at the shared reference.
fn at_reference(merged: &Merged) -> Vec<&Occurrence> {
    merged.index.documents[0]
        .occurrences
        .iter()
        .filter(|o| o.range[0] == 6)
        .collect()
}

#[test]
fn an_external_target_differing_only_in_package_is_one_occurrence_not_a_conflict() {
    // The .NET Framework build binds the call to mscorlib, the net10 build to
    // System.Runtime: the same external method, so an agreement. The kept
    // symbol is the lexicographically smallest — `System.Runtime` sorts before
    // `mscorlib` — not the first project's (`Fx.csproj`, mscorlib).
    let framework = "scip-dotnet nuget mscorlib 4.0.0.0 System/String#Trim().";
    let core = "scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim().";
    let (merged, _) = merge_one_shared_reference(framework, core);

    assert!(merged.conflicts.is_empty());
    let kept = at_reference(&merged);
    assert_eq!(
        kept.iter().map(|o| o.symbol.as_str()).collect::<Vec<_>>(),
        [core],
        "one merged occurrence"
    );
    assert!(kept[0].diagnostics.is_empty());
}

#[test]
fn a_namespace_differing_only_in_package_is_not_a_conflict_even_against_the_repository() {
    let (merged, _) = merge_one_shared_reference(
        "scip-dotnet nuget mscorlib 4.0.0.0 Resources/",
        &scip_sym("Resources/"),
    );

    assert!(merged.conflicts.is_empty());
    let kept = at_reference(&merged);
    assert_eq!(
        kept.iter().map(|o| o.symbol.as_str()).collect::<Vec<_>>(),
        [scip_sym("Resources/")]
    );
    assert!(kept[0].diagnostics.is_empty());
}

#[test]
fn a_repository_target_against_an_external_one_of_the_same_descriptor_stays_a_conflict() {
    // A polyfill: the Framework build compiles the repository's own type where
    // the net10 build finds the framework's. Different targets, really.
    let external = "scip-dotnet nuget System.Runtime 10.0.0.0 Threading/Lock#Enter().";
    let (merged, _) = merge_one_shared_reference(&scip_sym("Threading/Lock#Enter()."), external);

    assert_eq!(merged.conflicts.len(), 1);
    let kept = at_reference(&merged);
    assert_eq!(
        kept.iter().map(|o| o.symbol.as_str()).collect::<Vec<_>>(),
        [scip_sym("Threading/Lock#Enter()."), external.to_string()],
        "both targets survive, verbatim"
    );
    assert!(kept
        .iter()
        .all(|o| o.diagnostics.len() == 1 && o.diagnostics[0].code == CONFLICT_CODE));
}

#[test]
fn external_targets_with_different_descriptors_stay_a_conflict() {
    // The overload ordinal differs: the two frameworks have different overload
    // sets, and the descriptor alone cannot say these are one method.
    let framework = "scip-dotnet nuget mscorlib 4.0.0.0 System/String#Trim(+1).";
    let core = "scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim().";
    let (merged, _) = merge_one_shared_reference(framework, core);

    assert_eq!(merged.conflicts.len(), 1);
    assert_eq!(
        merged.conflicts[0]
            .targets
            .iter()
            .map(|t| (t.project.as_str(), t.symbols.clone()))
            .collect::<Vec<_>>(),
        [
            ("X/Fx.csproj", vec![framework.to_string()]),
            ("X/Net.csproj", vec![core.to_string()]),
        ]
    );
    assert_eq!(at_reference(&merged).len(), 2);
}

/// Each compilation numbers a document's locals on its own, so two projects
/// over one file give the same variable different numbers (16 977 ranges of
/// the reference C# repository). A local is never paired with a usage site, and a range whose
/// targets differ only in that number is one occurrence under the smallest.
#[test]
fn locals_differing_only_in_their_numbers_are_one_occurrence_not_a_conflict() {
    let (merged, _) = merge_one_shared_reference("local 35", "local 32");

    assert!(merged.conflicts.is_empty(), "{:?}", merged.conflicts);
    let kept = at_reference(&merged);
    assert_eq!(
        kept.iter().map(|o| o.symbol.as_str()).collect::<Vec<_>>(),
        ["local 32"]
    );
    assert!(kept[0].diagnostics.is_empty());
}

#[test]
fn a_local_against_a_symbol_stays_a_conflict() {
    let (merged, _) = merge_one_shared_reference("local 4", &scip_sym("Shared#Total()."));
    assert_eq!(merged.conflicts.len(), 1);
    assert_eq!(at_reference(&merged).len(), 2);
}

#[test]
fn paths_are_recomputed_from_each_project_root_and_untracked_ones_are_left_out() {
    let a = project(
        "src/App/App.csproj",
        "src/App",
        index_of(vec![
            scip_document("Sub\\Main.cs", vec![]),
            scip_document("..\\..\\..\\Outside.cs", vec![]),
        ]),
    );
    let b = project(
        "Lib.csproj",
        "",
        index_of(vec![scip_document("./Lib/Helper.cs", vec![])]),
    );
    let files = tracked(&["src/App/Sub/Main.cs", "Lib/Helper.cs"]);

    let merged = merge(vec![a, b], &files, ROOT_URI);

    assert_eq!(
        paths(&merged.index),
        ["Lib/Helper.cs", "src/App/Sub/Main.cs"]
    );
    assert_eq!(
        merged.untracked.get("src/App/App.csproj").unwrap(),
        &vec!["../../../Outside.cs".to_string()]
    );
}
