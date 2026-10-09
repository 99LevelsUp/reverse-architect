//! U11 integration tests: the acceptance harness over a fixture repository
//! reproducing the structural shape of a real research case (see the U11 plan,
//! KTD1, for why the fixture uses generic names, never the real case's).
//!
//! Two modules:
//! - `Notifications` (unrelated): `Sender.Configure()` makes the ONLY qualified
//!   call to `System.Net.ServicePointManager.FindServicePoint` in the repo
//!   (AE1's "framework API used in an unrelated module").
//! - `PartnerGateway` (queried): `Decryptor.Decrypt()` reads `CertificateData`
//!   (a DB-mapped property, per the plan's fixture design) and passes it to
//!   `System.Security.Cryptography.ProtectedData.Unprotect` (AE5's "DB column
//!   to external decrypt call" chain). Nothing in this module ever sets a
//!   client/mTLS certificate — the AE2/AE3 absence query.

mod common;

use common::{
    config_for, decl_line, find, init_repo_with_files, scip_document, scip_occurrence, scip_sym,
    write_scip_index,
};
use reverse_architect::card::CardKind;
use reverse_architect::config::Config;
use reverse_architect::coverage::ModuleCoverage;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::generate;
use reverse_architect::graph::{merge, FactKind, FactValue, Graph, NodeKind};
use std::path::Path;

const CSPROJ: &str = r#"<Project>
  <ItemGroup>
    <PackageReference Include="System.Net" Version="4.3.0" />
    <PackageReference Include="System.Security.Cryptography.ProtectedData" Version="8.0.0" />
  </ItemGroup>
</Project>
"#;

const SENDER_CS: &str = r#"using System.Net;

namespace N.Notifications;

public class Sender
{
    public void Configure(System.Uri target)
    {
        System.Net.ServicePointManager.FindServicePoint(target);
    }
}
"#;

const DECRYPTOR_CS: &str = r#"using System.Security.Cryptography;

namespace N.PartnerGateway;

public class Decryptor
{
    public byte[] CertificateData { get; set; }

    public byte[] Decrypt()
    {
        return System.Security.Cryptography.ProtectedData.Unprotect(this.CertificateData, null, DataProtectionScope.LocalMachine);
    }
}
"#;

/// The exact readable name the extractor gives `Sender.Configure()`'s external
/// call — observed empirically (`n.facts` `FactKind::ExternalName`) while
/// writing this unit, and pinned here so every AE1/AE3 assertion checks the
/// same exact string instead of a substring a mutated name could still satisfy.
const FIND_SERVICE_POINT_LABEL: &str = "System.Net.ServicePointManager.FindServicePoint";

/// Same as [`FIND_SERVICE_POINT_LABEL`], for `Decryptor.Decrypt()`'s external
/// call (AE5).
const UNPROTECT_LABEL: &str = "System.Security.Cryptography.ProtectedData.Unprotect";

type FixtureFile = (&'static str, &'static str);

const FIXTURE_FILES: &[FixtureFile] = &[
    ("proj.csproj", CSPROJ),
    ("src/Notifications/Sender.cs", SENDER_CS),
    ("src/PartnerGateway/Decryptor.cs", DECRYPTOR_CS),
];

fn build_fixture(dir: &Path) {
    init_repo_with_files(dir, "Alice", FIXTURE_FILES);
}

/// A literal, case-sensitive substring search over raw file text, one line at
/// a time, with no regex extensions — the `rg`-equivalent baseline per KTD4.
fn grep<'a>(files: &'a [FixtureFile], needle: &str) -> Vec<(&'a str, u32)> {
    let mut hits = Vec::new();
    for (path, contents) in files {
        for (i, line) in contents.lines().enumerate() {
            if line.contains(needle) {
                hits.push((*path, (i + 1) as u32));
            }
        }
    }
    hits
}

/// The subset of `files` whose path sits under `module_prefix` — the
/// "already knows where to look" baseline variant per KTD4.
fn scoped(files: &[FixtureFile], module_prefix: &str) -> Vec<FixtureFile> {
    files
        .iter()
        .copied()
        .filter(|(p, _)| p.starts_with(module_prefix))
        .collect()
}

