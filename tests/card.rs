//! U8 integration tests: the entity-centric card corpus over git fixtures.
//!
//! Every scenario runs the real extraction pipeline (the same one `generate`
//! runs, merge included) and then builds cards from the resulting graph — so a
//! card is only ever asserted against facts an extractor genuinely emitted.

mod common;

use common::{find, init_repo_with_files};
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::pages::build_pages;
use reverse_architect::card::render::{render_card, Lang};
use reverse_architect::card::{
    build_card_files, build_cards, build_cards_with_insight, Card, CardKind,
};
use reverse_architect::cli::LinkStyle;
use reverse_architect::extract::test_pairing::{HAS_TEST, NO_TEST};
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{merge, Confidence, Edge, EdgeKind, Graph, Rule, Tier};

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

#[test]
fn a_type_card_carries_its_members_anchors_and_source() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);

    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::Type, "Invoice");

    assert_eq!(card.entity_kind.as_deref(), Some("class"));
    // The LINES, not merely their presence. An anchor is what a reader follows
    // to check a statement; one that names the right file at the wrong line is
    // worse than none, and "is_some() and starts_with the file name" accepts it.
    let source = card.source.as_ref().expect("a type card knows its source");
    assert_eq!(
        source.anchor(),
        "src/Invoice.cs:4-14",
        "the class spans its declaration line to its closing brace"
    );

    let anchors: Vec<(&str, String)> = card
        .members
        .iter()
        .map(|m| {
            (
                m.name.as_str(),
                m.source
                    .as_ref()
                    .expect("every member row carries an anchor")
                    .anchor(),
            )
        })
        .collect();
    assert_eq!(
        anchors,
        vec![
            ("Recalculate", "src/Invoice.cs:11-13".to_string()),
            ("Total", "src/Invoice.cs:6-9".to_string()),
        ],
        "members, in card order, at their real ranges"
    );
}

#[test]
fn moving_a_type_to_another_file_keeps_its_card_path_and_moves_only_the_anchors() {
    let before = {
        let tmp = tempfile::tempdir().unwrap();
        init_repo_with_files(tmp.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);
        let cards = cards_over(tmp.path());
        find(&cards, CardKind::Type, "Invoice").clone()
    };
    let after = {
        let tmp = tempfile::tempdir().unwrap();
        // Same namespace and type, a different file and a leading blank line so
        // the line numbers cannot coincide by accident.
        let moved = format!("\n\n{INVOICE_CS}");
        init_repo_with_files(
            tmp.path(),
            "Alice",
            &[("src/billing/Documents.cs", moved.as_str())],
        );
        let cards = cards_over(tmp.path());
        find(&cards, CardKind::Type, "Invoice").clone()
    };

    assert_eq!(before.identity, after.identity);
    assert_eq!(before.path, after.path);
    assert_ne!(
        before.source.as_ref().unwrap().anchor(),
        after.source.as_ref().unwrap().anchor(),
        "the anchor is content and must follow the source"
    );
}

/// Card schema v4: every file owns a card, so a mention of a file links. The
/// old rule — a type-declaring file gets no card of its own — is gone; what
/// still holds is that the TYPE owns the documentation, so the file card over
/// the same source repeats none of the type's members.
#[test]
fn every_file_gets_a_file_card_and_a_type_declaring_one_repeats_none_of_its_members() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            (
                "appsettings.json",
                "{ \"Logging\": { \"Level\": \"Info\" } }",
            ),
        ],
    );

    let cards = cards_over(tmp.path());
    // The config file declares nothing, so it owns its own card (R8).
    let _ = find(&cards, CardKind::File, "appsettings.json");
    // The C# file declares a type: it has a card too now (v4), but the TYPE
    // owns the documentation, so the file card carries none of its members.
    let file_card = find(&cards, CardKind::File, "src/Invoice.cs");
    assert!(
        file_card.members.is_empty(),
        "a type-declaring file's card must not repeat the type's members: {:#?}",
        file_card.members
    );
}

/// R1/U1: a free function — a `Declares`-target that is neither a type nor a
/// claimed member (KTD3) — becomes a member row on its file's card, carrying
/// its own signature, not a bare "declares" relation stripped of it.
#[test]
fn a_free_function_only_file_gets_a_member_row_with_its_signature() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/helpers.rs",
            "pub fn increment(x: i32) -> i32 {\n    x + 1\n}\n",
        )],
    );

    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::File, "src/helpers.rs");
    assert_eq!(
        card.members.len(),
        1,
        "the free function must appear as a member row, not a bare relation"
    );
    let member = &card.members[0];
    assert_eq!(member.name, "increment");
    assert!(
        member
            .signature
            .as_deref()
            .is_some_and(|s| s.contains("i32")),
        "the member row must carry the function's own signature: {:?}",
        member.signature
    );
}

/// R2/U1: a free function's own outgoing call is lifted onto its file's
/// card, mirroring how a type card already lifts a member's calls — without
/// this, a free function's own call graph would be invisible everywhere.
///
/// Plan A, U3 (R17, KTD13): a call that stays inside the file — between two
/// of the file's own free functions, the file being their unit — is the
/// unit's inside and is left off, as it is between two members of one type.
/// A call to a function in another file is a collaboration and is lifted,
/// tagged with the calling function and its parameters.
#[test]
fn a_free_functions_own_calls_are_lifted_onto_its_file_card() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/helpers.rs",
                "pub fn increment(x: i32) -> i32 {\n    helper(local(x))\n}\n\npub fn local(x: i32) -> i32 {\n    x\n}\n",
            ),
            (
                "src/util.rs",
                "pub fn helper(x: i32) -> i32 {\n    x + 1\n}\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::File, "src/helpers.rs");
    assert!(
        card.outgoing
            .iter()
            .any(|r| r.label.starts_with("helper") && r.via.as_deref() == Some("increment(i32)")),
        "increment's own call to helper must be lifted onto the file card, \
         tagged via increment and its parameters: {:#?}",
        card.outgoing
    );
    assert!(
        !card
            .outgoing
            .iter()
            .chain(&card.incoming)
            .any(|r| r.label.starts_with("local")),
        "a call between two functions of one file stays off its card: {:#?}",
        card.outgoing
    );
}

/// U1: the rewritten Czech `CardKind::File` lead sentence (no longer "no
/// extracted type of its own") renders on an actual file card, not just in
/// the constant's own source — a translation typo or a reverted copy-paste
/// of the old wording would compile and pass every other test. Schema v4
/// reworded the lead again (every file now has a card, so it opens with what
/// the file is before saying what this card holds); the quote follows it.
#[test]
fn a_czech_file_card_uses_the_new_unclaimed_declarations_lead_text() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/helpers.rs",
            "pub fn increment(x: i32) -> i32 {\n    x + 1\n}\n",
        )],
    );

    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::File, "src/helpers.rs");
    let rendered = render_card(
        card,
        Lang::Cs,
        LinkStyle::Portable,
        &CardIndex::build(&cards),
    );
    assert!(
        rendered.contains(
            "Soubor tohoto repozitáře. Typy, které deklaruje, mají vlastní karty; tahle \
             karta nese to, co si žádný typ nenárokuje"
        ),
        "the Czech file card must use the new unclaimed-declarations lead \
         text:\n{rendered}"
    );
}

/// R4/R5/KTD1: a file that declares a type ALONGSIDE a free function gets
/// BOTH a type card (unaffected, no duplicated content) AND a file card
/// whose only member is the free function — the mixed case that was
/// previously invisible everywhere.
#[test]
fn a_file_declaring_a_type_and_a_free_function_gets_both_a_type_card_and_a_file_card() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/money.rs",
            "pub struct Money {\n    pub cents: i64,\n}\n\nimpl Money {\n    pub fn zero() -> Money {\n        Money { cents: 0 }\n    }\n}\n\npub fn format_money(m: &Money) -> String {\n    m.cents.to_string()\n}\n",
        )],
    );

    let cards = cards_over(tmp.path());
    let type_card = find(&cards, CardKind::Type, "Money");
    assert_eq!(
        type_card
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["cents", "zero"],
        "the type card must list its own claimed members — the field and the \
         method — never the unclaimed free function"
    );

    let file_card = find(&cards, CardKind::File, "src/money.rs");
    assert_eq!(
        file_card
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["format_money"],
        "the file card must carry only the unclaimed declaration, never the \
         type or its member"
    );
    // The lead sentence must not claim this file has no extracted type — it
    // does; that type just lives on its own separate card.
    let rendered = render_card(
        file_card,
        Lang::En,
        LinkStyle::Portable,
        &CardIndex::build(&cards),
    );
    assert!(
        !rendered.contains("no extracted type"),
        "the file card must not falsely claim no type was extracted, when \
         Money's own type card exists:\n{rendered}"
    );
}

/// R3/U1: an ambiguity the extractor could not resolve inside a free
/// function surfaces as a caveat on its file's card, mirroring how a type
/// card already surfaces a member's ambiguity admission (R23).
#[test]
fn an_ambiguous_call_inside_a_free_function_surfaces_as_a_caveat_on_its_file_card() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/a.rs", "pub fn shared() -> i32 {\n    1\n}\n"),
            ("src/b.rs", "pub fn shared() -> i32 {\n    2\n}\n"),
            (
                "src/caller.rs",
                "pub fn call_shared() -> i32 {\n    shared()\n}\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::File, "src/caller.rs");
    assert!(
        card.caveats
            .iter()
            .any(|c| c.text.contains("call_shared") && c.text.contains("ambiguous")),
        "call_shared's own ambiguous call must surface as a caveat on its \
         file card, naming which member it came from: {:#?}",
        card.caveats
    );
}

/// KTD3: a type is never treated as an unclaimed declaration, even when a
/// free function and a SECOND top-level type sit beside it in the same
/// file — `own_card` excludes every type regardless of how many others share
/// its file, and each keeps its own type-card treatment untouched by the
/// new file-card logic.
#[test]
fn two_top_level_types_beside_a_free_function_are_never_misclassified_as_unclaimed() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/outer.rs",
            "pub struct Outer {\n    pub inner: Inner,\n}\n\npub struct Inner {\n    pub n: i32,\n}\n\npub fn helper() -> i32 {\n    1\n}\n",
        )],
    );

    let cards = cards_over(tmp.path());
    // Outer and Inner are both top-level types here (this fixture does not
    // nest one inside the other's body) — each gets its own type card, and
    // neither may appear as a member row on the file card, only `helper` may.
    let file_card = find(&cards, CardKind::File, "src/outer.rs");
    assert_eq!(
        file_card
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["helper"],
        "no type must ever be misclassified as an unclaimed declaration"
    );
}

/// KTD3: the successful counterpart to the orphaned-member test below — a
/// C# `partial class` spread across two files (module = the shared C#
/// namespace, not either file's own path) correctly claims a member
/// declared in the OTHER file. `claimed` is a graph-wide set precisely so
/// this real, common cross-file case (the reference C# repository uses partial classes
/// throughout) works, not just the single-file case other tests cover.
#[test]
fn a_partial_class_members_across_two_files_are_correctly_claimed_by_one_type_card() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/Invoice.Core.cs",
                "namespace Orbit.Billing\n{\n    public partial class Invoice\n    {\n        public int Total(int quantity)\n        {\n            return quantity;\n        }\n    }\n}\n",
            ),
            (
                "src/Invoice.Extra.cs",
                "namespace Orbit.Billing\n{\n    public partial class Invoice\n    {\n        public void Recalculate()\n        {\n        }\n    }\n}\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());
    let type_card = find(&cards, CardKind::Type, "Invoice");
    assert_eq!(
        type_card
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Recalculate", "Total"],
        "both partial-class members, declared in two different files, must \
         be claimed by the one Invoice type card: {:#?}",
        type_card.members
    );
    // v4: both files have cards now (every file does), but every declaration
    // in both is claimed by the one Invoice type, so neither file card may
    // list a member of its own.
    for file in ["src/Invoice.Core.cs", "src/Invoice.Extra.cs"] {
        let file_card = find(&cards, CardKind::File, file);
        assert!(
            file_card.members.is_empty(),
            "{file}'s card must not repeat a member the Invoice type claims: {:#?}",
            file_card.members
        );
    }
}

/// Risks & Dependencies: a pre-existing extractor limitation this plan does
/// not fix, made newly visible. A `HasMember` edge only attaches when its
/// owning type's node already exists in the graph at apply time; files apply
/// in a fixed alphabetical order, so a member declared in a file that sorts
/// BEFORE its type's own file never gets linked. Before this plan such an
/// orphaned member rendered as a bare, signature-less "declares" relation;
/// after this plan it is promoted to a full member row on its own file's
/// card, indistinguishable from a genuine free function. Documented as
/// accepted, not asserted as ideal.
#[test]
fn an_extractor_orphaned_cross_file_member_renders_as_a_member_row_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            // Sorts BEFORE the type's own file, so `Money`'s node does not
            // exist yet when this file's HasMember op would apply.
            (
                "src/a_impl.rs",
                "impl Money {\n    pub fn format(&self) -> String {\n        String::new()\n    }\n}\n",
            ),
            (
                "src/z_type.rs",
                "pub struct Money {\n    pub cents: i64,\n}\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());
    let type_card = find(&cards, CardKind::Type, "Money");
    assert_eq!(
        type_card
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["cents"],
        "Money's own field (declared in the same file) still links correctly; \
         the orphaned cross-file method must never reach it: {:#?}",
        type_card.members
    );
    let orphan_file_card = find(&cards, CardKind::File, "src/a_impl.rs");
    assert_eq!(
        orphan_file_card
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["format"],
        "the orphaned member renders as a member row on its own file's card \
         rather than vanishing or panicking"
    );
}

/// R7: `build_cards` output depends only on graph content, never on
/// iteration or insertion order — extended by U1 to a file card that now
/// carries unclaimed declarations.
#[test]
fn a_file_cards_unclaimed_declarations_render_deterministically_across_two_builds() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/money.rs",
            "pub struct Money {\n    pub cents: i64,\n}\n\nimpl Money {\n    pub fn zero() -> Money {\n        Money { cents: 0 }\n    }\n}\n\npub fn format_money(m: &Money) -> String {\n    m.cents.to_string()\n}\n",
        )],
    );

    let first = cards_over(tmp.path());
    let second = cards_over(tmp.path());
    let first_file = find(&first, CardKind::File, "src/money.rs");
    let second_file = find(&second, CardKind::File, "src/money.rs");
    assert_eq!(
        first_file, second_file,
        "two builds over the same graph must produce byte-identical file cards"
    );
}

#[test]
fn a_module_card_lists_what_it_holds_in_both_directions() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("src/notes.md", "# notes"),
            (
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());
    let module = find(&cards, CardKind::Module, "src");

    let contained: Vec<&str> = module
        .outgoing
        .iter()
        .map(|r| r.identity.as_str())
        .collect();
    assert!(contained.contains(&"src/Invoice.cs"), "{contained:?}");
    assert!(contained.contains(&"src/notes.md"), "{contained:?}");

    // The root module contains `src`, so `src` has an incoming relation — the
    // "dovnitř" half of R13.
    let incoming: Vec<&str> = module
        .incoming
        .iter()
        .map(|r| r.identity.as_str())
        .collect();
    assert!(incoming.contains(&"."), "{incoming:?}");
}

#[test]
fn a_doc_comment_is_quoted_verbatim_and_its_absence_leaves_no_section() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            (
                "src/Plain.cs",
                "namespace Orbit.Billing\n{\n    public class Plain\n    {\n    }\n}\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());

    let documented = find(&cards, CardKind::Type, "Invoice");
    let doc = documented.doc.as_ref().expect("Invoice has a doc comment");
    assert!(
        doc.text.contains("Faktura vydaná odběrateli."),
        "diacritics must survive byte for byte: {:?}",
        doc.text
    );
    assert!(!doc.redacted);

    let undocumented = find(&cards, CardKind::Type, "Plain");
    assert!(undocumented.doc.is_none());
    let rendered = render_card(
        undocumented,
        Lang::En,
        LinkStyle::Portable,
        &CardIndex::default(),
    );
    assert!(!rendered.contains("## Documentation"), "{rendered}");
}

