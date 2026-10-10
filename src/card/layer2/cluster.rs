//! Core functionalities of a type: entry points grown into clusters (KTD12),
//! keyed by their representative (KTD11) and named from code (R16).
//!
//! 1. **Seeds** are the entry points ([`super::entry`]), split by the
//!    branches of their dispatcher.
//! 2. Seeds whose call or command leads to the same member — of this type, or
//!    of another one reached through an inherited member — are one
//!    functionality (F4 and the menu item “check errors” both execute
//!    `EditorViewModel.CheckCommand`). The inherited member gone through
//!    (`viewModel`) is not what is used, so it joins nothing.
//! 3. A member joins a cluster when the cluster calls it or binds it as a
//!    command delegate, and when it reads or writes a field or a property of
//!    the type the cluster uses (shared state).
//! 4. A **hub** joins nothing: a member of the type or of one of its bases
//!    that more than a quarter of the type's members — and at least four —
//!    use, or that the clusters of three or more seeds reach. Seeds that call
//!    each other count as one here, or a command and the method it executes
//!    would make the method's helpers hubs.
//!
//! The representative of a cluster is its entry point of the first kind in
//! [`super::entry::EntryKind::rank`], and among those of one kind the one with the least key
//! by code points — never the one most referenced, so a change of what a
//! cluster holds cannot move its key. The denominator of the quarter is every
//! member of the type the first layer gives it (`HasMember`), nested types
//! left out.

use super::entry::{EntryPoint, Seed, Seeds};
use super::resolve::Index;
use crate::card::keywords::split_identifier;
use crate::graph::NodeKey;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// How many words a functionality's name carries (R16).
pub const NAME_WORDS: usize = 3;

/// A member used by more than this share of the type's members is a hub.
const HUB_SHARE: (usize, usize) = (1, 4);

/// …and by at least this many of them.
const HUB_MEMBERS: usize = 4;

/// The clusters of at least this many seeds reaching a member make it a hub.
const HUB_SEEDS: usize = 3;

/// The name of a functionality (R16): its representative and the words its
/// members' names use most.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionalityName {
    /// The representative's name, with its discriminator (`CheckCommand`,
    /// `Editor_PreviewKeyDown#Key.F4`).
    pub representative: String,
    /// Most frequent first, a tie broken by the word.
    pub words: Vec<String>,
}

/// One core functionality of a type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Functionality {
    /// The representative's key (KTD11).
    pub key: String,
    pub name: FunctionalityName,
    /// The representative first, then by rank and key.
    pub entry_points: Vec<EntryPoint>,
    /// The type's own members in it, by key.
    pub members: Vec<NodeKey>,
    /// Members of other types — the base's included — it leads to, by key.
    pub targets: Vec<NodeKey>,
}

impl Functionality {
    pub fn representative(&self) -> &EntryPoint {
        &self.entry_points[0]
    }
}

/// A union-find over seed indices; the root of a set is its least index.
struct Sets(Vec<usize>);

impl Sets {
    fn new(n: usize) -> Sets {
        Sets((0..n).collect())
    }

    fn find(&mut self, i: usize) -> usize {
        let mut root = i;
        while self.0[root] != root {
            root = self.0[root];
        }
        let mut at = i;
        while self.0[at] != root {
            let next = self.0[at];
            self.0[at] = root;
            at = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            let (low, high) = (a.min(b), a.max(b));
            self.0[high] = low;
        }
    }
}

/// What the clustering reads of a type.
struct Ctx<'a> {
    ix: &'a Index<'a>,
    ty: &'a NodeKey,
    member_uses: &'a BTreeMap<NodeKey, BTreeSet<NodeKey>>,
    ancestors: Vec<&'a NodeKey>,
}

impl Ctx<'_> {
    fn owner(&self, x: &NodeKey) -> Option<&NodeKey> {
        self.ix.symbol(x).and_then(|s| s.owner.as_ref())
    }

    fn own(&self, x: &NodeKey) -> bool {
        self.owner(x) == Some(self.ty)
    }

    /// A member of the type or of one of its bases: what may be a hub.
    fn local(&self, x: &NodeKey) -> bool {
        self.owner(x)
            .is_some_and(|o| o == self.ty || self.ancestors.contains(&o))
    }

    /// Everything reachable from `start` through the type's own members,
    /// hubs left out (but `keep`, a seed's own member, always in).
    fn closure(
        &self,
        start: impl IntoIterator<Item = NodeKey>,
        keep: &BTreeSet<NodeKey>,
        hubs: &BTreeSet<NodeKey>,
    ) -> BTreeSet<NodeKey> {
        let mut seen: BTreeSet<NodeKey> = BTreeSet::new();
        let mut queue: Vec<NodeKey> = start.into_iter().collect();
        while let Some(x) = queue.pop() {
            if (hubs.contains(&x) && !keep.contains(&x)) || !seen.insert(x.clone()) {
                continue;
            }
            if self.own(&x) {
                if let Some(next) = self.member_uses.get(&x) {
                    queue.extend(next.iter().cloned());
                }
            }
        }
        seen
    }
}

