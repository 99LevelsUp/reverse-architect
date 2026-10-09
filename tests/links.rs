//! U9 integration tests: links and backlinks across a whole rendered corpus.
//!
//! The unit tests in `card::links` pin the path arithmetic. These pin the thing
//! that arithmetic exists for: that every link a real corpus contains resolves to
//! a document that corpus actually wrote, and that everything without a card
//! stays readable text instead of becoming a link into nothing.

mod common;

use common::init_repo_with_files;
use reverse_architect::card::detail::render_detail_files;
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::map::render_map_files;
use reverse_architect::card::render::Lang;
use reverse_architect::card::{build_cards, render_card_files, CardKind};
use reverse_architect::cli::LinkStyle;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{merge, Graph};
use reverse_architect::render::RenderedFile;
use std::collections::BTreeSet;
use std::path::Path;

/// Two types in one namespace plus a type-less file and a nested module, so the
/// corpus has links that climb, links that descend and links that stay put.
fn fixture(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            (
                "src/Invoice.cs",
                "namespace Orbit.Billing\n\
                 {\n\
                 \x20   public class Invoice\n\
                 \x20   {\n\
                 \x20       public Money Total(Money amount)\n\
                 \x20       {\n\
                 \x20           return amount;\n\
                 \x20       }\n\
                 \x20   }\n\
                 }\n",
            ),
            (
                "src/money/Money.cs",
                "namespace Orbit.Billing\n{\n    public class Money\n    {\n    }\n}\n",
            ),
            ("appsettings.json", "{ \"Level\": \"Info\" }"),
        ],
    );
}

/// Every card, every detail and every map of the fixture: since plan B, U9 a
/// type's card links its detail, and its relation lists — the links into
/// other cards they carry — are in the detail, so a link check over the cards
/// alone would both fail on the one and miss the other. The same holds for a
/// module card and its map since U10, whose rows link every type's card.
fn corpus(dir: &Path, style: LinkStyle) -> Vec<RenderedFile> {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    let cards = build_cards(&graph);
    let index = CardIndex::build(&cards);
    let mut files = render_card_files(&cards, &index, Lang::En, style);
    files.extend(render_detail_files(&cards, &index, Lang::En, style));
    files.extend(render_map_files(&cards, &index, Lang::En, style));
    files
}

/// Every markdown link target in `contents`, as written.
fn portable_targets(contents: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = contents;
    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        match after.find(')') {
            Some(close) => {
                out.push(after[..close].to_string());
                rest = &after[close..];
            }
            None => break,
        }
    }
    out
}

/// Every wikilink target in `contents`, as written (without the label).
fn wikilink_targets(contents: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = contents;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        match after.find("]]") {
            Some(close) => {
                let inner = &after[..close];
                out.push(inner.split('|').next().unwrap_or(inner).to_string());
                rest = &after[close..];
            }
            None => break,
        }
    }
    out
}

