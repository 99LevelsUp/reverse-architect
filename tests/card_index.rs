//! U10 integration tests: the corpus signpost (`## Modules` / `## Notable
//! types`) appended to `index.md`. Split out of `tests/card.rs` once that
//! file crossed 1000 lines, mirroring `tests/links.rs` sitting alongside
//! `src/card/links.rs`.

mod common;

use common::{find, init_repo_with_files};
use reverse_architect::card::explain::module_label;
use reverse_architect::card::index::render_signpost;
use reverse_architect::card::links::{mention, CardIndex};
use reverse_architect::card::{build_cards, Card, CardKind, Relation};
use reverse_architect::cli::LinkStyle;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{merge, EdgeKind, Graph, Resolution};

fn cards_over(dir: &std::path::Path) -> Vec<Card> {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    build_cards(&graph)
}

/// A C# fixture, because a C# namespace is declared in the source rather than
/// derived from the file path — the only place where "moving a type between
/// files leaves its identity alone" (R14) is even expressible.
const INVOICE_CS: &str = r#"namespace Orbit.Billing
{
    /// <summary>Faktura vydaná odběrateli.</summary>
    public class Invoice
    {
        public int Total(int quantity)
        {
            return quantity;
        }

        internal void Recalculate()
        {
        }
    }
}
"#;

/// A synthetic type card with a controlled backlink count, for testing the
/// "Notable types" ranking without needing dozens of real fixture types.
/// `story.users_certain` stays 0; the ranking test sets it where it matters.
fn synthetic_type_card(identity: &str, title: &str, incoming_count: usize) -> Card {
    Card {
        identity: identity.to_string(),
        path: format!("cards/type/{title}.md"),
        kind: CardKind::Type,
        kind_word: "type".into(),
        title: title.to_string(),
        entity_kind: None,
        visibility: None,
        signature: None,
        header: None,
        source: None,
        doc: None,
        members: Vec::new(),
        outgoing: Vec::new(),
        incoming: (0..incoming_count)
            .map(|i| Relation {
                kind: EdgeKind::Calls,
                kind_word: "calls".into(),
                label: format!("caller{i}"),
                identity: format!("sym:caller{identity}{i}"),
                resolution: Resolution::Heuristic,
                source: None,
                via: None,
                ambiguous_candidates: None,
                rung: None,
                via_identity: None,
                sites: Vec::new(),
            })
            .collect(),
        externals: Vec::new(),
        maybe_also: Vec::new(),
        test: None,
        caveats: Vec::new(),
        story: Default::default(),
        navigators: Default::default(),
        detail: None,
        map: None,
        generated: false,
    }
}

#[test]
fn the_signpost_links_every_module_and_the_link_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/lib.rs", "pub fn f() {}\n"),
            ("src/util/helpers.rs", "pub fn g() {}\n"),
        ],
    );
    let cards = cards_over(tmp.path());
    let index = CardIndex::build(&cards);
    let signpost = render_signpost(&cards, &index, LinkStyle::Portable);

    assert!(signpost.contains("## Modules"), "{signpost}");
    let modules: Vec<&Card> = cards
        .iter()
        .filter(|c| c.kind == CardKind::Module)
        .collect();
    assert!(
        !modules.is_empty(),
        "fixture must produce at least one module card"
    );
    for m in &modules {
        // v4: the Modules table labels the root `(repository root)` instead of
        // a bare `.`; the link target is the same card either way.
        let link = mention(
            "index.md",
            &m.identity,
            module_label(&m.title),
            LinkStyle::Portable,
            &index,
        );
        assert!(
            signpost.contains(&link),
            "missing working link for module {:?} in:\n{signpost}",
            m.title
        );
    }
}

/// v4: the ranking is certain users first (`story.users_certain` — other
/// types and files reaching this one on a relation that is not an ambiguous
/// name match), then the raw backlink count, then identity. The table's
/// `Used by` column shows the first key; `referenced N times` is gone.
#[test]
fn notable_types_are_ranked_by_certain_users_then_backlinks_with_a_deterministic_tiebreak() {
    let mut used = synthetic_type_card("sym:e", "Used", 2);
    used.story.users_certain = 3;
    let cards = vec![
        synthetic_type_card("sym:a", "Rare", 1),
        synthetic_type_card("sym:b", "Popular", 10),
        synthetic_type_card("sym:c", "TiedFirst", 5),
        synthetic_type_card("sym:d", "TiedSecond", 5),
        used,
    ];
    let index = CardIndex::build(&cards);
    let signpost = render_signpost(&cards, &index, LinkStyle::Portable);

    let pos = |needle: &str| {
        signpost
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} not found in:\n{signpost}"))
    };
    // Certain users outrank raw backlinks: 3 users beat 10 uncertain rows...
    assert!(pos("[Used]") < pos("[Popular]"), "{signpost}");
    // ...then descending by backlink count...
    assert!(pos("[Popular]") < pos("[TiedFirst]"));
    assert!(pos("[TiedSecond]") < pos("[Rare]"));
    // ...and a tie (both 5 backlinks, 0 users) breaks on identity,
    // deterministically.
    assert!(
        pos("[TiedFirst]") < pos("[TiedSecond]"),
        "identity \"sym:c\" sorts before \"sym:d\" on a tied count"
    );
    assert!(
        signpost.contains("| [Used](cards/type/Used.md) |  |  | 3 |  |"),
        "the Used by column carries the certain-user count: {signpost}"
    );
}