/// Where a seed starts: what it uses directly, and the member it is.
fn start(seed: &Seed) -> (BTreeSet<NodeKey>, BTreeSet<NodeKey>) {
    let keep: BTreeSet<NodeKey> = seed.identity.iter().cloned().collect();
    let mut from = seed.uses.clone();
    from.extend(keep.iter().cloned());
    (from, keep)
}

/// The hubs of a type (step 4 of the module docs).
fn hubs(
    ctx: &Ctx<'_>,
    seeds: &[Seed],
    full_uses: &BTreeMap<NodeKey, BTreeSet<NodeKey>>,
) -> BTreeSet<NodeKey> {
    let total = full_uses.len();
    let mut users: BTreeMap<&NodeKey, usize> = BTreeMap::new();
    for used in full_uses.values() {
        for x in used {
            *users.entry(x).or_default() += 1;
        }
    }
    let mut hubs: BTreeSet<NodeKey> = users
        .into_iter()
        .filter(|(x, n)| ctx.local(x) && *n >= HUB_MEMBERS && n * HUB_SHARE.1 > total * HUB_SHARE.0)
        .map(|(x, _)| x.clone())
        .collect();

    // Seeds that call each other, or lead to the same member of another
    // type, count as one.
    let mut groups = Sets::new(seeds.len());
    let mut reaching: BTreeMap<&NodeKey, Vec<usize>> = BTreeMap::new();
    for (i, seed) in seeds.iter().enumerate() {
        for x in &seed.uses {
            if !ctx.local(x) {
                reaching.entry(x).or_default().push(i);
            }
        }
    }
    for (i, seed) in seeds.iter().enumerate() {
        if let Some(id) = seed.identity.as_ref().filter(|id| !hubs.contains(*id)) {
            for (j, other) in seeds.iter().enumerate() {
                if other.uses.contains(id) {
                    groups.union(i, j);
                }
            }
        }
    }
    for list in reaching.values() {
        for pair in list.windows(2) {
            groups.union(pair[0], pair[1]);
        }
    }
    let mut members_of_group: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..seeds.len() {
        let root = groups.find(i);
        members_of_group.entry(root).or_default().push(i);
    }

    loop {
        let mut reached: Vec<(BTreeSet<NodeKey>, BTreeSet<NodeKey>)> = Vec::new();
        for list in members_of_group.values() {
            let mut from = BTreeSet::new();
            let mut keep = BTreeSet::new();
            for &i in list {
                let (f, k) = start(&seeds[i]);
                from.extend(f);
                keep.extend(k);
            }
            let closure = ctx.closure(from.iter().cloned(), &keep, &hubs);
            reached.push((from, closure));
        }
        let mut count: BTreeMap<&NodeKey, usize> = BTreeMap::new();
        for (_, closure) in &reached {
            for x in closure {
                if ctx.local(x) {
                    *count.entry(x).or_default() += 1;
                }
            }
        }
        let candidates: BTreeSet<NodeKey> = count
            .into_iter()
            .filter(|(_, n)| *n >= HUB_SEEDS)
            .map(|(x, _)| x.clone())
            .collect();
        if candidates.is_empty() {
            break;
        }
        // The nearest candidates first: one a seed uses directly, or a
        // member that is no candidate uses. A helper only a candidate calls
        // is cut off with it.
        let nearest: BTreeSet<NodeKey> = candidates
            .iter()
            .filter(|x| {
                reached.iter().any(|(from, closure)| {
                    closure.contains(*x)
                        && (from.contains(*x)
                            || closure.iter().any(|y| {
                                !candidates.contains(y)
                                    && ctx.own(y)
                                    && ctx.member_uses.get(y).is_some_and(|u| u.contains(*x))
                            }))
                })
            })
            .cloned()
            .collect();
        hubs.extend(if nearest.is_empty() {
            candidates
        } else {
            nearest
        });
    }
    hubs
}

