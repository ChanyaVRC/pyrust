//! Selection policy for `collections.Counter.most_common`.
//!
//! This is deliberately kept out of the module declaration body: the public
//! method owns argument/backing access, while this file owns the fallible
//! stable ordering and bounded min-heap used to select the result.

use std::cmp::Ordering;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{
    Interpreter, coerce_subclass_backing, compare_values, value_type_name_str,
};
use crate::value::{PyBigIntSign, PyKey, PyToPrimitive, Value, ValueKind};

/// Use bounded selection only when the requested prefix is materially smaller
/// than the mapping. Near-full requests stay on the stable sort path: its
/// contiguous accesses and lower comparison overhead win there.
const HEAP_RATIO: usize = 4;

#[derive(Clone)]
struct Entry {
    key: PyKey,
    count: Value,
    /// Counter/dict insertion order. For equal counts, an earlier entry is
    /// better and must appear first in the result.
    order: usize,
}

pub(super) fn select(
    interp: &mut Interpreter,
    pairs: Vec<(PyKey, Value)>,
    n: Option<&Value>,
) -> Result<Vec<(PyKey, Value)>> {
    let limit = resolve_limit(n, pairs.len())?;
    let mut entries = pairs
        .into_iter()
        .enumerate()
        .map(|(order, (key, count))| Entry { key, count, order })
        .collect::<Vec<_>>();

    let selected = match limit {
        Some(0) => Vec::new(),
        Some(1) => select_one(interp, entries)?,
        Some(limit)
            if limit < entries.len() && limit.saturating_mul(HEAP_RATIO) <= entries.len() =>
        {
            select_bounded(interp, entries, limit)?
        }
        limit => {
            sort_all_desc(interp, &mut entries)?;
            entries.truncate(limit.unwrap_or(entries.len()).min(entries.len()));
            entries
        }
    };
    Ok(selected
        .into_iter()
        .map(|entry| (entry.key, entry.count))
        .collect())
}

/// Resolve the public `n` argument without narrowing Python's unbounded ints.
///
/// `heapq.nlargest`, which backs CPython's implementation, has two observable
/// non-int corner cases worth retaining: `1.0` takes its `n == 1` shortcut,
/// while other floats fail either in `range()` or the full-sort slice.
fn resolve_limit(n: Option<&Value>, len: usize) -> Result<Option<usize>> {
    let Some(n) = n else {
        return Ok(None);
    };
    if n.is_none() {
        return Ok(None);
    }
    let normalized = coerce_subclass_backing(n, &[]).unwrap_or_else(|| n.clone());
    match normalized.kind() {
        ValueKind::Int(value) if value <= 0 => Ok(Some(0)),
        ValueKind::Int(value) => Ok(Some(value as usize)),
        ValueKind::Bool(value) => Ok(Some(value as usize)),
        ValueKind::BigInt(value) => match value.sign() {
            PyBigIntSign::Minus | PyBigIntSign::NoSign => Ok(Some(0)),
            PyBigIntSign::Plus => Ok(Some(value.to_usize().unwrap_or(usize::MAX))),
        },
        ValueKind::Float(1.0) => Ok(Some(1)),
        ValueKind::Float(value) if value >= len as f64 => Err(PyError::named(
            "TypeError",
            "slice indices must be integers or None or have an __index__ method".to_string(),
        )),
        ValueKind::Float(_) => Err(PyError::named(
            "TypeError",
            "'float' object cannot be interpreted as an integer".to_string(),
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'>=' not supported between instances of '{}' and 'int'",
                value_type_name_str(n)
            ),
        )),
    }
}

/// Python `<` for Counter counts. Primitive values use the direct comparator;
/// user instances retain their rich-comparison dispatch and truth conversion.
fn count_less(interp: &mut Interpreter, left: &Value, right: &Value) -> Result<bool> {
    if !matches!(left.kind(), ValueKind::PyInstance(_))
        && !matches!(right.kind(), ValueKind::PyInstance(_))
    {
        return Ok(compare_values(left, right)?.is_lt());
    }
    let compared = interp.eval_binary(left.clone(), BinaryOp::Lt, right.clone())?;
    interp.truthy_value(&compared)
}

