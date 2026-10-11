//! Plan B, U14: the regression fixture of acceptance case A2 — from the text of a
//! message to the member and the line, over the corpus a run WRITES.
//!
//! The case: after a backup was restored, support had the name of a method and
//! of a table out of an error report, and walked directories and grepped the
//! sources for tens of minutes to find which action wrote that table and why.
//! The success criterion of plan B is that one search over the corpus lands on
//! the method and its line, and that the module map leads to `LoadAction`
//! without walking directories.
//!
//! **Why through the binary.** `tests/card.rs` holds each navigator to its
//! rendering (AE1, AE6, AE7, AE9) over cards it builds in memory. What no test
//! there can see is the search itself: which files a run actually writes,
//! under which paths, and whether the links between the three levels resolve
//! on disk. The writer lives in the binary, so the binary runs here and every
//! assertion reads files back from the output directory.
//!
//! **The fixture** is shaped like the real one, with placeholder content: an
//! action contract, `LoadAction` whose `SetScheduledTasksInvalid` runs its SQL
//! from a C# raw literal (`"""…"""`, the shape the real code has — a statement
//! that updates the table and reads it in its own `FROM`), a method that calls
//! a procedure and reads a table, the enum the actions switch on, a message a
//! user would read out to support, further actions in the same module (one
//! with a doc comment, one without, one holding a secret in a comment and a
//! connection string), a static helper class, and a C file with SQL in a
//! literal. It does not use the six-grammar fixture.

mod common;

use assert_cmd::Command;
use common::{init_repo_with_files, output_files};
use std::path::{Path, PathBuf};

/// The action contract every restore action implements.
const ILOAD_ACTION_CS: &str = r#"namespace Orbit.Actions.Backup
{
    /// <summary>Contract of an action run while a backup is restored.</summary>
    public interface ILoadAction
    {
        void Execute(BackupLoadType loadType);
    }
}
"#;

const BACKUP_LOAD_TYPE_CS: &str = r#"namespace Orbit.Actions.Backup
{
    public enum BackupLoadType
    {
        None = 0,
        Full = 10,
        Differential = 20,
    }
}
"#;

/// `LoadAction` the way acceptance case A2 met it. The `UPDATE` sits in a raw
/// literal across several lines and names the table twice: as the target and
/// in its own `FROM`.
const LOAD_ACTION_CS: &str = r#"using System;

namespace Orbit.Actions.Backup
{
    /// <summary>Restores a backup into the current database.</summary>
    public class LoadAction : ILoadAction
    {
        public void Execute(BackupLoadType loadType)
        {
            switch (loadType)
            {
                case BackupLoadType.Full:
                    SetScheduledTasksInvalid();
                    break;
                case BackupLoadType.Differential:
                    RefreshTasks();
                    break;
            }
        }

        public void SetScheduledTasksInvalid()
        {
            Database.Execute("""
                update dba.ops_schedule_tasks
                   set invalid = 1
                  from dba.ops_schedule_tasks
                 where server_name <> @@servername
                """);
        }

        public void RefreshTasks()
        {
            Database.Execute("exec dba.ops_refresh_tasks");
            var last = Database.Query("select last_run from dba.ops_backup_history");
        }

        public void Fail()
        {
            throw new InvalidOperationException("Po obnově zálohy nelze zneplatnit naplánované úlohy");
        }
    }
}
"#;

/// A second action, with no doc comment: its purpose can only come from the
/// structure (AE4).
const CHECK_ACTION_CS: &str = r#"namespace Orbit.Actions.Backup
{
    public class CheckAction : ILoadAction
    {
        public void Execute(BackupLoadType loadType)
        {
            var rows = Database.Query("select file_name from dba.ops_backup_files");
        }
    }
}
"#;

/// The secret the fixture plants in a comment, and the one in a connection
/// string. Neither may reach any file of the corpus.
const COMMENT_SECRET: &str = "fixture-comment-pw-case-a2";
const CONNECTION_SECRET: &str = "fixture-conn-pw-case-a2";

fn cleanup_action_cs() -> String {
    format!(
        r#"namespace Orbit.Actions.Backup
{{
    /// <summary>Removes the temporary files a restore leaves behind.</summary>
    public class CleanupAction : ILoadAction
    {{
        public void Execute(BackupLoadType loadType)
        {{
            // password: {COMMENT_SECRET}
            var target = "Server=sql01;Database=restore;User Id=sa;Password={CONNECTION_SECRET}";
            Database.Execute("delete from dba.ops_backup_files");
        }}
    }}
}}
"#
    )
}

