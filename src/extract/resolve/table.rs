//! The repo-wide symbol table a usage site resolves against (U4), extended for
//! the shared resolver's rules (plan A, KTD5).
//!
//! Built in ONE linear pass over the graph's Symbol nodes and edges after every
//! file has been walked. Everything it needs is already a fact or an edge:
//! `SymbolName` gives the name, `SymbolKind` the role, that fact's own
//! [`Provenance`] the declaring file and line range (U1), `SymbolSignature` the
//! modifiers and parameters, `HasMember` the declaring type, and `Extends` /
//! `Implements` the ancestors. Nothing is re-parsed and nothing is buffered
//! between the two passes but the declaration-name positions the graph does not
//! hold — which is what keeps a 13,270-file repository inside a flat memory
//! budget.
//!
//! The name maps are keyed by the SIMPLE name, not by identity: a heuristic has
//! no type information, so a bare `Get` is all it ever gets to match on.
//! Several declarations under one name is therefore the normal case, not an
//! error — it is the ambiguity R23 asks the graph to admit, and the rules
//! exist to sift.

use crate::extract::treesitter::{lang_family, symbol_role, Role};
use crate::graph::{EdgeKind, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Where a declaration's NAME is written, by file and key (plugin contract
/// 0.2, `place.name`): `(line, column)`, both 1-based. The graph keeps lines
/// only, so the declaration pass hands these over beside it.
pub(crate) type NamePositions = HashMap<(String, NodeKey), (u32, u32)>;

/// One declaration and the source lines it spans — how a usage site finds the
/// symbol it sits inside.
struct Decl {
    key: NodeKey,
    start: u32,
    end: u32,
    /// Where its name is written, when the plugin said (contract 0.2).
    name_at: Option<(u32, u32)>,
}

/// The range of argument counts a callable accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Arity {
    pub min: u32,
    /// `None` for a `params` array: no upper bound.
    pub max: Option<u32>,
}

impl Arity {
    pub(crate) fn admits(self, n: u32) -> bool {
        n >= self.min && self.max.is_none_or(|max| n <= max)
    }
}

/// What the rules know about one symbol beyond its name.
#[derive(Debug, Default)]
pub(crate) struct SymbolInfo {
    pub role: Option<Role>,
    /// The `SymbolKind` value (`method`, `constructor`, `interface`, …).
    pub kind: String,
    /// The type that declares it (`HasMember`), for a member.
    pub declaring_type: Option<NodeKey>,
    /// The namespace its key names, or `None` where the key's module is not a
    /// namespace — a C# file with no namespace declaration is keyed by its
    /// path stem, which no using directive can name.
    pub namespace: Option<String>,
    /// For a callable whose parameter list the signature spelled readably.
    pub arity: Option<Arity>,
    pub is_static: bool,
    /// For an extension method: the simple name of its `this` parameter's
    /// type, generic arguments dropped — `None` inside when that type is one
    /// of the method's own type parameters, which any receiver fits.
    pub extends: Option<Option<String>>,
    /// The generic type parameters it declares (`T`, `TKey`).
    pub generic_params: Vec<String>,
    /// For a type: the simple names its base list spells, repository types and
    /// external ones alike (`Stream`, `IDisposable`).
    pub spelled_bases: Vec<String>,
}

/// The repo-wide symbol table (see the module docs).
#[derive(Default)]
pub(crate) struct SymbolTable {
    /// Simple name → every callable the repo declares under it. Overloads are
    /// distinct identities (KTD3) and land here as distinct candidates.
    callables: BTreeMap<String, Vec<NodeKey>>,
    /// Simple name → every type the repo declares under it.
    types: BTreeMap<String, Vec<NodeKey>>,
    /// File → its declarations, sorted by start line ascending and, for equal
    /// starts, by end line DESCENDING — so a backwards scan meets the innermost
    /// range first (see [`SymbolTable::owner`]).
    by_file: BTreeMap<String, Vec<Decl>>,
    info: HashMap<NodeKey, SymbolInfo>,
    /// Type → its direct repository bases (`Extends`, `Implements`).
    bases: HashMap<NodeKey, Vec<NodeKey>>,
}

