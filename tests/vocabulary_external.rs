//! U8 — doklad zvenčí: celá projekce z pohledu volajícího, který není tenhle
//! repozitář.
//!
//! **Proč to není další soubor testů vedle `vocabulary_declared.rs`.** Ten
//! soubor se ptá, jestli jednotlivé části mechanismu dělají, co slibují.
//! Tenhle se ptá na něco jiného a slabšího na první pohled, ale v praxi
//! rozhodujícího: **jde těch devět kroků zavolat zvenčí, ve správném pořadí,
//! bez `Repo` a bez `generate`, a vypadne z toho korpus na disku?** Odpověď
//! na to nedá žádný test, který skončí u `build_cards`.
//!
//! Volající tady používá **jen veřejné API** — přesně tolik, kolik ho má
//! externí konzument k dispozici. Nic `pub(crate)`, žádná testovací zkratka.
//!
//! **Jména jsou záměrně abstraktní (`ext:*`).** Doména, kvůli které celá
//! práce vznikla, do tohoto repozitáře nesmí — a abstraktní jméno je navíc
//! silnější důkaz než doménové: dokazuje, že mechanismus je obecný, ne že se
//! povedlo jedno konkrétní mapování. Tvar se drží cílové domény (kontejner,
//! definice s kartou, dvě „pořadové" hrany, vrstva jako skalární fakt),
//! slovní zásoba ne.
//!
//! Tento soubor je trvalou podobou návrhové brány, kterou RG2 spustila
//! empiricky na dočasném souboru. RG3 ji spouští znovu — a spustí ji na
//! tomhle.

use reverse_architect::card::index::append_signpost;
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::render::Lang;
use reverse_architect::card::{build_cards, build_cards_with_insight, render_card_files, CardKind};
use reverse_architect::cli::LinkStyle;
use reverse_architect::coverage::{build_coverage, Coverage};
use reverse_architect::extract::languages::Languages;
use reverse_architect::graph::{
    merge, CardPolicy, Cardinality, Edge, EdgeKind, ExtractorId, Fact, FactKind, FactValue, Graph,
    LinkRole, NodeKey, NodeKind, Provenance, Ranks, StructuralRole, Tier,
};
use reverse_architect::manifest::{build_manifest, Manifest};
use reverse_architect::render::RenderedFile;
use std::collections::BTreeMap;
use std::path::Path;

const EXTRACTOR: &str = "ext-consumer";

fn prov(file: &str) -> Provenance {
    Provenance::new(file, Some(1), ExtractorId::from(EXTRACTOR), Tier::Measured)
}

fn ranks() -> Ranks {
    Ranks::new(BTreeMap::from([(ExtractorId::from(EXTRACTOR), 10)]))
}

// ---------------------------------------------------------------------------
// Konzument
// ---------------------------------------------------------------------------

/// Co volající dostane z devíti kroků, než to zapíše na disk.
///
/// Jmenovitě NE [`reverse_architect::Generated`]: ten vzniká jen uvnitř
/// `generate_reporting`, které otevírá `Repo`. Konzument, který si graf staví
/// sám, na něj nedosáhne — a tenhle typ je tedy jeho vlastní, což je přesně to
/// tření, které tahle jednotka měla změřit.
struct Corpus {
    files: Vec<RenderedFile>,
    cards: Vec<RenderedFile>,
    manifest: Manifest,
    coverage: Coverage,
}

/// Devět kroků projekce, ve stejném pořadí, v jakém je volá
/// `generate_reporting` — jen bez repozitáře a bez vyrovnávací paměti.
///
/// `merge` je v seznamu vědomě: `add_fact` ho nevolá, takže bez něj je pořadí
/// faktů závislé na pořadí vkládání a determinismus padá. Volající si musí
/// dodat i vlastní `Ranks`; bez nich jde jeho extraktor na `UNKNOWN_RANK` a
/// prohraje každý skalární konflikt s kýmkoli.
fn project(mut graph: Graph, lang: Lang) -> Corpus {
    merge(&mut graph, &ranks());

    let mut corpus = reverse_architect::render::build_corpus(&graph, LinkStyle::Portable, None)
        .expect("the corpus must render");
    let manifest = build_manifest(&graph, None).expect("the manifest must build");
    let (card_set, _, card_rows) = build_cards_with_insight(&graph);
    let card_index = CardIndex::build(&card_set);
    let cards = render_card_files(&card_set, &card_index, lang, LinkStyle::Portable);
    append_signpost(&mut corpus, &card_set, &card_index, LinkStyle::Portable);
    // Revizi si nese volající, ne graf — nemá repozitář, ze kterého by ji četl.
    let coverage = build_coverage(&graph, None, &Languages::bundled(), Some(card_rows));

    Corpus {
        files: corpus.files,
        cards,
        manifest,
        coverage,
    }
}