/// AE3: a static class is announced as one.
const BACKUP_PATHS_CS: &str = r#"namespace Orbit.Actions.Backup
{
    public static class BackupPaths
    {
        public static string Folder(string root) { return root; }
    }
}
"#;

/// A C file has no plugin: the query in its literal belongs to the file.
const RESTORE_LOG_C: &str = r#"#include "restore.h"

void mark_restored(void) {
    const char* q = "update dba.ops_restore_log set done = 1";
    exec_sql(q);
}
"#;

const LOAD_ACTION: &str = "Actions/Backup/LoadAction.cs";
const RESTORE_LOG: &str = "native/restore_log.c";

/// The byte budget of a short type card and of a map row (plan B, Success
/// Criteria: 1.2 kB and 200 bytes). Spelled out rather than imported, so the
/// test breaks when the budget a constant states moves, not with it. The card
/// budget holds for a run without the second layer, as every run here is; a
/// C# type's card with it has none (plan 2a, Key Decisions).
const SHORT_CARD_BYTES: usize = 1200;
const MAP_ROW_BYTES: usize = 200;

fn fixture(dir: &Path) {
    let cleanup = cleanup_action_cs();
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("Actions/Backup/ILoadAction.cs", ILOAD_ACTION_CS),
            ("Actions/Backup/BackupLoadType.cs", BACKUP_LOAD_TYPE_CS),
            (LOAD_ACTION, LOAD_ACTION_CS),
            ("Actions/Backup/CheckAction.cs", CHECK_ACTION_CS),
            ("Actions/Backup/CleanupAction.cs", &cleanup),
            ("Actions/Backup/BackupPaths.cs", BACKUP_PATHS_CS),
            (RESTORE_LOG, RESTORE_LOG_C),
        ],
    );
}

/// One generation run of the binary over `repo` into `out`.
fn run(repo: &Path, out: &Path) {
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(repo)
        .arg("--out")
        .arg(out)
        .assert()
        .success();
}

/// The fixture, generated once.
struct Corpus {
    _repo: tempfile::TempDir,
    out: PathBuf,
}

impl Corpus {
    fn new() -> Corpus {
        let repo = tempfile::tempdir().unwrap();
        fixture(repo.path());
        let out = repo.path().join("corpus");
        run(repo.path(), &out);
        Corpus { _repo: repo, out }
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.out.join(path))
            .unwrap_or_else(|e| panic!("{path} is not in the written corpus: {e}"))
    }

    /// The written Markdown files whose path matches `pred`, by path.
    fn markdown(&self, pred: impl Fn(&str) -> bool) -> Vec<(String, String)> {
        output_files(&self.out)
            .into_iter()
            .filter(|(path, _)| path.ends_with(".md") && pred(path))
            .map(|(path, bytes)| (path, String::from_utf8(bytes).unwrap()))
            .collect()
    }

    /// One search over every written Markdown file: each file with a line
    /// holding `needle`, with those lines' numbers (0-based).
    fn search(&self, needle: &str) -> Vec<(String, Vec<usize>)> {
        self.markdown(|_| true)
            .into_iter()
            .filter_map(|(path, text)| {
                let hits: Vec<usize> = text
                    .lines()
                    .enumerate()
                    .filter(|(_, l)| l.contains(needle))
                    .map(|(n, _)| n)
                    .collect();
                (!hits.is_empty()).then_some((path, hits))
            })
            .collect()
    }

    /// The written file a relative link in `from` leads to — asserted to
    /// exist, which is what "without walking directories" rests on.
    fn follow(&self, from: &str, link: &str) -> String {
        let mut parts: Vec<&str> = from.split('/').collect();
        parts.pop();
        for segment in link.split('/') {
            match segment {
                ".." => {
                    parts.pop();
                }
                "." => {}
                s => parts.push(s),
            }
        }
        let target = parts.join("/");
        assert!(
            self.out.join(&target).is_file(),
            "the link `{link}` in {from} leads to {target}, which was not written"
        );
        target
    }
}

/// The 1-based line of the first source line holding `needle`.
fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` is not in the fixture"))
        + 1
}

/// The body of one `##` section of a written page.
fn section<'a>(page: &'a str, name: &str) -> &'a str {
    let heading = format!("\n## {name}\n");
    let start = page
        .find(&heading)
        .unwrap_or_else(|| panic!("no {name} section:\n{page}"))
        + heading.len();
    let end = page[start..]
        .find("\n## ")
        .map_or(page.len(), |n| start + n);
    &page[start..end]
}

