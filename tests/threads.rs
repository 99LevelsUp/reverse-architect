//! The corpus must not depend on how many threads produced it (P3/KD2).
//!
//! The three per-file walks parse in parallel and apply in file order. That
//! order is a correctness requirement, not tidiness: `external` emits a foreign
//! symbol's identity facts only on FIRST observation, so which file's provenance
//! survives is decided by application order, and `Graph::add_edge` drops an edge
//! whose endpoint does not exist yet. Both are invisible on a small fixture and
//! both would show up here as a byte difference.
//!
//! ## Why a scoped pool and not `RAYON_NUM_THREADS`
//!
//! The environment variable configures rayon's GLOBAL pool, and only the first
//! time anything asks for it. Two runs in one test process would therefore share
//! whichever pool was built first, and this gate would pass no matter what the
//! code did. `ThreadPoolBuilder::install` gives each run a pool of its own, so
//! the two runs genuinely differ in thread count and the assertion can fail.

use rayon::ThreadPoolBuilder;
use reverse_architect::cli::LinkStyle;
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::{generate, Generated};
use std::path::Path;

mod common;

/// A repository whose output actually depends on the order its files are
/// applied in — otherwise the gate would be green over a fixture with nothing
/// at stake.
///
/// Three things are planted deliberately:
///
/// 1. **One external referenced from many files.** A declared package — an
///    import no manifest declares is not evidence of anything external — is
///    imported by every generated file, so its `ExternalName` provenance names
///    exactly one of them: the first in tracked order.
/// 2. **A type declared in one file and used in others**, so the usage walk has
///    cross-file edges to resolve against the symbol table.
/// 3. **Enough files to interleave.** Sixty is well past the point where twelve
///    threads run concurrently, so a completion-order bug has room to appear.
fn order_sensitive_files() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();

    // A declared dependency, so the imports below are evidence of a real
    // external rather than of a name this repository never heard of — see the
    // `external` extractor: an import no manifest declares is not evidence of
    // anything.
    files.push((
        "proj.csproj".to_string(),
        "<Project>\n\
         \x20 <ItemGroup>\n\
         \x20   <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\n\
         \x20 </ItemGroup>\n\
         </Project>\n"
            .to_string(),
    ));

    // The shared type every other file refers to.
    files.push((
        "src/Shared.cs".to_string(),
        "namespace Shared;\n\
         public class Repository\n{\n    public void Save() {}\n}\n\
         public enum Mode\n{\n    Full,\n    Diff,\n}\n"
            .to_string(),
    ));

    for i in 0..60 {
        files.push((
            format!("src/Service{i:02}.cs"),
            format!(
                "using Newtonsoft.Json;\n\
                 using Shared;\n\
                 \n\
                 namespace App;\n\
                 \n\
                 public class Service{i:02}\n\
                 {{\n\
                 \x20   public string Run()\n\
                 \x20   {{\n\
                 \x20       var repo = new Repository();\n\
                 \x20       repo.Save();\n\
                 \x20       return Newtonsoft.Json.JsonConvert.SerializeObject(repo);\n\
                 \x20   }}\n\
                 \n\
                 \x20   public void Apply(Mode mode)\n\
                 \x20   {{\n\
                 \x20       switch (mode)\n\
                 \x20       {{\n\
                 \x20           case Mode.Full:\n\
                 \x20               Db.Execute(\"update dba.service_{i:02} set done = 1\");\n\
                 \x20               break;\n\
                 \x20       }}\n\
                 \x20       // Service {i:02} applies the mode\n\
                 \x20       throw new System.Exception(\"Service {i:02} cannot apply the mode\");\n\
                 \x20   }}\n\
                 }}\n"
            ),
        ));
    }
    files
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
        // Kept, so the comparison below covers the usage-site records too
        // (plan A, KTD7): what the resolver decided must not depend on the
        // thread count either.
        sites: true,
        rights: reverse_architect::extract::RightsTable::granted(),
        model_verification: false,
        model_purpose: false,
        trace_over_ceiling: false,
    }
}

/// Generate the corpus inside a pool of exactly `threads` threads.
fn generate_on(threads: usize, repo: &Path, out: &Path) -> Generated {
    let pool = ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("a thread pool of a fixed size");
    pool.install(|| generate(&config_for(repo, out)).expect("the pipeline runs"))
}

/// Everything that reaches disk, as comparable bytes.
fn corpus_bytes(g: &Generated) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for f in &g.corpus.files {
        out.push((f.path.clone(), f.contents.clone()));
    }
    // The details and the maps (plan B, U8–U10) render in parallel like the
    // cards, so they are held to the same bytes at every thread count (R15).
    for c in g.cards.iter().chain(&g.details).chain(&g.maps) {
        out.push((c.path.clone(), c.contents.clone()));
    }
    out.push((
        "manifest.json".to_string(),
        reverse_architect::manifest::to_json(&g.manifest),
    ));
    out.push((
        "coverage.json".to_string(),
        reverse_architect::coverage::to_json(&g.coverage),
    ));
    out.push((
        reverse_architect::manifest::SITES_FILENAME.to_string(),
        reverse_architect::manifest::to_sites_jsonl(&g.graph),
    ));
    out.sort();
    out
}