#[test]
fn a_secret_shaped_doc_comment_line_is_quoted_without_its_value() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/Conn.cs",
            "namespace Orbit.Infra\n\
             {\n\
             \x20   /// <summary>Example: password = \"hunter2-do-not-ship\"</summary>\n\
             \x20   public class Conn\n\
             \x20   {\n\
             \x20   }\n\
             }\n",
        )],
    );

    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::Type, "Conn");
    let doc = card.doc.as_ref().expect("Conn has a doc comment");

    assert!(doc.redacted, "the secret-shaped line must be flagged");
    assert!(
        !doc.text.contains("hunter2-do-not-ship"),
        "R22: the value must never reach the card: {:?}",
        doc.text
    );
    let rendered = render_card(card, Lang::En, LinkStyle::Portable, &CardIndex::default());
    assert!(!rendered.contains("hunter2-do-not-ship"), "{rendered}");
    // The comment and the note about it are the detail's since the card trim:
    // the short card quotes no comment, so it has nothing to confess about one.
    assert!(!rendered.contains("## Notes"), "{rendered}");
    assert!(!rendered.contains("## Documentation"), "{rendered}");
    // The purpose leaves a summary the filter cut into out (plan B, U7), so
    // the detail quotes the comment, marker and all, rather than drop it, and
    // says in its notes that a line was replaced.
    let details = details(&cards, Lang::En);
    let detail = detail_of(&cards, &details, CardKind::Type, "Conn");
    assert!(!detail.contains("hunter2-do-not-ship"), "{detail}");
    assert!(
        section(detail, "Documentation").contains("[redacted: "),
        "{detail}"
    );
    assert!(
        section(detail, "Notes").contains("secret pattern"),
        "{detail}"
    );
}

#[test]
fn two_runs_over_one_repository_render_byte_identical_cards() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[("src/Invoice.cs", INVOICE_CS), ("src/notes.md", "# notes")],
    );

    let repo = Repo::open(tmp.path()).unwrap();
    let build = || {
        let mut graph = Graph::new();
        run_all(&repo, &mut graph).unwrap();
        merge(&mut graph, &trust_ranks());
        build_card_files(&graph, Lang::En, LinkStyle::Portable)
    };

    let first = build();
    let second = build();
    assert!(!first.is_empty(), "the fixture must produce cards");
    assert_eq!(first, second);
}

#[test]
fn lang_switches_card_prose_and_leaves_the_english_skeleton() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());

    let en = build_card_files(&graph, Lang::En, LinkStyle::Portable);
    let cs = build_card_files(&graph, Lang::Cs, LinkStyle::Portable);

    let paths_en: Vec<&String> = en.iter().map(|f| &f.path).collect();
    let paths_cs: Vec<&String> = cs.iter().map(|f| &f.path).collect();
    assert_eq!(paths_en, paths_cs, "prose language must not move a card");

    let invoice_en = en
        .iter()
        .find(|f| f.path.ends_with("/Invoice.md"))
        .expect("an Invoice card");
    let invoice_cs = cs
        .iter()
        .find(|f| f.path.ends_with("/Invoice.md"))
        .expect("an Invoice card");

    // v6: the short card's prose is the purpose's provenance and the role;
    // the lead sentence is gone in both languages (plan B, R17).
    assert!(invoice_en.contents.contains("(from the comment)"));
    assert!(invoice_cs.contents.contains("(z komentáře)"));
    assert!(invoice_cs
        .contents
        .contains("V tomto repozitáři nebylo vidět"));
    // v4: a type card's members were its `## API` list (`## Members` is kept
    // only on declared cards); since v6 that list is the detail's. Every
    // heading is skeleton, English either way.
    for heading in [
        "# Invoice",
        "- Identity:",
        "- Detail:",
        "## Purpose",
        "## Role",
        "## Keywords",
    ] {
        assert!(invoice_en.contents.contains(heading), "{invoice_en:?}");
        assert!(invoice_cs.contents.contains(heading), "{invoice_cs:?}");
    }
    let cards = build_cards(&graph);
    for lang in [Lang::En, Lang::Cs] {
        let details = details(&cards, lang);
        let detail = detail_of(&cards, &details, CardKind::Type, "Invoice");
        assert!(detail.contains("\n## API\n"), "{detail}");
    }
}

#[test]
fn a_deeply_nested_namespace_stays_inside_the_windows_path_budget() {
    let namespace = (0..12)
        .map(|i| format!("VeryDeeplyNestedNamespaceSegment{i}"))
        .collect::<Vec<_>>()
        .join(".");
    let source = format!(
        "namespace {namespace}\n{{\n    public class DeeplyNestedTypeName\n    {{\n    }}\n}}\n"
    );

    let build = || {
        let tmp = tempfile::tempdir().unwrap();
        init_repo_with_files(tmp.path(), "Alice", &[("src/Deep.cs", source.as_str())]);
        let cards = cards_over(tmp.path());
        find(&cards, CardKind::Type, "DeeplyNestedTypeName")
            .path
            .clone()
    };

    let first = build();
    let second = build();
    assert_eq!(first, second, "a shortened path must be stable across runs");
    assert!(
        first.len() <= 120,
        "{} chars is over the budget: {first}",
        first.len()
    );
    // Under budget is not the requirement — taking the HASHED FALLBACK is. Any
    // short path at all satisfies the length assertion, including one produced
    // by a card_path that silently truncated the namespace and collided.
    assert!(
        first.contains("/_hashed/"),
        "an over-long identity must fall back to the hashed name: {first}"
    );
    assert!(
        first.contains("DeeplyNestedTypeName-"),
        "and keep a readable stub, so the file is still findable: {first}"
    );
}

#[test]
fn a_type_card_reports_the_test_pairing_of_the_file_that_declares_it() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            (
                "src/Paid.cs",
                "namespace Orbit.Billing\n{\n    public class Paid\n    {\n    }\n}\n",
            ),
            (
                "tests/PaidTests.cs",
                "namespace Orbit.Billing.Tests\n{\n    public class PaidTests\n    {\n    }\n}\n",
            ),
        ],
    );

    let cards = cards_over(tmp.path());

    // The pairing heuristic pairs files; a type card must still be able to answer
    // R9's "which test corresponds to this" through its declaring file.
    let paired = find(&cards, CardKind::Type, "Paid");
    let pairing = paired
        .test
        .as_ref()
        .expect("a type card inherits its file's pairing statement");
    assert!(pairing.statement.contains(HAS_TEST), "{pairing:?}");
    // The pairing is CONCLUDED from how the files are named, never read at a
    // place in the source. The card carries that, so a conclusion cannot be
    // mistaken for an observation — it is the one non-measured thing it shows.
    assert_eq!(pairing.tier, Tier::Derived);

    // …and it says the pairing was measured on the FILE, not on the type. The
    // heuristic pairs files, so without this a file declaring three types hands
    // all three the same sentence as if it were a claim about each of them.
    assert_eq!(pairing.from_file.as_deref(), Some("src/Paid.cs"));

    let unpaired = find(&cards, CardKind::Type, "Invoice");
    let pairing = unpaired.test.as_ref().expect("an honest not-found");
    assert!(
        pairing.statement.contains(NO_TEST),
        "never claims 'untested': {pairing:?}"
    );
}

#[test]
fn two_types_in_one_file_are_not_each_told_the_file_has_their_test() {
    // The shape the finding is about. Both cards carry the same sentence,
    // because it is the same file — so the rendered line has to name the file
    // it is about, or each card reads as a claim about its own type.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/Billing.cs",
                "namespace Orbit.Billing\n{\n    public class Money\n    {\n        \
                 public void Add() { }\n    }\n\n    public class Invoice\n    {\n        \
                 public void Close() { }\n    }\n}\n",
            ),
            (
                "tests/BillingTests.cs",
                "namespace Orbit.Billing.Tests\n{\n    public class BillingTests\n    {\n    }\n}\n",
            ),
        ],
    );

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    let cards = build_cards(&graph);
    // The pairing is in the detail since the card trim; a public method
    // each gives both types one.
    let details = details(&cards, Lang::En);

    for name in ["Money", "Invoice"] {
        let rendered = section(
            detail_of(&cards, &details, CardKind::Type, name),
            "Test pairing",
        );
        assert!(
            rendered.contains("— for the file `src/Billing.cs`"),
            "{name}'s detail must attribute the pairing to the file:\n{rendered}"
        );
        // And it still answers the question the reader came with: which test.
        assert!(
            rendered.contains("`tests/BillingTests.cs`"),
            "{name}'s detail must name the test:\n{rendered}"
        );
    }
}

#[test]
fn golden_card_corpus_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("src/notes.md", "# notes"),
            (
                "appsettings.json",
                "{ \"Logging\": { \"Level\": \"Info\" } }",
            ),
        ],
    );

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());

    let mut combined = String::new();
    for file in build_card_files(&graph, Lang::En, LinkStyle::Portable) {
        combined.push_str(&format!("===== {} =====\n{}\n", file.path, file.contents));
    }
    insta::assert_snapshot!("phase_c_cards", combined);
}

#[test]
fn every_card_path_is_unique_and_lives_under_the_cards_tree() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("src/notes.md", "# notes"),
            ("appsettings.json", "{}"),
        ],
    );

    let cards = cards_over(tmp.path());
    let mut seen = std::collections::BTreeSet::new();
    for card in &cards {
        assert!(
            card.path.starts_with("cards/"),
            "{} escapes the card tree",
            card.path
        );
        assert!(
            seen.insert(card.path.to_lowercase()),
            "two cards claim {}",
            card.path
        );
    }
}

#[test]
fn every_card_opens_with_front_matter_naming_the_format_version() {
    // A card travels alone — pulled out as a retrieval chunk, pasted into a
    // ticket, handed to a model. A consumer that meets one in isolation must be
    // able to tell a skeleton it understands from one it does not, without
    // reading a word of the Markdown beneath.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[("src/Invoice.cs", INVOICE_CS), ("README.md", "# Readme\n")],
    );

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    let files = build_card_files(&graph, Lang::En, LinkStyle::Portable);

    assert!(files.len() > 2, "the fixture must produce several cards");
    // v5: `## Maybe also`, the name-only label, relation rows with every site
    // and `via` with parameters (plan A, U3). v6: the short type card, its
    // lists moved to the detail (plan B, U9).
    assert_eq!(reverse_architect::card::render::CARD_SCHEMA_VERSION, 6);
    // v4 added a `kind:` line, so a lone card also says what it describes. The
    // word must be the card's own kind, which the path's first segment names.
    let expected = format!(
        "---\nschema: {}\ntool: reverse-architect\nkind: ",
        reverse_architect::card::render::CARD_SCHEMA_VERSION
    );
    for file in &files {
        let opening = &file.contents[..file.contents.len().min(160)];
        let rest = file.contents.strip_prefix(&expected).unwrap_or_else(|| {
            panic!(
                "{} does not open with the front matter:\n{opening}",
                file.path
            )
        });
        let (kind, rest) = rest.split_once('\n').expect("a kind line");
        let path_kind = file
            .path
            .strip_prefix("cards/")
            .and_then(|p| p.split('/').next())
            .expect("a card lives under cards/<kind>/");
        assert!(!kind.is_empty(), "{}: an empty kind line", file.path);
        // The three core kinds name their own tree; a declared kind's word
        // (`namespace:name`) lives under `cards/declared/`, so it is not compared.
        if ["type", "file", "module"].contains(&path_kind) {
            assert_eq!(kind, path_kind, "{}: kind line vs path", file.path);
        }
        assert!(
            rest.starts_with("---\n\n# "),
            "{} does not close its front matter before the title:\n{opening}",
            file.path
        );
    }
}

/// Plan A, U3 (R19, AE3): a call whose name two declarations answer is ONE
/// unsettled claim with both alternatives, not two rows. Measured on the R17
/// target before U3: 32% of relation rows on type cards were byte-identical to
/// another row on the same card — an ambiguous call resolves to several real
/// candidates (R23), and each rendered as its own row. U2 told those rows
/// apart by container and parameters; U3 takes them out of the main list into
/// one `## Maybe also` claim that names every alternative, two different
/// types' `IsValid` as much as two overloads of `Log.Write`. Since plan A, U7
/// the receivers say why each stays unsettled: `m` is declared by `var` from a
/// call, which states no type, so no rule picks an `IsValid`; `log` is a `Log`,
/// which backs BOTH overloads of `Write`, and a backing that picks no one
/// target settles nothing.
#[test]
fn two_same_named_candidates_are_one_maybe_also_claim_naming_both() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/Billing.cs",
            "namespace Orbit.Billing\n\
             {\n\
             \x20   public class Money\n\
             \x20   {\n\
             \x20       public bool IsValid() { return true; }\n\
             \x20   }\n\
             \x20   public class Invoice\n\
             \x20   {\n\
             \x20       public bool IsValid() { return true; }\n\
             \x20   }\n\
             \x20   public class Log\n\
             \x20   {\n\
             \x20       public void Write(string s) { }\n\
             \x20       public void Write(int n) { }\n\
             \x20   }\n\
             \x20   public class Checker\n\
             \x20   {\n\
             \x20       public bool Run(Log log) { var m = Load(); log.Write(1); return m.IsValid(); }\n\
             \x20   }\n\
             }\n",
        )],
    );

    let cards = cards_over(tmp.path());
    let checker = find(&cards, CardKind::Type, "Checker");
    assert!(
        !checker
            .outgoing
            .iter()
            .any(|r| r.label.contains("IsValid") || r.label.contains("Write")),
        "an ambiguous call is no main-list row: {:#?}",
        checker.outgoing
    );

    let claims: Vec<(String, Vec<&str>, usize, Vec<u32>)> = checker
        .maybe_also
        .iter()
        .map(|c| {
            (
                c.source.label.clone(),
                c.alternatives.iter().map(|a| a.label.as_str()).collect(),
                c.candidates,
                c.sites.iter().map(|s| s.line).collect(),
            )
        })
        .collect();
    assert_eq!(
        claims,
        [
            (
                "Run(Log)".to_string(),
                vec!["Invoice.IsValid()", "Money.IsValid()"],
                2,
                vec![18]
            ),
            (
                "Run(Log)".to_string(),
                vec!["Log.Write(int)", "Log.Write(string)"],
                2,
                vec![18]
            ),
        ],
        "one claim per unsettled name, each naming WHICH candidates: {:#?}",
        checker.maybe_also
    );
}

/// The `GeoSolver` shape from plan A's Problem Frame, in miniature. Inside the
/// solver `CleanTyp` calls `FoundTables`; a foreign `CleanServer.Run` calls
/// `FoundTables` from three lines (7, 8, 9) and makes a bare `Solve(1)` call
/// (line 10) that the solver's two overloads both answer. Each type lives in
/// its own directory. The server imports the solver's namespace on its first
/// line — valid C#, and the line numbers above stay where they were; without
/// it `namespace-visibility` rightly disputes the parameter type (plan A, U7).
/// The solver is `static`, as the real one is, so its card's header has a
/// modifier the stated visibility drops (plan B, U1, AE3).
const GEO_SOLVER_CS: &str =
    "namespace Orbit.Geo\n{\n    public static class GeoSolver\n    {\n        \
    public void CleanTyp()\n        {\n            FoundTables();\n        }\n\n        \
    public void FoundTables() { }\n\n        public int Solve(int n) { return n; }\n\n        \
    public int Solve(string s) { return 0; }\n    }\n}\n";
const CLEAN_SERVER_CS: &str = "using Orbit.Geo; namespace Orbit.Server\n{\n    public class CleanServer\n    {\n        \
    public void Run(GeoSolver solver)\n        {\n            solver.FoundTables();\n            \
    solver.FoundTables();\n            solver.FoundTables();\n            Solve(1);\n        }\n    \
    }\n}\n";

fn geo_cards() -> Vec<Card> {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Geo/GeoSolver.cs", GEO_SOLVER_CS),
            ("Server/CleanServer.cs", CLEAN_SERVER_CS),
        ],
    );
    cards_over(tmp.path())
}

