//! R22 gate over the WHOLE output, not one projection.
//!
//! The plan's Verification Contract asks for "a test that no secret-shaped value
//! appears in the corpus OR in the export". Until this file existed the only
//! redaction test looked at a rendered card, so a secret travelled into
//! `manifest.json` unredacted — the fact itself was never filtered, only one
//! projection of it. These tests assert over every artifact a run writes.

mod common;

use common::init_repo_with_files;
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::render::RenderedFile;
use reverse_architect::{card, cli::LinkStyle, coverage, generate, manifest, Generated};
use std::path::Path;

/// Every distinctive secret value the fixture contains. None of these may appear
/// anywhere in any artifact.
const SECRETS: &[&str] = &[
    "hunter2-do-not-ship",
    "SuperSecretConnString1",
    "MIIEowIBAAKCAQEAx7fakekeymaterial",
    "sk-live-9f2b7c11ea-fake",
];

/// What a redaction leaves behind. Spelled out here rather than imported: the
/// marker is what a READER sees, so a test that asserts on it must break when the
/// visible text changes, not follow the constant silently.
const REDACTION_MARKER: &str = "[redacted: ";

const BASE_FILES: [(&str, &str); 2] = [
    (
        "src/Conn.cs",
        "namespace Orbit.Infra\n\
                 {\n\
                 \x20   /// <summary>Example: password = \"hunter2-do-not-ship\"</summary>\n\
                 \x20   public class Conn\n\
                 \x20   {\n\
                 \x20       private const string Fallback = \
                 \"Server=sql01;Database=orbit;User Id=sa;Password=SuperSecretConnString1\";\n\
                 \x20   }\n\
                 }\n",
    ),
    (
        "src/keys.rs",
        "//! Deployment notes.\n\
                 //!\n\
                 //! -----BEGIN RSA PRIVATE KEY-----\n\
                 //! MIIEowIBAAKCAQEAx7fakekeymaterial\n\
                 //! -----END RSA PRIVATE KEY-----\n\
                 \n\
                 /// Rotate this monthly. api_key = \"sk-live-9f2b7c11ea-fake\"\n\
                 pub struct Keys {\n\
                 \x20   pub id: i64,\n\
                 }\n",
    ),
];

fn fixture(dir: &Path) {
    init_repo_with_files(dir, "Alice", &BASE_FILES);
}

fn config_for(repo: &Path, out: &Path) -> Config {
    Config {
        repo_root: repo.to_path_buf(),
        corpus_dir: out.to_path_buf(),
        links: LinkStyle::Portable,
        profile: None,
        ambiguity_ceiling: DEFAULT_AMBIGUITY_CEILING,
        secret_prefixes: Vec::new(),
        plugins: Vec::new(),
        scip_index: None,
        lang: "en".to_string(),
        sites: false,
        rights: reverse_architect::extract::RightsTable::granted(),
        model_verification: false,
        model_purpose: false,
        trace_over_ceiling: false,
        detail_level: None,
    }
}

/// Every rendered artifact of one run, as (label, contents).
fn all_artifacts(repo: &Path, out: &Path) -> Vec<(String, String)> {
    artifacts_of(
        &generate(&config_for(repo, out)).expect("the pipeline runs"),
        out,
    )
}

/// Where a run into `out` keeps the second layer's model of bodies (plan 2a,
/// KTD8): one record per file, stored before anything else reads it.
const BODY_RECORDS: &str = ".ra-cache/languages-bodies";

/// Every rendered artifact of `generated`, written into `out`, as (label,
/// contents) — and every record of the second layer under `out`, the one
/// surface a body's text reaches before any later unit renders it.
fn artifacts_of(generated: &Generated, out: &Path) -> Vec<(String, String)> {
    let mut artifacts: Vec<(String, String)> = Vec::new();
    for file in &generated.corpus.files {
        artifacts.push((format!("view {}", file.path), file.contents.clone()));
    }
    for file in &generated.cards {
        artifacts.push((format!("card {}", file.path), file.contents.clone()));
    }
    // A type's members, anchors and relations are its detail's since plan B,
    // U9 — the surface a secret would now travel on.
    for file in &generated.details {
        artifacts.push((format!("detail {}", file.path), file.contents.clone()));
    }
    // A type's row with its purpose and its database accesses (plan B, U10).
    for file in &generated.maps {
        artifacts.push((format!("map {}", file.path), file.contents.clone()));
    }
    artifacts.push((
        "manifest.json".to_string(),
        manifest::to_json(&generated.manifest),
    ));
    // The line-delimited form a large corpus is written in instead — which
    // one a run writes is decided by size, not by the caller, so both are
    // swept.
    for (name, form) in [
        (
            manifest::STRINGS_FILENAME,
            manifest::to_strings_json as fn(&manifest::Manifest) -> String,
        ),
        (manifest::NODES_FILENAME, manifest::to_nodes_jsonl),
        (manifest::EDGES_FILENAME, manifest::to_edges_jsonl),
        (manifest::DOCUMENTS_FILENAME, manifest::to_documents_jsonl),
    ] {
        artifacts.push((name.to_string(), form(&generated.manifest)));
    }
    artifacts.push((
        manifest::SITES_FILENAME.to_string(),
        manifest::to_sites_jsonl(&generated.graph),
    ));
    artifacts.push((
        "coverage.json".to_string(),
        coverage::to_json(&generated.coverage),
    ));
    if let Ok(entries) = std::fs::read_dir(out.join(BODY_RECORDS)) {
        let mut records: Vec<std::path::PathBuf> = entries.map(|e| e.unwrap().path()).collect();
        records.sort();
        for path in records {
            let bytes = std::fs::read(&path).unwrap();
            artifacts.push((
                format!("record {}", path.file_name().unwrap().to_string_lossy()),
                String::from_utf8_lossy(&bytes).into_owned(),
            ));
        }
    }
    artifacts
}

/// What plants one fixture into a repository directory.
type Plant = fn(&Path);

/// Every fixture of this file, by name: the gate below sweeps them all.
const FIXTURES: [(&str, Plant); 4] = [
    ("base", fixture),
    ("anchors", anchor_fixture),
    ("area b", b_fixture),
    ("bodies", body_fixture),
];

/// The levels a run is swept at: without the second layer, and with it.
const LEVELS: [Option<u8>; 2] = [None, Some(2)];

/// R22 over every fixture of this file, with the second layer and without
/// it: no secret of any of them reaches an artifact or a record of a body
/// (plan 2a, KTD16).
#[test]
fn no_secret_value_reaches_any_artifact_of_a_run() {
    for (name, plant) in FIXTURES {
        for level in LEVELS {
            let repo = tempfile::tempdir().unwrap();
            let out = tempfile::tempdir().unwrap();
            plant(repo.path());
            let config = Config {
                detail_level: level,
                ..b_config(repo.path(), out.path())
            };
            let generated = generate(&config).expect("the pipeline runs");
            let artifacts = artifacts_of(&generated, out.path());
            assert!(
                artifacts.len() > 5,
                "the fixture must produce a real corpus, saw {}",
                artifacts.len()
            );
            let records: Vec<&str> = artifacts
                .iter()
                .filter(|(label, _)| label.starts_with("record "))
                .map(|(_, contents)| contents.as_str())
                .collect();
            assert_eq!(
                records.is_empty(),
                level.is_none(),
                "{name}: a run with the second layer stores its bodies, one without none"
            );
            for (label, contents) in &artifacts {
                assert_no_b_secret(&format!("{label} ({name}, level {level:?})"), contents);
            }
            // Over-redaction is the failure direction, but not this far: the
            // messages support searches for stay in the model (AE5).
            if name == "area b" && level.is_some() {
                assert!(
                    records
                        .iter()
                        .any(|r| MESSAGES.iter().all(|m| r.contains(m))),
                    "the messages stay in the record of their body: {records:#?}"
                );
            }
        }
    }
}