/// Zápis korpusu na disk, který si konzument musí napsat sám.
///
/// **Tření č. 1, potvrzené:** `write_corpus` a jeho pomocníci jsou privátní
/// funkce v `src/main.rs`, takže tohle je reimplementace, ne volání. Verze tady
/// je záměrně minimální — bez úklidu osiřelých karet a bez „nepřepisuj, když se
/// bajty nezměnily", protože ani jedno není potřeba, aby korpus na disku
/// vznikl. To je nález do plánu, ne věc, kterou tahle jednotka opravuje.
fn write(dir: &Path, corpus: &Corpus) {
    let put = |rel: &str, bytes: &[u8]| {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the corpus directory must be creatable");
        }
        std::fs::write(&path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    };
    for f in corpus.files.iter().chain(&corpus.cards) {
        put(&f.path, f.contents.as_bytes());
    }
    put(
        "manifest.json",
        reverse_architect::manifest::to_json(&corpus.manifest).as_bytes(),
    );
    put(
        "coverage.json",
        reverse_architect::coverage::to_json(&corpus.coverage).as_bytes(),
    );
}

/// Každý soubor pod `dir` jako `(relativní cesta, bajty)`, seřazeno.
fn on_disk(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("the corpus directory must be readable") {
            let path = entry.expect("a directory entry must be readable").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("every file lies under the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&path).expect("a written file must read"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Dva tvary jedné struktury — jádrové druhy a deklarované druhy
// ---------------------------------------------------------------------------

/// Cílový tvar postavený **z jádrových druhů**: strom z `Module`, listy jako
/// `File`, vazba `Contains`.
///
/// KTD6 slibuje, že tahle cesta zůstává otevřená — plugin, jehož entity se dají
/// poctivě popsat jádrovými druhy, žádné deklarace potřebovat nemá.
fn core_shape() -> Graph {
    let mut g = Graph::new();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("area"), NodeKind::Module)
        .unwrap();
    g.ensure_node(NodeKey::new("area/alfa"), NodeKind::File)
        .unwrap();
    g.ensure_node(NodeKey::new("area/beta"), NodeKind::File)
        .unwrap();
    for (from, to) in [(".", "area"), ("area", "area/alfa"), ("area", "area/beta")] {
        g.add_edge(
            &NodeKey::new(from),
            &NodeKey::new(to),
            Edge::new(EdgeKind::Contains, prov(from)),
        );
    }
    g
}

/// Týž tvar, tytéž klíče, **z deklarovaných druhů**: kontejner v roli
/// `Container`, list v roli `Definition`, vazba v roli `Containment`.
///
/// Klíče jsou schválně identické s [`core_shape`]. Kdyby se lišily, rozdíl mezi
/// oběma korpusy by šel svést na jiná jména uzlů a tvrzení „je to tentýž tvar"
/// by neplatilo.
fn declared_shape() -> Graph {
    let mut g = Graph::new();
    let card = g.declare_card_kind("ext", "definition").unwrap();
    let container = g
        .declare_node_kind(
            "ext",
            "container",
            StructuralRole::Container,
            CardPolicy::None,
        )
        .unwrap();
    let definition = g
        .declare_node_kind(
            "ext",
            "definition",
            StructuralRole::Definition,
            CardPolicy::Card(card),
        )
        .unwrap();
    let holds = g
        .declare_edge_kind("ext", "holds", LinkRole::Containment)
        .unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("area"), NodeKind::Declared(container))
        .unwrap();
    for leaf in ["area/alfa", "area/beta"] {
        g.ensure_node(NodeKey::new(leaf), NodeKind::Declared(definition))
            .unwrap();
    }
    for (from, to) in [(".", "area"), ("area", "area/alfa"), ("area", "area/beta")] {
        g.add_edge(
            &NodeKey::new(from),
            &NodeKey::new(to),
            Edge::new(EdgeKind::Declared(holds), prov(from)),
        );
    }
    g
}

/// Jeden vyrenderovaný pohled podle jména souboru.
fn view<'a>(corpus: &'a Corpus, path: &str) -> &'a str {
    corpus
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path}"))
        .contents
        .as_str()
}