/// The target of the first Markdown link in `line` whose text is `text`.
fn link_target<'a>(line: &'a str, text: &str) -> &'a str {
    let open = format!("[{text}](");
    let start = line
        .find(&open)
        .unwrap_or_else(|| panic!("no link `{text}` in: {line}"))
        + open.len();
    let end = line[start..].find(')').expect("a closed link") + start;
    &line[start..end]
}

/// The path of the one written detail of the type `name`.
fn type_detail(corpus: &Corpus, name: &str) -> (String, String) {
    let mut found = corpus
        .markdown(|p| p.starts_with("cards/detail/type/") && p.ends_with(&format!("/{name}.md")));
    assert_eq!(
        found.len(),
        1,
        "{name} must have exactly one detail: {found:?}"
    );
    found.remove(0)
}

/// Success criterion and AE1 (R9, R11): ONE search for the method's name, and
/// one for the table's, over the written files lands on `LoadAction`'s detail;
/// the hit there names the method, and the line under it says it writes
/// `dba.ops_schedule_tasks` at the line of the statement in the source.
#[test]
fn one_search_for_the_method_or_the_table_lands_on_the_method_and_its_line() {
    let corpus = Corpus::new();
    let (detail_path, detail) = type_detail(&corpus, "LoadAction");
    let lines: Vec<&str> = detail.lines().collect();
    // Both places the statement names the table — the target and its own
    // FROM — are writes (KTD2), each with its line.
    let write = format!(
        "  - writes `dba.ops_schedule_tasks` — `{LOAD_ACTION}:{}, {}`",
        line_of(LOAD_ACTION_CS, "update dba.ops_schedule_tasks"),
        line_of(LOAD_ACTION_CS, "from dba.ops_schedule_tasks")
    );

    let by_method = corpus.search("SetScheduledTasksInvalid");
    let hits = &by_method
        .iter()
        .find(|(path, _)| path == &detail_path)
        .unwrap_or_else(|| panic!("the search misses {detail_path}: {by_method:?}"))
        .1;
    assert!(
        hits.iter()
            .any(|&n| lines[n] == "- `SetScheduledTasksInvalid()`"
                && lines.get(n + 1) == Some(&write.as_str())),
        "the method's hit must be followed by its write and line:\n{detail}"
    );

    let by_table = corpus.search("ops_schedule_tasks");
    let hits = &by_table
        .iter()
        .find(|(path, _)| path == &detail_path)
        .unwrap_or_else(|| panic!("the search misses {detail_path}: {by_table:?}"))
        .1;
    assert!(
        hits.iter().any(|&n| lines[n] == write
            && lines[..n]
                .iter()
                .rev()
                .find(|l| l.starts_with("- "))
                .is_some_and(|m| *m == "- `SetScheduledTasksInvalid()`")),
        "the table's hit must sit under the method that writes it:\n{detail}"
    );

    // KTD2: the table in the statement's own FROM is the target of the
    // change, so it is a write and nothing else — no read of it is claimed.
    let database = section(&detail, "Database");
    let method = database
        .split("\n- ")
        .find(|block| {
            block
                .trim_start_matches("- ")
                .starts_with("`SetScheduledTasksInvalid()`")
        })
        .unwrap_or_else(|| panic!("no block for the method:\n{database}"));
    let accesses: Vec<&str> = method.lines().filter(|l| l.starts_with("  - ")).collect();
    assert_eq!(accesses, [write.as_str()], "{database}");
}

/// AE9 (R11): the method that calls a procedure and reads a table lists the
/// call and the read with their lines, and no write.
#[test]
fn the_method_calling_a_procedure_lists_the_call_and_the_read_and_no_write() {
    let corpus = Corpus::new();
    let (_, detail) = type_detail(&corpus, "LoadAction");
    let database = section(&detail, "Database");
    let expected = format!(
        "- `RefreshTasks()`\n  - reads `dba.ops_backup_history` — `{LOAD_ACTION}:{}`\n  \
         - calls `dba.ops_refresh_tasks` — `{LOAD_ACTION}:{}`\n",
        line_of(LOAD_ACTION_CS, "select last_run"),
        line_of(LOAD_ACTION_CS, "exec dba.ops_refresh_tasks")
    );
    assert!(database.contains(&expected), "{database}");
}