/// The two type cards of the `GeoSolver` fixture as rendered (plan A, U3):
/// the `## Maybe also` section, the `name only` label, a row carrying three
/// sites, `via` with parameters, and no row between the solver's own members.
/// Since plan B, U9 the lists are in each type's detail, rendered after its
/// short card, so the snapshot still holds all of them.
#[test]
fn golden_truthful_graph_cards_snapshot() {
    let cards = geo_cards();
    let mut combined = String::new();
    for title in ["GeoSolver", "CleanServer"] {
        let card = find(&cards, CardKind::Type, title);
        combined.push_str(&format!(
            "===== {} =====\n{}\n",
            card.path,
            rendered(&cards, title)
        ));
        combined.push_str(&format!(
            "===== {} =====\n{}\n",
            card.detail.as_deref().expect("a detail"),
            rendered_detail(&cards, title)
        ));
    }
    insta::assert_snapshot!("truthful_graph_cards", combined);
}

/// Plan A, AE7 (R17): a call between two members of one type is the type's
/// inside. It is in neither list of the type's card — before U3 it showed
/// twice, once each way, and the `Referenced by` half read backwards.
#[test]
fn a_call_between_members_of_one_type_is_on_neither_list_of_its_card() {
    let cards = geo_cards();
    let solver = find(&cards, CardKind::Type, "GeoSolver");
    let inside: Vec<&reverse_architect::card::Relation> = solver
        .outgoing
        .iter()
        .chain(&solver.incoming)
        .filter(|r| r.identity.contains("#GeoSolver."))
        .collect();
    assert!(
        inside.is_empty(),
        "rows between the solver's own members: {inside:#?}"
    );

    let rendered = rendered_detail(&cards, "GeoSolver");
    let referenced = rendered
        .split("## Referenced by")
        .nth(1)
        .expect("the solver is referenced");
    assert!(!referenced.contains("CleanTyp"), "{rendered}");
}

/// Plan A, AE7 and R18: the foreign caller appears in `Referenced by` exactly
/// once for the member it calls, naming itself and the called member with its
/// parameters — and, calling that member from three lines, it is one row
/// carrying all three sites. The receiver is declared as `GeoSolver`, so
/// `declared-receiver` backs the call (plan A, U7): a rules-backed row in the
/// main list, not repeated under `Maybe also`.
#[test]
fn a_foreign_caller_is_referenced_once_with_the_called_member_and_every_site() {
    let cards = geo_cards();
    let solver = find(&cards, CardKind::Type, "GeoSolver");
    let calls: Vec<&reverse_architect::card::Relation> = solver
        .incoming
        .iter()
        .filter(|r| r.identity.contains("CleanServer.Run") && r.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1, "{:#?}", solver.incoming);
    let call = calls[0];
    assert_eq!(call.label, "CleanServer.Run(GeoSolver)");
    assert_eq!(call.via.as_deref(), Some("FoundTables()"));
    assert_eq!(call.rung, Some(Confidence::RulesBacked));
    let sites: Vec<(&str, u32)> = call
        .sites
        .iter()
        .map(|s| (s.file.as_str(), s.line))
        .collect();
    assert_eq!(
        sites,
        [
            ("Server/CleanServer.cs", 7),
            ("Server/CleanServer.cs", 8),
            ("Server/CleanServer.cs", 9)
        ]
    );

    let rendered = rendered_detail(&cards, "GeoSolver");
    // Each site with the column of the called name, which the C# plugin
    // reports since contract 0.2 (plan A, U6) — and, in the detail, the rung
    // the row stands on.
    assert!(
        rendered.contains(
            "(heuristic, rules-backed) via `FoundTables()` — \
             `Server/CleanServer.cs:7:20, 8:20, 9:20`\n"
        ),
        "{rendered}"
    );
    assert!(
        !solver.maybe_also.iter().any(|c| c
            .alternatives
            .iter()
            .any(|a| a.label.contains("FoundTables"))),
        "a rules-backed relation is not a maybe-also claim: {:#?}",
        solver.maybe_also
    );
}

/// Plan A, AE3 (R7, R19): the bare `Solve(1)` that two overloads answer is one
/// claim — "1 of 2 candidates" naming both — on the card of the type whose
/// overloads they are and on the caller's own card, and no row in either
/// main list. Before U3 it was two `Referenced by` rows that read identically.
#[test]
fn a_bare_call_two_overloads_answer_is_one_maybe_also_claim() {
    let cards = geo_cards();
    let solver = find(&cards, CardKind::Type, "GeoSolver");
    let server = find(&cards, CardKind::Type, "CleanServer");
    for card in [solver, server] {
        assert!(
            !card
                .outgoing
                .iter()
                .chain(&card.incoming)
                .any(|r| r.identity.contains("Solve(")),
            "{}: an ambiguous call is no main-list row: {:#?} / {:#?}",
            card.title,
            card.outgoing,
            card.incoming
        );
        let claims: Vec<_> = card
            .maybe_also
            .iter()
            .map(|c| {
                (
                    c.incoming,
                    c.source.label.as_str(),
                    c.alternatives
                        .iter()
                        .map(|a| a.label.as_str())
                        .collect::<Vec<_>>(),
                    c.candidates,
                    c.sites.iter().map(|s| s.line).collect::<Vec<_>>(),
                )
            })
            .collect();
        let (incoming, source) = if card.title == "GeoSolver" {
            (true, "CleanServer.Run(GeoSolver)")
        } else {
            (false, "Run(GeoSolver)")
        };
        assert_eq!(
            claims,
            [(
                incoming,
                source,
                vec!["GeoSolver.Solve(int)", "GeoSolver.Solve(string)"],
                2,
                vec![10]
            )],
            "{}",
            card.title
        );
    }

    let en = rendered_detail(&cards, "GeoSolver");
    assert_eq!(en.matches("1 of 2 candidates").count(), 1, "{en}");
    let cs = details(&cards, Lang::Cs);
    let cs = detail_of(&cards, &cs, CardKind::Type, "GeoSolver");
    assert!(cs.contains("1 ze 2 kandidátů"), "{cs}");
    // The short card names no guess (plan B Key Decision on "maybe also").
    assert!(!rendered(&cards, "GeoSolver").contains("1 of 2 candidates"));
}

/// A card none of whose relations is ambiguous has no `Maybe also` section —
/// an empty one would read as "checked, nothing uncertain".
#[test]
fn a_card_without_an_ambiguous_relation_has_no_maybe_also_section() {
    let cards = geo_cards();
    let module = find(&cards, CardKind::Module, "Geo");
    assert!(module.maybe_also.is_empty());
    let index = CardIndex::build(&cards);
    let out = render_card(module, Lang::En, LinkStyle::Portable, &index);
    assert!(!out.contains("## Maybe also"), "{out}");
    let solver = rendered_detail(&cards, "GeoSolver");
    assert!(solver.contains("## Maybe also"), "control: {solver}");
}

/// Plan A, U3: the backlink cap of 50 per kind counts the CLEAN list, so calls
/// between the type's own members cannot crowd out a real foreign caller. 52
/// foreign callers and five of the solver's own members all call
/// `FoundTables`: 50 foreign callers are listed and 2 are said to be left out —
/// not 7, which is what the cap said while the five inside calls still counted.
#[test]
fn the_backlink_cap_counts_only_the_clean_list() {
    use reverse_architect::card::links::{backlinks, BACKLINK_CAP};
    let mut own = String::new();
    for i in 0..5 {
        own.push_str(&format!(
            "        public void Inner{i}() {{ FoundTables(); }}\n"
        ));
    }
    let solver = format!(
        "namespace Orbit.Geo\n{{\n    public class GeoSolver\n    {{\n        \
         public void FoundTables() {{ }}\n{own}    }}\n}}\n"
    );
    let mut files: Vec<(String, String)> = vec![("Geo/GeoSolver.cs".into(), solver)];
    for i in 0..52 {
        files.push((
            format!("Callers/Caller{i:02}.cs"),
            format!(
                "namespace Orbit.Callers\n{{\n    public class Caller{i:02}\n    {{\n        \
                 public void Go() {{ FoundTables(); }}\n    }}\n}}\n"
            ),
        ));
    }
    let tmp = tempfile::tempdir().unwrap();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    init_repo_with_files(tmp.path(), "Alice", &borrowed);
    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::Type, "GeoSolver");

    let (shown, omitted) = backlinks(card);
    let shown_calls: Vec<&str> = shown
        .iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .map(|r| r.label.as_str())
        .collect();
    assert_eq!(shown_calls.len(), BACKLINK_CAP);
    assert!(
        shown_calls.iter().all(|l| l.starts_with("Caller")),
        "only foreign callers are listed: {shown_calls:?}"
    );
    let omitted: Vec<(String, usize)> = omitted
        .into_iter()
        .map(|(word, n)| (word.into_owned(), n))
        .collect();
    assert_eq!(omitted, [("calls".to_string(), 2)]);
}

/// Three C# modules for the rung scenarios (plan A, U2). `Invoice.Close` calls
/// `Post`, which two types declare — an `Ambiguous` pair — and `Send`, which one
/// type declares — `NameOnly`, the rung every unambiguous name match carries
/// until a sifting rule exists. Each type sits in its own directory so the
/// architecture page has module-to-module relations to draw.
fn rung_fixture() -> Graph {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "Billing/Invoice.cs",
                "namespace Orbit.Billing\n{\n    public class Invoice\n    {\n        \
                 public void Close()\n        {\n            Post();\n            Send();\n        \
                 }\n    }\n}\n",
            ),
            (
                "Ledger/Ledger.cs",
                "namespace Orbit.Ledger\n{\n    public class Ledger\n    {\n        \
                 public void Post() { }\n    }\n    public class Journal\n    {\n        \
                 public void Post() { }\n    }\n}\n",
            ),
            (
                "Mail/Mailer.cs",
                "namespace Orbit.Mail\n{\n    public class Mailer\n    {\n        \
                 public void Send() { }\n    }\n}\n",
            ),
        ],
    );
    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    graph
}

/// The calls `Invoice.Close` makes, with the rung each one carries.
fn close_calls(graph: &Graph) -> Vec<(String, Option<Confidence>)> {
    graph
        .edges_sorted()
        .into_iter()
        .filter(|e| e.edge.kind == EdgeKind::Calls && e.from.as_str().contains("Close"))
        .map(|e| (e.to.as_str().to_string(), e.edge.provenance.rung()))
        .collect()
}

/// A copy of `graph` with every edge passed through `rerate`; `None` drops it.
///
/// No rule produces `RulesBacked` yet, so a test that needs one has to state it
/// on an extracted graph rather than wait for an extractor to emit it.
fn rerated(graph: &Graph, rerate: impl Fn(&str, &str, &Edge) -> Option<Edge>) -> Graph {
    let mut out = Graph::new();
    for node in graph.nodes_sorted() {
        out.ensure_node(node.key.clone(), node.kind).unwrap();
        for fact in &node.facts {
            out.add_fact(&node.key, fact.clone());
        }
    }
    for e in graph.edges_sorted() {
        if let Some(edge) = rerate(e.from.as_str(), e.to.as_str(), e.edge) {
            out.add_edge(e.from, e.to, edge);
        }
    }
    out
}

/// The arrow the collaboration diagram draws between the carded entity and the
/// peer labelled `label`, on either side.
fn arrow_to(rendered: &str, label: &str) -> String {
    let lines: Vec<&str> = rendered.lines().map(str::trim).collect();
    let node = lines
        .iter()
        .find_map(|l| l.strip_suffix(&format!("[\"{label}\"]")))
        .filter(|id| id.starts_with('u') || id.starts_with('d'))
        .unwrap_or_else(|| panic!("no diagram node for {label}:\n{rendered}"));
    let edge = lines
        .iter()
        .find(|l| {
            (l.starts_with("self ") && l.ends_with(&format!(" {node}")))
                || (l.starts_with(&format!("{node} ")) && l.ends_with(" self"))
        })
        .unwrap_or_else(|| panic!("no diagram edge for {label}:\n{rendered}"));
    edge.split_whitespace()
        .nth(1)
        .and_then(|a| a.split('|').next())
        .unwrap()
        .to_string()
}

fn rendered(cards: &[Card], title: &str) -> String {
    render_card(
        find(cards, CardKind::Type, title),
        Lang::En,
        LinkStyle::Portable,
        &CardIndex::build(cards),
    )
}

/// A type's detail as rendered: since plan B, U9 the relation lists, the
/// `Maybe also` claims and the collaboration diagram left the short card for
/// it, so what plan A proved of the card is proved here of the detail.
fn rendered_detail(cards: &[Card], title: &str) -> String {
    let details = details(cards, Lang::En);
    detail_of(cards, &details, CardKind::Type, title).to_string()
}

/// Plan A, U2: a name match nothing backed is not a certain relation, so the
/// diagram never draws it as one — and it is not ambiguous either, so it is
/// drawn, dotted, where a type reached only through an ambiguous match is not
/// drawn at all. It still stands in the main lists (Key Decision on routing):
/// the role text names its user and the API list says what the member reaches.
#[test]
fn a_type_reached_only_by_a_name_only_call_has_no_solid_line_in_the_diagram() {
    let graph = rung_fixture();
    let calls = close_calls(&graph);
    assert!(
        calls
            .iter()
            .any(|(to, r)| to.contains("Mailer") && *r == Some(Confidence::NameOnly)),
        "the fixture must give Send a single NameOnly candidate: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .filter(|(to, _)| to.contains("Post"))
            .all(|(_, r)| *r == Some(Confidence::Ambiguous)),
        "the fixture must leave Post ambiguous: {calls:?}"
    );

    let cards = build_cards(&graph);
    // The diagram is the detail's since plan B, U9.
    let invoice = rendered_detail(&cards, "Invoice");
    assert_eq!(
        arrow_to(&invoice, "Mailer"),
        "-.->",
        "a NameOnly relation is drawn dotted, never solid:\n{invoice}"
    );
    // Ledger and Journal are reached only through the ambiguous `Post`: no
    // peer, no diagram node — one count sentence on the card says they may
    // be (R7).
    assert!(
        !invoice.contains("[\"Ledger\"]") && !invoice.contains("[\"Journal\"]"),
        "a type reached only through an Ambiguous relation is not drawn:\n{invoice}"
    );
    let card = rendered(&cards, "Invoice");
    assert!(
        card.contains("The detail lists 2 more neighbours (all uncertain).\n"),
        "the two ambiguous candidates are counted, not named:\n{card}"
    );

    let mailer = rendered(&cards, "Mailer");
    assert!(
        mailer.contains("Used by 1 in 1 module, chiefly "),
        "a NameOnly user stays in the main lists:\n{mailer}"
    );
    let close = find(&cards, CardKind::Type, "Invoice")
        .story
        .api
        .iter()
        .find(|r| r.name.starts_with("Close"))
        .expect("Close is on Invoice's API list");
    let reached: Vec<&str> = close.reaches.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(
        reached,
        ["Mailer"],
        "Close reaches Mailer by name and neither ambiguous candidate"
    );
}