/// Which declaration encloses a usage site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner<'t> {
    /// This declaration, and no other on the site's line could be meant.
    Mapped(&'t NodeKey),
    /// Several declarations share the site's lines and no column decided
    /// between them (KTD7). The site is unmapped: the relation hangs off the
    /// nearest declaration known to enclose all of them, or the file.
    Unmapped(Option<&'t NodeKey>),
    /// Module-level code: no declaration encloses it.
    None,
}

impl SymbolTable {
    pub(crate) fn build(graph: &Graph, names: &NamePositions) -> SymbolTable {
        let mut table = SymbolTable::default();
        for node in graph.nodes_sorted() {
            if node.kind != NodeKind::Symbol {
                continue;
            }
            let mut name: Option<&str> = None;
            let mut kind: Option<&str> = None;
            let mut prov: Option<&Provenance> = None;
            let mut signature: Option<&str> = None;
            for fact in &node.facts {
                match (fact.kind, &fact.value) {
                    (FactKind::SymbolName, FactValue::Text(v)) => {
                        name = Some(v.as_str());
                        prov = Some(&fact.provenance);
                    }
                    (FactKind::SymbolKind, FactValue::Text(v)) => kind = Some(v.as_str()),
                    (FactKind::SymbolSignature, FactValue::Text(v)) => signature = Some(v.as_str()),
                    // A tuple match over (kind, value), so a wildcard is
                    // unavoidable — the value axis is open on its own. Nothing
                    // declared reaches here anyway: this table is built from
                    // Symbol nodes the language plugins emitted, and the facts
                    // it looks for are core.
                    _ => {}
                }
            }
            let (Some(name), Some(kind), Some(prov)) = (name, kind, prov) else {
                continue;
            };
            let role = symbol_role(kind);
            match role {
                Some(Role::Callable) => table
                    .callables
                    .entry(name.to_string())
                    .or_default()
                    .push(node.key.clone()),
                Some(Role::Type) => table
                    .types
                    .entry(name.to_string())
                    .or_default()
                    .push(node.key.clone()),
                None => {}
            }
            // EVERY declaration can enclose a usage site, whatever its role: a call
            // in a field initializer belongs to the field, not to its type.
            if let (Some(start), Some(end)) = (prov.span.start(), prov.span.end()) {
                table
                    .by_file
                    .entry(prov.file.clone())
                    .or_default()
                    .push(Decl {
                        key: node.key.clone(),
                        start,
                        end,
                        name_at: names.get(&(prov.file.clone(), node.key.clone())).copied(),
                    });
            }
            let parts = signature
                .map(|s| parse_signature(s, name, role))
                .unwrap_or_default();
            let arity = parts
                .params
                .as_ref()
                .filter(|p| Some(p.len()) == key_param_count(node.key.as_str()))
                .map(|p| arity_of(p));
            let extends = parts.params.as_ref().and_then(|p| {
                let first = without_attributes(p.first()?);
                if !is_extension_receiver(first) {
                    return None;
                }
                let ty = simple_type(&param_type(first));
                Some((!parts.generic_params.contains(&ty)).then_some(ty))
            });
            table.info.insert(
                node.key.clone(),
                SymbolInfo {
                    role,
                    kind: kind.to_string(),
                    declaring_type: None,
                    namespace: namespace_of(node.key.as_str(), &prov.file),
                    arity,
                    is_static: parts.is_static,
                    extends,
                    generic_params: parts.generic_params,
                    spelled_bases: parts.bases,
                },
            );
        }
        for edge in graph.edges_sorted() {
            match edge.edge.kind {
                EdgeKind::HasMember => {
                    if let Some(info) = table.info.get_mut(edge.to) {
                        info.declaring_type = Some(edge.from.clone());
                    }
                }
                EdgeKind::Extends | EdgeKind::Implements => table
                    .bases
                    .entry(edge.from.clone())
                    .or_default()
                    .push(edge.to.clone()),
                _ => {}
            }
        }
        for decls in table.by_file.values_mut() {
            decls.sort_by(|a, b| {
                (a.start, std::cmp::Reverse(a.end), &a.key).cmp(&(
                    b.start,
                    std::cmp::Reverse(b.end),
                    &b.key,
                ))
            });
        }
        table
    }

    /// The declaration in `file` that encloses a site at `line` (and `column`,
    /// when the plugin reports one).
    ///
    /// Declaration ranges come from a syntax tree, so they nest and never cross.
    /// Among the declarations that START at or before `line`, the innermost
    /// container is therefore the LAST one, in start order, that still reaches
    /// `line` — a binary search plus a short backwards scan, never a scan of the
    /// file's whole symbol list per site.
    ///
    /// Lines alone cannot tell apart declarations that share the site's line
    /// (KTD7). Where they share all of their lines —
    /// `class C { void M() { X(); } void N() {} }` on one line — the column
    /// decides: the site belongs to the declaration whose name is written last
    /// before it, which is the right one for a member nested in its type and
    /// for members side by side. Without a column, or with no such name, the
    /// site is [`Owner::Unmapped`] rather than handed to whichever declaration
    /// sorts last. Where they only meet on it — one ends on the line another
    /// starts on — the column first rules out what cannot hold the site
    /// ([`left_behind`]), and the lines decide among the rest.
    pub(crate) fn owner(&self, file: &str, line: u32, column: Option<u32>) -> Owner<'_> {
        let Some(decls) = self.by_file.get(file) else {
            return Owner::None;
        };
        let upper = decls.partition_point(|d| d.start <= line);
        let gone = column.map_or_else(Vec::new, |c| left_behind(&decls[..upper], line, c));
        let live = |i: usize| !gone.contains(&i);
        let Some(inner) = (0..upper).rev().find(|&i| live(i) && decls[i].end >= line) else {
            return Owner::None;
        };
        let range = (decls[inner].start, decls[inner].end);
        // Equal ranges are adjacent in the sort order, ending at `inner`.
        let first_tied = decls[..inner]
            .iter()
            .rposition(|d| (d.start, d.end) != range)
            .map_or(0, |i| i + 1);
        let tied: Vec<&Decl> = (first_tied..=inner)
            .filter(|&i| live(i))
            .map(|i| &decls[i])
            .collect();
        if let [only] = tied.as_slice() {
            return Owner::Mapped(&only.key);
        }
        if let Some(column) = column {
            if let Some(d) = tied
                .iter()
                .filter(|d| d.name_at.is_some_and(|at| at <= (line, column)))
                .max_by_key(|d| d.name_at)
            {
                return Owner::Mapped(&d.key);
            }
        }
        // The nearest declaration that encloses the whole tied range.
        let outer = (0..first_tied)
            .rev()
            .find(|&i| live(i) && decls[i].end >= range.1)
            .map(|i| &decls[i].key);
        Owner::Unmapped(outer)
    }

    pub(crate) fn candidates(&self, role: Role, name: &str) -> Option<&[NodeKey]> {
        let map = match role {
            Role::Callable => &self.callables,
            Role::Type => &self.types,
        };
        map.get(name).map(Vec::as_slice)
    }

    /// The language family of the declarations `file` holds — the family a
    /// type spelled at one of its sites is looked for in.
    pub(crate) fn family_of_file(&self, file: &str) -> Option<&str> {
        self.by_file
            .get(file)?
            .iter()
            .find_map(|d| lang_family(d.key.as_str()))
    }

    pub(crate) fn info(&self, key: &NodeKey) -> Option<&SymbolInfo> {
        self.info.get(key)
    }

    /// The type a site's owner belongs to: the owner itself when it is a type,
    /// its declaring type when it is a member.
    pub(crate) fn own_type<'a>(&'a self, owner: &'a NodeKey) -> Option<&'a NodeKey> {
        let info = self.info.get(owner)?;
        if info.role == Some(Role::Type) {
            Some(owner)
        } else {
            info.declaring_type.as_ref()
        }
    }

    /// `types` and every repository type they extend or implement, however
    /// far up. A cycle — only broken code has one — ends the walk rather than
    /// looping.
    pub(crate) fn with_ancestors<'a>(
        &'a self,
        types: impl IntoIterator<Item = &'a NodeKey>,
    ) -> HashSet<&'a NodeKey> {
        let mut seen: HashSet<&NodeKey> = HashSet::new();
        let mut stack: Vec<&NodeKey> = types.into_iter().collect();
        while let Some(t) = stack.pop() {
            if seen.insert(t) {
                if let Some(bases) = self.bases.get(t) {
                    stack.extend(bases);
                }
            }
        }
        seen
    }

    /// The repository types of `family` a spelled type name can mean: by its
    /// last segment, and — when the spelling is qualified — only those whose
    /// full name ends with the spelled path.
    pub(crate) fn types_named(&self, family: &str, spelled: &str) -> Vec<&NodeKey> {
        let spelled = strip_generics(spelled);
        let spelled = spelled.strip_prefix("global::").unwrap_or(&spelled);
        let simple = spelled.rsplit('.').next().unwrap_or(spelled);
        let Some(keys) = self.types.get(simple) else {
            return Vec::new();
        };
        keys.iter()
            .filter(|k| lang_family(k.as_str()) == Some(family))
            .filter(|k| {
                !spelled.contains('.') || {
                    let full = self.full_name(k);
                    full == spelled || full.ends_with(&format!(".{spelled}"))
                }
            })
            .collect()
    }

    /// `Namespace.Outer.Inner` for a type key, the namespace left out where
    /// the key has none (see [`SymbolInfo::namespace`]).
    fn full_name(&self, key: &NodeKey) -> String {
        let path = key
            .as_str()
            .split_once('#')
            .map_or("", |(_, p)| p.split('~').next().unwrap_or(p));
        match self.info.get(key).and_then(|i| i.namespace.as_deref()) {
            Some(ns) if !ns.is_empty() => format!("{ns}.{path}"),
            _ => path.to_string(),
        }
    }
}

