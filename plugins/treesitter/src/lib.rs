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
#[cfg(not(feature = "csharp-body"))]
wit_bindgen::generate!({
    path: "../../wit-v2",
    world: "language-plugin",
});
#[cfg(not(feature = "csharp-body"))]
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
const ID: &str = FALLBACK_ID;

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
    if cfg!(any(feature = "csharp", feature = "csharp-body")) {
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
#[cfg(not(feature = "csharp-body"))]
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
#[cfg(all(target_family = "wasm", not(feature = "csharp-body")))]
export!(Plugin);

// The id of a build that enables none of the six language features: the
// crate's own name, or `csharp-body` for the component that reads the bodies
// of C# members. Defined down here so that no line above moves: the bytes of
// the `csharp` component depend on where its code is written.
#[cfg(not(feature = "csharp-body"))]
#[allow(dead_code)]
const FALLBACK_ID: &str = "treesitter";
#[cfg(feature = "csharp-body")]
const FALLBACK_ID: &str = "csharp-body";

// ---- `csharp-body`: contract 0.3, the `bodies` pass only ----
//
// The same crate built against `wit-v3` instead of `wit-v2`. It serves one
// pass, `bodies`, for `.cs` files ([`body`]); the first layer of those files
// stays with the `csharp` component, which this feature does not change.

#[cfg(feature = "csharp-body")]
wit_bindgen::generate!({
    path: "../../wit-v3",
    world: "language-plugin",
    additional_derives: [PartialEq],
});

#[cfg(feature = "csharp-body")]
mod body;

// `crate::convert` is not built here (it spells contract 0.2's records), which
// leaves the shim's wire words unread. They stay what the walkers' vocabulary
// means; this keeps them in use rather than moving a line of `shim.rs`.
#[cfg(feature = "csharp-body")]
const _: [fn(&shim::EdgeKind) -> &'static str; 1] = [|k| k.word()];
#[cfg(feature = "csharp-body")]
const _: [fn(&shim::FactKind) -> &'static str; 1] = [|k| k.word()];

#[cfg(feature = "csharp-body")]
impl Guest for Plugin {
    fn describe() -> Manifest {
        Manifest {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            languages: languages(),
            trust_rank: TRUST_RANK,
            depends_on: Vec::new(),
            passes: vec![ra::plugin::types::Pass::Bodies],
            emits: body::EMITS.iter().map(|e| e.to_string()).collect(),
        }
    }

    fn declare() -> Option<Vocabulary> {
        None
    }

    /// Not served (`passes` lists only `bodies`): the host never calls it.
    fn declarations(
        _path: String,
        _source: Vec<u8>,
        _config: Config,
    ) -> Result<ra::plugin::types::FileDeclarations, String> {
        Ok(ra::plugin::types::FileDeclarations {
            ops: Vec::<Op>::new(),
            name_refs: Vec::new(),
            blocks: Vec::new(),
        })
    }

    /// Not served, as `declarations`.
    fn usages(_path: String, _source: Vec<u8>) -> Result<FileUsages, String> {
        Ok(FileUsages {
            scopes: Vec::new(),
            usages: Vec::new(),
        })
    }

    fn bodies(path: String, source: Vec<u8>) -> Result<ra::plugin::types::FileBodies, String> {
        body::bodies(&path, &source)
    }
}

#[cfg(all(target_family = "wasm", feature = "csharp-body"))]
export!(Plugin);