/// Resolve a relative link against the directory of the document it appears in,
/// collapsing `..` — the same way a Markdown reader would.
fn resolve(from_doc: &str, target: &str) -> String {
    let mut parts: Vec<&str> = from_doc.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[test]
fn no_portable_link_in_the_corpus_points_at_a_document_that_was_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let files = corpus(tmp.path(), LinkStyle::Portable);
    let written: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();

    let mut checked = 0usize;
    for file in &files {
        for target in portable_targets(&file.contents) {
            let resolved = resolve(&file.path, &target);
            assert!(
                written.contains(resolved.as_str()),
                "{} links to {target} → {resolved}, which no card wrote",
                file.path
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the fixture must produce some links to check");
}

#[test]
fn no_obsidian_link_in_the_corpus_points_at_a_document_that_was_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let files = corpus(tmp.path(), LinkStyle::Obsidian);
    // Wikilinks are corpus-root-relative and carry no extension.
    let written: BTreeSet<String> = files
        .iter()
        .map(|f| f.path.trim_end_matches(".md").to_string())
        .collect();

    let mut checked = 0usize;
    for file in &files {
        for target in wikilink_targets(&file.contents) {
            assert!(
                written.contains(&target),
                "{} links to [[{target}]], which no card wrote",
                file.path
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the fixture must produce some wikilinks");
}

#[test]
fn a_type_mentioned_by_another_type_is_a_link_in_both_styles() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());

    let portable = corpus(tmp.path(), LinkStyle::Portable);
    let invoice = portable
        .iter()
        .find(|f| f.path.ends_with("/Invoice.md"))
        .expect("an Invoice card");
    assert!(
        invoice.contents.contains("[Money]("),
        "Invoice references Money and must link to its card: {}",
        invoice.contents
    );

    let obsidian = corpus(tmp.path(), LinkStyle::Obsidian);
    let invoice = obsidian
        .iter()
        .find(|f| f.path.ends_with("/Invoice.md"))
        .expect("an Invoice card");
    assert!(
        invoice.contents.contains("|Money]]"),
        "the same mention in the other style: {}",
        invoice.contents
    );
}

/// Schema v4 gives every file a card (the old R8 rule — a type-declaring file
/// has none — is gone), so the declaring file is now a link. What this test
/// still pins is the half that mattered: the link lands on the card that was
/// actually written for that file, and the anchor keeps the repo path.
#[test]
fn the_file_that_declares_a_type_is_linked_to_its_own_written_file_card() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let files = corpus(tmp.path(), LinkStyle::Portable);
    let written: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();

    // The declaring file is a relation, so since plan B, U9 it is a row of
    // the type's detail, not of its short card.
    let invoice = files
        .iter()
        .find(|f| f.path.starts_with("cards/detail/") && f.path.ends_with("/Invoice.md"))
        .expect("an Invoice detail");

    // The source anchor still names the repo path…
    assert!(
        invoice.contents.contains("`src/Invoice.cs"),
        "{}",
        invoice.contents
    );
    // …and the file mention links to the file's own card, which exists.
    let file_links: Vec<String> = portable_targets(&invoice.contents)
        .into_iter()
        .filter(|t| t.contains("Invoice.cs"))
        .map(|t| resolve(&invoice.path, &t))
        .collect();
    assert!(
        file_links
            .iter()
            .any(|t| t == "cards/file/src/Invoice.cs.md"),
        "the declaring file must link to its card: {file_links:?}\n{}",
        invoice.contents
    );
    for target in &file_links {
        assert!(
            written.contains(target.as_str()),
            "a file link must land on a written card: {target}"
        );
    }
}

#[test]
fn a_called_types_card_names_its_caller_in_the_backlinks() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let cards = {
        let repo = Repo::open(tmp.path()).unwrap();
        let mut graph = Graph::new();
        run_all(&repo, &mut graph).unwrap();
        merge(&mut graph, &trust_ranks());
        build_cards(&graph)
    };

    let money = cards
        .iter()
        .find(|c| c.kind == CardKind::Type && c.title == "Money")
        .expect("a Money card");
    let referrers: Vec<&str> = money.incoming.iter().map(|r| r.identity.as_str()).collect();
    assert!(
        referrers.iter().any(|id| id.contains("Invoice")),
        "Money is referenced from Invoice: {referrers:?}"
    );

    // Since plan B, U9 a type's card names its callers in its role sentences
    // — the `Referenced by` list is its detail's, and `Money` has none: one
    // caller is all the role needs to name.
    assert_eq!(money.detail, None);
    let files = corpus(tmp.path(), LinkStyle::Portable);
    let rendered = files
        .iter()
        .find(|f| f.path == money.path)
        .expect("the Money card was written");
    assert!(
        rendered
            .contents
            .contains("Used by 1 in 1 module, chiefly ")
            && rendered.contents.contains("[Invoice]("),
        "{}",
        rendered.contents
    );
}

#[test]
fn an_external_target_without_a_card_stays_text() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/Repo.cs",
                "namespace Orbit.Data\n\
                 {\n\
                 \x20   public class Repo\n\
                 \x20   {\n\
                 \x20       public void Load()\n\
                 \x20       {\n\
                 \x20           var sql = \"select id from dbo.Orders\";\n\
                 \x20       }\n\
                 \x20   }\n\
                 }\n",
            ),
            (
                "Cargo.toml",
                "[package]\nname = \"f\"\nversion = \"0.1.0\"\n",
            ),
        ],
    );

    let files = corpus(tmp.path(), LinkStyle::Portable);
    // A type's externals are rows of its detail's `Relations` since plan B,
    // U9.
    let repo_card = files
        .iter()
        .find(|f| f.path.starts_with("cards/detail/") && f.path.ends_with("/Repo.md"))
        .expect("a Repo detail");

    if repo_card.contents.contains("dbo.Orders") {
        // The database object has no card, so it is named in code style; a link
        // would resolve to nothing.
        assert!(
            repo_card.contents.contains("`dbo.Orders`"),
            "{}",
            repo_card.contents
        );
        for target in portable_targets(&repo_card.contents) {
            assert!(
                !target.contains("dbo.Orders"),
                "an external must never be linked: {target}"
            );
        }
    }
}

#[test]
fn backlinks_are_stably_ordered_across_runs() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());

    let first = corpus(tmp.path(), LinkStyle::Portable);
    let second = corpus(tmp.path(), LinkStyle::Portable);
    assert_eq!(first, second, "two renders must agree byte for byte");
}

/// Every `.md` file a run wrote under `dir`, as `(corpus-relative path,
/// contents)`.
fn written_markdown(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Plan B, U11: what reaches disk links only to what reaches disk. The
/// in-memory checks above prove the arithmetic; this one proves the write —
/// a card's `Detail` line and a module card's `Map` line point at files the
/// run actually wrote, and so does every other link in the written corpus.
#[test]
fn no_link_in_the_written_corpus_points_at_a_file_that_was_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    fixture(&repo);
    let out = tmp.path().join("corpus");
    assert_cmd::Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&repo)
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let files = written_markdown(&out);
    let mut into_detail = 0;
    let mut into_map = 0;
    for (path, contents) in &files {
        for target in portable_targets(contents) {
            if target.contains("://") || target.starts_with('#') {
                continue;
            }
            let target = target.split('#').next().unwrap_or(&target);
            let resolved = resolve(path, target);
            assert!(
                out.join(&resolved).is_file(),
                "{path} links {target}, which the run did not write ({resolved})"
            );
            into_detail += usize::from(resolved.starts_with("cards/detail/"));
            into_map += usize::from(resolved.starts_with("cards/map/"));
        }
    }
    assert!(
        into_detail > 0 && into_map > 0,
        "the fixture must link a written detail and a written map, or this checks neither"
    );
}