#[test]
fn one_thread_and_many_threads_produce_the_same_corpus() {
    let repo = tempfile::tempdir().unwrap();
    let out_one = tempfile::tempdir().unwrap();
    let out_many = tempfile::tempdir().unwrap();

    let owned = order_sensitive_files();
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    common::init_repo_with_files(repo.path(), "Alice", &files);

    let one = generate_on(1, repo.path(), out_one.path());
    let many = generate_on(12, repo.path(), out_many.path());

    let a = corpus_bytes(&one);
    let b = corpus_bytes(&many);

    // Guard the guard: a fixture that produced nothing interesting would make
    // the comparison below pass while testing nothing at all.
    let manifest = &a
        .iter()
        .find(|(path, _)| path == "manifest.json")
        .expect("a manifest")
        .1;
    assert!(
        manifest.contains("ext:csharp:Newtonsoft.Json"),
        "the fixture must reference an external from many files, or first-observation \
         ordering is never exercised"
    );
    assert!(
        manifest.contains("sym:csharp:Shared#Repository"),
        "the fixture must declare a type used from other files, or cross-file \
         resolution is never exercised"
    );
    assert!(a.len() > 60, "the fixture must be big enough to interleave");
    // Plan B (U15): the anchors, the database accesses and the enum decisions
    // are read per file in the same parallel walk, so the comparison must
    // cover details that hold each of them, and the maps.
    let detail = |suffix: &str| {
        &a.iter()
            .find(|(path, _)| path.starts_with("cards/detail/type/") && path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no detail ending {suffix}"))
            .1
    };
    let service = detail("/Service07.md");
    assert!(
        service.contains("\n## Anchors\n") && service.contains("\n## Database\n"),
        "{service}"
    );
    // Sixty decisions are past the detail's part threshold, so the section
    // is a `###` inside `## Part N` — the cut is under the comparison too.
    let mode = detail("/Mode.md");
    assert!(
        mode.contains("\n## Part 2\n")
            && mode.contains("\n### Enum decisions\n")
            && mode.contains("Service59.Apply("),
        "{mode}"
    );
    assert!(
        a.iter().any(|(path, _)| path.starts_with("cards/map/")),
        "the comparison must cover a map"
    );

    assert_eq!(
        a.len(),
        b.len(),
        "the two runs disagree on how many files the corpus has"
    );
    for ((path_a, text_a), (path_b, text_b)) in a.iter().zip(b.iter()) {
        assert_eq!(path_a, path_b, "the two runs produced different files");
        assert_eq!(
            text_a, text_b,
            "{path_a} differs between a 1-thread and a 12-thread run"
        );
    }
}

/// The gate above compares 1 thread against 12, which catches a result that
/// VARIES with thread count. It cannot catch a wrong order that is applied
/// consistently — reverse the apply loop and both runs reverse together, so they
/// still agree. Verified by doing exactly that: the comparison stayed green.
///
/// So this second test pins the order itself. `external` gives a foreign symbol
/// its identity from the file that reaches it FIRST, in the sorted order of
/// tracked files, and here that is `src/Service00.cs` — sixty files reference
/// the same package and only one of them may name it. Apply the batch in
/// completion order, or reversed, or by any other rule, and this says so.
#[test]
fn an_externals_identity_names_the_first_file_that_reached_it() {
    use reverse_architect::extract::{run_all, Repo};
    use reverse_architect::graph::{FactKind, Graph, NodeKey};

    let repo_dir = tempfile::tempdir().unwrap();
    let owned = order_sensitive_files();
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    common::init_repo_with_files(repo_dir.path(), "Alice", &files);

    // Inside a MULTI-thread pool on purpose. On a single thread, completion
    // order and file order coincide, so an implementation that applied results
    // as they finished would still put `Service00.cs` first and this would pass
    // on a one-core runner while failing to test anything.
    let pool = ThreadPoolBuilder::new()
        .num_threads(12)
        .build()
        .expect("a thread pool of a fixed size");
    let mut graph = Graph::new();
    pool.install(|| {
        let repo = Repo::open(repo_dir.path()).expect("the fixture is a repository");
        run_all(&repo, &mut graph).expect("extraction runs");
    });

    // The premise, stated rather than assumed: `Service00.cs` really is the
    // first tracked file that references the external. If the fixture's naming
    // or git's ordering ever changed, the assertions below would fail in a way
    // that looked like a regression in the code.
    {
        let repo = Repo::open(repo_dir.path()).expect("the fixture is a repository");
        let first = repo
            .tracked_files_with_paths()
            .expect("the index is readable")
            .into_iter()
            .map(|tf| tf.key)
            .find(|key| key.ends_with(".cs") && key.contains("Service"))
            .expect("the fixture has Service files");
        assert_eq!(
            first, "src/Service00.cs",
            "the fixture's first referencing file changed; the expectations below \
             are about tracked order and must be updated with it"
        );
    }

    let node = graph
        .node(&NodeKey::new("ext:csharp:Newtonsoft.Json"))
        .expect("the declared package is referenced, so it is an External node");
    let name = node
        .facts
        .iter()
        .find(|f| f.kind == FactKind::ExternalName)
        .expect("an External node carries its name");

    assert_eq!(
        name.provenance.file, "src/Service00.cs",
        "the identity must come from the first file in TRACKED ORDER that \
         referenced the external — not from whichever file happened to finish \
         parsing first"
    );
    // The kind is the other half of the identity and is emitted in the same
    // guarded group, so it must name the same file. Asserting only the name
    // would let half the identity drift.
    let kind = node
        .facts
        .iter()
        .find(|f| f.kind == FactKind::ExternalKind)
        .expect("an External node carries its kind");
    assert_eq!(
        kind.provenance.file, "src/Service00.cs",
        "both halves of an external's identity come from the same first observation"
    );

    // …and exactly once each: a second claim would mean the first-observation
    // guard stopped guarding, and every referencing file stacked its own.
    for kind in [FactKind::ExternalName, FactKind::ExternalKind] {
        assert_eq!(
            node.facts.iter().filter(|f| f.kind == kind).count(),
            1,
            "sixty files reference this external; only the first may state {kind:?}"
        );
    }
}