/// Plan A, U2: what makes a relation certain is its rung, not the candidate
/// count. A rule that excluded every candidate but one keeps the count the name
/// match found (`ambiguous_candidates: Some(2)`) and still backs the survivor,
/// so the role text, the collaboration diagram, the API list and the module map
/// all treat it as certain. No rule produces `RulesBacked` yet, so the rung is
/// stated on the extracted graph.
#[test]
fn a_rules_backed_call_counts_as_certain_in_the_role_text_and_the_module_map() {
    let graph = rerated(&rung_fixture(), |from, to, edge| {
        if edge.kind != EdgeKind::Calls || !from.contains("Close") {
            return Some(edge.clone());
        }
        if to.contains("Journal") {
            // Excluded by the rule that backs the Ledger candidate.
            return None;
        }
        let mut edge = edge.clone();
        if to.contains("Ledger") {
            assert_eq!(edge.provenance.ambiguous_candidates, Some(2));
            edge.provenance = edge
                .provenance
                .clone()
                .rated(Confidence::RulesBacked, Rule::NameMatch);
        }
        Some(edge)
    });
    let (cards, insight, _) = build_cards_with_insight(&graph);

    let invoice = rendered_detail(&cards, "Invoice");
    assert_eq!(
        arrow_to(&invoice, "Ledger"),
        "-->",
        "a RulesBacked relation is drawn solid:\n{invoice}"
    );
    assert!(
        invoice.contains("- calls `Ledger.Post()` (heuristic, rules-backed) via `Close()`"),
        "the candidate count a rule settled no longer marks the row ambiguous:\n{invoice}"
    );
    assert!(
        rendered(&cards, "Invoice").contains("[Ledger]("),
        "and the short card's role names it as a peer"
    );
    let ledger = rendered(&cards, "Ledger");
    assert!(
        ledger.contains("Used by 1 in 1 module, chiefly "),
        "a RulesBacked user counts in the role text:\n{ledger}"
    );
    let close = find(&cards, CardKind::Type, "Invoice")
        .story
        .api
        .iter()
        .find(|r| r.name.starts_with("Close"))
        .expect("Close is on Invoice's API list");
    assert!(
        close.reaches.iter().any(|(_, l)| l == "Ledger"),
        "Close reaches Ledger on a RulesBacked call: {:?}",
        close.reaches
    );

    let index = CardIndex::build(&cards);
    let pages = build_pages(&insight, &cards, &index, LinkStyle::Portable);
    let architecture = &pages
        .iter()
        .find(|p| p.path == "architecture.md")
        .unwrap()
        .contents;
    let node = |module: &str| {
        architecture
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_suffix(&format!("[\"{module}\"]")))
            .filter(|id| id.starts_with('m'))
            .unwrap_or_else(|| panic!("no module map node for {module}:\n{architecture}"))
            .to_string()
    };
    let (billing, ledger_module) = (node("Billing"), node("Ledger"));
    assert!(
        architecture
            .lines()
            .map(str::trim)
            .any(|l| l == format!("{billing} -->|1| {ledger_module}")),
        "the module map draws the RulesBacked relation:\n{architecture}"
    );
}

// U10 signpost tests live in tests/card_index.rs.

/// Plan A, R7 on inheritance: an `extends` whose base rests only on an
/// ambiguous name match makes the base no base — not in the deriving type's
/// role text or `## Inheritance`, not among the base's builders, not in the
/// hierarchy page. Stated on an extracted graph, as the rung tests above do.
#[test]
fn an_ambiguous_extends_is_no_base_on_either_card_nor_in_the_hierarchy() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "Billing/Invoice.cs",
                "using Orbit.Docs;\nnamespace Orbit.Billing\n{\n    public class Invoice : Document\n    {\n    }\n}\n",
            ),
            (
                "Docs/Document.cs",
                "namespace Orbit.Docs\n{\n    public class Document\n    {\n    }\n}\n",
            ),
        ],
    );
    let repo = Repo::open(tmp.path()).unwrap();
    let mut extracted = Graph::new();
    run_all(&repo, &mut extracted).unwrap();
    merge(&mut extracted, &trust_ranks());
    let graph = rerated(&extracted, |_, to, edge| {
        let mut edge = edge.clone();
        if edge.kind == EdgeKind::Extends && to.contains("Document") {
            edge.provenance = edge
                .provenance
                .clone()
                .rated(Confidence::Ambiguous, Rule::NameMatch);
        }
        Some(edge)
    });
    let extends: Vec<_> = graph
        .edges_sorted()
        .into_iter()
        .filter(|e| e.edge.kind == EdgeKind::Extends)
        .map(|e| (e.to.as_str().to_string(), e.edge.provenance.rung()))
        .collect();
    assert_eq!(
        extends,
        [(
            "sym:csharp:Orbit.Docs#Document".to_string(),
            Some(Confidence::Ambiguous)
        )],
        "the fixture must extend Document once, ambiguously"
    );

    let (cards, insight, _) = build_cards_with_insight(&graph);
    let invoice = rendered(&cards, "Invoice");
    let document = rendered(&cards, "Document");
    assert!(
        !invoice.contains("## Inheritance") && !invoice.contains("It extends"),
        "an ambiguous base is no base of Invoice:\n{invoice}"
    );
    assert!(
        !document.contains("## Inheritance") && !document.contains("build on it"),
        "Invoice does not build on Document through a guess:\n{document}"
    );
    let index = CardIndex::build(&cards);
    let pages = build_pages(&insight, &cards, &index, LinkStyle::Portable);
    let hierarchy = &pages
        .iter()
        .find(|p| p.path == "hierarchy.md")
        .unwrap()
        .contents;
    assert!(
        !hierarchy.contains("Invoice"),
        "the hierarchy holds no guessed inheritance:\n{hierarchy}"
    );
}

/// The bold strip under a type card's title — the header plan B, U1 reads from
/// the signature.
fn header_strip(rendered: &str) -> &str {
    rendered
        .lines()
        .find(|l| l.starts_with("**"))
        .and_then(|l| l.split(" · ").next())
        .unwrap_or_else(|| panic!("no header strip:\n{rendered}"))
}

/// Every type card's header strip, by title.
fn header_strips(cards: &[Card]) -> std::collections::BTreeMap<String, String> {
    let index = CardIndex::build(cards);
    cards
        .iter()
        .filter(|c| c.kind == CardKind::Type)
        .map(|c| {
            let out = render_card(c, Lang::En, LinkStyle::Portable, &index);
            (c.title.clone(), header_strip(&out).to_string())
        })
        .collect()
}

/// Plan B, U1 (AE3, R4, KTD6): the header says what the signature says. The C#
/// plugin keeps only the ACCESS modifiers as visibility, so a header built from
/// visibility and kind announced `public static class GeoSolver` as an ordinary
/// `public class`. Attributes before the modifiers and the bases and
/// constraints after the name are not part of the header.
#[test]
fn a_type_header_carries_the_modifiers_its_signature_states() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Head/Kinds.cs",
            "namespace Orbit.Head\n{\n    public class Base { }\n\n    \
             public static class Solver\n    {\n        public static int Solve(int n) { return n; }\n    }\n\n    \
             [Serializable] public static class Tools<T> : Base where T : new()\n    {\n    }\n\n    \
             public abstract class Shape { }\n\n    \
             internal sealed class Leaf { }\n\n    \
             public readonly struct Point { }\n}\n",
        )],
    );
    let strips = header_strips(&cards_over(tmp.path()));
    let expect = |title: &str, strip: &str| {
        assert_eq!(
            strips.get(title).map(String::as_str),
            Some(strip),
            "{title}: {strips:#?}"
        );
    };
    expect("Solver", "**public static class**");
    expect("Tools", "**public static class**");
    expect("Shape", "**public abstract class**");
    expect("Leaf", "**internal sealed class**");
    expect("Point", "**public readonly struct**");
    // The control: a signature with no modifier beyond access reads as before.
    expect("Base", "**public class**");
}

/// Plan B, U1: `sealed` on any part of a `partial` type seals the whole type,
/// so the header is the union of every part's modifiers — whichever file the
/// part that states it sorts into, and in the same bytes either way.
#[test]
fn a_partial_type_header_unions_the_modifiers_of_every_part() {
    let open = "namespace P\n{\n    public partial class Part\n    {\n        public void A() { }\n    }\n}\n";
    let sealed = "namespace P\n{\n    sealed partial class Part\n    {\n        public void B() { }\n    }\n}\n";
    for (first, second) in [(open, sealed), (sealed, open)] {
        let tmp = tempfile::tempdir().unwrap();
        init_repo_with_files(
            tmp.path(),
            "Alice",
            &[("a/Part.cs", first), ("b/Part.cs", second)],
        );
        let strips = header_strips(&cards_over(tmp.path()));
        assert_eq!(
            strips.get("Part").map(String::as_str),
            Some("**public sealed partial class**"),
            "{strips:#?}"
        );
    }
}

/// Plan B, U1 verification: the `GeoSolver` fixture, declared
/// `public static class` as the real one is, carries a static-class header.
#[test]
fn the_geo_solver_card_says_static_class() {
    let cards = geo_cards();
    assert_eq!(
        header_strip(&rendered(&cards, "GeoSolver")),
        "**public static class**"
    );
}

// ---- Plan B, U6: keywords ----

/// The words of one card's keywords, each followed by its diacritics-free
/// variant when it has one.
fn keyword_words(card: &Card) -> Vec<String> {
    card.story
        .keywords
        .iter()
        .flat_map(|k| std::iter::once(k.word.clone()).chain(k.folded.clone()))
        .collect()
}

/// `LoadAction` the way the acceptance case A2 met it: a method whose SQL writes
/// `dba.ops_schedule_tasks` and a comment about restoring a backup — beside an
/// unrelated `Invoice`, so a word of `LoadAction` is rarer than a word of both.
const LOAD_ACTION_CS: &str = "namespace Orbit.Backup\n{\n    public class LoadAction\n    {\n        \
    public void SetScheduledTasksInvalid()\n        {\n            // Při obnově zálohy\n            \
    var sql = \"update dba.ops_schedule_tasks set x = 1\";\n        }\n    }\n}\n";

/// R8, KTD4: the four sources. An identifier splits into its words, a
/// database object gives its whole name and its parts, and a comment word
/// keeps its diacritics with the plain spelling beside it.
#[test]
fn keywords_come_from_identifiers_database_objects_literals_and_comments() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/LoadAction.cs", LOAD_ACTION_CS),
            ("src/Invoice.cs", INVOICE_CS),
        ],
    );
    let cards = cards_over(tmp.path());
    let words = keyword_words(find(&cards, CardKind::Type, "LoadAction"));
    for expected in [
        "scheduled",
        "tasks",
        "invalid",
        "ops_schedule_tasks",
        "ops",
        "schedule",
        "obnově",
        "obnove",
        "zálohy",
        "zalohy",
    ] {
        assert!(
            words.iter().any(|w| w == expected),
            "{expected} in {words:?}"
        );
    }
    // Short and common words are no keywords: `set` (an identifier part and
    // SQL), `při`, `update`, `x`.
    for dropped in ["set", "při", "update", "x"] {
        assert!(
            !words.iter().any(|w| w == dropped),
            "{dropped} in {words:?}"
        );
    }
}

/// `FindAllColumns` splits into its words on the card too; `all` is a common
/// word and stays out.
#[test]
fn a_member_name_gives_its_words() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/Schema.cs",
                "namespace N\n{\n    public class Schema\n    {\n        public void FindAllColumns() { }\n    }\n}\n",
            ),
            ("src/Invoice.cs", INVOICE_CS),
        ],
    );
    let cards = cards_over(tmp.path());
    let words = keyword_words(find(&cards, CardKind::Type, "Schema"));
    assert_eq!(words, ["columns", "find", "schema"], "{words:?}");
}

/// A word the type has only in its doc comment — anchors leave doc comments
/// out — is still one of its keywords.
#[test]
fn a_word_only_in_the_doc_comment_is_a_keyword() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/Fees.cs",
                "namespace N\n{\n    /// <summary>Účtuje penále za pozdní úhradu.</summary>\n    public class Fees\n    {\n        public void Apply() { }\n    }\n}\n",
            ),
            ("src/Invoice.cs", INVOICE_CS),
        ],
    );
    let cards = cards_over(tmp.path());
    let words = keyword_words(find(&cards, CardKind::Type, "Fees"));
    assert!(words.iter().any(|w| w == "penále"), "{words:?}");
    assert!(words.iter().any(|w| w == "penale"), "{words:?}");
    assert!(!words.iter().any(|w| w == "summary"), "{words:?}");
}

/// Three types that share `refresh` three times each and have one word of
/// their own: the shared word weighs nothing, so it comes after the own one
/// although it occurs three times as often.
const REFRESHERS: [(&str, &str); 3] = [
    (
        "src/Alpha.cs",
        "namespace N\n{\n    public class Alpha\n    {\n        public void RefreshCache() { }\n        public void RefreshIndex() { }\n        public void RefreshView() { }\n    }\n}\n",
    ),
    (
        "src/Beta.cs",
        "namespace N\n{\n    public class Beta\n    {\n        public void RefreshCache() { }\n        public void RefreshIndex() { }\n        public void RefreshView() { }\n    }\n}\n",
    ),
    (
        "src/Gamma.cs",
        "namespace N\n{\n    public class Gamma\n    {\n        public void RefreshCache() { }\n        public void RefreshIndex() { }\n        public void RefreshView() { }\n    }\n}\n",
    ),
];

#[test]
fn a_word_in_every_type_does_not_rank_before_a_word_of_the_type() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &REFRESHERS);
    let cards = cards_over(tmp.path());
    for title in ["Alpha", "Beta", "Gamma"] {
        let words = keyword_words(find(&cards, CardKind::Type, title));
        let own = title.to_lowercase();
        assert_eq!(words.first(), Some(&own), "{title}: {words:?}");
        assert!(words.iter().any(|w| w == "refresh"), "{title}: {words:?}");
    }
}

fn cards_with_threads(dir: &std::path::Path, threads: usize) -> Vec<Card> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| cards_over(dir))
}

/// R15: the same corpus gives the same keywords in the same order, run after
/// run and whatever the thread count — through a scoped pool for each run, as
/// `tests/threads.rs` explains.
#[test]
fn keywords_are_the_same_across_runs_and_thread_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let mut files: Vec<(&str, &str)> = REFRESHERS.to_vec();
    files.push(("src/LoadAction.cs", LOAD_ACTION_CS));
    files.push(("src/Invoice.cs", INVOICE_CS));
    init_repo_with_files(tmp.path(), "Alice", &files);
    let all = |cards: &[Card]| -> Vec<(String, Vec<String>)> {
        cards
            .iter()
            .map(|c| (c.identity.clone(), keyword_words(c)))
            .collect()
    };
    let one = all(&cards_with_threads(tmp.path(), 1));
    assert!(one.iter().filter(|(_, w)| !w.is_empty()).count() >= 5);
    assert_eq!(one, all(&cards_with_threads(tmp.path(), 1)));
    assert_eq!(one, all(&cards_with_threads(tmp.path(), 4)));
}

/// An editor with every shape the second layer draws: a key handler, a
/// context menu whose label is quoted, a command, a parameter that chooses (a
/// flowchart) and a toggled property (a state diagram).
const EDITOR_LAYERED_CS: &str = r#"namespace Acme.Editor
{
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        public RelayCommand(Action<object> execute) { _execute = execute; }
        public void Execute(object p) { _execute(p); }
    }

    public class EditorView
    {
        private ContextMenu menu;
        private EditorViewModel viewModel;
        public string GetResourceString(string key) { return key; }
        void OnKey(object sender, KeyEventArgs e)
        {
            if (e.Key == Key.F4) { viewModel.CheckCommand.Execute(null); }
            else if (e.Key == Key.F5) { viewModel.Toggle(); }
        }
        void BuildMenu()
        {
            ContextMenuItem f4 = new ContextMenuItem();
            f4.Header = GetResourceString("EditorView_checkErrors") /*"Kontrola chyb"*/;
            f4.Command = viewModel.CheckCommand;
            menu.Items.Add(f4);
        }
    }

    public class EditorViewModel
    {
        private bool isFullscreen;
        public RelayCommand CheckCommand => new RelayCommand(p => Check());
        public void Check() { Check(false); }
        void Check(bool syntaxOnly)
        {
            if (syntaxOnly) { Parse(1); } else { Parse(2); }
        }
        void Parse(int kind) { }
        public bool IsFullscreen
        {
            get { return isFullscreen; }
            set { if (isFullscreen == value) return; isFullscreen = value; }
        }
        public void Toggle() { IsFullscreen = !IsFullscreen; }
        public void Leave() { IsFullscreen = false; }
        public void Draw() { if (IsFullscreen) { Parse(0); } }
    }
}
"#;

/// A run with the second layer at `level` over `repo`, written into `out`, on
/// a pool of `threads` workers of its own.
fn layered_with_threads(
    repo: &std::path::Path,
    out: &std::path::Path,
    level: u8,
    threads: usize,
) -> reverse_architect::Generated {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            reverse_architect::generate(&reverse_architect::config::Config {
                detail_level: Some(level),
                ..common::config_for(repo, out)
            })
            .expect("the pipeline runs")
        })
}