/// R9: the message a user reads out to support leads to the member that
/// throws it and its line.
#[test]
fn the_message_a_user_reads_out_leads_to_the_member_and_the_line() {
    let corpus = Corpus::new();
    let message = "Po obnově zálohy nelze zneplatnit naplánované úlohy";
    let (detail_path, detail) = type_detail(&corpus, "LoadAction");
    assert!(
        corpus
            .search(message)
            .iter()
            .any(|(p, _)| p == &detail_path),
        "a search for the message must land on {detail_path}"
    );
    let anchor = format!(
        "- `{message}` → `Fail()` · `{LOAD_ACTION}:{}`",
        line_of(LOAD_ACTION_CS, message)
    );
    assert!(
        section(&detail, "Anchors").lines().any(|l| l == anchor),
        "{detail}"
    );
}

/// AE7 (R1) and the second half of the success criterion: the module map of
/// the backup actions has `LoadAction`'s row with its purpose and the table
/// it writes, and the links lead map → card → detail on disk.
#[test]
fn the_module_map_leads_to_load_action_its_card_and_its_detail() {
    let corpus = Corpus::new();
    let map_path = "cards/map/Actions/Backup.md";
    let map = corpus.read(map_path);
    let row = map
        .lines()
        .find(|l| l.starts_with("- ") && l.contains("[LoadAction]("))
        .unwrap_or_else(|| panic!("no LoadAction row:\n{map}"));
    assert!(
        row.contains("Restores a backup into the current database."),
        "the row carries the purpose: {row}"
    );
    assert!(
        row.contains("writes `dba.ops_schedule_tasks`"),
        "and the table it writes: {row}"
    );
    // Every action of the module is on the map, not only the one sought.
    for name in [
        "ILoadAction",
        "BackupLoadType",
        "CheckAction",
        "CleanupAction",
        "BackupPaths",
    ] {
        assert!(map.contains(&format!("[{name}](")), "{name}:\n{map}");
    }

    let card_path = corpus.follow(map_path, link_target(row, "LoadAction"));
    assert!(card_path.starts_with("cards/type/"), "{card_path}");
    let card = corpus.read(&card_path);
    let detail_line = card
        .lines()
        .find(|l| l.starts_with("- Detail: "))
        .unwrap_or_else(|| panic!("the card names no detail:\n{card}"));
    let open = detail_line.find("](").expect("a link") + 2;
    let link = &detail_line[open..detail_line.rfind(')').expect("a closed link")];
    let detail_path = corpus.follow(&card_path, link);
    assert_eq!(detail_path, type_detail(&corpus, "LoadAction").0);
}

/// AE6 (R12): the enum's detail lists where `LoadAction` decides by its
/// values, with the lines.
#[test]
fn the_enums_detail_lists_the_decisions_in_load_action_with_lines() {
    let corpus = Corpus::new();
    let (_, detail) = type_detail(&corpus, "BackupLoadType");
    let decisions = section(&detail, "Enum decisions");
    for (value, needle) in [
        ("Differential", "case BackupLoadType.Differential"),
        ("Full", "case BackupLoadType.Full"),
    ] {
        let at = format!("`{LOAD_ACTION}:{}`", line_of(LOAD_ACTION_CS, needle));
        assert!(
            decisions.lines().any(|l| l
                .starts_with(&format!("- `{value}` — case label in [LoadAction.Execute("))
                && l.ends_with(&format!(" — {at}"))),
            "{value} at {at}:\n{decisions}"
        );
    }
}

/// KTD7, R13: the C file's detail leads from the SQL in its literal to the
/// line, and says the statement writes the table.
#[test]
fn the_c_files_detail_leads_from_its_sql_to_the_line_and_the_write() {
    let corpus = Corpus::new();
    let detail = corpus.read(&format!("cards/detail/file/{RESTORE_LOG}.md"));
    let line = line_of(RESTORE_LOG_C, "update dba.ops_restore_log");
    assert!(
        section(&detail, "Database").contains(&format!(
            "  - writes `dba.ops_restore_log` — `{RESTORE_LOG}:{line}`\n"
        )),
        "{detail}"
    );
    assert!(
        section(&detail, "Anchors").contains(&format!(
            "- `update dba.ops_restore_log set done = 1` · `{RESTORE_LOG}:{line}`\n"
        )),
        "{detail}"
    );
}