// ---- The bodies of the second layer (plan 2a, U8: KTD16) ----

/// The secret values the bodies fixture carries: compared with a name that
/// says what the value is, a provider token given to a field whose name says
/// it and one an attribute's argument is, and a comment inside a condition
/// and after a call. The field's value has a shape of its own: a value only
/// its name names reaches the first layer's signature of the field, which
/// this unit leaves as it is (the second layer withholds it, see the unit
/// tests of `extract::bodies`).
const BODY_SECRETS: &[&str] = &[
    "brindlecairn",
    "ghp_Sm7Tp2Pw9Rd4Xk8Lq3Vn6Bz1",
    "ghp_Lq4Wm8Zt2Xv6Nb1Kp9Rs3Dy7Hc5",
    "hollyquartz",
    "ghp_Wm8Zt2Xv6Nb1Kp9Rs3Dy7Hc5Lq4",
];

/// A body for every shape of [`BODY_SECRETS`]; a name with a character of
/// two bytes stands before the compared literal on its line (that the
/// pairing counts columns in characters is shown by the unit tests of
/// `extract::bodies`).
const GATE_CS: &str = "namespace Orbit.Gate\n\
{\n\
\x20   public class Gate\n\
\x20   {\n\
\x20       private const string SmtpPassword = \"ghp_Sm7Tp2Pw9Rd4Xk8Lq3Vn6Bz1\";\n\
\x20       [DefaultValue(\"ghp_Lq4Wm8Zt2Xv6Nb1Kp9Rs3Dy7Hc5\")]\n\
\x20       public string Mode { get; set; }\n\
\x20       public bool Check(string token, string žeton)\n\
\x20       {\n\
\x20           if (žeton == \"plain\" || token == \"brindlecairn\") { return true; }\n\
\x20           if (GetResourceString(\"k\") /* heslo: hollyquartz */ == null) { return false; }\n\
\x20           return GetResourceString(\"k\") /* heslo: hollyquartz */ != null;\n\
\x20       }\n\
\x20       public string GetResourceString(string key) { return key; }\n\
\x20       public void Admit(string token, string mode)\n\
\x20       {\n\
\x20           if (token == \"ghp_Wm8Zt2Xv6Nb1Kp9Rs3Dy7Hc5Lq4\") { Grant(); }\n\
\x20           if (mode == \"plain\") { Deny(); }\n\
\x20       }\n\
\x20       void Grant() { }\n\
\x20       void Deny() { }\n\
\x20   }\n\
}\n";

/// The base fixture plus the bodies file.
fn body_fixture(dir: &Path) {
    let mut files = BASE_FILES.to_vec();
    files.push(("src/Gate.cs", GATE_CS));
    init_repo_with_files(dir, "Alice", &files);
}

/// The second layer read the bodies fixture — the compared literal that is
/// no secret is in its record — and withheld every secret of it there.
#[test]
fn a_body_keeps_its_plain_literals_and_withholds_its_secrets_in_the_record() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    body_fixture(repo.path());
    let config = Config {
        detail_level: Some(2),
        ..b_config(repo.path(), out.path())
    };
    let generated = generate(&config).expect("the pipeline runs");
    let records: Vec<(String, String)> = artifacts_of(&generated, out.path())
        .into_iter()
        .filter(|(label, _)| label.starts_with("record "))
        .collect();
    let gate = records
        .iter()
        .map(|(_, r)| r)
        .find(|r| r.contains("\"path\":\"src/Gate.cs\""))
        .unwrap_or_else(|| panic!("a record of the bodies file: {records:#?}"));
    assert!(gate.contains("\"value\":\"plain\""), "{gate}");
    assert!(gate.contains(REDACTION_MARKER), "{gate}");
    for secret in BODY_SECRETS {
        assert!(
            !gate.contains(secret),
            "`{secret}` is in the record: {gate}"
        );
    }
}

/// A key-shaped word in the comment after a call argument — the label of a
/// menu item — is cut the way the first layer cuts it in any comment
/// (`long token`): the quote is withheld whole, in the record of the body,
/// on the card and in the manifest.
#[test]
fn a_long_token_in_a_label_comment_is_withheld_in_the_record_and_on_the_card() {
    const TOKEN: &str = "9f2b8c1d4e5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c";
    let view = format!(
        "namespace Acme.Menus\n\
{{\n\
\x20   public class MenuView\n\
\x20   {{\n\
\x20       private ContextMenu menu;\n\
\x20       public string GetResourceString(string key) {{ return key; }}\n\
\x20       void BuildMenu()\n\
\x20       {{\n\
\x20           ContextMenuItem f4 = new ContextMenuItem();\n\
\x20           f4.Header = GetResourceString(\"k\") /* {TOKEN} */;\n\
\x20           f4.Command = CheckCommand;\n\
\x20           menu.Items.Add(f4);\n\
\x20       }}\n\
\x20       public RelayCommand CheckCommand => new RelayCommand(p => Check());\n\
\x20       public void Check() {{ }}\n\
\x20   }}\n\
}}\n"
    );
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Menus.cs", &view)]);
    let config = Config {
        detail_level: Some(2),
        ..config_for(repo.path(), out.path())
    };
    let generated = generate(&config).expect("the pipeline runs");
    let artifacts = artifacts_of(&generated, out.path());
    for (label, contents) in &artifacts {
        assert!(
            !contents.contains(TOKEN),
            "the token is in {label}: {contents}"
        );
    }
    let record = artifacts
        .iter()
        .find(|(label, contents)| {
            label.starts_with("record ") && contents.contains("\"path\":\"src/Menus.cs\"")
        })
        .map(|(_, contents)| contents)
        .unwrap_or_else(|| panic!("a record of the menu file"));
    assert!(
        record.contains("[redacted: long token"),
        "the quote is withheld, not dropped: {record}"
    );
    let card = generated
        .cards
        .iter()
        .find(|c| c.path.starts_with("cards/type/") && c.contents.contains("\n# MenuView\n"))
        .unwrap_or_else(|| panic!("the card of MenuView"));
    let item = card
        .contents
        .lines()
        .find(|l| l.starts_with("- Menu item `f4`"))
        .unwrap_or_else(|| panic!("no menu item f4:\n{}", card.contents));
    assert!(item.contains("[redacted: long token"), "{item}");
}

/// The card of the type `Gate` of the bodies fixture, from a run at `level`
/// in `lang`, and every artifact and record of that run.
fn gate_run(level: u8, lang: &str) -> (String, Vec<(String, String)>) {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    body_fixture(repo.path());
    let config = Config {
        detail_level: Some(level),
        lang: lang.to_string(),
        ..b_config(repo.path(), out.path())
    };
    let generated = generate(&config).expect("the pipeline runs");
    let card = generated
        .cards
        .iter()
        .find(|c| c.path.starts_with("cards/type/") && c.contents.contains("\n# Gate\n"))
        .unwrap_or_else(|| panic!("the card of Gate"))
        .contents
        .clone();
    (card, artifacts_of(&generated, out.path()))
}