#[test]
fn the_notable_types_section_caps_at_its_limit_without_padding_a_smaller_set() {
    let many: Vec<Card> = (0..25)
        .map(|i| synthetic_type_card(&format!("sym:t{i}"), &format!("Type{i:02}"), 25 - i))
        .collect();
    let index = CardIndex::build(&many);
    let signpost = render_signpost(&many, &index, LinkStyle::Portable);
    let shown = many
        .iter()
        .filter(|c| signpost.contains(&format!("]({})", c.path)))
        .count();
    assert!(
        shown < many.len(),
        "a 25-type set must be capped, not listed in full: {shown} shown"
    );
    // v4: the cut is confessed, never silent.
    assert!(
        signpost.contains("The 20 most used of 25 types; every module card lists its own."),
        "{signpost}"
    );

    let few = vec![synthetic_type_card("sym:only", "Only", 3)];
    let index = CardIndex::build(&few);
    let signpost = render_signpost(&few, &index, LinkStyle::Portable);
    assert!(signpost.contains("Only"), "{signpost}");
    // v4: the section is a table — count its link rows, not list bullets.
    assert_eq!(
        signpost.matches("\n| [").count(),
        1,
        "a single type must not be padded with fabricated entries: {signpost}"
    );
    assert!(
        !signpost.contains("most used of"),
        "an uncut list must not claim a cut: {signpost}"
    );
}

#[test]
fn a_repo_with_no_cards_gets_an_honest_note_not_a_broken_section() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("appsettings.json", "{}")]);
    let cards = cards_over(tmp.path());
    let index = CardIndex::build(&cards);
    let signpost = render_signpost(&cards, &index, LinkStyle::Portable);

    let type_cards = cards.iter().filter(|c| c.kind == CardKind::Type).count();
    assert_eq!(type_cards, 0, "fixture must produce no type cards");
    let notable_types_section = signpost
        .split("## Notable types")
        .nth(1)
        .unwrap_or_else(|| panic!("no Notable types section: {signpost}"));
    assert!(
        notable_types_section.contains("No type cards exist"),
        "{signpost}"
    );
    assert!(
        !notable_types_section.contains("- [") && !notable_types_section.contains("| ["),
        "no type cards means no fabricated entries in that section: {signpost}"
    );
}

/// A real repository always yields at least the root module card
/// (`build_cards` produces one unconditionally), so the "no modules" branch
/// is unreachable via a repo fixture — only a synthetic, empty card slice
/// exercises it.
#[test]
fn a_signpost_with_no_module_cards_gets_an_honest_note_not_a_broken_section() {
    let cards: Vec<Card> = Vec::new();
    let index = CardIndex::build(&cards);
    let signpost = render_signpost(&cards, &index, LinkStyle::Portable);

    let modules_section = signpost
        .split("## Modules")
        .nth(1)
        .unwrap_or_else(|| panic!("no Modules section: {signpost}"))
        .split("## Notable types")
        .next()
        .unwrap();
    assert!(
        modules_section.contains("No modules were extracted"),
        "{signpost}"
    );
    assert!(
        !modules_section.contains("- [") && !modules_section.contains("| ["),
        "no module cards means no fabricated entries in that section: {signpost}"
    );
}

#[test]
fn two_runs_over_one_repository_render_a_byte_identical_signpost() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("src/util/helpers.rs", "pub fn g() {}\n"),
        ],
    );
    let first = cards_over(tmp.path());
    let second = cards_over(tmp.path());
    let index_a = CardIndex::build(&first);
    let index_b = CardIndex::build(&second);
    assert_eq!(
        render_signpost(&first, &index_a, LinkStyle::Portable),
        render_signpost(&second, &index_b, LinkStyle::Portable),
    );
}

#[test]
fn obsidian_and_portable_link_styles_both_resolve_module_and_type_mentions() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);
    let cards = cards_over(tmp.path());
    let index = CardIndex::build(&cards);

    let type_card = find(&cards, CardKind::Type, "Invoice");
    for style in [LinkStyle::Portable, LinkStyle::Obsidian] {
        let signpost = render_signpost(&cards, &index, style);
        let module_link = cards
            .iter()
            .find(|c| c.kind == CardKind::Module)
            // v4: the root module is labelled `(repository root)` in the table.
            .map(|m| {
                mention(
                    "index.md",
                    &m.identity,
                    module_label(&m.title),
                    style,
                    &index,
                )
            })
            .unwrap();
        let type_link = mention(
            "index.md",
            &type_card.identity,
            &type_card.title,
            style,
            &index,
        );
        assert!(signpost.contains(&module_link), "{style:?}: {signpost}");
        assert!(signpost.contains(&type_link), "{style:?}: {signpost}");
    }
}
