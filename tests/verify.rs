//! `verify` end to end (plan A, U12, R13–R16, KTD12): which sites the model is
//! asked about, what reaches it, what is cached, and what generation does with
//! the layer — only when asked, and never more than a mark and an order.
//!
//! The fixture is two calls of `Run` on receivers nothing is known about, in
//! two methods far enough apart that their snippets do not overlap; `A.Run` and
//! `B.Run` both fit each call, so both sites are ambiguous. Every backend here
//! is a test double — a real model is a manual step outside CI.

mod common;

use assert_cmd::Command;
use common::{
    at, config_for, init_repo_with_files, occ, scip_document, scip_occurrence_at, scip_sym,
    symbol_nodes, write_and_commit, write_scip_index,
};
use reverse_architect::config::Config;
use reverse_architect::coverage::ModelVerification;
use reverse_architect::enrich::backend::{BackendError, FakeBackend, LlmBackend};
use reverse_architect::progress::Silent;
use reverse_architect::verify::{self, Answer, BackendSettings, Layer, VerifyError, VerifyReport};
use reverse_architect::{generate, Generated};
use std::cell::RefCell;
use tempfile::TempDir;

const WORKERS_CS: &str = r#"namespace N
{
    public class A
    {
        public void Run(int count) { }
    }

    public class B
    {
        public void Run(int count) { }
    }
}
"#;

/// The token shape only the configured prefix `kst_` names.
const COMPANY_TOKEN: &str = "kst_ABCDEFGHIJKLMNOPQRSTUVWX";

/// `Go`'s call sits under a password and a company token; `Again`'s is twenty
/// lines further down, out of reach of `Go`'s window and its secrets.
fn caller_cs(factory_call: &str) -> String {
    let padding = "        // padding\n".repeat(20);
    format!(
        "namespace N\n\
         {{\n\
         \x20   public class Caller\n\
         \x20   {{\n\
         \x20       public void Go()\n\
         \x20       {{\n\
         \x20           string password = \"hunter2\";\n\
         \x20           var token = \"{COMPANY_TOKEN}\";\n\
         \x20           var x = Factory.{factory_call}();\n\
         \x20           x.Run(1);\n\
         \x20       }}\n\
         {padding}\
         \x20       public void Again()\n\
         \x20       {{\n\
         \x20           var y = Factory.Make();\n\
         \x20           y.Run(2);\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n"
    )
}

struct Fixture {
    repo: TempDir,
    out: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let repo = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        init_repo_with_files(
            repo.path(),
            "Alice",
            &[
                ("src/Workers.cs", WORKERS_CS),
                ("src/Caller.cs", &caller_cs("Make")),
            ],
        );
        Fixture { repo, out }
    }

    fn config(&self) -> Config {
        let mut config = config_for(self.repo.path(), &self.out.path().join("corpus"));
        config.secret_prefixes = vec!["kst_".to_string()];
        config
    }

    fn with_layer(&self) -> Config {
        Config {
            model_verification: true,
            ..self.config()
        }
    }

    fn commit(&self, files: &[(&str, &str)]) {
        let repository = git2::Repository::open(self.repo.path()).unwrap();
        write_and_commit(&repository, self.repo.path(), "Alice", "change", files);
    }

    fn layer_path(&self) -> std::path::PathBuf {
        self.out.path().join("corpus").join(verify::LAYER_FILENAME)
    }

    fn layer(&self) -> Layer {
        serde_json::from_str(&std::fs::read_to_string(self.layer_path()).unwrap()).unwrap()
    }
}

fn settings() -> BackendSettings {
    BackendSettings {
        backend: "fake".to_string(),
        model: Some("small".to_string()),
        base_url: None,
    }
}

/// A backend that records every prompt and answers through `answer`.
struct Recording<F: Fn(&str, usize) -> Result<String, BackendError>> {
    answer: F,
    prompts: RefCell<Vec<String>>,
}

fn recording<F: Fn(&str, usize) -> Result<String, BackendError>>(answer: F) -> Recording<F> {
    Recording {
        answer,
        prompts: RefCell::new(Vec::new()),
    }
}