/// The coverage entry for `path`, or panic naming every module coverage saw —
/// the shared home for what was three copies of the same lookup.
fn find_module<'a>(modules: &'a [ModuleCoverage], path: &str) -> &'a ModuleCoverage {
    modules.iter().find(|m| m.path == path).unwrap_or_else(|| {
        let seen: Vec<&String> = modules.iter().map(|m| &m.path).collect();
        panic!("no coverage entry for {path:?}; have: {seen:?}")
    })
}

/// True if any relation in `relations` has this EXACT label. A substring
/// check would let a mutated/renamed symbol whose name happens to be a
/// superstring of the expected one (e.g. `FindServicePointXXX`) slip past
/// undetected — caught empirically while proving the DoD's regression check,
/// which is exactly why this is exact-match, not `.contains()`.
fn mentions(relations: &[reverse_architect::card::Relation], exact_label: &str) -> bool {
    relations.iter().any(|r| r.label == exact_label)
}

/// The heuristic-tier graph over the fixture at `dir` — `run_all` + `merge`,
/// no SCIP index. Shared by every test that needs the raw graph directly
/// (the pre-flight probe, and the SCIP fixtures that anchor themselves to
/// the grammar's real declaration lines via `decl_line`).
fn baseline_graph(dir: &Path) -> Graph {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    graph
}

/// The full pipeline over the fixture at `dir` — `generate()` against a
/// default `config_for` config, panicking on failure. Shared by every AE
/// test that needs `Generated.cards`/`Generated.coverage`, mirroring
/// `tests/card.rs`'s `cards_over` helper for the full-pipeline case.
fn generated_over(dir: &Path) -> reverse_architect::Generated {
    generate(&config_for(dir, &dir.join("out"))).unwrap()
}

/// U1/Approach step 0: confirms today's heuristic extractor actually produces
/// a `Calls`/`Uses` relation for both call chains the fixture depends on,
/// BEFORE the rest of this suite builds on that assumption. A fully-qualified
/// static call (`Namespace.Type.Member(args)`) from a namespace the `.csproj`
/// declares as a `PackageReference` is what the extractor recognizes — a bare
/// property/field assignment (e.g. `X.Y = value;`) is NOT (verified empirically
/// against `src/extract/external.rs` while writing this unit).
#[test]
fn preflight_extractor_recognizes_the_two_call_chains() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let graph = baseline_graph(tmp.path());

    let externals: Vec<String> = graph
        .nodes_sorted()
        .iter()
        .filter(|n| n.kind == NodeKind::External)
        .filter_map(|n| {
            n.facts
                .iter()
                .find(|f| f.kind == FactKind::ExternalName)
                .and_then(|f| match &f.value {
                    FactValue::Text(t) => Some(t.clone()),
                    _ => None,
                })
        })
        .collect();

    assert!(
        externals.iter().any(|n| n == FIND_SERVICE_POINT_LABEL),
        "Sender.Configure()'s qualified call was not recognized as an external symbol: {externals:?}"
    );
    assert!(
        externals.iter().any(|n| n == UNPROTECT_LABEL),
        "Decryptor.Decrypt()'s qualified call was not recognized as an external symbol: {externals:?}"
    );
}

#[test]
fn covers_ae1_the_only_use_of_a_framework_api_is_anchored_and_the_queried_module_omits_it() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let generated = generated_over(tmp.path());
    let cards = reverse_architect::card::build_cards(&generated.graph);

    let sender = find(&cards, CardKind::Type, "Sender");
    assert!(
        mentions(&sender.externals, FIND_SERVICE_POINT_LABEL),
        "Sender (the unrelated module) must carry the sole ServicePointManager.FindServicePoint call, with an anchor: {:?}",
        sender.externals
    );
    let anchor = sender
        .externals
        .iter()
        .find(|r| r.label == FIND_SERVICE_POINT_LABEL)
        .and_then(|r| r.source.as_ref())
        .unwrap_or_else(|| {
            panic!(
                "the FindServicePoint relation carries no source anchor: {:?}",
                sender.externals
            )
        });
    assert!(anchor.anchor().contains("Sender.cs"), "{anchor:?}");

    let decryptor = find(&cards, CardKind::Type, "Decryptor");
    assert!(
        !mentions(&decryptor.externals, FIND_SERVICE_POINT_LABEL),
        "Decryptor (the queried module) must never mention the unrelated module's API use: {:?}",
        decryptor.externals
    );
}