/// Ordering used inside the bounded min-heap. It mirrors heapq's decorated
/// `(count, -insertion_order, item)` tuples: lower counts are worse, and among
/// equal counts later entries are worse.
fn heap_entry_less(interp: &mut Interpreter, left: &Entry, right: &Entry) -> Result<bool> {
    let equal = interp.eval_binary(left.count.clone(), BinaryOp::Eq, right.count.clone())?;
    if interp.truthy_value(&equal)? {
        return Ok(left.order > right.order);
    }
    count_less(interp, &left.count, &right.count)
}

fn heap_sift_up(interp: &mut Interpreter, heap: &mut [Entry], mut child: usize) -> Result<()> {
    while child > 0 {
        let parent = (child - 1) / 2;
        if !heap_entry_less(interp, &heap[child], &heap[parent])? {
            break;
        }
        heap.swap(child, parent);
        child = parent;
    }
    Ok(())
}

fn heap_sift_down(interp: &mut Interpreter, heap: &mut [Entry], mut parent: usize) -> Result<()> {
    loop {
        let left = parent.saturating_mul(2).saturating_add(1);
        if left >= heap.len() {
            return Ok(());
        }
        let right = left + 1;
        let child = if right < heap.len() && heap_entry_less(interp, &heap[right], &heap[left])? {
            right
        } else {
            left
        };
        if !heap_entry_less(interp, &heap[child], &heap[parent])? {
            return Ok(());
        }
        heap.swap(parent, child);
        parent = child;
    }
}

fn heap_push(interp: &mut Interpreter, heap: &mut Vec<Entry>, entry: Entry) -> Result<()> {
    heap.push(entry);
    let child = heap.len() - 1;
    heap_sift_up(interp, heap, child)
}

fn heap_pop(interp: &mut Interpreter, heap: &mut Vec<Entry>) -> Result<Entry> {
    let last = heap
        .pop()
        .ok_or_else(|| PyError::Runtime("internal: empty Counter heap".to_string()))?;
    if heap.is_empty() {
        return Ok(last);
    }
    let smallest = std::mem::replace(&mut heap[0], last);
    heap_sift_down(interp, heap, 0)?;
    Ok(smallest)
}

/// CPython's `n == 1` shortcut delegates to `max`, whose primary comparison is
/// `candidate > best` (including the observable `>` TypeError wording).
fn select_one(interp: &mut Interpreter, entries: Vec<Entry>) -> Result<Vec<Entry>> {
    let mut entries = entries.into_iter();
    let Some(mut best) = entries.next() else {
        return Ok(Vec::new());
    };
    for candidate in entries {
        let compared =
            interp.eval_binary(candidate.count.clone(), BinaryOp::Gt, best.count.clone())?;
        if interp.truthy_value(&compared)? {
            best = candidate;
        }
    }
    Ok(vec![best])
}

/// Select a small top-n prefix in O(m log n), retaining CPython's stable tie
/// rule without sorting the other `m - n` entries.
fn select_bounded(
    interp: &mut Interpreter,
    entries: Vec<Entry>,
    limit: usize,
) -> Result<Vec<Entry>> {
    let mut entries = entries.into_iter();
    let mut heap = Vec::with_capacity(limit);
    for entry in entries.by_ref().take(limit) {
        heap_push(interp, &mut heap, entry)?;
    }
    for entry in entries {
        // Like heapq.nlargest, equal-count later entries never displace the
        // earliest selected entry: admission compares counts only.
        if count_less(interp, &heap[0].count, &entry.count)? {
            heap[0] = entry;
            heap_sift_down(interp, &mut heap, 0)?;
        }
    }

    // Repeated min-pop is O(n log n) and produces worst-to-best. Reversing
    // gives descending count and earliest-first ties without a second sort.
    let mut selected = Vec::with_capacity(heap.len());
    while !heap.is_empty() {
        selected.push(heap_pop(interp, &mut heap)?);
    }
    selected.reverse();
    Ok(selected)
}

/// Stable full ordering for `n=None` and near-full prefixes. Capturing the
/// first comparator error lets the Rust stable sort finish without running
/// additional Python code, then propagates the original exception.
fn sort_all_desc(interp: &mut Interpreter, entries: &mut [Entry]) -> Result<()> {
    let mut comparison_error = None;
    entries.sort_by(|left, right| {
        if comparison_error.is_some() {
            return Ordering::Equal;
        }
        match interp.richcmp_order(&right.count, &left.count) {
            Ok(ordering) => ordering,
            Err(error) => {
                comparison_error = Some(error);
                Ordering::Equal
            }
        }
    });
    match comparison_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