// ---------------------------------------------------------------------------
// Kroky 1 a 2 — korpus vznikne a popisuje sám sebe
// ---------------------------------------------------------------------------

/// Krok 1 a 2 naráz, a **přes disk**, ne jen v paměti.
///
/// Disk je jediné, co dokáže, že `declared/<prostor>/<jméno>/…` je legální
/// cesta na téhle platformě a že za ní něco je. Klíče uzlů deklarovaného druhu
/// běžně nesou `:`, které ve Windows cestě být nesmí (R15) — a jestli to
/// `card_path` skutečně ošetřil, se v paměti pozná leda z řetězce, kdežto na
/// disku z toho, že zápis projde.
#[test]
fn an_outside_caller_writes_a_corpus_and_the_manifest_on_disk_carries_its_whole_vocabulary() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let corpus = project(declared_shape(), Lang::En);
    write(dir.path(), &corpus);

    let files = on_disk(dir.path());
    assert!(
        files.contains_key("index.md"),
        "the corpus root has to be there: {:?}",
        files.keys().collect::<Vec<_>>()
    );
    let card = files
        .keys()
        .find(|p| p.starts_with("cards/declared/ext/definition/"))
        .unwrap_or_else(|| {
            panic!(
                "a declared card has to reach the disk under its own tree: {:?}",
                files.keys().collect::<Vec<_>>()
            )
        })
        .clone();
    assert!(
        !files[&card].is_empty(),
        "and have content behind it, not just a path"
    );

    // Odsud dál čte cizí konzument: nic než bajty z disku.
    let manifest: Manifest = serde_json::from_slice(&files["manifest.json"])
        .expect("a consumer decodes the manifest without this crate's types in hand");
    let v = manifest
        .vocabulary
        .expect("a corpus that declares must publish what it declared (R3)");

    assert_eq!(
        v.digest, "fnv1a64",
        "the digest is named, not implied (R14)"
    );
    assert_eq!(
        v.node_kinds
            .iter()
            .map(|k| (k.kind.name.as_str(), k.role.as_str(), k.card.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("ext:container", "container", None),
            ("ext:definition", "definition", Some("ext:definition")),
        ],
        "every declared node kind, by name, with the role and the card policy \
         the core actually used — and sorted by name, not by digest"
    );
    assert_eq!(
        v.edge_kinds
            .iter()
            .map(|k| (k.kind.name.as_str(), k.role.as_str()))
            .collect::<Vec<_>>(),
        vec![("ext:holds", "containment")]
    );
    assert_eq!(
        v.card_kinds
            .iter()
            .map(|k| k.kind.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ext:definition"],
        "the card kind the node kind points at is itself enumerable, so the \
         join a consumer needs is closed inside the section"
    );
    assert_eq!(
        manifest.schema_version,
        reverse_architect::manifest::SCHEMA_VERSION_WITH_VOCABULARY
    );
}

/// R4 na úrovni schématu, z pohledu konzumenta: korpus bez deklarací nemá klíč
/// `vocabulary` **vůbec** a nese starou verzi.
///
/// Nulová hodnota v tom klíči by byla horší než nic — konzument, který na
/// starou verzi spoléhá, by dostal pole, o kterém nic neví.
#[test]
fn a_corpus_without_declarations_publishes_no_vocabulary_key_at_all() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    write(dir.path(), &project(core_shape(), Lang::En));
    let files = on_disk(dir.path());

    let raw = String::from_utf8(files["manifest.json"].clone()).unwrap();
    assert!(
        !raw.contains("vocabulary"),
        "an absent section must not appear as null: {raw}"
    );
    let manifest: Manifest = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        manifest.schema_version,
        reverse_architect::manifest::SCHEMA_VERSION,
        "a consumer that never heard of this work must not be asked to upgrade"
    );

    let coverage_raw = String::from_utf8(files["coverage.json"].clone()).unwrap();
    assert!(!coverage_raw.contains("declared_kinds_excluded"));
}

// ---------------------------------------------------------------------------
// Krok 4 — obě cesty, které KTD6 slibuje
// ---------------------------------------------------------------------------