/// R14, KTD16, AE5: a step whose text carried a secret keeps the marker and
/// says, once on its line, that a value was left out; the step beside it
/// that compares `mode` with a plain string shows the string. The secret is
/// in no artifact and no record of the run — in either language.
#[test]
fn a_withheld_value_is_admitted_in_the_card_and_a_plain_one_stays_visible() {
    for (lang, sentence, under_diagram, plain_step) in [
        (
            "en",
            "a value was left out as a likely secret; see the source",
            "A value in the diagram was left out as a likely secret; see the source.",
            "when `mode == \"plain\"`",
        ),
        (
            "cs",
            "hodnota vynechána jako pravděpodobné tajemství; viz zdroj",
            "Hodnota v diagramu vynechána jako pravděpodobné tajemství; viz zdroj.",
            "když `mode == \"plain\"`",
        ),
    ] {
        let (card, artifacts) = gate_run(2, lang);
        // The English run is swept by `no_secret_value_reaches_any_artifact_of_a_run`
        // over this very fixture; the Czech one is new.
        if lang == "cs" {
            for (label, contents) in &artifacts {
                assert_no_b_secret(&format!("{label} ({lang})"), contents);
            }
        }
        assert!(!card.contains("ghp_"), "{lang}: {card}");
        assert!(card.contains(plain_step), "{lang}: {plain_step}\n{card}");

        // Outside the diagrams: the marker stays, and its line says once
        // that a value was left out; no line says it without a marker.
        let mut in_diagram = false;
        let mut diagram_lines: Vec<&str> = Vec::new();
        let mut admitting = 0;
        for line in card.lines() {
            if line.starts_with("```mermaid") {
                in_diagram = true;
            } else if line.starts_with("```") {
                in_diagram = false;
            } else if in_diagram {
                diagram_lines.push(line);
            } else if line.contains(REDACTION_MARKER) {
                admitting += 1;
                assert_eq!(line.matches(sentence).count(), 1, "{lang}: {line}");
            } else {
                assert!(!line.contains(sentence), "{lang}: {line}");
            }
        }
        assert!(
            admitting >= 2,
            "{lang}: the steps with a withheld value are on the card"
        );

        // In the diagrams: the node keeps the marker, its quotes are
        // balanced, and one sentence under each diagram says what was left.
        let node = diagram_lines
            .iter()
            .find(|l| l.contains(REDACTION_MARKER))
            .unwrap_or_else(|| panic!("{lang}: no node with a marker"));
        assert_eq!(node.matches('"').count() % 2, 0, "{node}");
        assert!(!node.contains("-->"), "{node}");
        assert_eq!(
            card.matches(under_diagram).count(),
            card.matches("```mermaid").count(),
            "{lang}: every diagram of Gate holds a marker and says so"
        );
    }
}

/// `value` without the keys named, at the top level or one level down
/// (`noise.plugins_by_contract`).
fn without(mut value: serde_json::Value, keys: &[&str]) -> serde_json::Value {
    for key in keys {
        match key.split_once('.') {
            Some((outer, inner)) => {
                value[outer].as_object_mut().unwrap().remove(inner);
            }
            None => {
                value.as_object_mut().unwrap().remove(*key);
            }
        }
    }
    value
}

/// A card's front matter, without its fences, and the rest of it.
fn front_matter(card: &str) -> (&str, &str) {
    let rest = card.strip_prefix("---\n").expect("front matter");
    let end = rest.find("\n---\n\n").expect("the end of the front matter");
    (&rest[..end], &rest[end + "\n---\n\n".len()..])
}

/// The parts of a card the second layer leaves as they are: everything but
/// the purpose and the keywords, which it rewrites, and the functionalities
/// and states, which it adds (plan 2a, U6: KTD10).
fn first_layer_sections(body: &str) -> String {
    let mut out = String::new();
    let mut keep = true;
    for line in body.split_inclusive('\n') {
        if let Some(heading) = line.strip_prefix("## ") {
            keep = !["Purpose", "Keywords", "Functionality: ", "State: "]
                .iter()
                .any(|h| heading.starts_with(h));
        }
        if keep {
            out.push_str(line);
        }
    }
    out
}

/// The first layer's bytes do not move with the second layer: over every
/// fixture of this file, every view, detail and map, every card but a type's,
/// the first-layer sections of a type's card, the manifest forms and the sites
/// are what a run without the layer writes, and the manifest and coverage
/// differ only by what the layer adds to them.
#[test]
fn the_first_layer_is_the_same_bytes_with_the_second_layer_and_without() {
    for (name, plant) in FIXTURES {
        let repo = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        plant(repo.path());
        let run = |level: Option<u8>, dir: &str| {
            generate(&Config {
                detail_level: level,
                ..b_config(repo.path(), &out.path().join(dir))
            })
            .expect("the pipeline runs")
        };
        let plain = run(None, "plain");
        let layered = run(Some(2), "layered");
        assert_eq!(plain.corpus, layered.corpus, "{name}: the views differ");
        // Plan 2a, U6 (KTD9 point 2): a type's card names the second layer in
        // its front matter and, with the type's analysis, rewrites its purpose
        // and keywords and adds what the layer read; every other part of it,
        // and every other card, is the plain run's. The secrets of the cards
        // are checked with every other file of the corpus.
        assert_eq!(plain.cards.len(), layered.cards.len(), "{name}");
        for (p, l) in plain.cards.iter().zip(&layered.cards) {
            assert_eq!(p.path, l.path, "{name}");
            if !p.path.starts_with("cards/type/") {
                assert_eq!(p, l, "{name}: {}", p.path);
                continue;
            }
            let (plain_front, plain_body) = front_matter(&p.contents);
            let (front, body) = front_matter(&l.contents);
            let (_, word) = front.rsplit_once("\nlayer2: ").expect("a layer2 word");
            assert_eq!(
                front,
                format!(
                    "{}\ndetail_level: 2\nlayer2: {word}",
                    plain_front.replace("schema: 6\n", "schema: 7\n")
                ),
                "{name}: {}",
                p.path
            );
            if word == "present" {
                assert_eq!(
                    first_layer_sections(body),
                    first_layer_sections(plain_body),
                    "{name}: {}",
                    p.path
                );
            } else {
                assert_eq!(body, plain_body, "{name}: {}", p.path);
            }
        }
        assert_eq!(plain.details, layered.details, "{name}: the details differ");
        assert_eq!(plain.maps, layered.maps, "{name}: the maps differ");
        for form in [
            manifest::to_nodes_jsonl as fn(&manifest::Manifest) -> String,
            manifest::to_edges_jsonl,
        ] {
            assert_eq!(form(&plain.manifest), form(&layered.manifest), "{name}");
        }
        // A type card's document gains the chunks of its new sections.
        let unchunked = |m: &manifest::Manifest| -> Vec<serde_json::Value> {
            manifest::to_documents_jsonl(m)
                .lines()
                .map(|l| {
                    let mut d: serde_json::Value = serde_json::from_str(l).unwrap();
                    if d["kind"] == "card:type" {
                        d.as_object_mut().unwrap().remove("chunks");
                    }
                    d
                })
                .collect()
        };
        assert_eq!(
            unchunked(&plain.manifest),
            unchunked(&layered.manifest),
            "{name}"
        );
        // The strings form carries the manifest's head, the plugins with it.
        let strings = |g: &Generated, keys: &[&str]| {
            without(
                serde_json::from_str(&manifest::to_strings_json(&g.manifest)).unwrap(),
                keys,
            )
        };
        assert_eq!(
            strings(&plain, &["plugins"]),
            strings(&layered, &["plugins", "detail_level"]),
            "{name}: the strings form differs beyond the layer's own rows"
        );
        assert_eq!(
            manifest::to_sites_jsonl(&plain.graph),
            manifest::to_sites_jsonl(&layered.graph),
            "{name}"
        );
        let manifest_value = |g: &Generated, keys: &[&str]| {
            let mut v = without(serde_json::to_value(&g.manifest).unwrap(), keys);
            for d in v["documents"].as_array_mut().unwrap() {
                if d["kind"] == "card:type" {
                    d.as_object_mut().unwrap().remove("chunks");
                }
            }
            v
        };
        assert_eq!(
            manifest_value(&plain, &["plugins"]),
            manifest_value(&layered, &["plugins", "detail_level"]),
            "{name}: the manifest differs beyond the layer's own rows"
        );
        // The sizes of the type cards and the count of cards without keywords
        // are the second layer's; every other level of the cards is not.
        let other_levels = |c: &serde_json::Value| {
            let mut cards = c["cards"].clone();
            cards["sizes"].as_object_mut().unwrap().remove("type_card");
            cards.as_object_mut().unwrap().remove("without_keywords");
            cards
        };
        let (pc, lc) = (
            serde_json::to_value(&plain.coverage).unwrap(),
            serde_json::to_value(&layered.coverage).unwrap(),
        );
        assert_eq!(other_levels(&pc), other_levels(&lc), "{name}");
        assert_eq!(
            without(pc, &["noise.plugins_by_contract", "cards"]),
            without(lc, &["bodies", "noise.plugins_by_contract", "cards"]),
            "{name}: coverage differs beyond the layer's own rows"
        );
    }
}