/// R3, KTD9 (plan 2a, U9): the second layer's cards, details, maps, views,
/// manifest and coverage are the same bytes run after run and whatever the
/// thread count — one worker, one again, four, on every level — through a
/// scoped pool for each run, as `tests/threads.rs` explains. The fixture has
/// the sections and both diagrams the layer writes, so the bytes compared are
/// the layer's.
#[test]
fn the_second_layer_is_the_same_bytes_across_runs_and_thread_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo_with_files(&repo, "Alice", &[("src/Editor.cs", EDITOR_LAYERED_CS)]);
    for level in 1..=3u8 {
        let run = |name: &str, threads: usize| {
            layered_with_threads(
                &repo,
                &tmp.path().join(format!("{level}-{name}")),
                level,
                threads,
            )
        };
        let one = run("one", 1);
        let typed: String = one
            .cards
            .iter()
            .filter(|c| c.path.starts_with("cards/type/"))
            .map(|c| c.contents.as_str())
            .collect();
        for part in ["layer2: present", "## Functionality: ", "## State: "] {
            assert!(typed.contains(part), "level {level}: no {part:?}");
        }
        // The diagrams are the layer's bytes from level 2.
        assert_eq!(
            typed.contains("flowchart TD\n"),
            level >= 2,
            "level {level}"
        );
        assert_eq!(
            typed.contains("stateDiagram-v2\n"),
            level >= 2,
            "level {level}"
        );
        for (name, threads) in [("again", 1), ("four", 4)] {
            let other = run(name, threads);
            let label = format!("level {level}, {threads} threads ({name})");
            assert_eq!(one.corpus, other.corpus, "{label}: the views differ");
            assert_eq!(one.cards, other.cards, "{label}: the cards differ");
            assert_eq!(one.details, other.details, "{label}: the details differ");
            assert_eq!(one.maps, other.maps, "{label}: the maps differ");
            assert_eq!(
                one.manifest, other.manifest,
                "{label}: the manifests differ"
            );
            assert_eq!(
                one.coverage, other.coverage,
                "{label}: the coverage differs"
            );
        }
    }
}

/// Plan 2c, U8 (KTD11, KTD12, R19): with the second layer every analyzed
/// type's card has `## Dynamic access` on level 1 and on level 3 alike —
/// after the sections of 2a, before the notes — ending in the state of XAML
/// coverage, which in a run no plugin of which reads XAML is "not read", as
/// coverage says; without the second layer no card has it and coverage has
/// no section of XAML, so those cards stay the bytes the golden snapshots
/// pin.
#[test]
fn the_second_layer_puts_dynamic_access_on_every_level_and_nothing_without_it() {
    use reverse_architect::coverage::MarkupState;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo_with_files(&repo, "Alice", &[("src/Editor.cs", EDITOR_LAYERED_CS)]);
    let not_read = "XAML was not read in this run — no plugin of the run reads it — so a member \
                    without a marker from markup may still be bound, commanded or handled from \
                    XAML (coverage.json, `markup`).";
    for level in [1u8, 3] {
        let run = layered_with_threads(&repo, &tmp.path().join(format!("out-{level}")), level, 1);
        let analyzed: Vec<&str> = run
            .cards
            .iter()
            .filter(|c| c.contents.contains("\nlayer2: present\n"))
            .filter(|c| c.contents.contains("\n## Functionality: "))
            .map(|c| c.contents.as_str())
            .collect();
        assert!(!analyzed.is_empty(), "level {level}");
        for card in analyzed {
            let at = card
                .find("\n## Dynamic access\n")
                .unwrap_or_else(|| panic!("level {level}: no Dynamic access in\n{card}"));
            let last_2a = ["\n## Functionality: ", "\n## State: "]
                .iter()
                .filter_map(|h| card.rfind(h))
                .max()
                .unwrap();
            assert!(last_2a < at, "level {level}");
            let end = card.find("\n## Notes\n").unwrap_or(card.len());
            assert!(at < end, "level {level}");
            assert!(
                card[at..end].trim_end().ends_with(not_read),
                "level {level}: {}",
                &card[at..end]
            );
        }
        let markup = run.coverage.markup.as_ref().expect("a markup section");
        assert_eq!(markup.state, MarkupState::NotRead);
        assert!(markup.plugins.is_empty() && markup.files == 0);
    }
    let plain = reverse_architect::generate(&common::config_for(&repo, &tmp.path().join("plain")))
        .expect("the pipeline runs");
    assert!(plain
        .cards
        .iter()
        .all(|c| !c.contents.contains("## Dynamic access")));
    assert_eq!(plain.coverage.markup, None);
    let json = reverse_architect::coverage::to_json(&plain.coverage);
    assert!(!json.contains("\"markup\""));
}

/// The plan's risk row, measured and not enforced: adding a type that shares
/// no word with the others changes the corpus size every rarity is computed
/// against, so it may reorder the keywords of cards it has nothing to do with.
/// The test prints how many did.
#[test]
fn adding_an_unrelated_type_changes_few_keyword_lists_measured() {
    const NAMES: [&str; 8] = [
        "Ledger", "Parcel", "Courier", "Tariff", "Voucher", "Rebate", "Mandate", "Tenant",
    ];
    let base: Vec<(String, String)> = NAMES
        .iter()
        .map(|name| {
            (
                format!("src/{name}.cs"),
                format!(
                    "namespace N\n{{\n    public class {name}\n    {{\n        public void Open{name}() {{ }}\n        public void Close{name}() {{ }}\n        public void Archive() {{ }}\n    }}\n}}\n"
                ),
            )
        })
        .collect();
    let keywords_of =
        |files: &[(String, String)]| -> std::collections::BTreeMap<String, Vec<String>> {
            let tmp = tempfile::tempdir().unwrap();
            let refs: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, s)| (p.as_str(), s.as_str()))
                .collect();
            init_repo_with_files(tmp.path(), "Alice", &refs);
            cards_over(tmp.path())
                .iter()
                .filter(|c| c.kind == CardKind::Type)
                .map(|c| (c.title.clone(), keyword_words(c)))
                .collect()
        };
    let before = keywords_of(&base);
    let mut grown = base.clone();
    grown.push((
        "src/Zebra.cs".to_string(),
        "namespace N\n{\n    public class Zebra\n    {\n        public void Gallop() { }\n    }\n}\n"
            .to_string(),
    ));
    let after = keywords_of(&grown);
    let changed: Vec<&String> = before
        .iter()
        .filter(|(title, words)| after.get(*title) != Some(*words))
        .map(|(title, _)| title)
        .collect();
    eprintln!(
        "U6 risk row: adding one unrelated type changed the keywords of {} of {} cards: {changed:?}",
        changed.len(),
        before.len()
    );
    assert_eq!(after.len(), before.len() + 1);
}

/// AE5, R10 for keywords: a connection string's password, a commented
/// password, a doc-comment password and a long token in a doc comment give no
/// word — not even a fragment the identifier split would cut out of a key —
/// while the message `Chybné heslo` keeps its words.
#[test]
fn no_withheld_text_becomes_a_keyword_and_the_message_stays() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/Login.cs",
                "namespace N\n{\n    /// <summary>Checks the checksum AbCdEf0123456789XyZ987abc of a payload.</summary>\n    \
                 /// password: mandarinkovyjogurt\n    public class Login\n    {\n        public void Connect()\n        {\n            \
                 // password=kokosovyorech\n            \
                 var cs = \"Server=db;User Id=sa;Password=borovicovyles;\";\n            \
                 var m = \"Chybné heslo\";\n        }\n    }\n}\n",
            ),
            ("src/Invoice.cs", INVOICE_CS),
        ],
    );
    let cards = cards_over(tmp.path());
    let words = keyword_words(find(&cards, CardKind::Type, "Login"));
    for kept in ["chybné", "chybne", "heslo", "checksum", "payload"] {
        assert!(words.iter().any(|w| w == kept), "{kept} in {words:?}");
    }
    for withheld in [
        "mandarinkovyjogurt",
        "kokosovyorech",
        "borovicovyles",
        "abc",
        "redacted",
        "anchor",
    ] {
        assert!(
            !words.iter().any(|w| w == withheld),
            "{withheld} in {words:?}"
        );
    }
}

/// R13: a C file has no plugin and no type, so its own card carries the words
/// of its texts — the lexer's anchors are the file's.
#[test]
fn a_c_file_card_has_keywords_from_its_texts() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/frames.c",
                "/* Parser of serial frames */\nint main(void) { const char *s = \"Neplatný rámec\"; return 0; }\n",
            ),
            ("src/Invoice.cs", INVOICE_CS),
        ],
    );
    let cards = cards_over(tmp.path());
    let words = keyword_words(find(&cards, CardKind::File, "src/frames.c"));
    for expected in ["frames", "serial", "neplatný", "neplatny", "rámec", "ramec"] {
        assert!(
            words.iter().any(|w| w == expected),
            "{expected} in {words:?}"
        );
    }
    let cap = reverse_architect::card::keywords::KEYWORD_CAP;
    assert!(cards.iter().all(|c| c.story.keywords.len() <= cap));
}

// ---- Plan B, U7: purpose ----

fn purpose_of(card: &Card, lang: Lang) -> Vec<(&'static str, String)> {
    card.story
        .purpose
        .iter()
        .map(|s| (s.origin.token(), s.text(lang)))
        .collect()
}

/// R5: a doc comment's first sentence leads, said to come from the comment.
#[test]
fn a_documented_type_leads_its_purpose_with_the_comment() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);
    let cards = cards_over(tmp.path());
    let purpose = purpose_of(find(&cards, CardKind::Type, "Invoice"), Lang::En);
    assert_eq!(
        purpose,
        [("comment", "Faktura vydaná odběrateli.".to_string())],
        "an instance class with one comment: the comment and nothing invented"
    );
}

/// AE4 (R6): no doc comment and no stereotype — the kind and the members,
/// from the structure, and no sentence beyond them.
#[test]
fn an_undocumented_type_without_a_stereotype_gets_only_its_structure() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/Ledger.cs",
            "namespace N\n{\n    public class Ledger\n    {\n        public void Post() { }\n        private int Balance() { return 0; }\n        public int Count { get; set; }\n    }\n}\n",
        )],
    );
    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::Type, "Ledger");
    assert_eq!(
        purpose_of(card, Lang::En),
        [("structure", "`class`: 2 methods, 1 property.".to_string())]
    );
    assert_eq!(
        purpose_of(card, Lang::Cs),
        [("structure", "`class`: 2 metody, 1 vlastnost.".to_string())]
    );
}

const ACTION_CS: &str = "namespace Orbit.Backup\n{\n    public interface IAction\n    {\n        void Run();\n    }\n\n    \
    public class RestoreAction : IAction\n    {\n        public void Run()\n        {\n            \
    var sql = \"update dba.t set x = 1\";\n        }\n    }\n}\n";

/// R5, R14: implementing `IAction` and writing `dba.t` make one sentence with
/// both facts, from the structure — in English and, with the same token, in
/// Czech.
#[test]
fn a_base_and_a_database_write_make_one_structural_sentence_in_either_language() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/Restore.cs", ACTION_CS)]);
    let cards = cards_over(tmp.path());
    let card = find(&cards, CardKind::Type, "RestoreAction");
    assert_eq!(
        purpose_of(card, Lang::En),
        [
            ("structure", "`class`: 1 method.".to_string()),
            (
                "structure",
                "Implements `IAction`; writes `dba.t`.".to_string()
            ),
        ]
    );
    assert_eq!(
        purpose_of(card, Lang::Cs),
        [
            ("structure", "`class`: 1 metoda.".to_string()),
            (
                "structure",
                "Implementuje `IAction`; zapisuje do `dba.t`.".to_string()
            ),
        ]
    );
}

/// KTD5: the stereotype table — data structure, static helper, factory and
/// entry point — each read from facts, never from a name.
#[test]
fn the_stereotypes_are_read_from_the_members() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/Shapes.cs",
            "namespace N\n{\n    \
             public class Dto\n    {\n        public int Id { get; set; }\n        public string Name;\n    }\n    \
             public static class Util\n    {\n        public static int Twice(int x) { return x * 2; }\n    }\n    \
             public class Money\n    {\n        public static Money From(decimal amount) { return new Money(); }\n        public decimal Round() { return 0; }\n    }\n    \
             public class Program\n    {\n        public static void Main(string[] args) { }\n    }\n}\n",
        )],
    );
    let cards = cards_over(tmp.path());
    let first = |title: &str| purpose_of(find(&cards, CardKind::Type, title), Lang::En).remove(0);
    assert_eq!(
        first("Dto"),
        (
            "structure",
            "Data structure: 1 property, 1 field; no methods.".to_string()
        )
    );
    assert_eq!(
        first("Util"),
        (
            "structure",
            "Static helper: static members only — 1 method.".to_string()
        )
    );
    assert_eq!(
        first("Money"),
        (
            "structure",
            "Creates its own instances through the static method `From`.".to_string()
        )
    );
    assert_eq!(
        first("Program"),
        (
            "structure",
            "Entry point of the program: `Main`.".to_string()
        )
    );
    // Only a type has a purpose; a file card's stays empty.
    assert!(find(&cards, CardKind::File, "src/Shapes.cs")
        .story
        .purpose
        .is_empty());
}

/// KTD5, R6: a factory creates instances of ITS OWN type. A static method
/// returning a namesake of another namespace (`Other.Foo` in `N.Foo`) does
/// not, while one returning the type qualified by its own namespace does.
#[test]
fn a_namesake_of_another_namespace_makes_no_factory() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/N.cs",
                "namespace N\n{\n    \
                 public class Foo\n    {\n        public static Other.Foo Get() { return null; }\n        public decimal Round() { return 0; }\n    }\n    \
                 public class Bar\n    {\n        public static N.Bar Make() { return new Bar(); }\n        public decimal Round() { return 0; }\n    }\n}\n",
            ),
            (
                "src/Other.cs",
                "namespace Other\n{\n    public class Foo\n    {\n        public decimal Round() { return 0; }\n    }\n}\n",
            ),
        ],
    );
    let cards = cards_over(tmp.path());
    let by_key = |key: &str| {
        let card = cards
            .iter()
            .find(|c| c.kind == CardKind::Type && c.identity == key)
            .unwrap_or_else(|| panic!("no card {key}"));
        purpose_of(card, Lang::En)
    };
    let foo = by_key("sym:csharp:N#Foo");
    assert!(
        !foo.iter()
            .any(|(_, text)| text.starts_with("Creates its own instances")),
        "`Other.Foo Get()` creates no `N.Foo`: {foo:?}"
    );
    assert_eq!(
        by_key("sym:csharp:N#Bar").remove(0),
        (
            "structure",
            "Creates its own instances through the static method `Make`.".to_string()
        )
    );
}

/// KTD5, R6: `global::` makes a return type absolute. `global::Foo` inside
/// `N.Foo` is the root-level `Foo`, a different type — no factory — while
/// in the root-level `Foo` it is that very type, and `global::N.Bar` inside
/// `N.Bar` is `N.Bar`.
#[test]
fn a_global_return_type_is_the_type_it_spells_from_the_root() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/N.cs",
                "namespace N\n{\n    \
                 public class Foo\n    {\n        public static global::Foo Get() { return null; }\n        public decimal Round() { return 0; }\n    }\n    \
                 public class Bar\n    {\n        public static global::N.Bar Make() { return new Bar(); }\n        public decimal Round() { return 0; }\n    }\n}\n",
            ),
            (
                "src/Foo.cs",
                "public class Foo\n{\n    public static global::Foo Create() { return new Foo(); }\n    public decimal Round() { return 0; }\n}\n",
            ),
        ],
    );
    let cards = cards_over(tmp.path());
    let by_key = |key: &str| {
        let card = cards
            .iter()
            .find(|c| c.kind == CardKind::Type && c.identity == key)
            .unwrap_or_else(|| panic!("no card {key}"));
        purpose_of(card, Lang::En)
    };
    let foo = by_key("sym:csharp:N#Foo");
    assert!(
        !foo.iter()
            .any(|(_, text)| text.starts_with("Creates its own instances")),
        "`global::Foo Get()` creates the root-level `Foo`, not `N.Foo`: {foo:?}"
    );
    assert_eq!(
        by_key("sym:csharp:N#Bar").remove(0),
        (
            "structure",
            "Creates its own instances through the static method `Make`.".to_string()
        )
    );
    assert_eq!(
        // A type outside every namespace is keyed under its file.
        by_key("sym:csharp:src/Foo#Foo").remove(0),
        (
            "structure",
            "Creates its own instances through the static method `Create`.".to_string()
        )
    );
}