/// Of `decls` (a file's declarations starting at or before `line`, in the
/// table's order), the indices of those whose lines hold a site at `(line,
/// column)` but whose span cannot: the graph keeps no column of a span's
/// ends, so where one declaration ends on the line another starts on, the
/// declaration names written on it say which side of the boundary the site
/// is on.
///
/// - One that ENDS on the line has ended before the site when another, which
///   reaches further down, has its name written after this one's and at or
///   before the site: `void A() {} void B() { Call();` — `B` began after `A`
///   closed, and the call follows `B`'s name.
/// - One that STARTS on the line with its name after the site has not begun
///   when another, which started on an earlier line, ends on it: `X(); }
///   void F() {}` — the call is the other one's tail. With nothing ending
///   there, what precedes the name is its own: an attribute on the line
///   above `void G()` is `G`'s.
///
/// Empty — the lines decide alone, as without a column — when a declaration
/// holding the line has no name position. Scans back only to the first
/// declaration that holds the line strictly inside its range: everything
/// before it that reaches the line encloses it, and an enclosing
/// declaration's name comes before the names inside it, so it is never one
/// of these.
fn left_behind(decls: &[Decl], line: u32, column: u32) -> Vec<usize> {
    let site = (line, column);
    let mut held: Vec<usize> = Vec::new();
    for (i, d) in decls.iter().enumerate().rev() {
        if d.end < line {
            continue;
        }
        held.push(i);
        if d.start < line && d.end > line {
            break;
        }
    }
    if held.len() < 2 || held.iter().any(|&i| decls[i].name_at.is_none()) {
        return Vec::new();
    }
    let name = |i: usize| decls[i].name_at.expect("checked above");
    held.iter()
        .copied()
        .filter(|&x| {
            let dx = &decls[x];
            let ended = dx.end == line
                && held
                    .iter()
                    .any(|&y| decls[y].end > dx.end && name(y) > name(x) && name(y) <= site);
            let not_begun = dx.start == line
                && name(x) > site
                && held.iter().any(|&y| {
                    decls[y].end == line && decls[y].start < dx.start && name(y) < name(x)
                });
            ended || not_begun
        })
        .collect()
}