impl<F: Fn(&str, usize) -> Result<String, BackendError>> LlmBackend for Recording<F> {
    fn narrate(&self, prompt: &str) -> Result<String, BackendError> {
        let n = self.prompts.borrow().len();
        self.prompts.borrow_mut().push(prompt.to_string());
        (self.answer)(prompt, n)
    }

    fn id(&self) -> &'static str {
        "recording"
    }
}

impl<F: Fn(&str, usize) -> Result<String, BackendError>> Recording<F> {
    fn prompts(&self) -> Vec<String> {
        self.prompts.borrow().clone()
    }
}

fn run(config: &Config, backend: &dyn LlmBackend) -> VerifyReport {
    verify::run(config, &settings(), Some(backend), &Silent).expect("verify runs")
}

/// The candidates are labelled in the order of their node keys, so `A.Run`
/// is `c1` and `B.Run` is `c2`.
const B: &str = "c2";

/// `B.Run`'s node key.
fn b_run(config: &Config) -> String {
    let graph = generate(config).unwrap().graph;
    let keys = symbol_nodes(&graph, "Run");
    assert_eq!(keys.len(), 2, "{keys:?}");
    keys.into_iter()
        .map(|k| k.as_str().to_string())
        .find(|k| k.contains("B.Run"))
        .expect("B.Run's key")
}

/// The `## Maybe also` lines of `Caller`'s detail — the section left the
/// short card for it (plan B, U9).
fn maybe_also(generated: &Generated) -> Vec<String> {
    let card = generated
        .details
        .iter()
        .find(|f| f.path.ends_with("/Caller.md"))
        .expect("Caller's detail");
    card.contents
        .split("## Maybe also")
        .nth(1)
        .expect("a Maybe also section")
        .lines()
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with("- "))
        .map(str::to_string)
        .collect()
}

/// Everything generation produces, as bytes.
fn output(generated: &Generated) -> (Vec<(String, String)>, String, String) {
    let files = generated
        .corpus
        .files
        .iter()
        .chain(&generated.cards)
        .chain(&generated.details)
        .chain(&generated.maps)
        .map(|f| (f.path.clone(), f.contents.clone()))
        .collect();
    (
        files,
        reverse_architect::manifest::to_json(&generated.manifest),
        reverse_architect::coverage::to_json(&generated.coverage),
    )
}

#[test]
fn a_named_candidate_is_stored_and_the_card_lists_it_first_with_the_mark() {
    let f = Fixture::new();
    let b_run = b_run(&f.config());
    let report = run(&f.config(), &recording(|_, _| Ok(B.to_string())));
    assert_eq!((report.sites, report.asked, report.promoted), (2, 2, 2));

    let layer = f.layer();
    assert_eq!(layer.records.len(), 2);
    for record in &layer.records {
        assert_eq!(
            record.answer,
            Answer::Candidate {
                target: b_run.clone()
            }
        );
        assert_eq!(record.candidates.len(), 2, "{record:?}");
    }

    let plain = generate(&f.config()).unwrap();
    let marked = generate(&f.with_layer()).unwrap();
    let lines = maybe_also(&marked);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert!(
            line.contains("model-verified: the first candidate"),
            "{line}"
        );
        let (b, a) = (line.find("B.Run").unwrap(), line.find("A.Run").unwrap());
        assert!(
            b < a,
            "the promoted candidate first, the other kept: {line}"
        );
    }
    for line in maybe_also(&plain) {
        assert!(!line.contains("model-verified"), "{line}");
        assert!(line.find("A.Run").unwrap() < line.find("B.Run").unwrap());
    }
    assert_eq!(
        marked.coverage.model_verification,
        Some(ModelVerification {
            used: 2,
            stale: 0,
            unmatched: 0
        })
    );
    // The layer is a projection on the cards alone (R14, R16).
    assert_eq!(
        reverse_architect::manifest::to_json(&marked.manifest),
        reverse_architect::manifest::to_json(&plain.manifest)
    );
}

