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
//!    would make the method's helpers hubs. A member of another type that
//!    three or more of the type's public methods use directly is a hub too —
//!    a collaborator they share, not one thing they do — and stays among the
//!    targets of each; entry points of the user interface leading to one
//!    member of another type stay one functionality by what they run. A seed
//!    of the type that three or more other seeds call directly is a hub as
//!    well — a service they share: it stays a functionality of its own and
//!    its callers do not join through it, while one or two callers still
//!    join the seed they call, as a command joins the method it executes.
//! 5. A **driving method** ([`Seeds::dominant`]) is a functionality of its
//!    own. Other seeds stop at it as at a hub. A seed that leads into it — a
//!    command executing it, a public method calling it — joins it. A member
//!    shared by it and another seed merges neither, and it takes only what no
//!    other seed reached: a loop that calls `Dispose` on its way out leaves
//!    `Dispose` the functionality it was.
//! 6. A type with more than [`GROUPS_FROM`] functionalities **groups** them
//!    ([`Group`]): each joins the group of the field or property of the type
//!    it shares with others — a hub — that the fewest of the type's
//!    functionalities use, state more than half of them use being the type's
//!    backbone; one that shares none joins the group of the first word of its
//!    name. A group is no functionality: the functionalities, their keys and
//!    all that hangs on them stay.
//!
//! The representative of a cluster is its entry point of the first kind in
//! [`super::entry::EntryKind::rank`], and among those of one kind the one with the least key
//! by code points — never the one most referenced, so a change of what a
//! cluster holds cannot move its key. The denominator of the quarter is every
//! member of the type the first layer gives it (`HasMember`), nested types
//! left out.