#[test]
fn the_manifest_export_is_covered_not_just_the_card() {
    // The regression this file exists for: redaction lived in the card renderer
    // only, so the same fact reached manifest.json in the clear.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(repo.path());

    let generated = generate(&config_for(repo.path(), out.path())).unwrap();
    let json = manifest::to_json(&generated.manifest);

    assert!(
        json.contains("doc_comment"),
        "the export must still carry doc comments — this is not a test that they vanished"
    );
    for secret in SECRETS {
        assert!(
            !json.contains(secret),
            "R22: `{secret}` leaked into the export"
        );
    }
    assert!(
        json.contains("[redacted:"),
        "the export should show the marker where a secret was removed"
    );
}

#[test]
fn a_multi_line_pem_block_is_redacted_through_its_end_line() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(repo.path());

    let artifacts = all_artifacts(repo.path(), out.path());
    let joined: String = artifacts.iter().map(|(_, c)| c.as_str()).collect();

    // The key material is the line a line-wise filter would have published: the
    // BEGIN header matches the pattern, the base64 body matches nothing.
    assert!(
        !joined.contains("MIIEowIBAAKCAQEAx7fakekeymaterial"),
        "{joined}"
    );
    assert!(
        joined.contains("[redacted: private key"),
        "the block must be named as a private key: {joined}"
    );
}

#[test]
fn a_field_initializer_does_not_travel_in_the_signature() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(repo.path());

    let generated = generate(&config_for(repo.path(), out.path())).unwrap();

    // A declaration with no body keeps its initializer in the signature header, so
    // the signature is a secret carrier exactly like a doc comment. A member's
    // signature is in its type's detail since plan B, U9 (the API list).
    let signatures: String = generated
        .cards
        .iter()
        .chain(&generated.details)
        .filter(|f| {
            f.contents.contains("## Signature")
                || f.contents.contains("signature:")
                || f.contents.contains("## API")
        })
        .map(|f| f.contents.as_str())
        .collect();
    // Assert the thing exists before asserting what it does not contain. Without
    // this the test passes just as happily when signatures stop being rendered at
    // all — proving the absence of a secret in the absence of the artifact.
    assert!(
        !signatures.is_empty(),
        "the fixture must actually render a signature, or the assertion below is vacuous"
    );
    assert!(
        signatures.contains("Fallback"),
        "the field itself must still be listed, redacted rather than dropped: {signatures}"
    );
    assert!(
        signatures.contains(REDACTION_MARKER),
        "and its initializer must be visibly withheld: {signatures}"
    );
    assert!(
        !signatures.contains("SuperSecretConnString1"),
        "R22: a connection string travelled in a signature: {signatures}"
    );
}

#[test]
fn ordinary_prose_and_code_survive_the_filter() {
    // Over-redaction is the intended failure direction, but it must not eat the
    // whole comment: a filter that redacts everything would pass every assertion
    // above while destroying the corpus.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(repo.path());

    let artifacts = all_artifacts(repo.path(), out.path());
    let joined: String = artifacts.iter().map(|(_, c)| c.as_str()).collect();

    assert!(
        joined.contains("Deployment notes."),
        "prose above a secret must survive: {joined}"
    );
    // Prose sharing a line with the secret shape goes with that line — the whole
    // line is replaced by design, because a partial mask has to guess where the
    // value ends. What must survive is everything else.
    assert!(
        joined.contains("Keys"),
        "the declaration itself must survive: {joined}"
    );
    assert!(
        joined.contains("Example:") || joined.contains("[redacted: password"),
        "the C# doc comment must be present, redacted rather than dropped: {joined}"
    );
}

#[test]
fn two_runs_still_agree_after_redaction() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    fixture(repo.path());

    let first = all_artifacts(repo.path(), out.path());
    let second = all_artifacts(repo.path(), out.path());
    assert_eq!(first, second, "redaction must be deterministic (R18)");

    // The redaction of the second layer too: a cold run and a warm one, and
    // a run into another directory, write the same records (plan 2a, U8).
    let repo = tempfile::tempdir().unwrap();
    body_fixture(repo.path());
    let layered = |out: &Path| {
        let config = Config {
            detail_level: Some(2),
            ..b_config(repo.path(), out)
        };
        artifacts_of(&generate(&config).expect("the pipeline runs"), out)
    };
    let out = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let cold = layered(out.path());
    assert!(cold.iter().any(|(label, _)| label.starts_with("record ")));
    assert_eq!(cold, layered(out.path()), "a warm run differs");
    assert_eq!(cold, layered(other.path()), "another directory differs");
}

#[test]
fn the_card_still_flags_that_something_was_removed() {
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());

    let repo_handle = reverse_architect::extract::Repo::open(repo.path()).unwrap();
    let mut graph = reverse_architect::graph::Graph::new();
    reverse_architect::extract::run_all(&repo_handle, &mut graph).unwrap();
    reverse_architect::graph::merge(&mut graph, &reverse_architect::extract::trust_ranks());

    let cards = card::build_cards(&graph);
    let conn = cards
        .iter()
        .find(|c| c.title == "Conn")
        .expect("a Conn card");

    // The reader must still learn that a value was withheld and where to look.
    assert!(
        conn.doc.as_ref().is_some_and(|d| d.redacted),
        "the card must still report the redaction: {:?}",
        conn.doc
    );
    assert!(
        conn.caveats
            .iter()
            .any(|c| c.text.contains("secret pattern")),
        "and name it in the notes: {:?}",
        conn.caveats
    );
    // …on the page a reader receives: since the card trim (plan B) a type's
    // comment is quoted in its detail, and the note about it sits there too.
    let detail = rendered_detail(&cards, "Conn");
    assert!(
        detail.contains("\n## Documentation\n") && detail.contains(REDACTION_MARKER),
        "the detail quotes the comment with the marker: {detail}"
    );
    assert!(
        detail
            .split("\n## Notes\n")
            .nth(1)
            .is_some_and(|notes| notes.contains("secret pattern")),
        "and its notes say a line was replaced: {detail}"
    );
}

/// The rendered detail of the type card titled `title`.
fn rendered_detail(cards: &[card::Card], title: &str) -> String {
    let index = card::links::CardIndex::build(cards);
    let card = cards
        .iter()
        .find(|c| c.title == title)
        .unwrap_or_else(|| panic!("a {title} card"));
    card::detail::render_detail(card, card::render::Lang::En, LinkStyle::Portable, &index)
        .unwrap_or_else(|| panic!("{title} has a detail"))
}