/// KTD5, R6: in Java `static` on a nested class only says it is no inner
/// class — its members are what they say. A builder with instance methods is
/// no static helper; only a class whose members all state `static` is.
#[test]
fn a_java_static_nested_class_is_no_static_helper_by_its_modifier() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "src/Order.java",
            "package shop;\n\npublic class Order {\n    \
             public static class Builder {\n        \
             public Builder name(String n) { return this; }\n        \
             public Order build() { return null; }\n    }\n\n    \
             public static class Util {\n        \
             public static int twice(int x) { return x * 2; }\n    }\n}\n",
        )],
    );
    let cards = cards_over(tmp.path());
    let first = |title: &str| purpose_of(find(&cards, CardKind::Type, title), Lang::En).remove(0);
    let builder = first("Builder");
    assert!(
        !builder.1.starts_with("Static helper"),
        "a builder of instance methods: {builder:?}"
    );
    assert_eq!(
        first("Util"),
        (
            "structure",
            "Static helper: static members only — 1 method.".to_string()
        )
    );
}

// ---- Plan B, U8: the detail ----

use reverse_architect::card::detail::{render_detail_files, DETAIL_PART_BYTES};
use reverse_architect::card::links::relative;
use std::collections::BTreeMap;

fn cards_of(files: &[(&str, &str)]) -> Vec<Card> {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", files);
    cards_over(tmp.path())
}

/// Every detail of a card set as rendered, by path — each path once.
fn details(cards: &[Card], lang: Lang) -> BTreeMap<String, String> {
    let mut by_path = BTreeMap::new();
    for file in render_detail_files(cards, &CardIndex::build(cards), lang, LinkStyle::Portable) {
        let path = file.path.clone();
        assert!(
            by_path.insert(file.path, file.contents).is_none(),
            "two details at {path}"
        );
    }
    by_path
}

fn detail_of<'a>(
    cards: &[Card],
    details: &'a BTreeMap<String, String>,
    kind: CardKind,
    title: &str,
) -> &'a str {
    let card = find(cards, kind, title);
    let path = card
        .detail
        .as_deref()
        .unwrap_or_else(|| panic!("{title} has no detail"));
    details
        .get(path)
        .unwrap_or_else(|| panic!("{title}'s detail {path} was not rendered"))
}

/// The body of one `##` section of an unsplit detail.
fn section<'a>(detail: &'a str, name: &str) -> &'a str {
    let start = detail
        .find(&format!("\n## {name}\n"))
        .unwrap_or_else(|| panic!("no {name} section:\n{detail}"))
        + name.len()
        + 5;
    let end = detail[start..]
        .find("\n## ")
        .map_or(detail.len(), |n| start + n);
    &detail[start..end]
}

fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in the source"))
        + 1
}

/// KTD7: a C file has no plugin, so the query in its literal belongs to the
/// file — and the file's detail carries it, under `Anchors` and `Database`
/// alone, with the line. The file card names the detail in one line, and the
/// link leads to the detail that is rendered.
#[test]
fn a_c_file_with_sql_in_a_literal_has_a_file_detail_its_card_links_to() {
    let c = "#include \"q.h\"\n\nvoid run(void) {\n    const char* q = \"update dba.t set x=1\";\n    exec_sql(q);\n}\n";
    let cards = cards_of(&[("native/q.c", c)]);
    let details = details(&cards, Lang::En);
    let card = find(&cards, CardKind::File, "native/q.c");
    let path = card.detail.as_deref().expect("a file detail");
    assert_eq!(path, "cards/detail/file/native/q.c.md");
    let detail = &details[path];

    let headings: Vec<&str> = detail.lines().filter(|l| l.starts_with("## ")).collect();
    assert_eq!(headings, ["## Anchors", "## Database"], "{detail}");
    assert!(
        section(detail, "Database").contains(
            "- the file (no declaration read around it)\n  - writes `dba.t` — `native/q.c:4`\n"
        ),
        "{detail}"
    );
    assert!(
        section(detail, "Anchors").contains("- `update dba.t set x=1` · `native/q.c:4`\n"),
        "{detail}"
    );

    let rendered = render_card(
        card,
        Lang::En,
        LinkStyle::Portable,
        &CardIndex::build(&cards),
    );
    let line = format!(
        "- Detail: [anchors and database]({})\n",
        relative(&card.path, path)
    );
    assert!(rendered.contains(&line), "{rendered}");
    assert_eq!(rendered.matches("- Detail:").count(), 1, "{rendered}");
}

/// KTD7: a C# file whose every text sits inside a declaration owns none of
/// its own, so it has no detail and its card names none.
#[test]
fn a_csharp_file_without_texts_outside_declarations_has_no_file_detail() {
    let cards = cards_of(&[("src/Invoice.cs", INVOICE_CS)]);
    let card = find(&cards, CardKind::File, "src/Invoice.cs");
    assert_eq!(card.detail, None);
    let rendered = render_card(
        card,
        Lang::En,
        LinkStyle::Portable,
        &CardIndex::build(&cards),
    );
    assert!(!rendered.contains("- Detail:"), "{rendered}");
    assert!(details(&cards, Lang::En)
        .keys()
        .all(|p| !p.starts_with("cards/detail/file/")));
}

/// AE9 (R11): a method that calls a procedure and reads a table is listed
/// with the call and the read, each with its line — and no write.
#[test]
fn a_procedure_call_and_a_read_are_listed_per_method_with_no_write() {
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        Exec(\"exec dba.proc\");\n        var rows = Query(\"select y from dba.x\");\n    }\n}\n";
    let cards = cards_of(&[("Repo.cs", src)]);
    let details = details(&cards, Lang::En);
    let database = section(
        detail_of(&cards, &details, CardKind::Type, "Rep"),
        "Database",
    );
    let expected = format!(
        "- `Run()`\n  - reads `dba.x` — `Repo.cs:{}`\n  - calls `dba.proc` — `Repo.cs:{}`\n",
        line_of(src, "select y"),
        line_of(src, "exec dba.proc")
    );
    assert!(database.contains(&expected), "{database}");
    let accesses: Vec<&str> = database.lines().filter(|l| l.starts_with("  - ")).collect();
    assert_eq!(accesses.len(), 2, "{database}");
    assert!(accesses.iter().all(|l| !l.contains("writes")), "{database}");
}

/// AE1 shape: the method named in the message leads to the table it writes
/// and the line.
#[test]
fn a_method_writing_a_table_leads_to_the_table_and_the_line() {
    let src = "namespace Orbit.Actions.Backup;\n\npublic class LoadAction\n{\n    public void SetScheduledTasksInvalid()\n    {\n        Execute(\"update dba.ops_schedule_tasks set invalid = 1\");\n    }\n}\n";
    let cards = cards_of(&[("Backup/LoadAction.cs", src)]);
    let details = details(&cards, Lang::En);
    let database = section(
        detail_of(&cards, &details, CardKind::Type, "LoadAction"),
        "Database",
    );
    let expected = format!(
        "- `SetScheduledTasksInvalid()`\n  - writes `dba.ops_schedule_tasks` — `Backup/LoadAction.cs:{}`\n",
        line_of(src, "update dba.ops_schedule_tasks")
    );
    assert!(database.contains(&expected), "{database}");
}

const BACKUP_LOAD_TYPE: &str = "namespace Orbit.Actions.Backup;\n\npublic enum BackupLoadType\n{\n    Never = 0,\n    Full = 10,\n    Diff = 20,\n}\n";
const LOAD_ACTION: &str = "namespace Orbit.Actions.Backup;

public class LoadAction
{
    public void Run()
    {
        var t = Load();
        switch (t)
        {
            case BackupLoadType.Full:
                break;
            default:
                break;
        }
    }

    public int Weight()
    {
        var t = Load();
        return t switch
        {
            BackupLoadType.Diff => 2,
            _ => 0,
        };
    }

    public bool Changed()
    {
        var t = Load();
        return t == BackupLoadType.Never;
    }
}
";

/// AE6 (R12): the ENUM's detail says where its values decide, with the
/// lines — the verified `case` label and switch arm, then the comparison the
/// host cannot verify, apart and marked. The deciding type's own detail
/// carries no decision section: the decisions are grouped by the enum.
#[test]
fn an_enums_detail_lists_where_its_values_decide() {
    let cards = cards_of(&[
        ("Backup/BackupLoadType.cs", BACKUP_LOAD_TYPE),
        ("Backup/LoadAction.cs", LOAD_ACTION),
    ]);
    let details = details(&cards, Lang::En);
    let decisions = section(
        detail_of(&cards, &details, CardKind::Type, "BackupLoadType"),
        "Enum decisions",
    );
    let at = |needle: &str| format!("`Backup/LoadAction.cs:{}`", line_of(LOAD_ACTION, needle));
    let items: Vec<&str> = decisions.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(items.len(), 3, "{decisions}");
    assert!(
        items[0].starts_with("- `Diff` — switch arm in [LoadAction.Weight()]("),
        "{decisions}"
    );
    assert!(
        items[0].ends_with(&format!(" — {}", at("BackupLoadType.Diff =>"))),
        "{decisions}"
    );
    assert!(
        items[1].starts_with("- `Full` — case label in [LoadAction.Run()]("),
        "{decisions}"
    );
    assert!(
        items[1].ends_with(&format!(" — {}", at("case BackupLoadType.Full"))),
        "{decisions}"
    );
    assert!(
        items[2].starts_with("- `Never` — comparison in [LoadAction.Changed()]("),
        "{decisions}"
    );
    assert!(
        items[2].ends_with(&format!(
            " — {} (unverified)",
            at("== BackupLoadType.Never")
        )),
        "{decisions}"
    );
    assert!(!detail_of(&cards, &details, CardKind::Type, "LoadAction").contains("Enum decisions"));
}

/// R9, R10: a text leads to its member and line; a withheld one shows only
/// its place and the reason, never the value. The prose follows `--lang`,
/// the headings stay English.
#[test]
fn an_anchor_leads_to_its_member_and_a_withheld_one_keeps_only_place_and_reason() {
    let src = "namespace N\n{\n    public class Login\n    {\n        public string Check()\n        {\n            // password: hunter2-in-comment\n            return \"Chybné heslo\";\n        }\n    }\n}\n";
    let cards = cards_of(&[("src/Login.cs", src)]);
    for lang in [Lang::En, Lang::Cs] {
        let details = details(&cards, lang);
        let detail = detail_of(&cards, &details, CardKind::Type, "Login");
        assert!(!detail.contains("hunter2"), "{detail}");
        let anchors = section(detail, "Anchors");
        assert!(
            anchors.contains(&format!(
                "- `Chybné heslo` → `Check()` · `src/Login.cs:{}`\n",
                line_of(src, "Chybné heslo")
            )),
            "{anchors}"
        );
        let withheld = if lang == Lang::Cs {
            "vynecháno"
        } else {
            "withheld"
        };
        assert!(
            anchors.contains(&format!(
                "- {withheld}: password → `Check()` · `src/Login.cs:{}`\n",
                line_of(src, "password:")
            )),
            "{anchors}"
        );
        let lead = if lang == Lang::Cs {
            "Texty, podle kterých"
        } else {
            "Texts a reader may"
        };
        assert!(anchors.contains(lead), "{anchors}");
    }
}

/// R3: a type with nothing its card does not say — no texts, accesses,
/// decisions, members, or relations beyond what the card names — has no
/// detail. One public method is enough for one: the API left the short card
/// for the detail (R17).
#[test]
fn a_detail_exists_only_when_it_says_more_than_the_card() {
    let cards = cards_of(&[(
        "src/Shapes.cs",
        "namespace N\n{\n    public class Empty\n    {\n    }\n    public class One\n    {\n        public void Go() { }\n    }\n}\n",
    )]);
    assert_eq!(find(&cards, CardKind::Type, "Empty").detail, None);
    // A signature and a test pairing earn no detail (card trim): a type
    // without one shows neither, its header line says what the signature
    // would.
    let empty = type_card_text(&cards, "Empty", Lang::En);
    assert!(!empty.contains("## Signature"), "{empty}");
    assert!(!empty.contains("## Test pairing"), "{empty}");
    assert!(empty.contains("**public class**"), "{empty}");
    let details = details(&cards, Lang::En);
    let one = detail_of(&cards, &details, CardKind::Type, "One");
    let headings: Vec<&str> = one.lines().filter(|l| l.starts_with("## ")).collect();
    // The API earned the detail; the full lists, the signature and the test
    // pairing come with it.
    assert_eq!(
        headings,
        [
            "## Signature",
            "## API",
            "## Referenced by",
            "## Test pairing"
        ],
        "{one}"
    );
    assert!(section(one, "API").contains("Go"), "{one}");
    // Every card that names a detail has it rendered, and nothing else is.
    let named: std::collections::BTreeSet<&str> =
        cards.iter().filter_map(|c| c.detail.as_deref()).collect();
    assert_eq!(named, details.keys().map(String::as_str).collect());
}

/// A static type calling a hundred and four others, one method each — each
/// of its own name: a hundred and four `Run` declarations would be past the
/// ambiguity ceiling, and no relation at all.
fn hub_files() -> Vec<(String, String)> {
    let mut hub = String::from(
        "namespace Orbit.Geo\n{\n    public static class GeoSolver\n    {\n        public static void Solve()\n        {\n",
    );
    let mut parts = String::from("namespace Orbit.Geo\n{\n");
    for i in 0..104 {
        hub.push_str(&format!("            Part{i:03}.Run{i:03}();\n"));
        parts.push_str(&format!(
            "    public static class Part{i:03}\n    {{\n        public static void Run{i:03}() {{ }}\n    }}\n"
        ));
    }
    hub.push_str("        }\n    }\n}\n");
    parts.push_str("}\n");
    vec![
        ("Geo/GeoSolver.cs".into(), hub),
        ("Geo/Parts.cs".into(), parts),
    ]
}

fn hub_detail() -> String {
    let files = hub_files();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let cards = cards_of(&files);
    let details = details(&cards, Lang::En);
    detail_of(&cards, &details, CardKind::Type, "GeoSolver").to_string()
}

/// AE2 (R3) and KTD8: every one of the hundred and four relations is in the
/// detail, each with the rung it stands on; the detail is past the threshold,
/// so it is cut into parts between rows — no part over the threshold unless
/// it is one row alone, every row exactly once, and the same parts, in the
/// same order, from a second run over the same source.
#[test]
fn a_type_with_a_hundred_relations_has_every_one_in_its_detail_in_stable_parts() {
    let detail = hub_detail();
    let calls: Vec<&str> = detail
        .lines()
        .filter(|l| l.starts_with("- calls "))
        .collect();
    assert_eq!(calls.len(), 104, "{detail}");
    let distinct: std::collections::BTreeSet<&str> = calls.iter().copied().collect();
    assert_eq!(distinct.len(), 104, "every row exactly once");
    for i in 0..104 {
        assert!(
            calls
                .iter()
                .any(|l| l.contains(&format!("`Part{i:03}.Run{i:03}()`"))),
            "Part{i:03} missing"
        );
    }
    for line in &calls {
        assert!(
            ["rules-backed)", "name only)", "resolved by index)"]
                .iter()
                .any(|rung| line.contains(rung)),
            "a row without its rung: {line}"
        );
    }

    let header = detail
        .lines()
        .find(|l| l.starts_with("**"))
        .expect("a one-line header");
    let parts: Vec<&str> = detail.split("\n## Part ").skip(1).collect();
    assert!(parts.len() > 1, "{detail}");
    assert!(!detail.contains("\n## Relations\n"), "{detail}");
    for (n, part) in parts.iter().enumerate() {
        assert!(part.starts_with(&format!("{}\n", n + 1)), "{part}");
        let rows = part.lines().filter(|l| l.starts_with("- ")).count();
        let weight = header.len() + 1 + "\n## Part ".len() + part.len();
        assert!(
            weight <= DETAIL_PART_BYTES || rows == 1,
            "part {} weighs {weight} with {rows} rows",
            n + 1
        );
    }
    assert_eq!(detail, hub_detail(), "a second run cuts the same parts");
}

// ---- Plan B, U9: the short card ----

/// The byte budget of an ordinary short type card (plan B, Success Criteria:
/// median at most 1.2 kB on the reference C# repository; KTD13 guards it in bytes, because no
/// tokenizer is at hand and Czech tokenizes worse than English). It holds for
/// a run without the second layer, which every test here is: a C# type's card
/// with the second layer carries what its code does and has no byte budget
/// (plan 2a, Key Decisions; the measurement of step 0 watches its size).
const SHORT_CARD_BYTES: usize = 1200;
/// The budget of the `GeoSolver`-shaped card: 40 kB before plan B, under 4 kB
/// after it (Success Criteria).
const HUB_CARD_BYTES: usize = 4 * 1024;

/// Every section a short type card no longer carries (R17, plan B Key
/// Decisions on the short card and on "maybe also") — its detail does. The
/// signature and the test pairing left last (card trim to the 1.2 kB median):
/// the header line already states the kind and the modifiers the signature
/// does (R4), and the pairing was on almost every card, mostly to say that no
/// test was found.
const LEFT_FOR_THE_DETAIL: [&str; 14] = [
    "## Signature",
    "## Test pairing",
    "## API",
    "## Internals",
    "## Collaboration",
    "```mermaid",
    "## Works with",
    "## Used by",
    "## Inheritance",
    "## Tests",
    "## Relations",
    "## Referenced by",
    "## Maybe also",
    "## External usage",
];

fn type_card_text(cards: &[Card], title: &str, lang: Lang) -> String {
    render_card(
        find(cards, CardKind::Type, title),
        lang,
        LinkStyle::Portable,
        &CardIndex::build(cards),
    )
}

/// The cards of the `GeoSolver`-shaped hub: a static type calling a hundred
/// and four others.
fn hub_cards() -> Vec<Card> {
    let files = hub_files();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    cards_of(&files)
}

/// R2, R17: the short card is its header, its purpose with the provenance of
/// every sentence, the role sentences, the keywords with their plain spelling
/// beside them, and the way to its detail — and nothing the detail carries.
#[test]
fn a_short_type_card_holds_purpose_role_keywords_and_the_way_to_its_detail() {
    let cards = cards_of(&[
        ("src/LoadAction.cs", LOAD_ACTION_CS),
        ("src/Invoice.cs", INVOICE_CS),
    ]);
    let card = find(&cards, CardKind::Type, "LoadAction");
    let text = type_card_text(&cards, "LoadAction", Lang::En);

    assert!(
        !text.contains("A type this repository declares."),
        "the sentence every type card repeated is gone (R17):\n{text}"
    );
    for gone in LEFT_FOR_THE_DETAIL {
        assert!(!text.contains(gone), "{gone} on the short card:\n{text}");
    }
    assert!(
        text.contains(
            "\n## Purpose\n\n`class`: 1 method. (from the structure)\n\
             Writes `dba.ops_schedule_tasks`. (from the structure)\n"
        ),
        "{text}"
    );
    let keywords = text
        .split("\n## Keywords\n\n")
        .nth(1)
        .and_then(|rest| rest.lines().next())
        .unwrap_or_else(|| panic!("no keywords line:\n{text}"));
    for word in ["ops_schedule_tasks", "obnově (obnove)", "zálohy (zalohy)"] {
        assert!(keywords.contains(word), "{word} in {keywords}");
    }
    let detail = card.detail.as_deref().expect("LoadAction has a detail");
    assert!(
        text.contains(&format!(
            "- Detail: [detail]({})\n",
            relative(&card.path, detail)
        )),
        "{text}"
    );
    // The order the plan gives: header, purpose, relations, keywords.
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("no {needle}:\n{text}"))
    };
    assert!(at("**public class**") < at("## Purpose"), "{text}");
    assert!(at("## Purpose") < at("## Role"), "{text}");
    assert!(at("## Role") < at("## Keywords"), "{text}");
}