/// The namespace a `sym:` key names: its module part, unless that module is
/// the declaring file's path stem — how a namespace-less C# file is keyed,
/// which names no namespace a using directive could import.
pub(crate) fn namespace_of(key: &str, file: &str) -> Option<String> {
    let rest = key.strip_prefix("sym:")?;
    let (_, rest) = rest.split_once(':')?;
    let module = rest.split_once('#')?.0;
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    (module != stem).then(|| module.to_string())
}

/// `List<int>` → `List`, `Dictionary<string, List<int>>` → `Dictionary`.
pub(crate) fn strip_generics(spelled: &str) -> String {
    let mut out = String::with_capacity(spelled.len());
    let mut depth = 0usize;
    for c in spelled.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 && !c.is_whitespace() => out.push(c),
            _ => {}
        }
    }
    out
}

/// The words C# lets stand before a parameter's type.
const PARAM_MODIFIERS: &[&str] = &["this", "ref", "in", "out", "params", "scoped", "readonly"];

/// Whether a parameter is an extension method's receiver: `this` among the
/// modifiers before its type, in whatever order C# accepts them (`this`,
/// `ref this`, `this in`, `scoped ref this`).
fn is_extension_receiver(param: &str) -> bool {
    param
        .split_whitespace()
        .take_while(|w| PARAM_MODIFIERS.contains(w))
        .any(|w| w == "this")
}