/// AE3 (R4), AE4 (R5, R6) and the size budgets (R17): the static helper says
/// it is static; the action with a doc comment states a purpose from it, the
/// one without states only what the structure shows; every short type card
/// and every map row stays within its budget.
#[test]
fn headers_purposes_and_sizes_hold_on_the_written_cards() {
    let corpus = Corpus::new();
    let card = |name: &str| {
        let mut found = corpus
            .markdown(|p| p.starts_with("cards/type/") && p.ends_with(&format!("/{name}.md")));
        assert_eq!(found.len(), 1, "{name}'s card: {found:?}");
        found.remove(0).1
    };

    let paths = card("BackupPaths");
    assert!(paths.contains("**public static class**"), "{paths}");

    let documented = card("LoadAction");
    assert!(
        section(&documented, "Purpose")
            .lines()
            .any(|l| l.contains("Restores a backup") && l.ends_with("(from the comment)")),
        "{documented}"
    );
    let bare = card("CheckAction");
    let purpose: Vec<&str> = section(&bare, "Purpose")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(!purpose.is_empty(), "{bare}");
    assert!(
        purpose.iter().all(|l| l.ends_with("(from the structure)")),
        "{bare}"
    );

    // The budget is the median (Success Criteria), the statistic the run
    // reports in coverage.json — so both are read: the files on disk, and the
    // number an operator measuring a large repository looks at.
    let cards = corpus.markdown(|p| p.starts_with("cards/type/"));
    assert!(cards.len() >= 6, "every type has a card: {}", cards.len());
    let mut sizes: Vec<usize> = cards.iter().map(|(_, text)| text.len()).collect();
    sizes.sort_unstable();
    // Nearest rank, as coverage states it.
    let median = sizes[sizes.len().div_ceil(2) - 1];
    assert!(
        median <= SHORT_CARD_BYTES,
        "the median short card is {median} bytes: {sizes:?}"
    );
    let coverage: serde_json::Value = serde_json::from_str(&corpus.read("coverage.json")).unwrap();
    assert_eq!(
        coverage["cards"]["sizes"]["type_card"]["median"], median,
        "{sizes:?}"
    );
    let maps = corpus.markdown(|p| p.starts_with("cards/map/"));
    let mut rows = 0;
    for (path, text) in &maps {
        for row in text.lines().filter(|l| l.starts_with("- ")) {
            // The line break counts: a row is a line of the file.
            let bytes = row.len() + "\n".len();
            assert!(bytes <= MAP_ROW_BYTES, "{path}: {bytes} bytes: {row}");
            rows += 1;
        }
    }
    assert!(rows >= 6, "every type has a map row: {rows}");
}

/// R10: neither the secret in the comment nor the one in the connection
/// string reaches any file the run writes — its cache included — while the
/// place of the withheld comment is still named.
#[test]
fn no_file_of_the_corpus_holds_a_secret_of_the_fixture() {
    let corpus = Corpus::new();
    let mut every_file = Vec::new();
    collect(&corpus.out, &mut every_file);
    assert!(
        every_file.iter().any(|(p, _)| p.contains(".ra-cache")),
        "the sweep must cover the cache too: {:?}",
        every_file.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    for (path, bytes) in &every_file {
        for secret in [COMMENT_SECRET, CONNECTION_SECRET] {
            assert!(
                !contains(bytes, secret.as_bytes()),
                "`{secret}` reached {path}"
            );
        }
    }
    let (_, detail) = type_detail(&corpus, "CleanupAction");
    assert!(
        section(&detail, "Anchors")
            .lines()
            .any(|l| l.starts_with("- withheld: password → `Execute(")),
        "{detail}"
    );
}

/// R15: two runs over the same commit write the same bytes, and a rerun over
/// a warm cache rewrites none of them differently.
#[test]
fn two_runs_write_the_same_bytes() {
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    let first = repo.path().join("first");
    let second = repo.path().join("second");
    run(repo.path(), &first);
    run(repo.path(), &second);
    let written = output_files(&first);
    assert!(
        written
            .iter()
            .any(|(p, _)| p.starts_with("cards/detail/") || p.starts_with("cards/map/")),
        "the comparison must cover the details and the maps"
    );
    assert_eq!(written, output_files(&second));
    run(repo.path(), &first);
    assert_eq!(written, output_files(&first), "a warm rerun moved a byte");
}

/// Every file under `dir`, the cache included, as (relative path, bytes).
fn collect(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    walk(dir, dir, out);
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
