//! Usings in force in a C# file without being written in it (plan A, KTD6):
//! `global using` directives written in OTHER files of its project, the
//! `<Using Include="…"/>` items of its project file, and the SDK's implicit
//! usings when the project enables them.
//!
//! `namespace-visibility` disputes a type whose namespace no using in the
//! site's own scope chain imports. Without this module every type a project
//! imports only through a `GlobalUsings.cs` or its `.csproj` was disputed, and
//! a true lone candidate sank to "maybe also" and out of the card's
//! aggregates. What is collected here only ever turns a dispute into silence,
//! so every reading below errs toward MORE visibility: a `Condition` is
//! ignored, `<Using Remove>` is ignored, `ImplicitUsings` counts as enabled
//! wherever any project file around the source enables it, a file that belongs
//! to no project sees everything collected anywhere. And where a project file
//! holds a `<Using>` item this reading cannot follow (an MSBuild property, an
//! item transform, a wildcard), the rule abstains for that whole project
//! rather than dispute on a set it knows is incomplete.
//!
//! The scan is the host's, over the tracked blobs, before the usage pass:
//! the per-file usage records the cache keeps hold resolved sites, not the
//! plugin's scopes, so a warm run could never rebuild the set from them, and
//! a set that depended on which files missed the cache would make cold and
//! warm runs disagree. Every input is a tracked file, so a changed `.csproj`
//! or `GlobalUsings.cs` changes the run fingerprint like any other blob.
//!
//! What still stays invisible — usings a source generator emits, a
//! `Directory.Build.props` this layout does not reach, a `<Using>` written as
//! child elements — is why the rule keeps no right to delete.
//!
//! The same scan keeps the project-wide `using static` targets and alias
//! names (`global using static Outer;`, `<Using Include="Outer"
//! Static="true"/>`, `global using L = …;`, `<Using … Alias="L"/>`) for
//! `nested-type-visibility`: either can make a nested type visible by its
//! simple name in every file of the project. A static item this reading cannot
//! follow makes the project unknown, as a namespace item does.

use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// The SDK's implicit global usings (`<ImplicitUsings>enable`), by SDK, as
/// the .NET SDK documents them ("Implicit using directives",
/// learn.microsoft.com/dotnet/core/project-sdk/overview). Every SDK starts
/// from the base set; WPF removes two of it, which is ignored here (more
/// visibility, never less).
const IMPLICIT_BASE: &[&str] = &[
    "System",
    "System.Collections.Generic",
    "System.IO",
    "System.Linq",
    "System.Net.Http",
    "System.Threading",
    "System.Threading.Tasks",
];
const IMPLICIT_EXTRA: &[(&str, &[&str])] = &[
    (
        "Microsoft.NET.Sdk.Web",
        &[
            "System.Net.Http.Json",
            "Microsoft.AspNetCore.Builder",
            "Microsoft.AspNetCore.Hosting",
            "Microsoft.AspNetCore.Http",
            "Microsoft.AspNetCore.Routing",
            "Microsoft.Extensions.Configuration",
            "Microsoft.Extensions.DependencyInjection",
            "Microsoft.Extensions.Hosting",
            "Microsoft.Extensions.Logging",
        ],
    ),
    (
        "Microsoft.NET.Sdk.Worker",
        &[
            "Microsoft.Extensions.Configuration",
            "Microsoft.Extensions.DependencyInjection",
            "Microsoft.Extensions.Hosting",
            "Microsoft.Extensions.Logging",
        ],
    ),
    (
        "Microsoft.NET.Sdk.WindowsDesktop",
        &["System.Drawing", "System.Windows.Forms"],
    ),
];

/// What the project-wide usings make visible in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectWide {
    /// These, beyond what the file's own scopes import.
    Known(Imports),
    /// A project file around it imports something this reading cannot
    /// follow: `namespace-visibility` and `nested-type-visibility` abstain
    /// for the file.
    Unknown,
}

/// What a project imports into every one of its files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Imports {
    pub namespaces: HashSet<String>,
    /// The types a `using static` names, as written (`Orbit.Mcp.Catalog`).
    pub statics: HashSet<String>,
    /// The names an alias introduces (`L` of `L = Orbit.Mcp.Catalog.List`).
    pub aliases: HashSet<String>,
}