/// The words of a functionality's name (R16): the parts of its members'
/// names of three letters or more, most frequent first.
fn words(ix: &Index<'_>, nodes: &BTreeSet<&NodeKey>) -> Vec<String> {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for x in nodes {
        for part in split_identifier(ix.name_of(x)) {
            if part.chars().count() >= 3 && part.chars().any(char::is_alphabetic) {
                *count.entry(part).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<(String, usize)> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(NAME_WORDS)
        .map(|(w, _)| w)
        .collect()
}

/// The functionalities of `ty` from its seeds, and its hubs.
pub(crate) fn cluster(
    ix: &Index<'_>,
    ty: &NodeKey,
    seeds: Seeds,
) -> (Vec<Functionality>, Vec<NodeKey>) {
    let Seeds {
        seeds,
        member_uses,
        full_uses,
        ..
    } = seeds;
    let ctx = Ctx {
        ix,
        ty,
        member_uses: &member_uses,
        ancestors: ix.ancestors(ty),
    };
    let hubs = hubs(&ctx, &seeds, &full_uses);

    // Steps 2 and 3: seeds whose clusters share a member are one.
    let mut sets = Sets::new(seeds.len());
    let mut owner_of: BTreeMap<NodeKey, usize> = BTreeMap::new();
    let claim = |sets: &mut Sets, owner_of: &mut BTreeMap<NodeKey, usize>, x: NodeKey, i: usize| {
        match owner_of.get(&x) {
            Some(&j) => sets.union(i, j),
            None => {
                owner_of.insert(x, i);
            }
        }
    };
    for (i, seed) in seeds.iter().enumerate() {
        let (from, keep) = start(seed);
        for x in ctx.closure(from, &keep, &hubs) {
            claim(&mut sets, &mut owner_of, x, i);
        }
    }
    // Shared state: a member no cluster reaches joins the cluster whose
    // field or property it reads or writes, and brings what it reaches.
    loop {
        let mut changed = false;
        for (m, used) in &member_uses {
            if owner_of.contains_key(m) || hubs.contains(m) {
                continue;
            }
            let state: Vec<usize> = used
                .iter()
                .filter(|x| ctx.own(x) && ix.symbol(x).is_some_and(|s| s.is_value()))
                .filter_map(|x| owner_of.get(x).copied())
                .collect();
            let Some(&first) = state.first() else {
                continue;
            };
            for &other in &state[1..] {
                sets.union(first, other);
            }
            let reach = ctx.closure(std::iter::once(m.clone()), &BTreeSet::new(), &hubs);
            for x in reach {
                claim(&mut sets, &mut owner_of, x, first);
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }

    let mut entries: BTreeMap<usize, Vec<&Seed>> = BTreeMap::new();
    for (i, seed) in seeds.iter().enumerate() {
        entries.entry(sets.find(i)).or_default().push(seed);
    }
    let mut nodes: BTreeMap<usize, BTreeSet<&NodeKey>> = BTreeMap::new();
    for (x, &i) in &owner_of {
        nodes.entry(sets.find(i)).or_default().insert(x);
    }
    let mut out: Vec<Functionality> = entries
        .into_iter()
        .map(|(root, mut list)| {
            list.sort_by(|a, b| {
                (a.entry.kind.rank(), &a.entry.key).cmp(&(b.entry.kind.rank(), &b.entry.key))
            });
            let held = nodes.remove(&root).unwrap_or_default();
            let members: Vec<NodeKey> = held
                .iter()
                .filter(|x| ctx.own(x))
                .map(|x| (*x).clone())
                .collect();
            let targets: Vec<NodeKey> = held
                .iter()
                .filter(|x| !ctx.own(x))
                .map(|x| (*x).clone())
                .collect();
            let first = &list[0].entry;
            let mut representative = ix.name_of(&first.member).to_string();
            if let Some(d) = &first.discriminator {
                representative = format!("{representative}#{d}");
            }
            Functionality {
                key: first.key.clone(),
                name: FunctionalityName {
                    representative,
                    words: words(ix, &held),
                },
                entry_points: list.into_iter().map(|s| s.entry.clone()).collect(),
                members,
                targets,
            }
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    (out, hubs.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_keeps_its_least_index_as_root() {
        let mut sets = Sets::new(5);
        sets.union(4, 2);
        sets.union(2, 3);
        sets.union(1, 0);
        assert_eq!(sets.find(4), 2);
        assert_eq!(sets.find(3), 2);
        assert_eq!(sets.find(1), 0);
        sets.union(3, 1);
        assert_eq!(sets.find(4), 0);
    }
}