#[test]
fn an_unterminated_key_block_does_not_swallow_the_rest_of_the_comment() {
    // Someone quoting the SHAPE of a key in a comment writes the BEGIN line and
    // then stops. A filter that waits for `-----END` before letting go redacts
    // every line after it to the end of the text — the comment loses everything
    // it was actually written to say, and the corpus loses it silently.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[(
            "src/notes.rs",
            "//! Key rotation.\n\
             //!\n\
             //! The deployment secret is stored as a PEM block, i.e.\n\
             //! -----BEGIN RSA PRIVATE KEY-----\n\
             //!\n\
             //! Ask the platform team for the current one, and file it under\n\
             //! the rotation ticket before the quarter closes.\n\
             \n\
             pub struct Notes;\n",
        )],
    );

    let artifacts = all_artifacts(repo.path(), out.path());
    let joined: String = artifacts.iter().map(|(_, c)| c.as_str()).collect();

    assert!(
        joined.contains("[redacted: private key"),
        "the BEGIN line itself must still go: {joined}"
    );
    assert!(
        joined.contains("Ask the platform team for the current one"),
        "prose after the unterminated block must survive: {joined}"
    );
    assert!(
        joined.contains("rotation ticket before the quarter closes"),
        "including the last line of it: {joined}"
    );
}

#[test]
fn a_credential_carried_in_a_url_never_reaches_an_artifact() {
    // The shape with no keyword on the line: the password is the URL's userinfo.
    // The literal path already treated this as a credential; a doc comment
    // reached the card through the pattern table alone, which had no rule for it.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[(
            "src/store.rs",
            "/// Connects to mongodb://app:hunter2-in-a-url@db01:27017/orbit on start.\n\
             ///\n\
             /// Clone the fixtures from git+ssh://git@github.com/orbit/fixtures first.\n\
             pub struct Store;\n",
        )],
    );

    let artifacts = all_artifacts(repo.path(), out.path());
    for (label, contents) in &artifacts {
        assert!(
            !contents.contains("hunter2-in-a-url"),
            "R22: a URL credential leaked into {label}"
        );
    }

    let joined: String = artifacts.iter().map(|(_, c)| c.as_str()).collect();
    assert!(
        joined.contains("[redacted: credentials in a URL"),
        "and the reader must be told what was withheld: {joined}"
    );
    // A userinfo with no password is not a secret, and half the READMEs ever
    // written contain one. Redacting that line would cost documentation for
    // nothing.
    assert!(
        joined.contains("git+ssh://git@github.com/orbit/fixtures"),
        "a passwordless userinfo must survive: {joined}"
    );
}

#[test]
fn a_secret_withheld_inside_a_method_is_confessed_in_the_owning_types_detail() {
    // A member owns no card (KTD5). Its `RedactedLiteral` fact therefore has
    // nowhere to surface unless the owning type's card lifts it — the same
    // reason U9 lifts member relations. Without that the corpus records the
    // withholding and tells no reader about it. Since the card trim (plan B)
    // the confession is in the type's detail, beside the anchors the withheld
    // literal left its place in.
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[(
            "src/Wire.cs",
            "namespace Orbit.Infra\n\
             {\n\
             \x20   public class Wire\n\
             \x20   {\n\
             \x20       public string Open()\n\
             \x20       {\n\
             \x20           return \
             \"Server=sql01;Database=orbit;User Id=sa;Password=method-local-secret\";\n\
             \x20       }\n\
             \x20   }\n\
             }\n",
        )],
    );

    let repo_handle = reverse_architect::extract::Repo::open(repo.path()).unwrap();
    let mut graph = reverse_architect::graph::Graph::new();
    reverse_architect::extract::run_all(&repo_handle, &mut graph).unwrap();
    reverse_architect::graph::merge(&mut graph, &reverse_architect::extract::trust_ranks());

    let cards = card::build_cards(&graph);
    let wire = cards
        .iter()
        .find(|c| c.title == "Wire")
        .expect("a Wire card");

    let notes = rendered_detail(&cards, "Wire")
        .split("\n## Notes\n")
        .nth(1)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("Wire's detail has notes: {:?}", wire.caveats));
    assert!(
        notes
            .lines()
            .any(|l| l.contains("`Open`") && l.contains("`password`")),
        "the note must name the member it came from and the shape withheld: {notes}"
    );
    assert!(!notes.contains("method-local-secret"), "{notes}");
}

#[test]
fn a_provider_token_in_prose_is_redacted_end_to_end() {
    // The shape C28 is about: a credential with no keyword and no separator
    // anywhere on the line. Nothing in the pattern table saw it before, because
    // every rule needed a `password =`-style assignment to fire.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[(
            "src/deploy.rs",
            "//! Deployment.\n\
             //!\n\
             //! Paste ghp_Zz9Yy8Xx7Ww6Vv5Uu4Tt3Ss2Rr1Qq into the CI secret store.\n\
             //!\n\
             //! GitHub personal access tokens start with ghp_ and OpenAI keys with sk-.\n\
             pub struct Deploy;\n",
        )],
    );

    let artifacts = all_artifacts(repo.path(), out.path());
    for (label, contents) in &artifacts {
        assert!(
            !contents.contains("ghp_Zz9Yy8Xx7Ww6Vv5Uu4Tt3Ss2Rr1Qq"),
            "R22: a provider token leaked into {label}"
        );
    }

    let joined: String = artifacts.iter().map(|(_, c)| c.as_str()).collect();
    assert!(
        joined.contains("[redacted: provider token"),
        "and the reader must be told what shape was withheld: {joined}"
    );
    // The sentence that merely NAMES the prefixes must survive. A match replaces
    // the whole line, so this is the expensive mistake, and the one that makes a
    // prefix rule safe to run without a keyword.
    assert!(
        joined.contains("GitHub personal access tokens start with"),
        "prose naming a prefix must not be redacted: {joined}"
    );
}

#[test]
fn a_configured_company_prefix_is_redacted_and_the_public_ones_still_are() {
    // `--secret-prefix` exists so a company token shape no public list knows can
    // be recognized without a rebuild. It must ADD to the built-in list, never
    // replace it: an operator who names their own must not silently lose the
    // public ones.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[(
            "src/wire.rs",
            "/// Ours: acmetok-AaBbCcDdEeFfGgHhIiJjKk\n\
             /// Theirs: ghp_Mm1Nn2Oo3Pp4Qq5Rr6Ss7Tt8Uu\n\
             pub struct Wire;\n",
        )],
    );

    let mut config = config_for(repo.path(), out.path());
    config.secret_prefixes = vec!["acmetok-".to_string()];
    let generated = generate(&config).expect("the pipeline runs");
    // The comment is quoted in the type's detail since the card trim.
    let joined: String = generated
        .cards
        .iter()
        .chain(&generated.details)
        .chain(generated.corpus.files.iter())
        .map(|f| f.contents.as_str())
        .chain(std::iter::once(
            manifest::to_json(&generated.manifest).as_str(),
        ))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !joined.contains("acmetok-AaBbCcDdEeFfGgHhIiJjKk"),
        "the configured prefix must be honoured: {joined}"
    );
    // Negative control: without the configuration the same token survives, so
    // the assertion above is about the wiring and not about some other rule
    // happening to catch it.
    let out_default = tempfile::tempdir().unwrap();
    let unconfigured = generate(&config_for(repo.path(), out_default.path())).unwrap();
    let plain: String = unconfigured
        .cards
        .iter()
        .chain(&unconfigured.details)
        .map(|f| f.contents.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plain.contains("acmetok-AaBbCcDdEeFfGgHhIiJjKk"),
        "a company prefix nobody configured is NOT a built-in one: {plain}"
    );
    assert!(
        !joined.contains("ghp_Mm1Nn2Oo3Pp4Qq5Rr6Ss7Tt8Uu"),
        "and the public list must still be in force: {joined}"
    );
}