/// Tentýž tvar oběma cestami dá **stejně informativní** pohled na strukturu.
///
/// To je doklad, že R9 je mechanismus, ne výjimka: kdyby deklarovaná cesta
/// vedla k chudšímu stromu, byla by strukturální role ústupkem, a ne opravou,
/// za kterou se vydává.
///
/// Rozdíl mezi oběma pohledy smí být jen ve **slovech**, nikdy v uzlech ani ve
/// vazbách. Přijatý rozpor z RG2 („tracked file" o deklarovaném listu) je právě
/// takový rozdíl a je zapsaný jako známý.
#[test]
fn the_same_shape_from_core_kinds_and_from_declared_kinds_gives_the_same_tree() {
    let core = project(core_shape(), Lang::En);
    let declared = project(declared_shape(), Lang::En);

    // Strom: obě cesty musí pojmenovat každý uzel, kontejner i oba listy.
    for key in ["area", "area/alfa", "area/beta"] {
        let a = view(&core, "module-structure.md");
        let b = view(&declared, "module-structure.md");
        assert!(
            a.contains(key) && b.contains(key),
            "module-structure.md: both paths must name `{key}`\n--- core ---\n{a}\n--- declared ---\n{b}"
        );
    }

    // Přehled: obě cesty musí vidět kontejner jako položku nejvyšší úrovně.
    // Listy tam nejsou ani u jádrové cesty — přehled je přehled.
    for corpus in [&core, &declared] {
        let text = view(corpus, "overview.md");
        assert!(
            text.contains("Top-level entries: area."),
            "the container is a top-level entry on both paths: {text}"
        );
    }

    // A obojí musí projít i diagramem, kde se uzly kreslí, ne vypisují — U6c
    // našel únik právě tam a jen tam.
    let diagram = view(&declared, "module-structure.md");
    assert!(
        diagram.contains("-->"),
        "the declared tree has to be drawn, not only listed: {diagram}"
    );
}

/// Pokrytí je poctivé v obou tvarech, a v každém jinak — právě proto, že měří
/// dosah jazykových extraktorů, ne tvar repozitáře.
#[test]
fn coverage_counts_the_core_shape_and_states_what_it_left_out_of_the_declared_one() {
    let core = project(core_shape(), Lang::En).coverage;
    assert!(
        !core.files.is_empty(),
        "core `File` nodes are tracked files and coverage counts them"
    );
    assert!(core.declared_kinds_excluded.is_none());

    let declared = project(declared_shape(), Lang::En).coverage;
    assert!(
        declared.files.is_empty(),
        "a declared node was never handed to a grammar, so it is not a measured file"
    );
    let excluded = declared
        .declared_kinds_excluded
        .expect("and the number stays honest only if that is said out loud");
    assert_eq!(
        excluded.node_kinds,
        vec!["ext:container".to_string(), "ext:definition".to_string()]
    );
    assert_eq!(excluded.nodes, 3, "one container and two definitions");
}

// ---------------------------------------------------------------------------
// Dvouvrstvá definice — třetí bod záporného verdiktu, na korpusu
// ---------------------------------------------------------------------------