#[test]
fn covers_ae3_an_absence_query_without_a_scip_index_is_not_found_not_confirmed_absent() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let generated = generated_over(tmp.path());
    let cards = reverse_architect::card::build_cards(&generated.graph);

    // The query: does PartnerGateway set a client/mTLS certificate anywhere?
    // Nothing in the fixture does — the card simply has no such relation, and
    // without a SCIP index the coverage artifact licenses no absence claim
    // over this module (R20), so a consumer must read this as "not found",
    // never as a confirmed "does not exist".
    let decryptor = find(&cards, CardKind::Type, "Decryptor");
    assert!(
        !mentions(&decryptor.externals, "ClientCertificate")
            && !mentions(&decryptor.outgoing, "ClientCertificate"),
        "fixture must not accidentally reference a client certificate API: {:?} / {:?}",
        decryptor.externals,
        decryptor.outgoing
    );

    let module = find_module(&generated.coverage.modules, "src/PartnerGateway");
    assert!(
        !module.fully_resolved,
        "without a SCIP index, PartnerGateway must not be fully_resolved — an absence claim here would be unlicensed (R20)"
    );
}

#[test]
fn covers_ae5_a_db_sourced_value_and_the_external_decrypt_call_are_both_on_the_method_card() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let generated = generated_over(tmp.path());
    let cards = reverse_architect::card::build_cards(&generated.graph);

    let decryptor = find(&cards, CardKind::Type, "Decryptor");
    // The DB-mapped source (CertificateData) and the external decrypt call
    // both surface on the one card that owns Decrypt() — CertificateData as a
    // sibling member row (KTD5: a card is a type, not a method), Unprotect as
    // an externals relation lifted from Decrypt() onto its owning type.
    assert!(
        decryptor
            .members
            .iter()
            .any(|m| m.name == "CertificateData"),
        "Decryptor's card must list CertificateData as a member: {:?}",
        decryptor.members
    );
    assert!(
        mentions(&decryptor.externals, UNPROTECT_LABEL),
        "Decryptor's card must carry the ProtectedData.Unprotect relation: {:?}",
        decryptor.externals
    );
    // Two facts sitting on the same card is not yet the chain AE5 claims —
    // `via` ties the lifted relation back to the specific method that reads
    // CertificateData, the same field tests/card.rs:201 uses for this exact
    // purpose. Without this, a mutation that decoupled the DB read from the
    // decrypt call within Decryptor would go undetected.
    assert!(
        decryptor
            .externals
            .iter()
            .any(|r| r.label == UNPROTECT_LABEL && r.via.as_deref() == Some("Decrypt()")),
        "the Unprotect relation must be tagged via Decrypt(), the method that also reads CertificateData: {:?}",
        decryptor.externals
    );
    let anchor = decryptor
        .externals
        .iter()
        .find(|r| r.label == UNPROTECT_LABEL)
        .and_then(|r| r.source.as_ref())
        .unwrap_or_else(|| {
            panic!(
                "the Unprotect relation carries no source anchor: {:?}",
                decryptor.externals
            )
        });
    assert!(anchor.anchor().contains("Decryptor.cs"), "{anchor:?}");
}

/// A SCIP index over `Decryptor.cs` that agrees with the grammar — enough to
/// mark the file `resolved` and, since `PartnerGateway` holds only this one
/// file, the module `fully_resolved` too.
fn write_agreeing_index(index_dir: &Path, baseline: &Graph) -> std::path::PathBuf {
    let decryptor_sym = scip_sym("N/PartnerGateway/Decryptor#");
    let decrypt_sym = scip_sym("N/PartnerGateway/Decryptor#Decrypt().");
    write_scip_index(
        index_dir,
        vec![scip_document(
            "src\\PartnerGateway\\Decryptor.cs",
            vec![
                scip_occurrence(&decryptor_sym, decl_line(baseline, "Decryptor"), true),
                scip_occurrence(&decrypt_sym, decl_line(baseline, "Decrypt"), true),
            ],
        )],
    )
}

