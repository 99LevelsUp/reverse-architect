//! U-A: what the pipeline reports while it works.
//!
//! Tests the LIBRARY half — the counts and the order. The terminal half (escape
//! codes, the clock, the estimate) lives in the binary and is unit-tested there,
//! because asserting on a redrawn line needs a pseudo-terminal to say anything
//! the code does not already say.

mod common;

use common::init_repo_with_files;
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::progress::{should_tick, Progress, TICK_EVERY};
use reverse_architect::{cli::LinkStyle, generate_reporting};
use std::cell::RefCell;
use std::path::Path;

/// Everything the pipeline said, in the order it said it.
#[derive(Debug, Default)]
struct Recorder {
    events: RefCell<Vec<Event>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Phase(String, Option<usize>),
    Total(usize),
    Tick(usize),
    Done,
}

impl Progress for Recorder {
    fn phase(&self, name: &str, total: Option<usize>) {
        self.events
            .borrow_mut()
            .push(Event::Phase(name.to_string(), total));
    }
    fn total(&self, total: usize) {
        self.events.borrow_mut().push(Event::Total(total));
    }
    fn tick(&self, done: usize) {
        self.events.borrow_mut().push(Event::Tick(done));
    }
    fn phase_done(&self) {
        self.events.borrow_mut().push(Event::Done);
    }
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
    }
}

fn run(files: &[(&str, &str)]) -> Vec<Event> {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", files);
    let recorder = Recorder::default();
    generate_reporting(&config_for(repo.path(), out.path()), &recorder).expect("the pipeline runs");
    recorder.events.into_inner()
}

const A_CLASS: &str = "namespace N\n{\n    public class A\n    {\n    }\n}\n";

#[test]
fn every_phase_of_the_pipeline_announces_itself_and_ends() {
    let events = run(&[("src/A.cs", A_CLASS)]);

    let names: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Phase(name, _) => Some(name.as_str()),
            _ => None,
        })
        .collect();

    // Every extractor by its own id, then each projection. A phase missing from
    // this list is a stretch of a long run during which the operator is told
    // nothing — which is the state this unit exists to end.
    assert_eq!(
        names,
        vec![
            "filetree",
            "languages",
            // `languages` walks every file TWICE — declarations, then usages —
            // and the second walk is the longer one. Without its own phase the
            // line reported "done" at 38% of the real duration.
            "languages usages",
            "manifest-deps",
            "metrics",
            // The README extractor (card schema v4) runs after metrics and
            // announces itself like every other extractor.
            "readme",
            "external",
            "scip",
            "test-pairing",
            "merge",
            "render views",
            "build manifest",
            "render cards",
            "coverage",
        ]
    );

    // Every phase that opened, ended — by its own `Done`, or by the next phase
    // superseding it, which the reporter turns into a close. An open phase left
    // dangling leaves a redrawn line on screen attributed to the wrong work.
    let mut in_flight = false;
    for event in &events {
        match event {
            Event::Phase(..) => in_flight = true,
            Event::Done => {
                assert!(in_flight, "a phase closed that never opened: {events:?}");
                in_flight = false;
            }
            _ => assert!(in_flight, "work reported outside a phase: {events:?}"),
        }
    }
    assert!(
        !in_flight,
        "the run ended with a phase still open: {events:?}"
    );
}

#[test]
fn a_file_walking_phase_reports_its_denominator_and_reaches_it() {
    // Four tracked files, of which only some have a grammar. The denominator is
    // every tracked file: a walk that counted only the files it could parse
    // would appear to stall on the others and then leap.
    let events = run(&[
        ("src/A.cs", A_CLASS),
        ("src/B.cs", A_CLASS.replace('A', "B").as_str()),
        ("README.md", "# readme\n"),
        ("data.bin", "\u{0}\u{1}\u{2}\n"),
    ]);

    for phase in ["languages", "external"] {
        // The denominator arrives on its own call, AFTER the phase opened: the
        // runner names the phase, the extractor is the only one that knows what
        // it is about to walk. One open, one close, whatever it learns between.
        let opened = events
            .iter()
            .position(|e| matches!(e, Event::Phase(name, _) if name == phase))
            .unwrap_or_else(|| panic!("{phase} announced no phase"));
        let closed = opened
            + events[opened..]
                .iter()
                .position(|e| *e == Event::Done)
                .expect("a phase that opened must close");
        assert!(
            events[opened..closed].contains(&Event::Total(4)),
            "{phase} must count every tracked file: {:?}",
            &events[opened..closed]
        );
    }

    // …and the walk reports arriving at that denominator, so a redrawn line
    // never stops short of the total it promised.
    assert!(
        events.contains(&Event::Tick(4)),
        "a phase must report reaching its own total: {events:?}"
    );
}

#[test]
fn a_walk_reports_often_enough_to_look_alive_and_rarely_enough_to_be_free() {
    // The tick interval is the whole design of the thing: one report per file
    // over 14,000 files costs more than the work between two of them, and one
    // report per phase leaves a redrawn line frozen for minutes.
    assert!(should_tick(TICK_EVERY, 10_000));
    assert!(!should_tick(TICK_EVERY + 1, 10_000));

    // Under one interval's worth of files, the only report is the last one —
    // which must still arrive, or a small repository would show a phase that
    // never finished.
    let events = run(&[("src/A.cs", A_CLASS)]);
    let ticks: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tick(n) => Some(*n),
            _ => None,
        })
        .collect();
    assert!(!ticks.is_empty(), "even one file must report: {events:?}");
    assert!(
        ticks.iter().all(|n| *n <= 1),
        "a tick may never exceed the total it was given: {ticks:?}"
    );
}

#[test]
fn reporting_changes_no_byte_of_the_corpus() {
    // R18 is the reason the library only counts and the binary owns the clock.
    // A pipeline that reported and a pipeline that did not must be
    // indistinguishable in their output, or progress has become an input.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/A.cs", A_CLASS)]);
    let config = config_for(repo.path(), out.path());

    let loud = generate_reporting(&config, &Recorder::default()).unwrap();
    let quiet = reverse_architect::generate(&config).unwrap();

    assert_eq!(loud.cards, quiet.cards);
    assert_eq!(loud.corpus.files, quiet.corpus.files);
    assert_eq!(
        reverse_architect::manifest::to_json(&loud.manifest),
        reverse_architect::manifest::to_json(&quiet.manifest)
    );
    assert_eq!(
        reverse_architect::coverage::to_json(&loud.coverage),
        reverse_architect::coverage::to_json(&quiet.coverage)
    );
}
