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

/// The component that reads the bodies of C# members: contract 0.3, the
/// `bodies` pass only, built from `plugins/treesitter` with the feature
/// `csharp-body`. Kept out of [`BUNDLED`] on purpose: the first layer never
/// loads it, so a run without the second layer (`--detail-level`) is exactly
/// what it was.
pub const CSHARP_BODY: (&str, &[u8]) = (
    "csharp-body",
    include_bytes!("../../plugins/prebuilt/csharp-body.wasm"),
);

/// Load [`CSHARP_BODY`], the way [`load_one`] loads a bundled plugin.
pub fn load_csharp_body() -> Result<Plugin, PluginError> {
    let (id, bytes) = CSHARP_BODY;
    Plugin::load(&format!("bundled:{id}"), bytes)
}

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

    /// The body component loads through the same path as every plugin, runs
    /// under contract 0.3 and serves `bodies` alone — and is not one of the
    /// plugins a run loads by default.
    #[test]
    fn the_csharp_body_component_loads_as_contract_0_3_serving_bodies_only() {
        use crate::plugin::host::Contract;
        use crate::plugin::manifest::Pass;
        let p = load_csharp_body().unwrap();
        assert_eq!(p.id(), CSHARP_BODY.0);
        assert_eq!(p.contract(), Contract::V0_3);
        assert_eq!(p.manifest().passes, [Pass::Bodies]);
        for kind in ["member-access", "invocation", "key-compare", "menu-item"] {
            assert!(
                p.manifest().emits.iter().any(|e| e == kind),
                "`{kind}` is not in emits: {:?}",
                p.manifest().emits
            );
        }
        assert_eq!(p.manifest().language_for("cs"), Some("csharp"));
        assert!(BUNDLED.iter().all(|(id, _)| *id != CSHARP_BODY.0));
    }

    /// The bodies of a C# file, read through the sandbox: the model comes
    /// back, a chain of 2 000 `else if`s and a sum of 5 000 operands fit the
    /// guest's stack and its fuel, and the first layer's passes are refused.
    #[test]
    fn the_csharp_body_component_reads_bodies_in_the_sandbox() {
        use crate::plugin::host::{BodyNodeKind, PluginError, WireConfig};
        use std::collections::HashSet;
        use std::sync::Arc;
        let p = load_csharp_body().unwrap();
        let mut s = p.session(Arc::new(HashSet::new())).unwrap();

        let src = "class EditorView {\n    void OnKeyDown(KeyEventArgs e) {\n        if (e.Key == Key.F4) Check();\n    }\n}\n";
        let file = s.bodies("src/EditorView.cs", src.as_bytes()).unwrap();
        assert_eq!(file.members.len(), 1);
        let m = &file.members[0];
        assert_eq!((m.name_position.line, m.name_position.column), (2, 10));
        let arm = m
            .nodes
            .iter()
            .find(|n| matches!(n.kind, BodyNodeKind::Arm))
            .expect("the F4 arm");
        assert!(arm
            .tags
            .iter()
            .any(|t| t.name == "key-compare" && t.value.as_deref() == Some("Key.F4")));

        let mut deep = String::from("class C {\n  int M(int x) {\n    if (x == 0) return 0;\n");
        for i in 1..2000 {
            deep.push_str(&format!("    else if (x == {i}) return {i};\n"));
        }
        deep.push_str("    else return -1;\n  }\n  int S(int a) => a");
        for _ in 1..5000 {
            deep.push_str(" + a");
        }
        deep.push_str(";\n}\n");
        let file = s.bodies("src/Deep.cs", deep.as_bytes()).unwrap();
        let arms = file.members[0]
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, BodyNodeKind::Arm))
            .count();
        assert_eq!(arms, 2001);
        assert_eq!(file.members[1].accesses.len(), 5000);

        let config = WireConfig {
            secret_prefixes: Vec::new(),
        };
        let err = s
            .declarations("src/a.cs", b"class C {}", &config)
            .unwrap_err();
        assert!(matches!(&err, PluginError::PassNotServed { .. }), "{err}");
    }
}