#[test]
fn covers_ae2_a_scip_index_licenses_a_definitive_absence_claim() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let baseline = baseline_graph(tmp.path());

    let index_dir = tempfile::tempdir().unwrap();
    let index_path = write_agreeing_index(index_dir.path(), &baseline);

    let mut config: Config = config_for(tmp.path(), &tmp.path().join("out"));
    config.scip_index = Some(index_path);
    let generated = generate(&config).unwrap();

    let module = find_module(&generated.coverage.modules, "src/PartnerGateway");
    assert!(
        module.fully_resolved,
        "a SCIP index covering every file under PartnerGateway must license a definitive absence claim (R20): {module:?}"
    );

    // The same absence query as AE3 — nothing changed in the card, only what
    // the corpus is entitled to CONCLUDE from it changed.
    let cards = reverse_architect::card::build_cards(&generated.graph);
    let decryptor = find(&cards, CardKind::Type, "Decryptor");
    assert!(
        !mentions(&decryptor.externals, "ClientCertificate")
            && !mentions(&decryptor.outgoing, "ClientCertificate"),
        "the card itself must not change between the heuristic and resolved runs: {:?} / {:?}",
        decryptor.externals,
        decryptor.outgoing
    );
}

#[test]
fn covers_ae2_and_ae3_together_an_index_that_does_not_cover_the_module_gives_no_false_license() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let baseline = baseline_graph(tmp.path());

    // An index that covers ONLY the unrelated module (Sender.cs), never
    // PartnerGateway — the absence query must stay unlicensed.
    let index_dir = tempfile::tempdir().unwrap();
    let sender_sym = scip_sym("N/Notifications/Sender#");
    let index_path = write_scip_index(
        index_dir.path(),
        vec![scip_document(
            "src\\Notifications\\Sender.cs",
            vec![scip_occurrence(
                &sender_sym,
                decl_line(&baseline, "Sender"),
                true,
            )],
        )],
    );

    let mut config: Config = config_for(tmp.path(), &tmp.path().join("out"));
    config.scip_index = Some(index_path);
    let generated = generate(&config).unwrap();

    let module = find_module(&generated.coverage.modules, "src/PartnerGateway");
    assert!(
        !module.fully_resolved,
        "an index that never covers PartnerGateway must not license an absence claim there, even though SOME index was supplied: {module:?}"
    );
}

#[test]
fn extraction_over_the_fixture_is_deterministic_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let first = generate(&config_for(tmp.path(), &tmp.path().join("out1"))).unwrap();
    let second = generate(&config_for(tmp.path(), &tmp.path().join("out2"))).unwrap();
    assert_eq!(first.cards, second.cards);
    // The two levels around the cards (plan B, U11): the details and the
    // maps, and the manifest documents that describe all three.
    assert!(
        !first.details.is_empty() && !first.maps.is_empty(),
        "the fixture renders a detail and a map, or this compares nothing"
    );
    assert_eq!(first.details, second.details);
    assert_eq!(first.maps, second.maps);
    assert_eq!(first.manifest.documents, second.manifest.documents);
}