#[test]
fn none_keeps_every_candidate_in_maybe_also_with_a_demotion_mark() {
    let f = Fixture::new();
    let report = run(&f.config(), &recording(|_, _| Ok("none".to_string())));
    assert_eq!((report.none, report.promoted), (2, 0));
    assert!(f.layer().records.iter().all(|r| r.answer == Answer::None));

    let plain = generate(&f.config()).unwrap();
    let marked = generate(&f.with_layer()).unwrap();
    let lines = maybe_also(&marked);
    assert_eq!(lines.len(), maybe_also(&plain).len());
    for line in &lines {
        assert!(line.contains("model-verified: none of them"), "{line}");
        assert!(line.contains("A.Run") && line.contains("B.Run"), "{line}");
    }
    assert_eq!(
        reverse_architect::manifest::to_json(&marked.manifest),
        reverse_architect::manifest::to_json(&plain.manifest),
        "nothing is deleted and no rung moves"
    );
}

#[test]
fn an_answer_outside_the_set_or_prose_leaves_the_site_unverified_and_counts_invalid() {
    let f = Fixture::new();
    let report = run(
        &f.config(),
        &recording(|_, n| {
            Ok(match n {
                0 => "c3".to_string(),
                _ => "It is most likely B.Run, the second one.".to_string(),
            })
        }),
    );
    assert_eq!(
        (report.asked, report.invalid, report.promoted, report.none),
        (2, 2, 0, 0)
    );
    assert!(f.layer().records.is_empty());

    // Not cached: the next run asks again.
    let again = recording(|_, _| Ok(B.to_string()));
    let report = run(&f.config(), &again);
    assert_eq!((report.cached, again.prompts().len()), (0, 2));
}

#[test]
fn neither_a_secret_nor_a_configured_prefix_token_reaches_the_backend() {
    let f = Fixture::new();
    let backend = recording(|_, _| Ok("none".to_string()));
    run(&f.config(), &backend);

    let prompts = backend.prompts();
    assert_eq!(prompts.len(), 2);
    let go = prompts
        .iter()
        .find(|p| p.contains(">>             x.Run(1);"))
        .expect("Go's site, marked");
    assert!(
        go.contains("[redacted: "),
        "the secrets' lines are in Go's window: {go}"
    );
    for prompt in &prompts {
        assert!(!prompt.contains("hunter2"), "{prompt}");
        assert!(!prompt.contains(COMPANY_TOKEN), "{prompt}");
        assert!(!prompt.contains("kst_ABCDEFGH"), "{prompt}");
    }
}

