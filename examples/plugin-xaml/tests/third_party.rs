//! The plugin is built the way a third party builds one (plan 2c, U3: AE1): in its own
//! workspace, from the published contract alone, with its findings about that contract
//! written down. These tests keep that true as the plugin changes.

use std::fs;
use std::path::Path;

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The lines of one `[section]` of a TOML file, comments and blank lines left out.
fn section<'t>(toml: &'t str, name: &str) -> Vec<&'t str> {
    let header = format!("[{name}]");
    toml.lines()
        .skip_while(|l| l.trim() != header)
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

#[test]
fn the_plugin_depends_on_nothing_of_the_host_and_is_a_workspace_of_its_own() {
    let manifest = read("Cargo.toml");
    let deps: Vec<&str> = section(&manifest, "dependencies")
        .into_iter()
        .map(|l| l.split('=').next().unwrap_or("").trim())
        .collect();
    assert_eq!(deps, ["wit-bindgen", "quick-xml"]);
    assert!(
        !manifest.contains("path ="),
        "a path dependency reaches into the repository"
    );
    assert!(manifest.lines().any(|l| l.trim() == "[workspace]"));
}

#[test]
fn the_bindings_come_from_the_published_contract() {
    let lib = read("src/lib.rs");
    assert!(lib.contains("wit_bindgen::generate!({"));
    assert!(lib.contains("path: \"../../wit-v3\""));
    let wit = read("../../wit-v3/extractor.wit");
    assert!(wit.contains("package ra:plugin@0.3.0;"));
}

#[test]
fn the_findings_book_gives_every_construct_a_verdict() {
    let book = read("FINDINGS.md");
    let rows: Vec<&str> = book
        .lines()
        .skip_while(|l| l.trim() != "## Constructs")
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with("| ") && !l.starts_with("| Construct"))
        .collect();
    assert!(rows.len() >= 10, "the book lists {} constructs", rows.len());
    for row in rows {
        let verdict = row.split('|').nth(3).unwrap_or("").trim();
        assert!(
            ["fits", "needs-ra-change", "sacrificed"]
                .iter()
                .any(|v| verdict.starts_with(v)),
            "a construct without a verdict: {row}"
        );
    }
}