/// U3/R24: the zamrzlý (frozen) catalog. Exactly 4 records — AE1, AE2/AE3
/// (shared query, two coverage regimes — see the two `covers_ae2_*` tests
/// above for the SCIP-index side), AE5, and one deliberately ambiguous term.
/// Every count below was observed by running this test and is now pinned: a
/// regression in the fixture or the baseline changes this test's own
/// expectations, which is the point of freezing them.
///
/// Unlike a first draft of this test, every record asserts the CORPUS side
/// here too, not just the baseline — a version that only ran `grep` could go
/// green on a fixture edit alone, with the "corpus adds X" half of each
/// claim living nowhere but a comment (caught by adversarial review before
/// this shipped).
#[test]
fn r24_frozen_catalog_shows_what_the_corpus_adds_over_the_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let generated = generated_over(tmp.path());
    let cards = reverse_architect::card::build_cards(&generated.graph);
    let sender = find(&cards, CardKind::Type, "Sender");
    let decryptor = find(&cards, CardKind::Type, "Decryptor");

    // Record 1 (AE1): a repo-wide search for the framework API name finds the
    // one real call. It cannot also state that PartnerGateway omits it —
    // that is an absence claim, and grep has no vocabulary for "and nowhere
    // else". The corpus states both from one query: the anchor on Sender's
    // card, and the mention's absence on Decryptor's.
    let hits = grep(FIXTURE_FILES, "FindServicePoint");
    assert_eq!(
        hits,
        vec![("src/Notifications/Sender.cs", 9)],
        "AE1's frozen baseline count changed"
    );
    assert!(
        mentions(&sender.externals, FIND_SERVICE_POINT_LABEL)
            && !mentions(&decryptor.externals, FIND_SERVICE_POINT_LABEL),
        "AE1's corpus-side claim changed: Sender {:?} / Decryptor {:?}",
        sender.externals,
        decryptor.externals
    );

    // Record 2 (AE2/AE3): a term that never appears returns zero hits either
    // way — indistinguishable from "the search missed it". Only the coverage
    // artifact can tell "confirmed absent" apart from "not found"; this
    // record freezes the "not found" (no index) side — the SCIP-index
    // "confirmed absent" side is `covers_ae2_...`'s own dedicated test.
    let repo_wide = grep(FIXTURE_FILES, "ClientCertificate");
    let partner_gateway_files = scoped(FIXTURE_FILES, "src/PartnerGateway/");
    let module_scoped = grep(&partner_gateway_files, "ClientCertificate");
    assert!(
        repo_wide.is_empty() && module_scoped.is_empty(),
        "AE2/AE3's baseline must find nothing in either scope: {repo_wide:?} / {module_scoped:?}"
    );
    let module = find_module(&generated.coverage.modules, "src/PartnerGateway");
    assert!(
        !module.fully_resolved,
        "AE2/AE3's corpus-side claim changed: PartnerGateway must not be fully_resolved without an index"
    );

    // Record 3 (AE5): baseline can locate each link of the chain IN
    // ISOLATION, but nothing ties the two searches into one fact — that
    // requires reading both results and noticing they share a file. The
    // corpus ties them structurally: CertificateData as a member, Unprotect
    // as an externals relation `via` the same method, `Decrypt`.
    let cert_hits = grep(FIXTURE_FILES, "CertificateData");
    let decrypt_hits = grep(FIXTURE_FILES, "Unprotect");
    assert_eq!(
        cert_hits,
        vec![
            ("src/PartnerGateway/Decryptor.cs", 7),
            ("src/PartnerGateway/Decryptor.cs", 11),
        ],
        "AE5's CertificateData baseline count changed"
    );
    assert_eq!(
        decrypt_hits,
        vec![("src/PartnerGateway/Decryptor.cs", 11)],
        "AE5's Unprotect baseline count changed"
    );
    assert!(
        decryptor
            .members
            .iter()
            .any(|m| m.name == "CertificateData")
            && decryptor
                .externals
                .iter()
                .any(|r| r.label == UNPROTECT_LABEL && r.via.as_deref() == Some("Decrypt()")),
        "AE5's corpus-side claim changed: members {:?} / externals {:?}",
        decryptor.members,
        decryptor.externals
    );

    // Record 4 (ambiguous term): a repo-wide search for a generic word
    // returns candidates spread across a build manifest AND actual code,
    // with no way to tell which one answers "what handles certificate data"
    // without reading every hit. The corpus instead names exactly one
    // relevant card — asserted directly here by counting how many TYPE cards
    // carry any externals relation whose label mentions the same term.
    let ambiguous_hits = grep(FIXTURE_FILES, "Cryptography");
    let files_hit: std::collections::BTreeSet<&str> =
        ambiguous_hits.iter().map(|(p, _)| *p).collect();
    assert!(
        ambiguous_hits.len() > 1 && files_hit.len() > 1,
        "the ambiguous-term catalog entry needs candidates spread across more than one file: {ambiguous_hits:?}"
    );
    let matching_cards: Vec<&str> = cards
        .iter()
        .filter(|c| c.kind == CardKind::Type)
        .filter(|c| c.externals.iter().any(|r| r.label.contains("Cryptography")))
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        matching_cards,
        vec!["Decryptor"],
        "the corpus's structured search for the ambiguous term must resolve to exactly one card, unlike the {} raw-text hits above",
        ambiguous_hits.len()
    );
}