/// Cílový tvar, ve kterém jedna definice stojí na druhé — a z korpusu musí jít
/// poznat, **kterým způsobem**.
///
/// Rozlišené jménem hrany, nikdy hodnotou faktu: kdyby to byl fakt, rozhodoval
/// by o něm `merge` podle důvěryhodnosti a dvě vrstvy by se mohly přebít. Obě
/// hrany jsou v roli `Opaque`, takže taxonomické pohledy neznečistí — přesně
/// to, co by `Extends` udělat nemohla.
#[test]
fn a_two_layer_definition_says_from_the_corpus_which_layer_stands_on_which() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("ext", "definition").unwrap();
    let kind = g
        .declare_node_kind(
            "ext",
            "definition",
            StructuralRole::Definition,
            CardPolicy::Card(card),
        )
        .unwrap();
    let composes = g
        .declare_edge_kind("ext", "composes", LinkRole::Opaque)
        .unwrap();
    let replaces = g
        .declare_edge_kind("ext", "replaces", LinkRole::Opaque)
        .unwrap();
    let layer = g
        .declare_fact_kind("ext", "layer", Cardinality::Scalar)
        .unwrap();
    let tag = g
        .declare_fact_kind("ext", "tag", Cardinality::Additive)
        .unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    let upper = NodeKey::new("area/upper");
    for (key, level) in [("area/base", "0"), ("area/upper", "1")] {
        let k = NodeKey::new(key);
        g.ensure_node(k.clone(), NodeKind::Declared(kind)).unwrap();
        assert!(g.add_fact(
            &k,
            Fact::new(
                FactKind::Declared(layer),
                FactValue::Text(level.into()),
                prov(key),
            ),
        ));
    }
    // Aditivní fakt: dvě hodnoty na jednom uzlu, obě musí `merge` nechat být.
    for value in ["one", "two"] {
        assert!(g.add_fact(
            &upper,
            Fact::new(
                FactKind::Declared(tag),
                FactValue::Text(value.into()),
                prov("area/upper"),
            ),
        ));
    }
    g.add_edge(
        &upper,
        &NodeKey::new("area/base"),
        Edge::new(EdgeKind::Declared(composes), prov("area/upper")),
    );
    g.add_edge(
        &upper,
        &NodeKey::new("area/base"),
        Edge::new(EdgeKind::Declared(replaces), prov("area/upper")),
    );

    let corpus = project(g, Lang::En);

    // Na kartě: obě vazby pod jmény, která jim dal plugin, a rozlišitelné.
    let cards = corpus
        .cards
        .iter()
        .find(|f| f.path.contains("declared/ext/definition/") && f.contents.contains("area/upper"))
        .map(|f| f.contents.clone())
        .expect("the upper layer earns a card of its own");
    assert!(
        cards.contains("ext:composes") && cards.contains("ext:replaces"),
        "both links, under the plugin's own words, on one card: {cards}"
    );

    // V manifestu: vrstva jako skalární fakt, štítky jako aditivní — `merge`
    // rozřešil každý podle deklarace, ne podle jednoho společného pravidla.
    let json = serde_json::to_string(&corpus.manifest).unwrap();
    assert!(
        json.contains("ext:layer") && json.contains("ext:tag"),
        "a declared fact reaches the manifest under its own name: {json}"
    );
    // The line above proves only that the kinds were DECLARED — the vocabulary
    // section carries those strings whether or not a single fact instance
    // survived projection. What the test claims is that the corpus shows the
    // layers, so the VALUES have to be asserted too. The external round's test
    // lens caught the difference.
    for hodnota in ["\"0\"", "\"1\"", "\"one\"", "\"two\""] {
        assert!(
            json.contains(hodnota),
            "a declared fact's value has to survive into the entity records, not \
             just its kind into the vocabulary: {hodnota} missing from {json}"
        );
    }
    let v = corpus.manifest.vocabulary.as_ref().unwrap();
    let cardinality = |name: &str| {
        v.fact_kinds
            .iter()
            .find(|k| k.kind.name == name)
            .unwrap()
            .cardinality
            .clone()
    };
    assert_eq!(cardinality("ext:layer"), "scalar");
    assert_eq!(cardinality("ext:tag"), "additive");
}

// ---------------------------------------------------------------------------
// Krok 3 — jádro nerozhodne nic z identity deklarovaného druhu
// ---------------------------------------------------------------------------

/// Brána R2 z Verification Contractu, znovu — ale na korpusu **se strukturou**,
/// ne na třech uzlech.
///
/// Ta v `vocabulary_declared.rs` staví jeden list pod kořenem. Tahle staví strom
/// s kontejnerem, dvěma definicemi, kartou a dvěma vrstvami — tedy korpus, ve
/// kterém má jádro co řadit, co kreslit a kam umístit cestu. Kdyby se cokoli z
/// toho rozhodovalo z otisku deklarovaného druhu, projeví se to tady, a ne tam.
#[test]
fn the_structured_corpus_keeps_nothing_of_a_declared_kind_but_its_name() {
    fn everything(container: &str, definition: &str, edge: &str) -> String {
        let mut g = Graph::new();
        let card = g.declare_card_kind("ext", definition).unwrap();
        let c = g
            .declare_node_kind(
                "ext",
                container,
                StructuralRole::Container,
                CardPolicy::None,
            )
            .unwrap();
        let d = g
            .declare_node_kind(
                "ext",
                definition,
                StructuralRole::Definition,
                CardPolicy::Card(card),
            )
            .unwrap();
        let e = g
            .declare_edge_kind("ext", edge, LinkRole::Containment)
            .unwrap();

        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
        g.ensure_node(NodeKey::new("area"), NodeKind::Declared(c))
            .unwrap();
        for leaf in ["area/alfa", "area/beta"] {
            g.ensure_node(NodeKey::new(leaf), NodeKind::Declared(d))
                .unwrap();
        }
        for (from, to) in [(".", "area"), ("area", "area/alfa"), ("area", "area/beta")] {
            g.add_edge(
                &NodeKey::new(from),
                &NodeKey::new(to),
                Edge::new(EdgeKind::Declared(e), prov(from)),
            );
        }

        let corpus = project(g, Lang::En);
        let mut out = serde_json::to_string_pretty(&corpus.manifest).unwrap();
        for f in corpus.files.iter().chain(&corpus.cards) {
            out.push_str(&f.path);
            out.push_str(&f.contents);
        }
        out.push_str(&reverse_architect::coverage::to_json(&corpus.coverage));
        out
    }

    // Druhá sada jmen je zvolená tak, aby si mezi sebou **zachovala abecední
    // pořadí** (`kontejner` < `struktura` jako `container` < `definition`).
    // Deklarované druhy se řadí jménem, takže přejmenování pozici posunout SMÍ
    // a gate by na tom padla, aniž by jádro udělalo cokoli špatně. Co tahle
    // brána tvrdí, je, že se z deklarovaného druhu nebere nic JINÉHO než jméno
    // — a řazení podle jména je záměr, ne nález. (První verze testu použila
    // `nadoba`/`definice`, které se řadí obráceně, a padla přesně na tomhle.)
    let a = everything("container", "definition", "holds");
    let b = everything("kontejner", "struktura", "drzi");
    let normalized = b
        .replace("kontejner", "container")
        .replace("struktura", "definition")
        .replace("drzi", "holds");
    assert_eq!(
        a, normalized,
        "the core kept something of a declared kind other than its name"
    );
}