/// AE2 (R2, R3): the 104-relation solver's card lists no relation, names the
/// three it leads with and says how many more its detail lists — the number
/// the detail's rows add up to beyond the three, so the card and the detail
/// agree — and the detail has every one with its rung
/// (`a_type_with_a_hundred_relations_has_every_one_in_its_detail_in_stable_parts`).
/// AE3 on the same card: a static class says so.
#[test]
fn the_hub_card_counts_what_its_detail_lists_and_stays_under_four_kilobytes() {
    let cards = hub_cards();
    let text = type_card_text(&cards, "GeoSolver", Lang::En);

    assert!(text.len() < HUB_CARD_BYTES, "{} bytes:\n{text}", text.len());
    assert!(text.contains("**public static class**"), "{text}");
    for gone in LEFT_FOR_THE_DETAIL {
        assert!(!text.contains(gone), "{gone} on the short card:\n{text}");
    }
    assert!(
        !text.lines().any(|l| l.starts_with("- calls ")),
        "no relation row on the card:\n{text}"
    );
    let named: Vec<String> = (0..104)
        .map(|i| format!("Part{i:03}"))
        .filter(|part| text.contains(&format!("[{part}](")))
        .collect();
    assert_eq!(named.len(), 3, "the role names three: {named:?}\n{text}");
    assert!(
        text.contains("The detail lists 101 more neighbours.\n"),
        "{text}"
    );

    let details = details(&cards, Lang::En);
    let detail = detail_of(&cards, &details, CardKind::Type, "GeoSolver");
    let listed: std::collections::BTreeSet<String> = detail
        .lines()
        .filter(|l| l.starts_with("- calls "))
        .filter_map(|l| l.split('`').nth(1))
        .filter_map(|label| label.split('.').next())
        .map(str::to_string)
        .collect();
    assert!(named.iter().all(|n| listed.contains(n)), "{listed:?}");
    assert_eq!(
        listed.len() - named.len(),
        101,
        "the card's count is what the detail lists beyond the named"
    );
}

/// The byte budget on ordinary types: every type card of the card fixtures —
/// a documented class, a writer of a table, an implementation, the two halves
/// of the `GeoSolver` shape with a "maybe also" claim each, the rung
/// fixture's guesses — stays within 1.2 kB, in either language.
#[test]
fn an_ordinary_short_type_card_stays_within_its_byte_budget() {
    let sets = [
        cards_of(&[
            ("src/LoadAction.cs", LOAD_ACTION_CS),
            ("src/Invoice.cs", INVOICE_CS),
            ("src/Restore.cs", ACTION_CS),
        ]),
        geo_cards(),
        build_cards(&rung_fixture()),
    ];
    let mut seen = 0;
    for cards in &sets {
        let index = CardIndex::build(cards);
        for card in cards.iter().filter(|c| c.kind == CardKind::Type) {
            for lang in [Lang::En, Lang::Cs] {
                let text = render_card(card, lang, LinkStyle::Portable, &index);
                assert!(
                    text.len() <= SHORT_CARD_BYTES,
                    "{} ({lang:?}) is {} bytes:\n{text}",
                    card.title,
                    text.len()
                );
            }
            seen += 1;
        }
    }
    assert!(
        seen >= 8,
        "the fixtures must give several type cards: {seen}"
    );
}

/// AE8 (R14): under `cs` the purpose's provenance, the role and the count
/// read Czech, while every heading and bullet key stays the English skeleton.
#[test]
fn a_czech_short_card_keeps_english_headings() {
    let cards = cards_of(&[
        ("src/LoadAction.cs", LOAD_ACTION_CS),
        ("src/Invoice.cs", INVOICE_CS),
    ]);
    let cs = type_card_text(&cards, "LoadAction", Lang::Cs);
    let en = type_card_text(&cards, "LoadAction", Lang::En);
    assert!(
        cs.contains("Zapisuje do `dba.ops_schedule_tasks`. (ze struktury)"),
        "{cs}"
    );
    assert!(
        cs.contains("V tomto repozitáři nebylo vidět, že by `LoadAction` používal"),
        "{cs}"
    );
    let invoice = type_card_text(&cards, "Invoice", Lang::Cs);
    assert!(
        invoice.contains("Faktura vydaná odběrateli. (z komentáře)"),
        "{invoice}"
    );
    assert!(invoice.contains("- Detail: [detail]("), "{invoice}");
    for skeleton in [
        "- Identity:",
        "- Detail:",
        "\n## Purpose\n",
        "\n## Role\n",
        "\n## Keywords\n",
    ] {
        assert!(en.contains(skeleton), "{skeleton}:\n{en}");
        assert!(cs.contains(skeleton), "{skeleton}:\n{cs}");
    }
    assert!(!cs.contains("(from the"), "{cs}");

    let hub = hub_cards();
    let text = type_card_text(&hub, "GeoSolver", Lang::Cs);
    assert!(
        text.contains("Detail uvádí ještě 101 dalších sousedů.\n"),
        "{text}"
    );
}

/// Plan A, R7 on the short card: a type reached only through an ambiguous
/// name match is named nowhere on the card, and the count says how many such
/// neighbours the detail holds; the detail names them under `Maybe also`.
#[test]
fn a_guessed_neighbour_is_only_counted_on_the_card_and_named_in_the_detail() {
    let cards = build_cards(&rung_fixture());
    let text = type_card_text(&cards, "Invoice", Lang::En);
    assert!(
        !text.contains("Ledger") && !text.contains("Journal"),
        "a guess on the short card:\n{text}"
    );
    assert!(text.contains("[Mailer]("), "{text}");
    assert!(
        text.contains("The detail lists 2 more neighbours (all uncertain).\n"),
        "{text}"
    );
    let details = details(&cards, Lang::En);
    let maybe = section(
        detail_of(&cards, &details, CardKind::Type, "Invoice"),
        "Maybe also",
    );
    assert!(
        maybe.contains("Ledger.Post()") && maybe.contains("Journal.Post()"),
        "{maybe}"
    );

    let cs = type_card_text(&cards, "Invoice", Lang::Cs);
    assert!(
        cs.contains("Detail uvádí ještě 2 další sousedy (všichni nejistí).\n"),
        "{cs}"
    );
}

// ---- Plan B, card trim: the signature, the test pairing and the header ----

/// The card trim to the 1.2 kB median: the signature and the test pairing
/// are the detail's, word for word as the card used to carry them, and the
/// short card has neither. The header line keeps saying what the signature
/// says (R4).
#[test]
fn the_signature_and_the_test_pairing_are_in_the_detail_not_on_the_card() {
    let cards = cards_of(&[
        ("src/Billing/Backup/LoadAction.cs", LOAD_ACTION_CS),
        ("src/Billing/Invoice.cs", INVOICE_CS),
    ]);
    for lang in [Lang::En, Lang::Cs] {
        let card = type_card_text(&cards, "LoadAction", lang);
        assert!(!card.contains("## Signature"), "{card}");
        assert!(!card.contains("## Test pairing"), "{card}");
        assert!(card.contains("**public class**"), "{card}");

        let rendered = details(&cards, lang);
        let detail = detail_of(&cards, &rendered, CardKind::Type, "LoadAction");
        assert_eq!(
            section(detail, "Signature"),
            "\n```text\npublic class LoadAction\n```\n",
            "{detail}"
        );
        assert_eq!(
            section(detail, "Test pairing"),
            "\n- test by convention not found — for the file \
             `src/Billing/Backup/LoadAction.cs` (derived)\n",
            "{detail}"
        );
        // Where they stand: the signature right under the header, before the
        // texts and relations; the pairing last but the notes.
        let at = |needle: &str| {
            detail
                .find(needle)
                .unwrap_or_else(|| panic!("no {needle}:\n{detail}"))
        };
        assert!(at("\n## Signature\n") < at("\n## Anchors\n"), "{detail}");
        assert!(
            at("\n## Referenced by\n") < at("\n## Test pairing\n"),
            "{detail}"
        );
    }
}

/// The header line names the module by its last segment; the link still
/// leads to the module's card by its full path, and the source anchor beside
/// it carries the full path in any case. The detail's header is the same strip.
#[test]
fn the_header_names_the_module_by_its_last_segment_and_links_its_full_path() {
    let cards = cards_of(&[
        ("src/Billing/Backup/LoadAction.cs", LOAD_ACTION_CS),
        ("src/Billing/Invoice.cs", INVOICE_CS),
    ]);
    let en = type_card_text(&cards, "LoadAction", Lang::En);
    assert!(
        en.contains(
            "**public class** · in [Backup](../../../module/src/Billing/Backup.md) · 8 lines · \
             `src/Billing/Backup/LoadAction.cs:3-10`\n"
        ),
        "{en}"
    );
    let cs = type_card_text(&cards, "LoadAction", Lang::Cs);
    assert!(
        cs.contains("· v [Backup](../../../module/src/Billing/Backup.md) ·"),
        "{cs}"
    );
    let rendered = details(&cards, Lang::En);
    let detail = detail_of(&cards, &rendered, CardKind::Type, "LoadAction");
    assert!(
        detail.contains("· in [Backup](../../../../module/src/Billing/Backup.md) ·"),
        "{detail}"
    );
}

// ---- Plan B, card trim: the comment and the evidence notes go to the detail ----

/// The card trim: a documented type's short card quotes no comment and lists
/// no caveat about what its detail shows; the detail opens with the whole
/// comment and ends with those caveats, word for word.
#[test]
fn the_short_card_leaves_the_comment_and_the_evidence_notes_to_the_detail() {
    let cards = cards_of(&[("src/Invoice.cs", INVOICE_CS)]);
    let card = type_card_text(&cards, "Invoice", Lang::En);
    assert!(!card.contains("## Documentation"), "{card}");
    assert!(!card.contains("<summary>"), "{card}");
    let rendered = details(&cards, Lang::En);
    let detail = detail_of(&cards, &rendered, CardKind::Type, "Invoice");
    assert_eq!(
        section(detail, "Documentation"),
        "\n> <summary>Faktura vydaná odběrateli.</summary>\n\n— `src/Invoice.cs:3`\n",
        "{detail}"
    );
    let headings: Vec<&str> = detail.lines().filter(|l| l.starts_with("## ")).collect();
    assert_eq!(headings.first(), Some(&"## Documentation"), "{detail}");

    // `CleanServer.Run` calls one of two `Solve` overloads: the shared name's
    // caveat qualifies the "maybe also" claim, which is the detail's.
    let cards = geo_cards();
    let card = type_card_text(&cards, "CleanServer", Lang::En);
    assert!(!card.contains("## Notes"), "{card}");
    assert!(!card.contains("Solve: 2 candidates"), "{card}");
    let details = details(&cards, Lang::En);
    let detail = detail_of(&cards, &details, CardKind::Type, "CleanServer");
    assert_eq!(
        section(detail, "Notes"),
        "\n- in `Run`: Solve: 2 candidates — ambiguous: a Maybe also claim where it leaves \
         this card's own type or file\n",
        "{detail}"
    );
    let headings: Vec<&str> = detail.lines().filter(|l| l.starts_with("## ")).collect();
    assert_eq!(headings.last(), Some(&"## Notes"), "{detail}");
}