/// `Go`'s call under the shapes a C# credential is usually written in, none
/// of which names a keyword the secret filter knows at a word boundary: a
/// field with a prefix, a constant whose name only ends in `Password` or
/// `ApiKey`, an abbreviation, a constructor's arguments, and the literal kinds
/// C# has besides the plain one. Every value is unique so a leak names itself.
const CREDENTIALS_CS: &str = r#"namespace N
{
    public class Caller
    {
        private string _password = "leak-field-1";
        const string SmtpPassword = "leak-smtp-2";
        const string GoogleApiKey = "AIzaLeak3";
        public void Go()
        {
            var dbPwd = "leak-db-4";
            var cred = new NetworkCredential("leak-user-5", "leak-cred-6");
            var path = @"C:\leak-verbatim-7\""leakquote""";
            var header = $"Bearer {dbPwd} leak-interp-8";
            var raw = """leak-raw-9""";
            var x = Factory.Make();
            x.Run(1);
        }
    }
}
"#;

const LEAKS: &[&str] = &[
    "leak-field-1",
    "leak-smtp-2",
    "AIzaLeak3",
    "leak-db-4",
    "leak-user-5",
    "leak-cred-6",
    "leak-verbatim-7",
    "leakquote",
    "leak-interp-8",
    "leak-raw-9",
];

/// A literal's value never decides which overload a call means, its type
/// does, so no value of a string literal leaves in a snippet — whatever the
/// name it is assigned to.
#[test]
fn no_string_literal_value_reaches_the_backend() {
    let f = Fixture::new();
    f.commit(&[("src/Caller.cs", CREDENTIALS_CS)]);
    let backend = recording(|_, _| Ok("none".to_string()));
    run(&f.config(), &backend);

    let prompts = backend.prompts();
    let go = prompts
        .iter()
        .find(|p| p.contains(">>             x.Run(1);"))
        .expect("Go's site, marked");
    for leak in LEAKS {
        assert!(!go.contains(leak), "`{leak}` reached the backend: {go}");
    }
    // The code around the literals is still there, the literals shown empty.
    assert!(go.contains(r#"new NetworkCredential("…", "…");"#), "{go}");
    assert!(go.contains(r#"const string SmtpPassword = "…";"#), "{go}");
}

#[test]
fn a_second_run_over_the_same_sites_asks_nothing() {
    let f = Fixture::new();
    run(&f.config(), &recording(|_, _| Ok(B.to_string())));
    let first = std::fs::read(f.layer_path()).unwrap();

    let report = run(
        &f.config(),
        &FakeBackend::failing("the backend must not be called"),
    );
    assert_eq!((report.sites, report.cached, report.asked), (2, 2, 0));
    assert_eq!(std::fs::read(f.layer_path()).unwrap(), first);
}

#[test]
fn a_changed_candidate_signature_with_the_same_id_is_asked_again() {
    let f = Fixture::new();
    let before = b_run(&f.config());
    run(&f.config(), &recording(|_, _| Ok(B.to_string())));

    // A parameter's name is in the signature, not in the identity.
    f.commit(&[(
        "src/Workers.cs",
        &WORKERS_CS.replacen("Run(int count)", "Run(int times)", 2),
    )]);
    assert_eq!(b_run(&f.config()), before, "the same candidate ids");

    let again = recording(|_, _| Ok(B.to_string()));
    let report = run(&f.config(), &again);
    assert_eq!((report.cached, report.asked), (0, 2));
    assert!(again.prompts().iter().all(|p| p.contains("Run(int times)")));
}

#[test]
fn a_code_change_near_a_site_makes_its_record_stale() {
    let f = Fixture::new();
    run(&f.config(), &recording(|_, _| Ok(B.to_string())));

    // Same lines, so `Go`'s site keeps its id; its window changed, `Again`'s
    // did not.
    f.commit(&[("src/Caller.cs", &caller_cs("Create"))]);
    let marked = generate(&f.with_layer()).unwrap();
    assert_eq!(
        marked.coverage.model_verification,
        Some(ModelVerification {
            used: 1,
            stale: 1,
            unmatched: 0
        })
    );
    let lines = maybe_also(&marked);
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("model-verified"))
            .count(),
        1,
        "{lines:?}"
    );

    let again = recording(|_, _| Ok(B.to_string()));
    let report = run(&f.config(), &again);
    assert_eq!((report.cached, report.asked), (1, 1));
    assert!(again.prompts()[0].contains("Factory.Create()"));
}

#[test]
fn without_the_option_generation_is_byte_identical_whether_a_layer_exists_or_not() {
    let f = Fixture::new();
    let before = output(&generate(&f.config()).unwrap());
    run(&f.config(), &recording(|_, _| Ok(B.to_string())));
    assert!(f.layer_path().is_file());
    let after = output(&generate(&f.config()).unwrap());
    assert!(
        before == after,
        "a layer nobody asked for changed the output"
    );
}

#[test]
fn a_site_the_index_resolved_is_never_selected() {
    let f = Fixture::new();
    let caller = caller_cs("Make");
    let index = write_scip_index(
        f.out.path(),
        vec![
            scip_document(
                "src/Caller.cs",
                vec![
                    occ(&caller, "N/Caller#", "Caller", 0, true),
                    occ(&caller, "N/Caller#Go().", "Go(", 0, true),
                    occ(&caller, "N/Caller#Again().", "Again(", 0, true),
                    // `Go`'s call, paired with `B.Run`; `Again`'s has none.
                    scip_occurrence_at(&scip_sym("N/B#Run()."), at(&caller, "Run(", 0), false),
                ],
            ),
            scip_document(
                "src/Workers.cs",
                vec![
                    occ(WORKERS_CS, "N/A#", "A", 0, true),
                    occ(WORKERS_CS, "N/A#Run().", "Run(", 0, true),
                    occ(WORKERS_CS, "N/B#", "B", 0, true),
                    occ(WORKERS_CS, "N/B#Run().", "Run(", 1, true),
                ],
            ),
        ],
    );
    let config = Config {
        scip_index: Some(index),
        ..f.config()
    };
    let backend = recording(|_, _| Ok("none".to_string()));
    let report = run(&config, &backend);
    assert_eq!(report.sites, 1);
    let prompts = backend.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(">>             y.Run(2);"),
        "{}",
        prompts[0]
    );
}

#[test]
fn a_dry_run_prints_the_count_and_an_estimate_and_never_calls_the_backend() {
    let f = Fixture::new();
    let corpus = f.out.path().join("corpus");
    // `claude-cli` inside a Claude session refuses every call, so a dry run
    // that called it would fail.
    let assert = Command::cargo_bin("reverse-architect")
        .unwrap()
        .args(["verify", "--backend", "claude-cli", "--dry-run", "--corpus"])
        .arg(&corpus)
        .arg(f.repo.path())
        .env("CLAUDECODE", "1")
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(stderr.contains("2 ambiguous sites"), "{stderr}");
    assert!(stderr.contains("input tokens"), "{stderr}");
    assert!(stderr.contains("Nothing was sent"), "{stderr}");
    assert!(!corpus.join(verify::LAYER_FILENAME).exists());
}

#[test]
fn claude_cli_inside_a_claude_session_fails_with_a_clear_error() {
    let f = Fixture::new();
    let assert = Command::cargo_bin("reverse-architect")
        .unwrap()
        .args(["verify", "--backend", "claude-cli", "--corpus"])
        .arg(f.out.path().join("corpus"))
        .arg(f.repo.path())
        .env("CLAUDECODE", "1")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(stderr.contains("Claude Code session"), "{stderr}");
}

#[test]
fn a_backend_failing_mid_run_keeps_the_answered_sites_cached_and_fails_the_run() {
    let f = Fixture::new();
    let failing = recording(|_, n| match n {
        0 => Ok(B.to_string()),
        _ => Err(BackendError::Unreachable("connection refused".to_string())),
    });
    let err =
        verify::run(&f.config(), &settings(), Some(&failing), &Silent).expect_err("the run fails");
    assert!(
        matches!(
            err,
            VerifyError::Backend {
                answered: 1,
                sites: 2,
                ..
            }
        ),
        "{err}"
    );
    assert!(!f.layer_path().exists(), "no layer from a run that failed");

    let rest = recording(|_, _| Ok("none".to_string()));
    let report = run(&f.config(), &rest);
    assert_eq!((report.cached, report.asked), (1, 1));
    assert_eq!(f.layer().records.len(), 2);
}

/// The cache lives beside the extraction cache, not inside it: generation
/// over a changed tree wipes `.ra-cache`, and must not take the answers with
/// it.
#[test]
fn a_generation_over_a_changed_tree_keeps_the_answers() {
    let f = Fixture::new();
    run(&f.config(), &recording(|_, _| Ok(B.to_string())));
    f.commit(&[("src/Other.cs", "namespace N { public class C { } }\n")]);
    generate(&f.config()).unwrap();

    let report = run(
        &f.config(),
        &FakeBackend::failing("the backend must not be called"),
    );
    assert_eq!(report.cached, 2);
}

// ---- The purpose layer (plan B, U13: R7, R10, KTD12) ----
//
// The same fixture: three types — `A`, `B` and `Caller` — whose facts the
// model is shown. `Caller.Go` holds a password and a company token, which the
// facts must not carry to the backend.

use reverse_architect::card::render::Lang;
use reverse_architect::coverage::ModelPurpose;
use reverse_architect::verify::purpose::{self, PurposeReport};

/// A sentence the facts license: no number, no code-shaped name.
const SENTENCE: &str = "Coordinates the work the program hands it.";

/// The label under which the card shows the model's sentence.
const MODEL_LABEL: &str = "(written by a model, not read from the code)";

impl Fixture {
    fn with_purpose(&self) -> Config {
        Config {
            model_purpose: true,
            ..self.config()
        }
    }

    fn purpose_layer_path(&self) -> std::path::PathBuf {
        self.out.path().join("corpus").join(purpose::LAYER_FILENAME)
    }
}

fn run_purpose(config: &Config, backend: &dyn LlmBackend) -> PurposeReport {
    purpose::run(config, &settings(), Lang::En, Some(backend), &Silent)
        .expect("verify --purpose runs")
}

/// The lines of `## Purpose` on a type's short card.
fn purpose_lines(generated: &Generated, name: &str) -> Vec<String> {
    let card = generated
        .cards
        .iter()
        .find(|f| f.path.starts_with("cards/type/") && f.path.ends_with(&format!("/{name}.md")))
        .unwrap_or_else(|| panic!("{name}'s card"));
    card.contents
        .split("## Purpose")
        .nth(1)
        .expect("a Purpose section")
        .lines()
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// The manifest's headers, the one place outside the card a purpose is said.
fn headers(generated: &Generated) -> String {
    let headers: Vec<_> = generated
        .manifest
        .documents
        .iter()
        .map(|d| serde_json::to_value(&d.header).unwrap())
        .collect();
    serde_json::to_string(&headers).unwrap()
}

#[test]
fn with_the_option_the_card_shows_the_sentence_labelled_under_the_purpose_and_nowhere_else() {
    let f = Fixture::new();
    let report = run_purpose(&f.config(), &recording(|_, _| Ok(SENTENCE.to_string())));
    assert_eq!(
        (report.types, report.asked, report.written, report.invalid),
        (3, 3, 3, 0)
    );

    let plain = generate(&f.config()).unwrap();
    let shown = generate(&f.with_purpose()).unwrap();
    let lines = purpose_lines(&shown, "Caller");
    assert_eq!(
        lines.last().unwrap(),
        &format!("{SENTENCE} {MODEL_LABEL}"),
        "the model's line comes last, under the code's: {lines:?}"
    );
    // Every line above it is the code's own, as without the option.
    assert_eq!(
        lines[..lines.len() - 1],
        purpose_lines(&plain, "Caller")[..]
    );
    assert!(purpose_lines(&plain, "Caller")
        .iter()
        .all(|l| !l.contains(MODEL_LABEL)));

    // Never in a map row, never in a header.
    let maps = |g: &Generated| {
        g.maps
            .iter()
            .map(|m| m.contents.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(maps(&shown), maps(&plain));
    assert!(maps(&shown).iter().all(|m| !m.contains(SENTENCE)));
    assert_eq!(headers(&shown), headers(&plain));
    assert!(!headers(&shown).contains(SENTENCE));

    assert_eq!(
        shown.coverage.model_purpose,
        Some(ModelPurpose {
            used: 3,
            stale: 0,
            unmatched: 0
        })
    );
    assert_eq!(plain.coverage.model_purpose, None);

    // Asked in English, the sentences are not shown on Czech cards.
    let czech = generate(&Config {
        lang: "cs".to_string(),
        ..f.with_purpose()
    })
    .unwrap();
    assert_eq!(
        czech.coverage.model_purpose,
        Some(ModelPurpose {
            used: 0,
            stale: 3,
            unmatched: 0
        })
    );
    assert!(purpose_lines(&czech, "Caller")
        .iter()
        .all(|l| !l.contains(SENTENCE)));
}

#[test]
fn without_the_option_generation_is_byte_identical_whether_a_purpose_layer_exists_or_not() {
    let f = Fixture::new();
    let before = output(&generate(&f.config()).unwrap());
    run_purpose(&f.config(), &recording(|_, _| Ok(SENTENCE.to_string())));
    assert!(f.purpose_layer_path().is_file());
    let after = output(&generate(&f.config()).unwrap());
    assert!(
        before == after,
        "a purpose layer nobody asked for changed the output"
    );
}

#[test]
fn a_second_purpose_run_over_the_same_facts_asks_nothing() {
    let f = Fixture::new();
    run_purpose(&f.config(), &recording(|_, _| Ok(SENTENCE.to_string())));
    let first = std::fs::read(f.purpose_layer_path()).unwrap();

    let report = run_purpose(
        &f.config(),
        &FakeBackend::failing("the backend must not be called"),
    );
    assert_eq!((report.types, report.cached, report.asked), (3, 3, 0));
    assert_eq!(std::fs::read(f.purpose_layer_path()).unwrap(), first);
}

#[test]
fn a_sentence_naming_what_the_facts_do_not_hold_is_rejected_and_not_cached() {
    let f = Fixture::new();
    let report = run_purpose(
        &f.config(),
        &recording(|_, n| {
            Ok(match n {
                0 => "Wraps `FakeThing` for its callers.".to_string(),
                1 => "Counts 9999 runs of the program.".to_string(),
                _ => "## Purpose\nA heading is no sentence.".to_string(),
            })
        }),
    );
    assert_eq!((report.asked, report.invalid, report.written), (3, 3, 0));
    let layer: purpose::PurposeLayer =
        serde_json::from_str(&std::fs::read_to_string(f.purpose_layer_path()).unwrap()).unwrap();
    assert!(layer.records.is_empty());

    // Not cached: the next run asks again.
    let again = recording(|_, _| Ok(SENTENCE.to_string()));
    let report = run_purpose(&f.config(), &again);
    assert_eq!((report.cached, again.prompts().len()), (0, 3));
}

#[test]
fn neither_a_secret_nor_a_configured_prefix_token_reaches_the_backend_with_the_facts() {
    let f = Fixture::new();
    let backend = recording(|_, _| Ok(SENTENCE.to_string()));
    run_purpose(&f.config(), &backend);

    let prompts = backend.prompts();
    assert_eq!(prompts.len(), 3);
    let caller = prompts
        .iter()
        .find(|p| p.contains("Name: `Caller`"))
        .expect("Caller's facts");
    // The facts, not the code: no line of `Go`'s body is in the prompt.
    assert!(!caller.contains("Factory.Make"), "{caller}");
    for prompt in &prompts {
        assert!(!prompt.contains("hunter2"), "{prompt}");
        assert!(!prompt.contains(COMPANY_TOKEN), "{prompt}");
        assert!(!prompt.contains("kst_ABCDEFGH"), "{prompt}");
    }
}

/// `A` gains a method, so its facts change; `B` is renamed, so its record
/// names a type that is gone; `Caller`'s facts are untouched.
#[test]
fn a_changed_type_is_stale_and_a_vanished_one_unmatched() {
    let f = Fixture::new();
    run_purpose(&f.config(), &recording(|_, _| Ok(SENTENCE.to_string())));
    f.commit(&[(
        "src/Workers.cs",
        &WORKERS_CS
            .replacen(
                "public void Run(int count) { }",
                "public void Run(int count) { }\n        public void Stop() { }",
                1,
            )
            .replace("public class B", "public class Bravo"),
    )]);
    let shown = generate(&f.with_purpose()).unwrap();
    assert_eq!(
        shown.coverage.model_purpose,
        Some(ModelPurpose {
            used: 1,
            stale: 1,
            unmatched: 1
        })
    );
    assert!(purpose_lines(&shown, "Caller")
        .last()
        .unwrap()
        .contains(MODEL_LABEL));
    assert!(purpose_lines(&shown, "A")
        .iter()
        .all(|l| !l.contains(MODEL_LABEL)));
}

#[test]
fn generation_asked_for_a_purpose_layer_that_is_not_there_says_so() {
    let f = Fixture::new();
    let err = match generate(&f.with_purpose()) {
        Ok(_) => panic!("generation over a missing layer must fail"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("verify --purpose"), "{err}");
}

#[test]
fn a_purpose_dry_run_prints_the_count_and_an_estimate_and_never_calls_the_backend() {
    let f = Fixture::new();
    let corpus = f.out.path().join("corpus");
    let assert = Command::cargo_bin("reverse-architect")
        .unwrap()
        .args([
            "verify",
            "--purpose",
            "--backend",
            "claude-cli",
            "--dry-run",
            "--corpus",
        ])
        .arg(&corpus)
        .arg(f.repo.path())
        .env("CLAUDECODE", "1")
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(stderr.contains("3 types"), "{stderr}");
    assert!(stderr.contains("input tokens"), "{stderr}");
    assert!(stderr.contains("Nothing was sent"), "{stderr}");
    assert!(!corpus.join(purpose::LAYER_FILENAME).exists());
}