// ---- The surfaces anchors added (plan B, U3 and U4: R10, KTD11) ----

/// The secret values the anchor surfaces carry: comments, literals read with
/// the code on their line, the Czech and SQL spellings, and a C header.
const ANCHOR_SECRETS: &[&str] = &[
    "MIIEcommentkeymaterialSentinel",
    "hunter2-comment-nextline",
    "pivo-heslo-sentinel",
    "kocka-secret-sentinel",
    "sql-pw-sentinel",
    "9f2b7c11ea3d40aa9c1fdeadbeef0123",
    "header-pw-sentinel",
    "define-pw-sentinel",
];

/// The base fixture plus one file per new surface.
fn anchor_fixture(dir: &Path) {
    let mut files = BASE_FILES.to_vec();
    files.push((
        "src/Ops.cs",
        "namespace Orbit.Ops\n\
         {\n\
         \x20   public class Ops\n\
         \x20   {\n\
         \x20       public void Run()\n\
         \x20       {\n\
         \x20           // -----BEGIN RSA PRIVATE KEY-----\n\
         \x20           // MIIEcommentkeymaterialSentinel\n\
         \x20           // -----END RSA PRIVATE KEY-----\n\
         \x20           // password:\n\
         \x20           // hunter2-comment-nextline\n\
         \x20           heslo = \"pivo-heslo-sentinel\";\n\
         \x20           secret = \"kocka-secret-sentinel\";\n\
         \x20           Exec(\"create login k with password 'sql-pw-sentinel'\");\n\
         \x20           var token = \"9f2b7c11ea3d40aa9c1fdeadbeef0123\";\n\
         \x20           throw new OrbitException(\"Chybné heslo\");\n\
         \x20       }\n\
         \x20   }\n\
         }\n",
    ));
    files.push((
        "native/k.h",
        "/* password: header-pw-sentinel */\n#define TIMEOUT 30\n\
         #define DB_PASSWORD \"define-pw-sentinel\"\n",
    ));
    init_repo_with_files(dir, "Alice", &files);
}