/// One file's share: a project file's, a `Directory.Build.*` file's, or the
/// `global using` directives of one source file.
#[derive(Debug, Default, Clone)]
struct Contribution {
    namespaces: BTreeSet<String>,
    statics: BTreeSet<String>,
    aliases: BTreeSet<String>,
    unknown: bool,
}

impl Contribution {
    fn merge(&mut self, other: &Contribution) {
        self.namespaces.extend(other.namespaces.iter().cloned());
        self.statics.extend(other.statics.iter().cloned());
        self.aliases.extend(other.aliases.iter().cloned());
        self.unknown |= other.unknown;
    }

    fn is_empty(&self) -> bool {
        self.namespaces.is_empty() && self.statics.is_empty() && self.aliases.is_empty()
    }
}

/// Collects the run's project-wide usings, one tracked file at a time.
#[derive(Default)]
pub(crate) struct ProjectUsingsBuilder {
    /// Directories holding a `.csproj` (repository-relative, `""` the root).
    projects: BTreeSet<String>,
    /// What the project files and `Directory.Build.*` files of a directory
    /// declare, merged — `X.csproj` and `X.NetCore.csproj` side by side
    /// compile the same sources, so a directory is one project here.
    declared: BTreeMap<String, Contribution>,
    /// What the `global using` directives of a source file import, by that
    /// file's path.
    sources: BTreeMap<String, Contribution>,
}

/// Whether the builder has any use for a tracked file of this path.
pub(crate) fn wants(path: &str) -> bool {
    let name = file_name(path).to_ascii_lowercase();
    name.ends_with(".cs")
        || name.ends_with(".csproj")
        || name == "directory.build.props"
        || name == "directory.build.targets"
}

impl ProjectUsingsBuilder {
    /// Read one tracked file the builder [`wants`].
    pub(crate) fn add(&mut self, path: &str, bytes: &[u8]) {
        let name = file_name(path).to_ascii_lowercase();
        if name.ends_with(".cs") {
            let imports = global_usings_in(bytes);
            if !imports.is_empty() {
                self.sources.insert(path.to_string(), imports);
            }
            return;
        }
        let is_project = name.ends_with(".csproj");
        if !is_project && name != "directory.build.props" && name != "directory.build.targets" {
            return;
        }
        let dir = parent(path).to_string();
        if is_project {
            self.projects.insert(dir.clone());
        }
        let contribution = read_project_file(&String::from_utf8_lossy(bytes));
        self.declared.entry(dir).or_default().merge(&contribution);
    }

    pub(crate) fn finish(self) -> ProjectUsings {
        let project_of = |path: &str| nearest(&self.projects, parent(path)).map(str::to_string);
        // Source globals by the project they belong to; `None` for a file
        // under no project, whose globals are taken to reach every file.
        let mut by_project: BTreeMap<Option<String>, Contribution> = BTreeMap::new();
        for (path, imports) in &self.sources {
            by_project
                .entry(project_of(path))
                .or_default()
                .merge(imports);
        }
        let everywhere = by_project.get(&None).cloned().unwrap_or_default();

        let mut visible: HashMap<String, ProjectWide> = HashMap::new();
        for project in &self.projects {
            let mut set = everywhere.clone();
            if let Some(own) = by_project.get(&Some(project.clone())) {
                set.merge(own);
            }
            // The project's own directory and every directory above it: a
            // `Directory.Build.*` there is imported by the project.
            for dir in ancestors(project) {
                if let Some(c) = self.declared.get(dir) {
                    set.merge(c);
                }
            }
            visible.insert(project.clone(), wide(set));
        }
        // A file under no project: its compilation is unknown, so it sees
        // everything collected anywhere.
        let mut set = Contribution::default();
        for c in by_project.values().chain(self.declared.values()) {
            set.merge(c);
        }
        ProjectUsings {
            projects: self.projects,
            visible,
            unattributed: wide(set),
        }
    }
}