// ---------------------------------------------------------------------------
// Determinismus — dva běhy, jeden korpus
// ---------------------------------------------------------------------------

/// Dva běhy nad týmž grafem dají **bajtově tytéž soubory**, měřeno na disku.
///
/// Jednotky výš to tvrdí o návratových hodnotách. Tahle to tvrdí o tom, co
/// opravdu zůstane na disku — včetně toho, že se cesty karet nesrazí a že
/// jeden běh nezapíše soubor, který druhý nezná.
#[test]
fn two_runs_of_one_graph_leave_identical_bytes_on_disk() {
    let first = tempfile::tempdir().expect("a temporary directory");
    let second = tempfile::tempdir().expect("a temporary directory");
    write(first.path(), &project(declared_shape(), Lang::En));
    write(second.path(), &project(declared_shape(), Lang::En));

    let a = on_disk(first.path());
    let b = on_disk(second.path());
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "the two runs wrote different file sets"
    );
    for (path, bytes) in &a {
        assert_eq!(bytes, &b[path], "{path} differs between two runs");
    }
}

/// Obě řeči, na korpusu, který na disk skutečně jde.
///
/// `render_card_files` bere `Lang` parametrem, takže korpus je vždy jednojazyčný
/// — konzument si vybírá. Deklarovaná karta musí vyjít v obou, a v obou musí
/// jmenovat svůj druh; víc než to jádro poctivě říct neumí.
#[test]
fn the_declared_card_reaches_the_disk_in_both_languages() {
    for lang in [Lang::En, Lang::Cs] {
        let dir = tempfile::tempdir().expect("a temporary directory");
        write(dir.path(), &project(declared_shape(), lang));
        let files = on_disk(dir.path());
        let (path, bytes) = files
            .iter()
            .find(|(p, _)| p.starts_with("cards/declared/ext/definition/"))
            .unwrap_or_else(|| panic!("{lang:?}: no declared card on disk"));
        let text = String::from_utf8(bytes.clone()).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            text.contains("ext:definition"),
            "{lang:?}: the card names its own kind: {text}"
        );
    }
}

/// Karta deklarovaného druhu vzniká, karta bez politiky ne — na korpusu, který
/// prošel celou projekcí, ne jen `build_cards`.
#[test]
fn only_the_card_bearing_declaration_earns_a_card_in_the_finished_corpus() {
    let corpus = project(declared_shape(), Lang::En);
    let declared: Vec<_> = build_cards(&declared_shape())
        .into_iter()
        .filter(|c| matches!(c.kind, CardKind::Declared(_)))
        .map(|c| c.identity)
        .collect();
    assert_eq!(
        declared,
        vec!["area/alfa".to_string(), "area/beta".to_string()],
        "the container declared `CardPolicy::None` and must earn none"
    );
    assert_eq!(
        corpus
            .cards
            .iter()
            .filter(|f| f.path.starts_with("cards/declared/"))
            .count(),
        2,
        "and the rendered corpus agrees with the card set"
    );
}