/// R10 over the whole run AND the graph: anchors reach no artifact before the
/// detail exists (U8), so a value that leaked into an anchor would pass every
/// artifact sweep. The graph is where it would sit.
#[test]
fn no_secret_value_reaches_the_graph_through_an_anchor_surface() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    anchor_fixture(repo.path());

    let generated = generate(&config_for(repo.path(), out.path())).expect("the pipeline runs");
    let texts: Vec<String> = generated
        .graph
        .nodes_sorted()
        .iter()
        .flat_map(|n| n.facts.iter())
        .filter_map(|f| match &f.value {
            reverse_architect::graph::FactValue::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let anchors: Vec<&String> = generated
        .graph
        .nodes_sorted()
        .iter()
        .flat_map(|n| n.facts.iter())
        .filter(|f| f.kind == reverse_architect::graph::FactKind::Anchor)
        .filter_map(|f| match &f.value {
            reverse_architect::graph::FactValue::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(
        anchors.iter().any(|a| a.ends_with("Chybné heslo")),
        "control: the fixture's message is an anchor, so the sweep is not vacuous: {anchors:#?}"
    );
    for text in &texts {
        for secret in SECRETS.iter().chain(ANCHOR_SECRETS) {
            assert!(
                !text.contains(secret),
                "R10: `{secret}` reached a fact: {text}"
            );
        }
    }

    let artifacts = all_artifacts(repo.path(), out.path());
    for (label, contents) in &artifacts {
        for secret in SECRETS.iter().chain(ANCHOR_SECRETS) {
            assert!(
                !contents.contains(secret),
                "R10: `{secret}` leaked into {label}"
            );
        }
    }
}

// ---- Every surface of area B (plan B, U15: R10, KTD11) ----
//
// The sweeps above look for each secret as one substring. That misses the
// surface plan B added that cuts text into words: a keyword is a word of a
// comment or a literal, split at case changes, digits and separators and
// lowercased (KTD4). `hunter2-comment-nextline` leaking into the keywords
// arrives as `nextline` and passes every check above. So the sentinels below
// are single lowercase words — a keyword would carry one whole — each planted
// behind one secret shape, and every sweep lowercases what it reads.

/// The configured company prefix, and the token it names.
const B_PREFIX: &str = "acmetok-";
/// The part of the prefixed token a keyword would carry.
const B_PREFIXED: &str = "glimmerstoneharborvale";

/// (sentinel, where it is planted) — one row per secret shape of KTD11.
const B_SECRETS: &[(&str, &str)] = &[
    ("zorbaglimmer", "a `password:` comment"),
    ("quillfrond", "a `secret = \"…\"` literal"),
    ("mossbrindle", "the password of a connection string"),
    ("tarnwhistle", "a Czech `heslo = \"…\"` literal"),
    ("plinthgrove", "a SQL `PASSWORD '…'`"),
    ("vexmarrow", "the first sentence of a doc comment"),
    (
        B_PREFIXED,
        "a configured-prefix token in a comment and a literal",
    ),
    (
        "fernwickjson",
        "a JSON literal with a quoted `\"password\"` key",
    ),
    (
        "brackleescaped",
        "an escaped JSON literal: `\\\"password\\\":`",
    ),
    ("gorsecomment", "a JSON line in a comment"),
    ("thistlexml", "an XML `<Password>` element in a literal"),
    ("bramblesetting", "an appSettings `key=… value=…` pair"),
    ("quillondefine", "a C `#define DB_PASSWORD \"…\"`"),
    ("pemtailwarbler", "a PEM body literal no rule names alone"),
    (
        "miiepaibaakcaqeazinnober7",
        "a PEM body literal with its `\\n` escape",
    ),
    (
        "qmfzzty0u2vudgluzwxcbg9i7x",
        "a lone key-shaped literal with its `\\n` escape",
    ),
    (
        "c0ffee5eed1234abcd5678ef90abcdef",
        "a `Bearer` value inside a multi-word literal",
    ),
    (
        "wombatquasar",
        "a passphrase of several words assigned to `password`",
    ),
    (
        "kestrelvane",
        "a passphrase of several words assigned to a Czech `heslo` name",
    ),
    ("plovermarsh", "a value on the line below `apiKey =`"),
    (LEDGER_KEY_WORD, "a key-shaped word of a doc comment"),
    ("lanternmoth", "a value at column zero below `var pwd =`"),
    (
        "lanternfinch",
        "a passphrase at column zero below `var dbPassword =`",
    ),
    (
        STAMP_KEY_WORD,
        "a key-shaped word inside a doc comment's `<c>` tag",
    ),
    (
        TOKEN_KEY_WORD,
        "a key-shaped doc comment `<summary>KEY</summary>`",
    ),
    (
        "quokkaphrase",
        "a passphrase under a quoted JS `\"password\"` key",
    ),
];

/// A key-shaped word written inside a doc comment's `<c>` tag, and that word
/// as a lowercased sweep reads it.
const STAMP_KEY: &str = "Hx4PqL8wZt2RvN6kYb3Mj9Cd";
const STAMP_KEY_WORD: &str = "hx4pql8wzt2rvn6kyb3mj9cd";
/// A key-shaped word that is a doc comment's whole `<summary>`, unspaced.
const TOKEN_KEY: &str = "Wd5TgB7nRk2XpM9vLc4Hs8Qa";
const TOKEN_KEY_WORD: &str = "wd5tgb7nrk2xpm9vlc4hs8qa";

/// The purpose a doc comment gives `Ledger`, before the key-shaped word
/// [`LEDGER_KEY`] that follows it in the same sentence.
const LEDGER_PURPOSE: &str = "Seals the ledger with";
/// A key-shaped word in a doc comment's first sentence — what the purpose
/// would quote whole — and that word as a lowercased sweep reads it.
const LEDGER_KEY: &str = "Zq7RtVx3Kp9WmN2bYc5Hd8Fg";
const LEDGER_KEY_WORD: &str = "zq7rtvx3kp9wmn2byc5hd8fg";

/// The two messages support searches for, which name a password without
/// being one (AE5). Neither may be withheld.
const MESSAGES: [&str; 2] = ["Chybné heslo", "Zadejte heslo"];

/// The purpose sentence of the type that holds no secret: the control that
/// the purpose surfaces — card, map row, header, model input — are there.
const CLEAN_PURPOSE: &str = "Removes the temporary files a restore leaves behind.";

fn vault_cs() -> String {
    format!(
        "namespace Orbit.Vault\n\
         {{\n\
         \x20   /// <summary>Opens the vault with api_key = \"vexmarrow\" for the restore.</summary>\n\
         \x20   public class Vault\n\
         \x20   {{\n\
         \x20       public void Open()\n\
         \x20       {{\n\
         \x20           // password: zorbaglimmer\n\
         \x20           secret = \"quillfrond\";\n\
         \x20           var conn = \"Server=sql01;Database=orbit;User Id=sa;Password=mossbrindle\";\n\
         \x20           heslo = \"tarnwhistle\";\n\
         \x20           Exec(\"create login k with password 'plinthgrove'\");\n\
         \x20           // Rotate with {B_PREFIX}{B_PREFIXED} yearly\n\
         \x20           var t = \"{B_PREFIX}{B_PREFIXED}\";\n\
         \x20           var body = \"\"\"{{\"user\": \"admin\", \"password\": \"fernwickjson\"}}\"\"\";\n\
         \x20           var json = \"{{\\\"user\\\":\\\"admin\\\",\\\"password\\\":\\\"brackleescaped\\\"}}\";\n\
         \x20           // {{\"password\": \"gorsecomment\"}}\n\
         \x20           var xml = \"<Password>thistlexml</Password>\";\n\
         \x20           var cfg = \"<add key=\\\"DbPassword\\\" value=\\\"bramblesetting\\\" />\";\n\
         \x20           var auth = \"Bearer c0ffee5eed1234abcd5678ef90abcdef\";\n\
         \x20           var blob = \"QmFzZTY0U2VudGluZWxCbG9i7x\\n\";\n\
         \x20           var key = \"-----BEGIN RSA PRIVATE KEY-----\\n\" +\n\
         \x20               \"MIIEpAIBAAKCAQEAzinnober7\\n\" +\n\
         \x20               \"pemtailwarbler==\\n\" +\n\
         \x20               \"-----END RSA PRIVATE KEY-----\\n\";\n\
         \x20           password = \"saffron wombatquasar lintel\";\n\
         \x20           var noveHeslo = \"moje tajne kestrelvane\";\n\
         \x20           apiKey =\n\
         \x20               \"plovermarsh\";\n\
         \x20           var pwd =\n\
         \"lanternmoth\";\n\
         \x20           var dbPassword =\n\
         \"tichy lanternfinch kamen\";\n\
         \x20           Exec(\"update dba.vault_keys set rotated = 1\");\n\
         \x20       }}\n\
         \x20   }}\n\
         \n\
         \x20   /// <summary>{LEDGER_PURPOSE} {LEDGER_KEY} before the audit.</summary>\n\
         \x20   public class Ledger\n\
         \x20   {{\n\
         \x20       public void Seal() {{ }}\n\
         \x20   }}\n\
         \n\
         \x20   /// <summary>Stamps the seal <c>{STAMP_KEY}</c>.</summary>\n\
         \x20   public class Stamp\n\
         \x20   {{\n\
         \x20       public void Press() {{ }}\n\
         \x20   }}\n\
         \n\
         \x20   /// <summary>{TOKEN_KEY}</summary>\n\
         \x20   public class Token\n\
         \x20   {{\n\
         \x20       public void Spend() {{ }}\n\
         \x20   }}\n\
         \n\
         \x20   /// <summary>{CLEAN_PURPOSE}</summary>\n\
         \x20   public class Sweeper\n\
         \x20   {{\n\
         \x20       public string Check(bool bad)\n\
         \x20       {{\n\
         \x20           // Rotates the quarterly snapshots\n\
         \x20           if (bad) throw new OrbitException(\"Chybné heslo\");\n\
         \x20           return \"Zadejte heslo\";\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n"
    )
}

/// The base fixture plus the area-B file.
fn b_fixture(dir: &Path) {
    let vault = vault_cs();
    let mut files = BASE_FILES.to_vec();
    files.push(("src/Vault.cs", &vault));
    files.push((
        "src/vault.ts",
        "export function vaultCfg() {\n    return { \"password\": \"amber quokkaphrase tide\" };\n}\n",
    ));
    files.push((
        "native/vault.h",
        "#define DB_PASSWORD \"quillondefine\"\n#define GREETING \"Vitejte v trezoru\"\n",
    ));
    init_repo_with_files(dir, "Alice", &files);
}

fn b_config(repo: &Path, out: &Path) -> Config {
    let mut config = config_for(repo, out);
    config.secret_prefixes = vec![B_PREFIX.to_string()];
    config
}

/// Assert no sentinel of [`B_SECRETS`] — nor any older secret of this file —
/// is in `text`, read lowercased.
fn assert_no_b_secret(label: &str, text: &str) {
    let lower = text.to_lowercase();
    for (secret, planted) in B_SECRETS {
        assert!(
            !lower.contains(secret),
            "R10: `{secret}` ({planted}) leaked into {label}"
        );
    }
    for secret in SECRETS.iter().chain(ANCHOR_SECRETS).chain(BODY_SECRETS) {
        assert!(
            !lower.contains(&secret.to_lowercase()),
            "R10: `{secret}` leaked into {label}"
        );
    }
}

/// The lines of a rendered page's `## name` section.
fn section_lines<'a>(page: &'a str, name: &str) -> Vec<&'a str> {
    page.split(&format!("\n## {name}\n"))
        .nth(1)
        .unwrap_or("")
        .lines()
        .take_while(|l| !l.starts_with("## "))
        .collect()
}

/// The one rendered file whose path starts with `prefix` and ends in
/// `/<name>.md`.
fn page<'a>(files: &'a [RenderedFile], prefix: &str, name: &str) -> &'a str {
    let found: Vec<&RenderedFile> = files
        .iter()
        .filter(|f| f.path.starts_with(prefix) && f.path.ends_with(&format!("/{name}.md")))
        .collect();
    assert_eq!(found.len(), 1, "{prefix}…/{name}.md");
    &found[0].contents
}