/// A type whose only content beyond its card is its comment gets a detail
/// for it; one whose comment is the purpose's sentence and nothing more gets
/// none — the card already says all of it.
#[test]
fn a_comment_alone_earns_a_detail_unless_the_purpose_says_all_of_it() {
    let cards = cards_of(&[(
        "src/Markers.cs",
        "namespace N\n{\n    /// <summary>Marks a legacy import.</summary>\n    \
         /// <remarks>Kept until the importer is retired.</remarks>\n    public class Legacy\n    {\n    }\n\n    \
         /// Marks nothing else.\n    public class Plain\n    {\n    }\n}\n",
    )]);
    assert_eq!(find(&cards, CardKind::Type, "Plain").detail, None);
    let details = details(&cards, Lang::En);
    let detail = detail_of(&cards, &details, CardKind::Type, "Legacy");
    let headings: Vec<&str> = detail.lines().filter(|l| l.starts_with("## ")).collect();
    // The file's `declares` comes with it, as on every type's detail.
    assert_eq!(
        headings,
        [
            "## Documentation",
            "## Signature",
            "## Referenced by",
            "## Test pairing"
        ],
        "{detail}"
    );
    assert!(
        section(detail, "Documentation").contains("Kept until the importer is retired."),
        "{detail}"
    );
    let card = type_card_text(&cards, "Legacy", Lang::En);
    assert!(card.contains("- Detail: ["), "{card}");
    assert!(
        !card.contains("Kept until the importer is retired."),
        "{card}"
    );
}

/// Nine `Run` methods are past the ambiguity ceiling (8), so the call shows no
/// relation. The short card says how many such names it used — its role
/// sentences would otherwise read as "uses nothing" — in one line, in either
/// language; the detail names each one.
#[test]
fn names_past_the_ceiling_are_counted_on_the_card_and_listed_in_the_detail() {
    let mut src = String::from("namespace N\n{\n");
    for i in 0..9 {
        src.push_str(&format!(
            "    public class Job{i}\n    {{\n        public void Run() {{ }}\n    }}\n"
        ));
    }
    src.push_str(
        "    public class Hub\n    {\n        public void Go()\n        {\n            \
         var job = Make();\n            job.Run();\n        }\n    }\n}\n",
    );
    let cards = cards_of(&[("src/Jobs.cs", &src)]);
    let en = type_card_text(&cards, "Hub", Lang::En);
    assert!(
        en.contains(
            "\n## Notes\n\n- names used past the ambiguity ceiling, with no relation shown: 1 \
             — the detail's notes list them\n"
        ),
        "{en}"
    );
    assert!(!en.contains("Run: 9 candidates"), "{en}");
    let cs = type_card_text(&cards, "Hub", Lang::Cs);
    assert!(
        cs.contains(
            "- použitá jména nad stropem nejednoznačnosti, bez uvedeného vztahu: 1 — vypisují je \
             poznámky v detailu\n"
        ),
        "{cs}"
    );
    let details = details(&cards, Lang::En);
    let notes = section(detail_of(&cards, &details, CardKind::Type, "Hub"), "Notes");
    assert_eq!(
        notes,
        "\n- in `Go`: ambiguous reference: Run: 9 candidates — no relation shown, the ambiguity \
         ceiling was exceeded\n",
        "{notes}"
    );
}

// ---- Plan B, U10: the module map ----

use reverse_architect::card::map::{render_map_files, MAP_PART_ROWS, MAP_ROW_BYTES};

/// Every map of a card set as rendered, by path — each path once.
fn maps(cards: &[Card], lang: Lang) -> BTreeMap<String, String> {
    let mut by_path = BTreeMap::new();
    for file in render_map_files(cards, &CardIndex::build(cards), lang, LinkStyle::Portable) {
        let path = file.path.clone();
        assert!(
            by_path.insert(file.path, file.contents).is_none(),
            "two maps at {path}"
        );
    }
    by_path
}

/// The rendered map of the module `module`.
fn map_of<'a>(cards: &[Card], maps: &'a BTreeMap<String, String>, module: &str) -> &'a str {
    let card = find(cards, CardKind::Module, module);
    let path = card
        .map
        .as_deref()
        .unwrap_or_else(|| panic!("{module} has no map"));
    maps.get(path)
        .unwrap_or_else(|| panic!("{module}'s map {path} was not rendered"))
}

/// The rows of a map: its `- ` lines.
fn rows_of(map: &str) -> Vec<&str> {
    map.lines().filter(|l| l.starts_with("- ")).collect()
}

/// `LoadAction` as the module map meets it in acceptance case A2: documented, it
/// implements an action contract and writes the scheduled-tasks table.
const MAPPED_LOAD_ACTION_CS: &str = "namespace Orbit.Backup\n{\n    public interface IAction\n    {\n        \
    void Run();\n    }\n\n    /// <summary>Restores a backup into the current database.</summary>\n    \
    public class LoadAction : IAction\n    {\n        public void Run()\n        {\n            \
    var sql = \"update dba.ops_schedule_tasks set invalid = 1\";\n        }\n    }\n}\n";

/// AE7 (R1, R4): the module holding the backup actions has a map, and
/// `LoadAction`'s row there says what it is for and which table it writes —
/// beside its kind and the link to its card. The row repeats no module: the
/// page title names it (R4). In Czech the purpose is the comment's own words,
/// and the entries stay the fact vocabulary.
#[test]
fn a_modules_map_gives_load_action_a_row_with_its_purpose_and_the_table_it_writes() {
    let cards = cards_of(&[("Backup/LoadAction.cs", MAPPED_LOAD_ACTION_CS)]);
    let maps_en = maps(&cards, Lang::En);
    let map = map_of(&cards, &maps_en, "Backup");
    let module = find(&cards, CardKind::Module, "Backup");
    let path = module.map.as_deref().unwrap();
    assert_eq!(path, "cards/map/Backup.md");
    let card = find(&cards, CardKind::Type, "LoadAction");
    let row = format!(
        "- class [LoadAction]({}) — Restores a backup into the current database. · writes \
         `dba.ops_schedule_tasks` · implements `IAction`",
        relative(path, &card.path)
    );
    assert!(rows_of(map).contains(&row.as_str()), "{map}");
    assert!(
        map.starts_with(
            "---\nschema: 6\ntool: reverse-architect\nkind: map\nof: module\n---\n\n# Backup\n\n"
        ),
        "{map}"
    );
    assert!(map.contains("\n## Types\n\n"), "{map}");
    assert!(!map.contains("## Part"), "{map}");
    // Every type of the module is on it once, the interface too.
    assert_eq!(rows_of(map).len(), 2, "{map}");

    let maps_cs = maps(&cards, Lang::Cs);
    let cs = map_of(&cards, &maps_cs, "Backup");
    assert!(
        rows_of(cs).iter().any(|r| r.contains("[LoadAction](")
            && r.ends_with(" · writes `dba.ops_schedule_tasks` · implements `IAction`")),
        "{cs}"
    );
    assert!(cs.contains("**mapa modulu**"), "{cs}");
    assert!(cs.contains("\n## Types\n"), "{cs}");
}

/// R1, KTD8: a module of nine hundred types is cut into parts of at most
/// [`MAP_PART_ROWS`] rows, in name order, and every type is on the map
/// exactly once — none dropped at a cut, none repeated.
#[test]
fn a_module_of_nine_hundred_types_is_cut_into_parts_and_keeps_every_type() {
    let files: Vec<(String, String)> = (0..9)
        .map(|f| {
            let mut src = String::from("namespace Big\n{\n");
            for i in 0..100 {
                // Interleaved across files, so name order is not file order.
                let n = i * 9 + f;
                src.push_str(&format!("    public class T{n:03}\n    {{\n    }}\n"));
            }
            src.push_str("}\n");
            (format!("Big/F{f}.cs"), src)
        })
        .collect();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let cards = cards_of(&files);
    let rendered = maps(&cards, Lang::En);
    let map = map_of(&cards, &rendered, "Big");

    let parts: Vec<&str> = map.split("\n## Part ").skip(1).collect();
    assert_eq!(parts.len(), 900_usize.div_ceil(MAP_PART_ROWS), "{map}");
    assert!(!map.contains("\n## Types\n"), "{map}");
    for (n, part) in parts.iter().enumerate() {
        assert!(part.starts_with(&format!("{}\n", n + 1)), "{part}");
        assert!(rows_of(part).len() <= MAP_PART_ROWS, "part {}", n + 1);
    }
    let names: Vec<String> = rows_of(map)
        .iter()
        .filter_map(|r| r.split('[').nth(1)?.split(']').next())
        .map(str::to_string)
        .collect();
    let expected: Vec<String> = (0..900).map(|n| format!("T{n:03}")).collect();
    assert_eq!(names, expected, "every type once, in name order");
    assert_eq!(map, map_of(&cards, &maps(&cards, Lang::En), "Big"));
}

/// R1: the types designer and generator output declares — by the file's name
/// or by its `<auto-generated>` header — are one row with their count, last;
/// a `partial` form whose own part is hand-written keeps its row.
#[test]
fn generated_types_are_one_counted_row_and_a_hand_written_part_keeps_its_row() {
    let cards = cards_of(&[
        (
            "Forms/Main.Designer.cs",
            "namespace F\n{\n    partial class Main\n    {\n        private void InitializeComponent() { }\n    }\n\n    \
             public class Button1\n    {\n    }\n\n    public class Button2\n    {\n    }\n}\n",
        ),
        (
            "Forms/Main.cs",
            "namespace F\n{\n    public partial class Main\n    {\n        public void Show() { }\n    }\n}\n",
        ),
        (
            "Forms/Proxy.cs",
            "//------------------------------------------------------------------------------\n\
             // <auto-generated>\n\
             //     This code was generated by a tool.\n\
             // </auto-generated>\n\
             //------------------------------------------------------------------------------\n\
             namespace F\n{\n    public class ServiceProxy\n    {\n    }\n}\n",
        ),
        (
            "Forms/Helper.cs",
            "namespace F\n{\n    public class Helper\n    {\n    }\n}\n",
        ),
    ]);
    assert!(find(&cards, CardKind::Type, "Button1").generated);
    assert!(find(&cards, CardKind::Type, "ServiceProxy").generated);
    assert!(!find(&cards, CardKind::Type, "Main").generated);
    assert!(!find(&cards, CardKind::Type, "Helper").generated);

    let rendered = maps(&cards, Lang::En);
    let map = map_of(&cards, &rendered, "Forms");
    let rows = rows_of(map);
    assert_eq!(rows.len(), 3, "{map}");
    assert!(rows[0].contains("[Helper]("), "{map}");
    assert!(rows[1].contains("[Main]("), "{map}");
    assert_eq!(
        rows[2],
        "- 3 generated types (designer or tool output, by file name or header) — no row of their own",
        "{map}"
    );
    for name in ["Button1", "Button2", "ServiceProxy"] {
        assert!(!map.contains(name), "{name} has a row of its own:\n{map}");
    }
    assert!(map.contains("**module map** · [Forms]("), "{map}");
    assert!(map.contains(" · 5 types\n"), "{map}");

    let rendered = maps(&cards, Lang::Cs);
    assert!(
        rows_of(map_of(&cards, &rendered, "Forms")).contains(
            &"- 3 generované typy (výstup návrháře nebo nástroje podle jména či hlavičky souboru) \
              — bez vlastních řádků"
        ),
        "{}",
        map_of(&cards, &rendered, "Forms")
    );
}

/// A documented type named `name` in `namespace` that writes five tables and
/// implements three contracts — a row built to overflow.
fn overflowing_type(namespace: &str, name: &str) -> String {
    let mut src = format!("namespace {namespace}\n{{\n");
    for i in 0..3 {
        src.push_str(&format!(
            "    public interface IContractNumber{i}\n    {{\n        void Do{i}();\n    }}\n"
        ));
    }
    src.push_str(
        "    /// <summary>Orchestrates the restore of a backup from the archive into the current \
         database, invalidating the scheduled tasks, rebuilding the indexes and notifying every \
         connected client that the data changed underneath it.</summary>\n    \
         public class ExtremelyLongNamedRestoreBackupOrchestrator : IContractNumber0, \
         IContractNumber1, IContractNumber2\n    {\n        public void Do0()\n        {\n",
    );
    for t in 0..5 {
        src.push_str(&format!(
            "            Execute(\"update dba.ops_table_number_{t} set x = 1\");\n"
        ));
    }
    src.push_str(
        "        }\n        public void Do1() { }\n        public void Do2() { }\n    }\n}\n",
    );
    src.replace("ExtremelyLongNamedRestoreBackupOrchestrator", name)
}

/// Success Criteria: a map row is at most [`MAP_ROW_BYTES`], its line break
/// included — over every fixture of this file that has types, in either
/// language, and over types built to overflow: a long documented name that
/// writes five tables and implements three contracts, once in a short
/// namespace and once so deep that the link alone takes most of the row. The
/// first keeps its kind, its link, the table it writes first and the start
/// of its purpose.
#[test]
fn every_map_row_stays_within_its_byte_budget() {
    let deep = overflowing_type(
        "Orbit.Very.Deep.Namespace.For.Testing",
        "ExtremelyLongNamedRestoreBackupOrchestrator",
    );
    let short = overflowing_type("Orbit.Deep", "LongNamedRestoreOrchestrator");
    let hub = hub_files();
    let sets = [
        cards_of(&[("Deep/Deeper.cs", &deep), ("Deep/Orchestrator.cs", &short)]),
        cards_of(&[("Backup/LoadAction.cs", MAPPED_LOAD_ACTION_CS)]),
        cards_of(&[
            ("src/LoadAction.cs", LOAD_ACTION_CS),
            ("src/Invoice.cs", INVOICE_CS),
            ("src/Restore.cs", ACTION_CS),
        ]),
        cards_of(
            &hub.iter()
                .map(|(p, s)| (p.as_str(), s.as_str()))
                .collect::<Vec<_>>(),
        ),
        geo_cards(),
        build_cards(&rung_fixture()),
    ];
    let mut seen = 0;
    for cards in &sets {
        for lang in [Lang::En, Lang::Cs] {
            for (path, map) in maps(cards, lang) {
                for row in rows_of(&map) {
                    // The line break counts: a row is a line of the file.
                    let bytes = row.len()
                        + "
"
                        .len();
                    assert!(
                        bytes <= MAP_ROW_BYTES,
                        "{path} ({lang:?}): {bytes} bytes: {row}"
                    );
                    seen += 1;
                }
            }
        }
    }
    assert!(seen > 200, "the fixtures must give many rows: {seen}");

    let cards = &sets[0];
    let rendered = maps(cards, Lang::En);
    let row = rows_of(map_of(cards, &rendered, "Deep"))
        .into_iter()
        .find(|r| r.contains("[LongNamedRestoreOrchestrator]("))
        .expect("the orchestrator's row")
        .to_string();
    assert!(row.starts_with("- class ["), "{row}");
    assert!(
        row.contains(" — Orchestrates the restore of a backup"),
        "{row}"
    );
    assert!(row.contains("…"), "the purpose is cut, not dropped: {row}");
    assert!(
        row.contains(" · writes `dba.ops_table_number_0` +4"),
        "two names do not fit, so the entry names one: {row}"
    );
}

/// KTD7: a module card links to its map, which is rendered at that path, and
/// the signpost's type count leads there too; a module without a type has
/// neither a map nor a link to one.
#[test]
fn a_module_card_and_the_signpost_link_to_the_modules_map() {
    let cards = cards_of(&[
        ("Backup/LoadAction.cs", MAPPED_LOAD_ACTION_CS),
        ("appsettings.json", "{}"),
    ]);
    let index = CardIndex::build(&cards);
    let module = find(&cards, CardKind::Module, "Backup");
    let map = module.map.as_deref().expect("Backup has a map");
    let card = render_card(module, Lang::En, LinkStyle::Portable, &index);
    assert!(
        card.contains(&format!(
            "- Map: [2 types]({})\n",
            relative(&module.path, map)
        )),
        "{card}"
    );
    let cs = render_card(module, Lang::Cs, LinkStyle::Portable, &index);
    assert!(cs.contains("- Map: [2 typy]("), "{cs}");
    assert!(maps(&cards, Lang::En).contains_key(map));

    let root = find(&cards, CardKind::Module, ".");
    assert_eq!(root.map, None);
    let root_card = render_card(root, Lang::En, LinkStyle::Portable, &index);
    assert!(!root_card.contains("- Map:"), "{root_card}");

    let signpost =
        reverse_architect::card::index::render_signpost(&cards, &index, LinkStyle::Portable);
    assert!(signpost.contains(&format!("| [2]({map}) |")), "{signpost}");
}