fn wide(set: Contribution) -> ProjectWide {
    if set.unknown {
        ProjectWide::Unknown
    } else {
        ProjectWide::Known(Imports {
            namespaces: set.namespaces.into_iter().collect(),
            statics: set.statics.into_iter().collect(),
            aliases: set.aliases.into_iter().collect(),
        })
    }
}

/// The run's project-wide usings, ready to be looked up per file.
pub(crate) struct ProjectUsings {
    projects: BTreeSet<String>,
    /// By project directory.
    visible: HashMap<String, ProjectWide>,
    /// For a file under no project.
    unattributed: ProjectWide,
}

impl Default for ProjectUsings {
    fn default() -> ProjectUsings {
        ProjectUsingsBuilder::default().finish()
    }
}

/// The project-wide usings of one run, handed from the languages extractor,
/// which reads them before its usage pass, to the external one, which checks
/// a decision by an enum's value against them (plan B, U5) — so the tracked
/// blobs are not read for them a second time. Both extractors get a clone of
/// one handoff when the run's extractors are built, and the languages
/// extractor runs first.
///
/// Empty until it is put: a reader that runs without the languages extractor
/// before it sees no project-wide using at all — it loses the enums reached
/// only through them and the aliases they declare. The run's extractor list
/// is what orders the two.
#[derive(Clone, Default)]
pub(crate) struct UsingsHandoff(Arc<Mutex<Option<Arc<ProjectUsings>>>>);

impl UsingsHandoff {
    pub(crate) fn put(&self, usings: Arc<ProjectUsings>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(usings);
    }

    pub(crate) fn get(&self) -> Arc<ProjectUsings> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_default()
    }
}

impl ProjectUsings {
    /// What the project-wide usings make visible in `file`: those of the
    /// project in the nearest directory above it that holds a `.csproj`.
    pub(crate) fn for_file(&self, file: &str) -> &ProjectWide {
        nearest(&self.projects, parent(file))
            .and_then(|p| self.visible.get(p))
            .unwrap_or(&self.unattributed)
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// `dir` and every directory above it, out to the root (`""`).
fn ancestors(dir: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(dir);
    std::iter::from_fn(move || {
        let at = next?;
        next = (!at.is_empty()).then(|| parent(at));
        Some(at)
    })
}

/// The nearest of `projects` at or above `dir`.
fn nearest<'p>(projects: &'p BTreeSet<String>, dir: &str) -> Option<&'p str> {
    ancestors(dir).find_map(|d| projects.get(d).map(String::as_str))
}

/// What the `global using` directives of one source file import: namespaces,
/// the types of static ones and the names of aliases, each kept apart. A
/// directive must start its line — C# allows them only at the top of a file,
/// before any declaration — which is what keeps this a line scan.
fn global_usings_in(bytes: &[u8]) -> Contribution {
    static RE: OnceLock<BytesRegex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        BytesRegex::new(r"(?m)^(?-u:\xEF\xBB\xBF)?[ \t]*global[ \t]+using[ \t]+([^;\r\n]+);")
            .expect("a valid pattern")
    });
    let mut out = Contribution::default();
    for caps in re.captures_iter(bytes) {
        let Ok(text) = std::str::from_utf8(&caps[1]) else {
            continue;
        };
        let text = text.trim();
        if let Some((alias, _)) = text.split_once('=') {
            let alias = alias.trim();
            if !alias.is_empty() {
                out.aliases.insert(alias.to_string());
            }
            continue;
        }
        let (text, set) = match text.strip_prefix("static") {
            Some(rest) if rest.starts_with(char::is_whitespace) => (rest, &mut out.statics),
            _ => (text, &mut out.namespaces),
        };
        let target: String = text.split_whitespace().collect();
        let target = target.strip_prefix("global::").unwrap_or(&target);
        if !target.is_empty() {
            set.insert(target.to_string());
        }
    }
    out
}