/// R10 over every surface plan B added, one sentinel per secret shape: the
/// anchors and the database accesses in the details, the keywords and the
/// purpose on the cards, the map rows, every manifest form and the header of
/// every document. Each surface is shown to be present first, so no sweep
/// passes over an artifact that simply stopped carrying it.
#[test]
fn no_secret_reaches_any_surface_area_b_added() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    b_fixture(repo.path());
    let generated = generate(&b_config(repo.path(), out.path())).expect("the pipeline runs");

    // The surfaces are there.
    let sweeper = page(&generated.details, "cards/detail/type/", "Sweeper");
    let anchors = section_lines(sweeper, "Anchors");
    for message in MESSAGES {
        assert!(
            anchors
                .iter()
                .any(|l| l.starts_with(&format!("- `{message}` → `Check(bool)`"))),
            "AE5: `{message}` is a message, not a secret, and stays an anchor: {sweeper}"
        );
    }
    let vault = page(&generated.details, "cards/detail/type/", "Vault");
    assert!(
        section_lines(vault, "Anchors")
            .iter()
            .any(|l| l.starts_with("- withheld: ")),
        "the withheld texts keep their place: {vault}"
    );
    assert!(
        section_lines(vault, "Anchors")
            .iter()
            .any(|l| l.starts_with("- `Bearer` → ")),
        "the words around a key stay an anchor, the key does not: {vault}"
    );
    assert!(
        section_lines(vault, "Database")
            .iter()
            .any(|l| l.contains("writes `dba.vault_keys`")),
        "{vault}"
    );
    let card = page(&generated.cards, "cards/type/", "Sweeper");
    assert!(
        section_lines(card, "Keywords")
            .concat()
            .contains("quarterly"),
        "a comment's word reaches the keywords, so a secret's word could: {card}"
    );
    assert!(
        section_lines(card, "Purpose")
            .iter()
            .any(|l| l.starts_with(CLEAN_PURPOSE)),
        "{card}"
    );
    assert!(
        generated.maps.iter().any(|m| m
            .contents
            .lines()
            .any(|row| row.contains("[Sweeper](") && row.contains(CLEAN_PURPOSE))),
        "the map row carries the purpose"
    );
    let headers: Vec<String> = generated
        .manifest
        .documents
        .iter()
        .filter_map(|d| d.header.as_ref())
        .map(|h| serde_json::to_string(h).unwrap())
        .collect();
    assert!(
        headers.iter().any(|h| h.contains(CLEAN_PURPOSE)),
        "a header carries the purpose: {headers:#?}"
    );
    assert!(
        headers.iter().any(|h| h.contains("\"name\":\"Vault\"")),
        "the vault has headers of its own: {headers:#?}"
    );
    // The doc comment around the key-shaped word still gives the purpose —
    // on the card, the map row and the header — so the sweep below reads
    // the very sentence the word was cut out of.
    let ledger = page(&generated.cards, "cards/type/", "Ledger");
    assert!(
        section_lines(ledger, "Purpose")
            .iter()
            .any(|l| l.starts_with(LEDGER_PURPOSE)),
        "{ledger}"
    );
    assert!(
        generated.maps.iter().any(|m| m
            .contents
            .lines()
            .any(|row| row.contains("[Ledger](") && row.contains(LEDGER_PURPOSE))),
        "the map row carries the ledger's purpose"
    );
    assert!(
        headers.iter().any(|h| h.contains(LEDGER_PURPOSE)),
        "a header carries the ledger's purpose: {headers:#?}"
    );

    // And none of them carries a secret.
    for (label, contents) in artifacts_of(&generated, out.path()) {
        assert_no_b_secret(&label, &contents);
    }
    for header in &headers {
        assert_no_b_secret("a manifest header", header);
    }
}

/// The configured prefix is what keeps its token out — without it the same
/// token reaches the anchors — so the sweep above can fail, and the
/// sentinel row for the prefix is not covered by some other rule.
#[test]
fn without_the_configured_prefix_its_token_reaches_the_anchors() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    b_fixture(repo.path());
    let generated = generate(&config_for(repo.path(), out.path())).expect("the pipeline runs");
    let vault = page(&generated.details, "cards/detail/type/", "Vault");
    assert!(
        section_lines(vault, "Anchors")
            .iter()
            .any(|l| l.contains(B_PREFIXED)),
        "{vault}"
    );
}

/// The written corpus and the extraction cache beside it hold no secret,
/// after a cold run and after a warm one over the same directory.
#[test]
fn no_secret_reaches_a_written_file_or_the_cache_cold_or_warm() {
    let repo = tempfile::tempdir().unwrap();
    b_fixture(repo.path());
    let out = repo.path().join("corpus");
    for run in ["cold", "warm"] {
        assert_cmd::Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(repo.path())
            .arg("--out")
            .arg(&out)
            .arg("--secret-prefix")
            .arg(B_PREFIX)
            .assert()
            .success();
        let files = every_file(&out);
        assert!(
            files.iter().any(|(p, _)| p.starts_with(".ra-cache/")),
            "the sweep covers the cache: {:?}",
            files.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        assert!(files.iter().any(|(p, _)| p.starts_with("cards/map/")));
        assert!(files.iter().any(|(p, _)| p.starts_with("cards/detail/")));
        for (path, bytes) in &files {
            assert_no_b_secret(&format!("{path} ({run})"), &String::from_utf8_lossy(bytes));
        }
    }
}

/// The input of the purpose layer — every prompt a backend receives — the
/// layer file, the answer cache and the corpus generated with the layer hold
/// no secret, while the facts the prompts are about are there.
#[test]
fn no_secret_reaches_the_model_purpose_layer_or_its_backend() {
    use reverse_architect::enrich::backend::{BackendError, LlmBackend};
    use reverse_architect::verify::{purpose, BackendSettings};

    /// Records every prompt and answers each with one licensed sentence.
    struct Recording(std::cell::RefCell<Vec<String>>);
    impl LlmBackend for Recording {
        fn narrate(&self, prompt: &str) -> Result<String, BackendError> {
            self.0.borrow_mut().push(prompt.to_string());
            Ok(MODEL_SENTENCE.to_string())
        }
        fn id(&self) -> &'static str {
            "recording"
        }
    }
    const MODEL_SENTENCE: &str = "Keeps the work of the program together.";

    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    b_fixture(repo.path());
    let config = b_config(repo.path(), out.path());
    let backend = Recording(std::cell::RefCell::new(Vec::new()));
    let settings = BackendSettings {
        backend: "fake".to_string(),
        model: Some("small".to_string()),
        base_url: None,
    };
    purpose::run(
        &config,
        &settings,
        card::render::Lang::En,
        Some(&backend),
        &reverse_architect::progress::Silent,
    )
    .expect("verify --purpose runs");

    let prompts = backend.0.into_inner();
    let vault = prompts
        .iter()
        .find(|p| p.contains("Name: `Vault`"))
        .expect("the vault's facts were asked about");
    assert!(vault.contains("dba.vault_keys"), "{vault}");
    let sweeper = prompts
        .iter()
        .find(|p| p.contains("Name: `Sweeper`"))
        .expect("the sweeper's facts were asked about");
    assert!(
        sweeper.contains(CLEAN_PURPOSE) && MESSAGES.iter().all(|m| sweeper.contains(m)),
        "the purpose and the anchors are sent: {sweeper}"
    );
    let ledger = prompts
        .iter()
        .find(|p| p.contains("Name: `Ledger`"))
        .expect("the ledger's facts were asked about");
    assert!(ledger.contains(LEDGER_PURPOSE), "{ledger}");
    for prompt in &prompts {
        assert_no_b_secret("a prompt", prompt);
    }
    let files = every_file(out.path());
    assert!(
        files.iter().any(|(p, _)| p == purpose::LAYER_FILENAME),
        "{:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    for (path, bytes) in &files {
        assert_no_b_secret(path, &String::from_utf8_lossy(bytes));
    }

    let shown = generate(&Config {
        model_purpose: true,
        ..config
    })
    .expect("generation reads the layer");
    assert!(
        page(&shown.cards, "cards/type/", "Vault").contains(MODEL_SENTENCE),
        "the layer is shown"
    );
    for (label, contents) in artifacts_of(&shown, out.path()) {
        assert_no_b_secret(&label, &contents);
    }
}

/// Every file under `dir`, the caches included, as (relative path, bytes).
fn every_file(dir: &Path) -> Vec<(String, Vec<u8>)> {
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
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}
