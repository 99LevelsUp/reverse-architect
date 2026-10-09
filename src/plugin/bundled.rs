//! The language plugins that ship inside the binary (KTD23).
//!
//! Each is a component built from `plugins/treesitter` and committed under
//! `plugins/prebuilt/` (implementation note D3), embedded here with
//! `include_bytes!` so an installed binary needs nothing beside itself. They are
//! loaded through exactly the path a third-party plugin takes — the same
//! [`Plugin::load`], the same manifest checks, the same sandbox — and differ
//! from one only in where their bytes come from (R1).

use super::host::{Plugin, PluginError};
use std::sync::OnceLock;

/// `(plugin id, component bytes)` for every bundled language plugin, in the
/// order the extractor runs them.
pub const BUNDLED: [(&str, &[u8]); 7] = [
    (
        "csharp",
        include_bytes!("../../plugins/prebuilt/csharp.wasm"),
    ),
    (
        "ecmascript",
        include_bytes!("../../plugins/prebuilt/ecmascript.wasm"),
    ),
    ("go", include_bytes!("../../plugins/prebuilt/go.wasm")),
    ("java", include_bytes!("../../plugins/prebuilt/java.wasm")),
    (
        "python",
        include_bytes!("../../plugins/prebuilt/python.wasm"),
    ),
    ("rust", include_bytes!("../../plugins/prebuilt/rust.wasm")),
    // Built from `examples/plugin-sql`, outside the plugin workspace and
    // against the published WIT alone — the way a third party builds one.
    ("sql", include_bytes!("../../plugins/prebuilt/sql.wasm")),
];

/// Every bundled plugin, loaded once per process.
///
/// Loading compiles the components, which is the expensive part of a plugin's
/// life; doing it once and handing out references keeps it off every file.
/// A bundled component that fails to load is a broken build, not a runtime
/// condition, so it panics with the reason.
pub fn bundled() -> &'static [Plugin] {
    static PLUGINS: OnceLock<Vec<Plugin>> = OnceLock::new();
    PLUGINS.get_or_init(|| {
        use rayon::prelude::*;
        // Compiled side by side: the first run on a machine pays the compile
        // once per component, and they are independent.
        BUNDLED
            .par_iter()
            .map(|(id, bytes)| {
                let plugin = Plugin::load(&format!("bundled:{id}"), bytes)
                    .unwrap_or_else(|e| panic!("bundled plugin `{id}` is broken: {e}"));
                assert_eq!(
                    plugin.id(),
                    *id,
                    "bundled component `{id}` describes itself under another id"
                );
                plugin
            })
            .collect()
    })
}

/// The bundled plugin that claims `ext`, if any.
pub fn for_extension(ext: &str) -> Option<&'static Plugin> {
    bundled()
        .iter()
        .find(|p| p.manifest().extensions().contains(ext))
}

/// Load one bundled plugin by id without loading the others — for tests and
/// diagnostics that need a single language.
pub fn load_one(id: &str) -> Result<Plugin, PluginError> {
    let (_, bytes) = BUNDLED
        .iter()
        .find(|(i, _)| *i == id)
        .unwrap_or_else(|| panic!("no bundled plugin `{id}`"));
    Plugin::load(&format!("bundled:{id}"), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// At most one bundled plugin per (extension, pass) pair (contract 0.3):
    /// two bundled components may share an extension only by serving
    /// different passes of it, so the order of [`BUNDLED`] never decides who
    /// reads a file.
    #[test]
    fn every_bundled_plugin_loads_under_its_own_id_and_no_extension_and_pass_overlap() {
        use crate::plugin::manifest::Pass;
        let t = std::time::Instant::now();
        let plugins = bundled();
        eprintln!(
            "loading {} bundled plugins took {:?}",
            plugins.len(),
            t.elapsed()
        );
        let mut served: BTreeSet<(&str, Pass)> = BTreeSet::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for p in plugins {
            for ext in p.manifest().extensions() {
                for pass in &p.manifest().passes {
                    assert!(
                        served.insert((ext, *pass)),
                        "`.{ext}` is served for `{}` twice",
                        pass.word()
                    );
                }
                seen.insert(ext);
            }
        }
        let expected: BTreeSet<&str> = [
            "cjs", "cs", "go", "java", "js", "jsx", "mjs", "py", "rs", "sql", "ts", "tsx",
        ]
        .into_iter()
        .collect();
        assert_eq!(seen, expected);
        assert_eq!(for_extension("tsx").map(Plugin::id), Some("ecmascript"));
    }
}