/// A project file's `<Using>` items and its `ImplicitUsings` switch, read as
/// text. An item it cannot follow marks the contribution unknown.
fn read_project_file(text: &str) -> Contribution {
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    static USING: OnceLock<Regex> = OnceLock::new();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    static IMPLICIT: OnceLock<Regex> = OnceLock::new();
    static SDK: OnceLock<Regex> = OnceLock::new();
    let comment = COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").expect("a valid pattern"));
    let using = USING.get_or_init(|| Regex::new(r"<Using\b([^>]*)>").expect("a valid pattern"));
    let attr = ATTR.get_or_init(|| {
        Regex::new(r#"\b(Include|Static|Alias)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
            .expect("a valid pattern")
    });
    let implicit = IMPLICIT.get_or_init(|| {
        Regex::new(r"<ImplicitUsings\b[^>]*>([^<]*)</ImplicitUsings>").expect("a valid pattern")
    });
    let sdk = SDK.get_or_init(|| {
        Regex::new(r#"<Project\b[^>]*\bSdk\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
            .expect("a valid pattern")
    });

    let text = comment.replace_all(text, "");
    let mut out = Contribution::default();
    for item in using.captures_iter(&text) {
        let mut include = None;
        let mut is_static = false;
        let mut alias = None;
        for a in attr.captures_iter(&item[1]) {
            let value = a.get(2).or_else(|| a.get(3)).map_or("", |m| m.as_str());
            match &a[1] {
                "Include" => include = Some(value),
                "Static" => is_static |= value.trim().eq_ignore_ascii_case("true"),
                _ => alias = Some(value.trim()),
            }
        }
        // `Remove` and `Update` items import nothing.
        let Some(include) = include else { continue };
        // An alias imports a name, not a namespace, whatever it names.
        if let Some(alias) = alias {
            if !alias.is_empty() {
                out.aliases.insert(alias.to_string());
            }
            continue;
        }
        if ["$(", "@(", "%(", "*", "?"]
            .iter()
            .any(|m| include.contains(m))
        {
            out.unknown = true;
            continue;
        }
        let set = if is_static {
            &mut out.statics
        } else {
            &mut out.namespaces
        };
        set.extend(
            include
                .split(';')
                .map(|t| t.split_whitespace().collect::<String>())
                .map(|t| t.strip_prefix("global::").map_or(t.clone(), str::to_string))
                .filter(|t| !t.is_empty()),
        );
    }
    let enabled = implicit.captures_iter(&text).any(|c| {
        let v = c[1].trim();
        !(v.is_empty() || v.eq_ignore_ascii_case("disable") || v.eq_ignore_ascii_case("false"))
    });
    if enabled {
        let sdk = sdk
            .captures(&text)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| {
                m.as_str()
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            });
        out.namespaces
            .extend(IMPLICIT_BASE.iter().map(|s| s.to_string()));
        for (name, extra) in IMPLICIT_EXTRA {
            // A file that names no SDK (`Directory.Build.props`, an
            // `<Sdk>` element) could be any: every set, more visibility.
            if sdk.as_deref().is_none_or(|s| s.eq_ignore_ascii_case(name)) {
                out.namespaces.extend(extra.iter().map(|s| s.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imports(wide: &ProjectWide) -> &Imports {
        match wide {
            ProjectWide::Known(imports) => imports,
            ProjectWide::Unknown => panic!("expected a known set"),
        }
    }

    fn known(wide: &ProjectWide) -> BTreeSet<&str> {
        imports(wide)
            .namespaces
            .iter()
            .map(String::as_str)
            .collect()
    }

    fn sorted(set: &HashSet<String>) -> Vec<&str> {
        let mut out: Vec<&str> = set.iter().map(String::as_str).collect();
        out.sort();
        out
    }

    #[test]
    fn a_global_using_reaches_every_file_of_its_project_and_no_other() {
        let mut b = ProjectUsingsBuilder::default();
        b.add(
            "App/App.csproj",
            b"<Project Sdk=\"Microsoft.NET.Sdk\"></Project>",
        );
        b.add(
            "App/GlobalUsings.cs",
            "\u{FEFF}global using Orbit.Model;\nglobal using static Orbit.Util;\nglobal using M = Orbit.Other;\n  global   using global::Orbit.Data ;\n// global using Orbit.Commented;\n".as_bytes(),
        );
        b.add(
            "Other/Other.csproj",
            b"<Project Sdk=\"Microsoft.NET.Sdk\"></Project>",
        );
        let p = b.finish();
        assert_eq!(
            known(p.for_file("App/Sub/Caller.cs")),
            ["Orbit.Data", "Orbit.Model"].into_iter().collect()
        );
        // A static using and an alias import no namespace, but each is kept
        // for `nested-type-visibility`.
        let app = imports(p.for_file("App/Sub/Caller.cs"));
        assert_eq!(sorted(&app.statics), ["Orbit.Util"]);
        assert_eq!(sorted(&app.aliases), ["M"]);
        assert!(known(p.for_file("Other/X.cs")).is_empty());
        assert!(imports(p.for_file("Other/X.cs")).statics.is_empty());
    }

    #[test]
    fn a_project_file_adds_its_using_items_and_the_implicit_usings() {
        let mut b = ProjectUsingsBuilder::default();
        b.add(
            "Web/Web.csproj",
            br#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup><ImplicitUsings>enable</ImplicitUsings></PropertyGroup>
  <ItemGroup>
    <Using Include="Orbit.Utils" />
    <Using Include="Orbit.A;Orbit.B" />
    <Using Include="Orbit.Static" Static="true" />
    <Using Include="Orbit.Aliased" Alias="K" />
    <Using Remove="System.Net.Http" />
    <!-- <Using Include="Orbit.Commented" /> -->
  </ItemGroup>
</Project>"#,
        );
        b.add(
            "Lib/Lib.csproj",
            b"<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><ImplicitUsings>disable</ImplicitUsings></PropertyGroup></Project>",
        );
        let p = b.finish();
        let web = known(p.for_file("Web/Program.cs"));
        for ns in [
            "Orbit.Utils",
            "Orbit.A",
            "Orbit.B",
            "System.Linq",
            "Microsoft.AspNetCore.Builder",
        ] {
            assert!(web.contains(ns), "{ns}: {web:?}");
        }
        for ns in ["Orbit.Static", "Orbit.Aliased", "Orbit.Commented"] {
            assert!(!web.contains(ns), "{ns}: {web:?}");
        }
        let web = imports(p.for_file("Web/Program.cs"));
        assert_eq!(sorted(&web.statics), ["Orbit.Static"]);
        assert_eq!(sorted(&web.aliases), ["K"]);
        assert!(known(p.for_file("Lib/X.cs")).is_empty());
    }

    #[test]
    fn a_using_item_that_cannot_be_read_makes_the_project_unknown() {
        let mut b = ProjectUsingsBuilder::default();
        b.add(
            "App/App.csproj",
            b"<Project><ItemGroup><Using Include=\"$(RootNamespace).Model\" /></ItemGroup></Project>",
        );
        // A static item names a type whose nested types every file then
        // sees: one that cannot be read leaves that unknown too.
        b.add(
            "Tools/Tools.csproj",
            b"<Project><ItemGroup><Using Include=\"$(RootNamespace).Catalog\" Static=\"true\" /></ItemGroup></Project>",
        );
        b.add("Lib/Lib.csproj", b"<Project></Project>");
        let p = b.finish();
        assert_eq!(p.for_file("App/X.cs"), &ProjectWide::Unknown);
        assert_eq!(p.for_file("Tools/X.cs"), &ProjectWide::Unknown);
        assert_eq!(known(p.for_file("Lib/X.cs")), BTreeSet::new());
    }

    #[test]
    fn directory_build_props_reach_every_project_below_it() {
        let mut b = ProjectUsingsBuilder::default();
        b.add(
            "Directory.Build.props",
            b"<Project><ItemGroup><Using Include=\"Orbit.Shared\" /></ItemGroup></Project>",
        );
        b.add(
            "src/App/App.csproj",
            b"<Project Sdk=\"Microsoft.NET.Sdk\" />",
        );
        let p = b.finish();
        assert_eq!(
            known(p.for_file("src/App/X.cs")),
            ["Orbit.Shared"].into_iter().collect()
        );
    }

    #[test]
    fn without_any_project_a_global_using_reaches_every_file() {
        let mut b = ProjectUsingsBuilder::default();
        b.add("Lib/GlobalUsings.cs", b"global using Orbit.Model;\n");
        let p = b.finish();
        assert_eq!(
            known(p.for_file("App/Caller.cs")),
            ["Orbit.Model"].into_iter().collect()
        );
    }
}
