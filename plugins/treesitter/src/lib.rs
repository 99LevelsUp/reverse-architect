//! The bundled language plugins of reverse-architect.
//!
//! One crate, one component per language family: `plugins/build.ps1` builds it
//! once per feature (`rust`, `ecmascript`, `python`, `csharp`, `go`, `java`) and
//! names each artifact after its feature. The host embeds them and loads them
//! through the same path a third-party plugin takes (R1).
//!
//! The walkers are the host's former tree-sitter extractor, moved across the
//! component boundary unchanged ([`walkers`]); [`convert`] is the only new code
//! on this side, and it only re-spells what they emit.

wit_bindgen::generate!({
    path: "../../wit-v2",
    world: "language-plugin",
});

mod convert;
mod shim;
#[cfg(test)]
mod tests;
mod walkers;

use exports::ra::plugin::extractor::Guest;
use ra::plugin::types::{Config, FileUsages, Language as WireLanguage, Manifest, Op, Vocabulary};

#[cfg(feature = "rust")]
const ID: &str = "rust";
#[cfg(feature = "ecmascript")]
const ID: &str = "ecmascript";
#[cfg(feature = "python")]
const ID: &str = "python";
#[cfg(feature = "csharp")]
const ID: &str = "csharp";
#[cfg(feature = "go")]
const ID: &str = "go";
#[cfg(feature = "java")]
const ID: &str = "java";
#[cfg(not(any(
    feature = "rust",
    feature = "ecmascript",
    feature = "python",
    feature = "csharp",
    feature = "go",
    feature = "java"
)))]
const ID: &str = "treesitter";

/// The same rank the in-process extractor had (KTD-B8): above file tree and
/// metrics, below manifest dependencies.
const TRUST_RANK: u8 = 30;

fn languages() -> Vec<WireLanguage> {
    let mut out = Vec::new();
    let mut add = |name: &str, exts: &[&str]| {
        out.push(WireLanguage {
            name: name.into(),
            extensions: exts.iter().map(|e| e.to_string()).collect(),
        })
    };
    if cfg!(feature = "rust") {
        add("rust", &["rs"]);
    }
    if cfg!(feature = "ecmascript") {
        add("typescript", &["ts", "tsx"]);
        add("javascript", &["cjs", "js", "jsx", "mjs"]);
    }
    if cfg!(feature = "python") {
        add("python", &["py"]);
    }
    if cfg!(feature = "csharp") {
        add("csharp", &["cs"]);
    }
    if cfg!(feature = "go") {
        add("go", &["go"]);
    }
    if cfg!(feature = "java") {
        add("java", &["java"]);
    }
    out
}

// Constructed only by the export glue, which exists only in the component.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
struct Plugin;

impl Guest for Plugin {
    fn describe() -> Manifest {
        Manifest {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            languages: languages(),
            trust_rank: TRUST_RANK,
            depends_on: Vec::new(),
        }
    }

    fn declare() -> Option<Vocabulary> {
        None
    }

    fn declarations(path: String, source: Vec<u8>, config: Config) -> Vec<Op> {
        convert::declarations(&path, &source, &config.secret_prefixes)
    }

    fn usages(path: String, source: Vec<u8>) -> FileUsages {
        convert::usages(&path, &source)
    }
}

// Only the component exports the interface. A native build exists for the
// tests, and the export names (`ra:plugin/extractor#…`) are not symbols a
// native linker's version script can carry.
#[cfg(target_family = "wasm")]
export!(Plugin);