/// The type a parameter declaration spells, its modifiers and its name left
/// out: `ref Dictionary<K, V> map` → `Dictionary<K, V>`.
fn param_type(param: &str) -> String {
    let mut depth = 0usize;
    let mut words: Vec<String> = vec![String::new()];
    for c in param.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if !words.last().is_some_and(String::is_empty) {
                words.push(String::new());
            }
        } else if let Some(w) = words.last_mut() {
            w.push(c);
        }
    }
    words
        .into_iter()
        .find(|w| !w.is_empty() && !PARAM_MODIFIERS.contains(&w.as_str()))
        .unwrap_or_default()
}

/// The simple name of a spelled type: generic arguments, array and nullable
/// marks and any qualification dropped (`System.Collections.Generic.IEnumerable<T>`
/// → `IEnumerable`).
pub(crate) fn simple_type(spelled: &str) -> String {
    let plain = strip_generics(spelled);
    let plain = plain.trim_end_matches(['[', ']', '?', ',']);
    plain.rsplit(['.', ':']).next().unwrap_or(plain).to_string()
}

/// The number of parameters the key's normalized list holds (`Run(int,string)`
/// → 2), or `None` for a key with no list.
fn key_param_count(key: &str) -> Option<usize> {
    let path = key.split_once('#')?.1;
    let open = path.find('(')?;
    let close = path.rfind(')')?;
    let list = path.get(open + 1..close)?;
    Some(split_top_level(list).len())
}

/// What a declaration header says that the rules read.
#[derive(Debug, Default, PartialEq)]
struct SignatureParts {
    is_static: bool,
    generic_params: Vec<String>,
    /// The parameters as written, trimmed — `None` when the header holds no
    /// list the parse could find.
    params: Option<Vec<String>>,
    bases: Vec<String>,
}

/// Read a declaration header (`SymbolSignature`) the way C# writes one:
/// modifiers, the name, its type parameters, then a parameter list for a
/// callable or a base list for a type. Attributes and string literals are
/// skipped, so `[Obsolete("Run(x)")]` is never taken for the list. A header
/// the parse cannot follow yields nothing, and a rule with nothing to read
/// says nothing (only C# sites carry context today, so only C# headers are
/// ever read).
fn parse_signature(sig: &str, name: &str, role: Option<Role>) -> SignatureParts {
    let mut out = SignatureParts::default();
    let bytes: Vec<char> = sig.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '@';
    // Find the name at depth 0, outside attributes and literals.
    let mut depth = 0i32;
    let mut in_str = false;
    let mut at: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if c == '\\' {
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '[' | '(' | '{' => depth += 1,
                ']' | ')' | '}' => depth -= 1,
                _ if depth == 0
                    && bytes[i..].starts_with(&name_chars)
                    && (i == 0 || !ident(bytes[i - 1]))
                    && bytes.get(i + name_chars.len()).is_none_or(|&n| !ident(n)) =>
                {
                    at = Some(i);
                    break;
                }
                _ => {}
            }
        }
        i += 1;
    }
    let Some(at) = at else {
        return out;
    };
    let head: String = bytes[..at].iter().collect();
    out.is_static = head.split_whitespace().any(|w| w == "static");
    let mut rest: &[char] = &bytes[at + name_chars.len()..];
    let skip_ws = |r: &mut &[char]| {
        while r.first().is_some_and(|c| c.is_whitespace()) {
            *r = &r[1..];
        }
    };
    skip_ws(&mut rest);
    if rest.first() == Some(&'<') {
        let (inside, after) = balanced(rest, '<', '>');
        out.generic_params = split_top_level(&inside)
            .into_iter()
            .filter_map(|p| p.split_whitespace().last().map(str::to_string))
            .collect();
        rest = after;
        skip_ws(&mut rest);
    }
    if rest.first() == Some(&'(') {
        let (inside, after) = balanced(rest, '(', ')');
        if role == Some(Role::Callable) {
            out.params = Some(split_top_level(&inside));
        }
        rest = after;
        skip_ws(&mut rest);
    }
    if role == Some(Role::Type) && rest.first() == Some(&':') {
        let list: String = rest[1..].iter().collect();
        let list = list.split(" where ").next().unwrap_or(&list);
        out.bases = split_top_level(list)
            .iter()
            .map(|b| simple_type(b.split('(').next().unwrap_or(b)))
            .filter(|b| !b.is_empty())
            .collect();
    }
    out
}