use super::entry::{EntryKind, EntryPoint, Seed, Seeds};
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
    fn own(&self, x: &NodeKey) -> bool {
        self.ix.owner_of(x) == Some(self.ty)
    }

    /// A member of the type or of one of its bases: what may be a hub.
    fn local(&self, x: &NodeKey) -> bool {
        self.ix.declared_along(x, self.ty, &self.ancestors)
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

/// The members of other types that [`HUB_SEEDS`] or more of the type's public
/// methods use directly (step 4 of the module docs): a collaborator they
/// share, not one thing they do. Entry points of the user interface are left
/// out — the key and the menu item that run one command are one
/// functionality by what they run.
fn shared_collaborators(ctx: &Ctx<'_>, seeds: &[Seed]) -> Vec<NodeKey> {
    let mut callers: BTreeMap<&NodeKey, usize> = BTreeMap::new();
    for seed in seeds
        .iter()
        .filter(|s| matches!(s.entry.kind, EntryKind::PublicMethod | EntryKind::Driving))
    {
        for x in seed.uses.iter().filter(|x| !ctx.local(x)) {
            *callers.entry(x).or_default() += 1;
        }
    }
    callers
        .into_iter()
        .filter(|(_, n)| *n >= HUB_SEEDS)
        .map(|(x, _)| x.clone())
        .collect()
}

/// The members of the type that are a seed and that [`HUB_SEEDS`] or more
/// other seeds call directly (step 4 of the module docs): a service they
/// share. Each stays a functionality of its own, and its callers do not join
/// through it; one or two callers still join the seed they call, as a command
/// joins the method it executes.
fn shared_services(seeds: &[Seed]) -> Vec<NodeKey> {
    let identities: BTreeSet<&NodeKey> = seeds.iter().filter_map(|s| s.identity.as_ref()).collect();
    let mut callers: BTreeMap<&NodeKey, usize> = BTreeMap::new();
    for seed in seeds {
        for x in seed.uses.iter().filter(|x| identities.contains(x)) {
            if seed.identity.as_ref() != Some(x) {
                *callers.entry(x).or_default() += 1;
            }
        }
    }
    callers
        .into_iter()
        .filter(|(_, n)| *n >= HUB_SEEDS)
        .map(|(x, _)| x.clone())
        .collect()
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
    hubs.extend(shared_collaborators(ctx, seeds));
    hubs.extend(shared_services(seeds));

    // Seeds that call each other, or lead to the same member of another
    // type, count as one.
    let mut groups = Sets::new(seeds.len());
    let mut reaching: BTreeMap<&NodeKey, Vec<usize>> = BTreeMap::new();
    for (i, seed) in seeds.iter().enumerate() {
        for x in &seed.uses {
            if !ctx.local(x) && !hubs.contains(x) {
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

/// A type with more functionalities than this has groups (step 6 of the
/// module docs). A threshold, not a cap: a type has as many groups as its
/// functionalities fall into.
pub const GROUPS_FROM: usize = 15;

/// State used by more than this share of a type's functionalities is the
/// type's own backbone and groups nothing.
const BACKBONE_SHARE: (usize, usize) = (1, 2);

/// What the functionalities of a group have in common.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum GroupBasis {
    /// A field or a property of the type, a hub, that each of them uses and
    /// that the fewest of the type's functionalities use.
    State(NodeKey),
    /// The first word of their representatives' names, lower-cased, for
    /// those that share no state.
    Name(String),
}

/// Functionalities of a large type that share state or a name (R2, R13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    /// The key of its first functionality by the representative's rule:
    /// the first kind in [`super::entry::EntryKind::rank`], then the least
    /// key — so a functionality added to it never renames it.
    pub key: String,
    pub basis: GroupBasis,
    /// The keys of its functionalities, by key.
    pub functionalities: Vec<String>,
}

/// The groups of a type's functionalities, by key; none for a type with
/// [`GROUPS_FROM`] functionalities or fewer. Each functionality joins one
/// group by what it alone uses or is named, so a functionality added to one
/// group moves no other.
fn group(functionalities: &[(Functionality, BTreeSet<NodeKey>)]) -> Vec<Group> {
    let n = functionalities.len();
    if n <= GROUPS_FROM {
        return Vec::new();
    }
    let mut users: BTreeMap<&NodeKey, usize> = BTreeMap::new();
    for (_, state) in functionalities {
        for x in state {
            *users.entry(x).or_default() += 1;
        }
    }
    let mut by_basis: BTreeMap<GroupBasis, Vec<&Functionality>> = BTreeMap::new();
    for (f, state) in functionalities {
        let basis = state
            .iter()
            .filter(|x| users[x] * BACKBONE_SHARE.1 <= n * BACKBONE_SHARE.0)
            .min_by_key(|x| (users[x], *x))
            .map(|x| GroupBasis::State(x.clone()))
            .unwrap_or_else(|| {
                let name = f.name.representative.split('#').next().unwrap_or_default();
                GroupBasis::Name(
                    split_identifier(name)
                        .into_iter()
                        .next()
                        .unwrap_or_default(),
                )
            });
        by_basis.entry(basis).or_default().push(f);
    }
    let mut groups: Vec<Group> = by_basis
        .into_iter()
        .map(|(basis, list)| {
            let first = list
                .iter()
                .min_by(|a, b| {
                    (a.representative().kind.rank(), &a.key)
                        .cmp(&(b.representative().kind.rank(), &b.key))
                })
                .expect("a group has a functionality");
            Group {
                key: first.key.clone(),
                basis,
                functionalities: list.iter().map(|f| f.key.clone()).collect(),
            }
        })
        .collect();
    groups.sort_by(|a, b| a.key.cmp(&b.key));
    groups
}

/// The functionalities of `ty` from its seeds, its hubs and the groups of its
/// functionalities.
pub(crate) fn cluster(
    ix: &Index<'_>,
    ty: &NodeKey,
    seeds: Seeds,
) -> (Vec<Functionality>, Vec<NodeKey>, Vec<Group>) {
    let Seeds {
        seeds,
        member_uses,
        full_uses,
        dominant,
        ..
    } = seeds;
    let ctx = Ctx {
        ix,
        ty,
        member_uses: &member_uses,
        ancestors: ix.ancestors(ty),
    };
    let hubs = hubs(&ctx, &seeds, &full_uses);
    let driving: Vec<bool> = seeds
        .iter()
        .map(|s| s.identity.as_ref().is_some_and(|id| dominant.contains(id)))
        .collect();
    let bounds: BTreeSet<NodeKey> = hubs.iter().chain(&dominant).cloned().collect();

    // Steps 2 and 3: seeds whose clusters share a member are one — unless
    // one of them is a driving method's.
    let mut sets = Sets::new(seeds.len());
    let mut owner_of: BTreeMap<NodeKey, usize> = BTreeMap::new();
    let share = |sets: &mut Sets, i: usize, j: usize| {
        if !driving[i] && !driving[j] {
            sets.union(i, j);
        }
    };
    let claim = |sets: &mut Sets, owner_of: &mut BTreeMap<NodeKey, usize>, x: NodeKey, i: usize| {
        match owner_of.get(&x) {
            Some(&j) => share(sets, i, j),
            None => {
                owner_of.insert(x, i);
            }
        }
    };
    // A seed that leads into a driving method — a command executing it, a
    // public method calling it — is a way into the driving method's
    // functionality and joins it; what the driving method itself calls
    // never joins it that way.
    let driver_of: BTreeMap<&NodeKey, usize> = seeds
        .iter()
        .enumerate()
        .filter(|(i, _)| driving[*i])
        .filter_map(|(i, s)| s.identity.as_ref().map(|id| (id, i)))
        .collect();
    // The other seeds first, so a driving method takes only what is left.
    let order = (0..seeds.len())
        .filter(|&i| !driving[i])
        .chain((0..seeds.len()).filter(|&i| driving[i]));
    for i in order {
        let (from, keep) = start(&seeds[i]);
        let reached = ctx.closure(from.iter().cloned(), &keep, &bounds);
        if !driver_of.is_empty() {
            let led_into = from.iter().chain(
                reached
                    .iter()
                    .filter(|x| ctx.own(x))
                    .filter_map(|x| member_uses.get(x))
                    .flatten(),
            );
            for d in led_into {
                if let Some(&j) = driver_of.get(d) {
                    sets.union(i, j);
                }
            }
        }
        for x in reached {
            claim(&mut sets, &mut owner_of, x, i);
        }
    }
    // Shared state: a member no cluster reaches joins the cluster whose
    // field or property it reads or writes, and brings what it reaches —
    // preferably a cluster that is no driving method's.
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
            let Some(first) = state
                .iter()
                .copied()
                .find(|&i| !driving[i])
                .or_else(|| state.first().copied())
            else {
                continue;
            };
            for &other in &state {
                share(&mut sets, first, other);
            }
            let reach = ctx.closure(std::iter::once(m.clone()), &BTreeSet::new(), &bounds);
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
    // The hubs a functionality's seeds and members use directly: the shared
    // state among them groups it, and a shared collaborator is still a member
    // of another type it leads to.
    let hubs_used = |list: &[&Seed], held: &BTreeSet<&NodeKey>| -> BTreeSet<NodeKey> {
        list.iter()
            .map(|s| &s.uses)
            .chain(held.iter().filter_map(|x| member_uses.get(*x)))
            .flatten()
            .filter(|x| hubs.contains(*x))
            .cloned()
            .collect()
    };
    let mut out: Vec<(Functionality, BTreeSet<NodeKey>)> = entries
        .into_iter()
        .map(|(root, mut list)| {
            list.sort_by(|a, b| {
                (a.entry.kind.rank(), &a.entry.key).cmp(&(b.entry.kind.rank(), &b.entry.key))
            });
            let held = nodes.remove(&root).unwrap_or_default();
            let (state, collaborators): (BTreeSet<NodeKey>, BTreeSet<NodeKey>) =
                hubs_used(&list, &held)
                    .into_iter()
                    .filter(|x| !ctx.local(x) || ix.symbol(x).is_some_and(|s| s.is_value()))
                    .partition(|x| ctx.local(x));
            let members: Vec<NodeKey> = held
                .iter()
                .filter(|x| ctx.own(x))
                .map(|x| (*x).clone())
                .collect();
            let targets: Vec<NodeKey> = held
                .iter()
                .filter(|x| !ctx.own(x))
                .map(|x| (*x).clone())
                .chain(collaborators)
                .collect::<BTreeSet<NodeKey>>()
                .into_iter()
                .collect();
            let first = &list[0].entry;
            let mut representative = ix.name_of(&first.member).to_string();
            if let Some(d) = &first.discriminator {
                representative = format!("{representative}#{d}");
            }
            let f = Functionality {
                key: first.key.clone(),
                name: FunctionalityName {
                    representative,
                    words: words(ix, &held),
                },
                entry_points: list.into_iter().map(|s| s.entry.clone()).collect(),
                members,
                targets,
            };
            (f, state)
        })
        .collect();
    out.sort_by(|a, b| a.0.key.cmp(&b.0.key));
    let groups = group(&out);
    let out: Vec<Functionality> = out.into_iter().map(|(f, _)| f).collect();
    (out, hubs.into_iter().collect(), groups)
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
