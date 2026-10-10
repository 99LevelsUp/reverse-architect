//! The one line a run with the second layer adds to the view `entry-points`
//! (plan 2b, U8: KTD12, R15): the coverage of triggers, right above the
//! sentence E9. The tests that hold a layered run's views to the plain run's
//! take it out first, after checking it is there once and where it belongs.
//!
//! Loaded with `#[path = "common/coverage_line.rs"] mod coverage_line;` by the
//! test crates that use it, so `tests/common/mod.rs` stays as it is.

#![allow(dead_code)]

use reverse_architect::render::Corpus;

/// The view the line goes into.
pub const ENTRY_POINTS: &str = "entry-points.md";

/// The lead of the line.
pub const COVERAGE_LINE: &str = "- Coverage of triggers by the second layer, by language — ";

/// The lead of the sentence E9 the line stands right above.
const E9: &str = "- Framework-level entry points — ";

/// `contents` of `entry-points.md` with the line taken out, after checking
/// it is there exactly once, right above E9.
pub fn without_line(contents: &str) -> String {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let at: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].starts_with(COVERAGE_LINE))
        .collect();
    assert_eq!(at.len(), 1, "one line of coverage:\n{contents}");
    assert!(
        lines.get(at[0] + 1).is_some_and(|l| l.starts_with(E9)),
        "right above E9:\n{contents}"
    );
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at[0])
        .map(|(_, l)| *l)
        .collect()
}

/// A layered run's views with the line taken out of `entry-points.md` —
/// what the plain run's views must equal.
pub fn without_coverage_line(corpus: &Corpus) -> Corpus {
    let mut out = corpus.clone();
    for f in out.files.iter_mut().filter(|f| f.path == ENTRY_POINTS) {
        f.contents = without_line(&f.contents);
    }
    out
}

/// The same for a file's bytes on disk: `entry-points.md` without the line,
/// every other file as it is.
pub fn without_coverage_bytes(path: &str, bytes: &[u8]) -> Vec<u8> {
    if path != ENTRY_POINTS {
        return bytes.to_vec();
    }
    without_line(&String::from_utf8_lossy(bytes)).into_bytes()
}