/// The text between an opening `open` at `chars[0]` and its matching `close`,
/// and what follows it.
fn balanced(chars: &[char], open: char, close: char) -> (String, &[char]) {
    let mut depth = 0i32;
    for (i, &c) in chars.iter().enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return (chars[1..i].iter().collect(), &chars[i + 1..]);
            }
        }
    }
    (chars.get(1..).unwrap_or(&[]).iter().collect(), &[])
}

/// Split on commas that are not inside brackets of any kind; trimmed, empty
/// pieces dropped (`""` is no parameter, not one).
fn split_top_level(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in list.chars() {
        match c {
            '(' | '[' | '<' | '{' => depth += 1,
            ')' | ']' | '>' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    out.push(current);
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The argument counts a parameter list accepts: a default value makes a
/// parameter optional, a `params` array lifts the upper bound.
fn arity_of(params: &[String]) -> Arity {
    let mut min = 0;
    let mut unbounded = false;
    for p in params {
        let p = without_attributes(p);
        if p.starts_with("params ") {
            unbounded = true;
        } else if !has_top_level(p, '=') {
            min += 1;
        }
    }
    let count = params.len() as u32;
    Arity {
        min,
        max: (!unbounded).then_some(count),
    }
}

/// A parameter without the attribute lists written before it
/// (`[NotNull] string s` → `string s`), which never hold the markers.
fn without_attributes(param: &str) -> &str {
    let mut rest = param.trim_start();
    while rest.starts_with('[') {
        let mut depth = 0i32;
        let end = rest.char_indices().find_map(|(i, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        });
        match end {
            Some(i) => rest = rest[i + 1..].trim_start(),
            None => break,
        }
    }
    rest
}

fn has_top_level(text: &str, wanted: char) -> bool {
    let mut depth = 0i32;
    text.chars().any(|c| {
        match c {
            '(' | '[' | '<' | '{' => depth += 1,
            ')' | ']' | '>' | '}' => depth -= 1,
            _ => {}
        }
        depth == 0 && c == wanted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(sig: &str, name: &str, role: Role) -> SignatureParts {
        parse_signature(sig, name, Some(role))
    }

    #[test]
    fn a_method_header_gives_its_modifiers_parameters_and_type_parameters() {
        let p = parse(
            "[Obsolete(\"Run(x, y)\")] public static T Run<T>(this IEnumerable<T> xs, int n = 3, params string[] rest)",
            "Run",
            Role::Callable,
        );
        assert!(p.is_static);
        assert_eq!(p.generic_params, vec!["T"]);
        let params = p.params.unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(arity_of(&params), Arity { min: 1, max: None });
    }

    #[test]
    fn a_type_header_gives_the_bases_it_spells() {
        let p = parse(
            "public class MyStream<T> : System.IO.Stream, IList<T> where T : class",
            "MyStream",
            Role::Type,
        );
        assert_eq!(p.generic_params, vec!["T"]);
        assert_eq!(p.bases, vec!["Stream", "IList"]);
    }

    #[test]
    fn an_empty_list_is_no_parameter() {
        let p = parse("public void Go()", "Go", Role::Callable);
        assert_eq!(p.params, Some(Vec::new()));
        assert_eq!(
            arity_of(&[]),
            Arity {
                min: 0,
                max: Some(0)
            }
        );
    }

    #[test]
    fn a_namespace_less_csharp_key_names_no_namespace() {
        assert_eq!(namespace_of("sym:csharp:src/A#T", "src/A.cs"), None);
        assert_eq!(
            namespace_of("sym:csharp:N.M#T.Run()", "src/A.cs"),
            Some("N.M".to_string())
        );
    }
    use crate::graph::{Edge, Fact, Tier};

    /// Declare `key` as a `kind` named `name` on `lines` of `file`, with
    /// `signature`, a member of `owner`.
    fn declare(
        g: &mut Graph,
        file: &str,
        key: &str,
        (kind, name): (&str, &str),
        lines: (u32, u32),
        signature: &str,
        owner: Option<&str>,
    ) {
        let key = NodeKey::new(key);
        g.ensure_node(key.clone(), NodeKind::Symbol).unwrap();
        let prov = Provenance::new(file, None, "csharp".into(), Tier::Measured)
            .with_range(lines.0, lines.1);
        for (k, v) in [
            (FactKind::SymbolName, name),
            (FactKind::SymbolKind, kind),
            (FactKind::SymbolSignature, signature),
        ] {
            g.add_fact(&key, Fact::new(k, FactValue::Text(v.into()), prov.clone()));
        }
        if let Some(owner) = owner {
            g.add_edge(
                &NodeKey::new(owner),
                &key,
                Edge::new(EdgeKind::HasMember, prov),
            );
        }
    }

    /// `this` marks an extension method wherever it stands among the first
    /// parameter's modifiers — `ref this`, `in this`, `scoped ref this` as
    /// well as `this ref` — and only there.
    #[test]
    fn this_among_the_first_parameters_modifiers_marks_an_extension() {
        let key = "sym:csharp:N#Ext.Touch(Point)";
        for (param, extends) in [
            ("this Point p", Some(Some("Point".to_string()))),
            ("ref this Point p", Some(Some("Point".to_string()))),
            ("in this Point p", Some(Some("Point".to_string()))),
            ("scoped ref this Point p", Some(Some("Point".to_string()))),
            ("this ref Point p", Some(Some("Point".to_string()))),
            (
                "[NotNull] ref readonly this Point p",
                Some(Some("Point".to_string())),
            ),
            ("ref Point p", None),
            ("Point @this", None),
        ] {
            let mut g = Graph::new();
            declare(
                &mut g,
                "src/Ext.cs",
                key,
                ("method", "Touch"),
                (1, 1),
                &format!("public static void Touch({param})"),
                None,
            );
            let table = SymbolTable::build(&g, &HashMap::new());
            let info = table.info(&NodeKey::new(key)).expect("declared");
            assert_eq!(info.extends, extends, "{param}");
        }
    }

    /// A site belongs to the declaration whose span holds its position, line
    /// AND column, where the plugin says where names are written — not to the
    /// one that merely sorts innermost by lines:
    ///
    /// ```text
    /// 1  class C { void A() {} void B() { Call();
    /// 2  }
    /// 5  void E() {
    /// 6  X(); } void F() { Y(); }
    /// 8  [Attr(typeof(Z))]
    /// 9  void G() {}
    /// ```
    #[test]
    fn the_column_picks_the_declaration_whose_span_holds_the_site() {
        let file = "src/A.cs";
        let mut g = Graph::new();
        let mut names: NamePositions = HashMap::new();
        let c = "sym:csharp:N#C";
        let mut decl =
            |key: &str, name: &str, lines: (u32, u32), at: (u32, u32), owner: Option<&str>| {
                declare(
                    &mut g,
                    file,
                    key,
                    (if owner.is_some() { "method" } else { "class" }, name),
                    lines,
                    &format!("void {name}()"),
                    owner,
                );
                names.insert((file.to_string(), NodeKey::new(key)), at);
            };
        decl(c, "C", (1, 10), (1, 7), None);
        decl("sym:csharp:N#C.A()", "A", (1, 1), (1, 16), Some(c));
        decl("sym:csharp:N#C.B()", "B", (1, 3), (1, 28), Some(c));
        decl("sym:csharp:N#C.E()", "E", (5, 6), (5, 6), Some(c));
        decl("sym:csharp:N#C.F()", "F", (6, 6), (6, 13), Some(c));
        decl("sym:csharp:N#C.G()", "G", (8, 9), (9, 6), Some(c));
        let table = SymbolTable::build(&g, &names);
        let at = |line, column| match table.owner(file, line, Some(column)) {
            Owner::Mapped(key) => key.as_str().to_string(),
            other => panic!("({line}, {column}) unmapped: {other:?}"),
        };
        // Line 1: inside `A`, then inside `B` after `A` has closed.
        assert_eq!(at(1, 20), "sym:csharp:N#C.A()");
        assert_eq!(at(1, 34), "sym:csharp:N#C.B()");
        // Line 6: the tail of `E`, then the body of `F`.
        assert_eq!(at(6, 1), "sym:csharp:N#C.E()");
        assert_eq!(at(6, 19), "sym:csharp:N#C.F()");
        // An attribute on the line above `G`'s name is still `G`'s.
        assert_eq!(at(8, 15), "sym:csharp:N#C.G()");
    }
}
