//! The chunker shared by the diagram and the prose module tree (U1, R2).
//!
//! Both halves of the module-structure view need the same last-resort answer to
//! "this is still over the threshold and there is no structural seam left to cut
//! along": walk the units in order and start a new chunk whenever the next one
//! would push the current chunk over. Keeping that single greedy pass here —
//! rather than copying it into the prose path — is what stops the two
//! projections from drifting apart (R2).
//!
//! What is deliberately *not* shared is the diagram's seam logic (`split` /
//! `split_by_prefix` in [`super::mermaid`]). The prose seam is keyed on a
//! module's **parent**, so every sibling carries the same key and the recursive
//! seam can never subdivide a prose group — the prose path always lands straight
//! in [`fixed_partition`], and generalising the seam too would pay for a
//! mechanism prose never reaches (KTD1).

/// An incremental weight accumulator over the units of one chunk.
///
/// Fold-style (`peek` / `add` / `reset`) rather than one `weigh(&[unit])` over
/// the whole candidate slice, because the diagram's weight is the number of
/// *distinct* nodes its edges touch: re-weighing a growing slice at every step
/// would be quadratic, and it would have to rebuild the `seen` set the greedy
/// pass already carries — exactly the kind of re-derivation that lets a chunk
/// boundary drift away from the one the diagram emits today.
pub trait ChunkWeight<T> {
    /// What the chunk would weigh if `unit` were appended. Must not mutate.
    fn peek(&self, unit: &T) -> usize;

    /// Append `unit` to the current chunk's accumulated weight.
    fn add(&mut self, unit: &T);

    /// Begin a fresh, empty chunk.
    fn reset(&mut self);
}

/// Greedily pack indivisible `units` into chunks, opening a new chunk whenever
/// the next unit would push the accumulated weight past `threshold`.
///
/// Units are indivisible by contract: a unit that alone outweighs `threshold`
/// becomes its own **over-budget** chunk rather than being cut apart. So the
/// guarantee is "every chunk is within `threshold`, *or* is a single indivisible
/// unit". The diagram's units are two-node edges and stay within any sane
/// threshold; a prose subtree can genuinely exceed it, and there keeping the
/// subtree whole outranks the size limit (KTD4).
///
/// Input order is preserved and nothing is reordered, so the output is
/// deterministic for deterministic input (R7).
/// Takes an `IntoIterator` rather than a `Vec` so a caller holding only a slice
/// can stream it in (`iter().copied()`) instead of materialising a copy first.
pub fn fixed_partition<T, W: ChunkWeight<T>>(
    units: impl IntoIterator<Item = T>,
    threshold: usize,
    weight: &mut W,
) -> Vec<Vec<T>> {
    let mut out: Vec<Vec<T>> = Vec::new();
    let mut cur: Vec<T> = Vec::new();
    for unit in units {
        if !cur.is_empty() && weight.peek(&unit) > threshold {
            out.push(std::mem::take(&mut cur));
            weight.reset();
        }
        weight.add(&unit);
        cur.push(unit);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The simplest possible weight: a unit contributes its own number. Enough
    /// to exercise the packing contract without the diagram's distinct-node
    /// bookkeeping.
    struct Additive(usize);

    impl ChunkWeight<usize> for Additive {
        fn peek(&self, unit: &usize) -> usize {
            self.0 + unit
        }
        fn add(&mut self, unit: &usize) {
            self.0 += unit;
        }
        fn reset(&mut self) {
            self.0 = 0;
        }
    }

    fn pack(units: &[usize], threshold: usize) -> Vec<Vec<usize>> {
        fixed_partition(units.to_vec(), threshold, &mut Additive(0))
    }

    /// Units fill a chunk up to and including the threshold, then spill into the
    /// next one; order is never disturbed.
    #[test]
    fn packs_up_to_the_threshold_and_preserves_order() {
        // Exactly-at-threshold must fit — the boundary is `>`, not `>=`.
        assert_eq!(
            pack(&[10, 10, 10, 10, 10], 30),
            vec![vec![10, 10, 10], vec![10, 10]]
        );
        // Flattening the chunks reproduces the input, in order.
        let flat: Vec<usize> = pack(&[7, 9, 8, 6, 11], 20).into_iter().flatten().collect();
        assert_eq!(flat, vec![7, 9, 8, 6, 11]);
    }

    /// KTD4: an indivisible unit larger than the threshold is emitted whole, as
    /// its own over-budget chunk — never split, and never merged with a
    /// neighbour to disguise the overflow.
    #[test]
    fn an_oversized_unit_becomes_its_own_over_budget_chunk() {
        assert_eq!(pack(&[1, 100, 1], 30), vec![vec![1], vec![100], vec![1]]);
        // The oversized unit alone is also fine as the only input.
        assert_eq!(pack(&[100], 30), vec![vec![100]]);
    }

    /// No units means no chunks — the caller emits nothing rather than one empty
    /// section.
    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(pack(&[], 30).is_empty());
    }

    /// R7: the same input packs identically every time — the accumulator is
    /// reset per chunk, so no state leaks across chunk boundaries or runs.
    #[test]
    fn packing_is_deterministic() {
        let units = [4, 9, 2, 15, 7, 3, 11, 8];
        assert_eq!(pack(&units, 20), pack(&units, 20));
    }
}
