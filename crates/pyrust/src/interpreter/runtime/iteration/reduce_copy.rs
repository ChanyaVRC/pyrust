/// Copying a built-in iterator object (#2974).
///
/// CPython has no generic iterator copy: `copy.copy` runs the object through
/// `__reduce__`, and every iterator type spells its own reduction. Two shapes
/// cover the built-ins:
///
/// * **Sequence-shaped** — `(iter, (sequence,), index)`. The copy retains the
///   *same* sequence and resumes at the same index, so a later mutation of that
///   sequence is observed by both cursors.
/// * **Cursor-shaped** — `(iter, ([remaining, …],))`. A dict or set cursor
///   cannot be resumed by index, so CPython drains what is left into a plain
///   `list` and reduces to a *`list_iterator` over that list*. The copy has
///   therefore left the mapping entirely: later mutations of the container are
///   invisible to it and can never raise `RuntimeError` at it, while the
///   original keeps its live guard.
///
/// Generators reduce to nothing at all — `copy.copy` of one raises
/// `TypeError: cannot pickle 'generator' object`.
///
/// This domain owns every built-in cursor representation, so it owns the
/// reduce-equivalent state; the `copy` module owns the recursion, the memo, and
/// the Python-visible errors and consumes only [`IteratorCopy`].
pub(crate) enum IteratorCopy {
    /// Not an iterator whose copy this domain defines. The `copy` module keeps
    /// its own rules for the value.
    Unowned,
    /// CPython's copy protocol refuses the object outright. Carries the type
    /// noun for `cannot pickle '<noun>' object`.
    Unpicklable(&'static str),
    /// An independent iterator resuming from the same reduce state.
    Rebuilt(Value),
    /// A live bytearray iterator's `(iter, (carrier,), position)` reduction.
    ///
    /// The copy module must execute this reduction because `carrier` may be a
    /// subclass whose `__iter__` changes the reconstructed iterator type, and
    /// `deepcopy` must recurse into the carrier before memoising the outer
    /// iterator (the ordering CPython's `_reconstruct` uses for cycles).
    BytearrayReduction { carrier: Value, position: usize },
    /// A legacy sequence cursor's or real `reversed` subclass's
    /// `(constructor, (owner,)[, position])` reduction. `constructor` is
    /// `iter`, `reversed`, or the concrete `reversed` subclass that owns the
    /// visible cursor.
    ///
    /// The copy module executes this reduction instead of cloning the frame:
    /// deepcopy must recurse into `owner` before constructing and memoising
    /// the outer iterator, exactly as `copy._reconstruct` does.
    GetItemReduction {
        constructor: Value,
        owner: Value,
        position: Option<i64>,
    },
}

include!("reduce_copy/legacy_iterators.rs");
include!("reduce_copy/builtin_iterators.rs");
include!("reduce_copy/itertools_copy.rs");
