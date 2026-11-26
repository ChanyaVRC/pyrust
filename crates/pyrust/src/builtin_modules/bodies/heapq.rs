// `heapq` — 100% native Rust implementation of CPython 3.12's `heapq` module
// (issues #2784, #3363).
//
// CPython's `Lib/heapq.py` is a Python layer over the C accelerator `_heapq`
// (`Modules/_heapqmodule.c`).  This file ports both halves directly, so there
// is no Python-source injection:
//
//  * `heappush` / `heappop` / `heapify` / `heapreplace` / `heappushpop` and
//    the `_heappop_max` / `_heapify_max` / `_heapreplace_max` variants follow
//    the C code: the argument must be a `list` (or subclass) and the sift
//    helpers swap elements in place, checking after every comparison that the
//    comparison did not resize the list (`RuntimeError: list changed size
//    during iteration`).
//  * `_siftdown` / `_siftup` / `_siftdown_max` / `_siftup_max` are the
//    pure-Python helpers CPython keeps in `heapq.py`; they work on any
//    sequence via `__getitem__` / `__setitem__`.
//  * `nlargest` / `nsmallest` / `merge` are the pure-Python algorithms of
//    `heapq.py`, driven through the same builtins (`iter`, `min`, `max`,
//    `sorted`, `zip`, `range`, `len`) and the same `<` comparisons, so
//    tie-breaking and error wording match.  `merge` is lazy: it returns a
//    generator-like iterator that does no work until its first `next()`.
//
// Reference: <https://docs.python.org/3/library/heapq.html>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, ProviderIterator, builtin_data_backing, is_stop_iteration_error,
    make_iterator, value_type_name_str,
};
use crate::value::Value;
use pyrust_derive::pyrust_module;

const HEAPQ_DOC: &str = r#"Heap queue algorithm (a.k.a. priority queue).

Heaps are arrays for which a[k] <= a[2*k+1] and a[k] <= a[2*k+2] for
all k, counting elements from 0.  For the sake of comparison,
non-existing elements are considered to be infinite.  The interesting
property of a heap is that a[0] is always its smallest element.

Usage:

heap = []            # creates an empty heap
heappush(heap, item) # pushes a new item on the heap
item = heappop(heap) # pops the smallest item from the heap
item = heap[0]       # smallest item on the heap without popping it
heapify(x)           # transforms list into a heap, in-place, in linear time
item = heappushpop(heap, item) # pushes a new item and then returns
                               # the smallest item; the heap size is unchanged
item = heapreplace(heap, item) # pops and returns smallest item, and adds
                               # new item; the heap size is unchanged

Our API differs from textbook heap algorithms as follows:

- We use 0-based indexing.  This makes the relationship between the
  index for a node and the indexes for its children slightly less
  obvious, but is more suitable since Python uses 0-based indexing.

- Our heappop() method returns the smallest item, not the largest.

These two make it possible to view the heap as a regular Python list
without surprises: heap[0] is the smallest item, and heap.sort()
maintains the heap invariant!
"#;

const HEAPQ_ABOUT: &str = r#"Heap queues

[explanation by François Pinard]

Heaps are arrays for which a[k] <= a[2*k+1] and a[k] <= a[2*k+2] for
all k, counting elements from 0.  For the sake of comparison,
non-existing elements are considered to be infinite.  The interesting
property of a heap is that a[0] is always its smallest element.

The strange invariant above is meant to be an efficient memory
representation for a tournament.  The numbers below are `k', not a[k]:

                                   0

                  1                                 2

          3               4                5               6

      7       8       9       10      11      12      13      14

    15 16   17 18   19 20   21 22   23 24   25 26   27 28   29 30


In the tree above, each cell `k' is topping `2*k+1' and `2*k+2'.  In
a usual binary tournament we see in sports, each cell is the winner
over the two cells it tops, and we can trace the winner down the tree
to see all opponents s/he had.  However, in many computer applications
of such tournaments, we do not need to trace the history of a winner.
To be more memory efficient, when a winner is promoted, we try to
replace it by something else at a lower level, and the rule becomes
that a cell and the two cells it tops contain three different items,
but the top cell "wins" over the two topped cells.

If this heap invariant is protected at all time, index 0 is clearly
the overall winner.  The simplest algorithmic way to remove it and
find the "next" winner is to move some loser (let's say cell 30 in the
diagram above) into the 0 position, and then percolate this new 0 down
the tree, exchanging values, until the invariant is re-established.
This is clearly logarithmic on the total number of items in the tree.
By iterating over all items, you get an O(n ln n) sort.

A nice feature of this sort is that you can efficiently insert new
items while the sort is going on, provided that the inserted items are
not "better" than the last 0'th element you extracted.  This is
especially useful in simulation contexts, where the tree holds all
incoming events, and the "win" condition means the smallest scheduled
time.  When an event schedule other events for execution, they are
scheduled into the future, so they can easily go into the heap.  So, a
heap is a good structure for implementing schedulers (this is what I
used for my MIDI sequencer :-).

Various structures for implementing schedulers have been extensively
studied, and heaps are good for this, as they are reasonably speedy,
the speed is almost constant, and the worst case is not much different
than the average case.  However, there are other representations which
are more efficient overall, yet the worst cases might be terrible.

Heaps are also very useful in big disk sorts.  You most probably all
know that a big sort implies producing "runs" (which are pre-sorted
sequences, which size is usually related to the amount of CPU memory),
followed by a merging passes for these runs, which merging is often
very cleverly organised[1].  It is very important that the initial
sort produces the longest runs possible.  Tournaments are a good way
to that.  If, using all the memory available to hold a tournament, you
replace and percolate items that happen to fit the current run, you'll
produce runs which are twice the size of the memory for random input,
and much better for input fuzzily ordered.

Moreover, if you output the 0'th item on disk and get an input which
may not fit in the current tournament (because the value "wins" over
the last output value), it cannot fit in the heap, so the size of the
heap decreases.  The freed memory could be cleverly reused immediately
for progressively building a second heap, which grows at exactly the
same rate the first heap is melting.  When the first heap completely
vanishes, you switch heaps and start a new run.  Clever and quite
effective!

In a word, heaps are useful memory structures to know.  I use them in
a few applications, and I think it is good to keep a `heap' module
around. :-)

--------------------
[1] The disk balancing algorithms which are current, nowadays, are
more annoying than clever, and this is a consequence of the seeking
capabilities of the disks.  On devices which cannot seek, like big
tape drives, the story was quite different, and one had to be very
clever to ensure (far in advance) that each tape movement will be the
most effective possible (that is, will best participate at
"progressing" the merge).  Some tapes were even able to read
backwards, and this was also used to avoid the rewinding time.
Believe me, real good tape sorts were quite spectacular to watch!
From all times, sorting has always been a Great Art! :-)
"#;

// ── small interpreter helpers ────────────────────────────────────────────────

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (name, value) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*name).to_string()),
            value: value.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    call_kw(interp, callee, args, &[])
}

fn call_builtin(interp: &mut Interpreter, name: &'static str, args: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), args)
}

/// `PyObject_RichCompareBool(lhs, rhs, Py_LT)` / the `<` operator in a
/// condition.
fn py_lt(interp: &mut Interpreter, lhs: &Value, rhs: &Value) -> Result<bool> {
    let res = interp.eval_binary(lhs.clone(), BinaryOp::Lt, rhs.clone())?;
    interp.truthy_value(&res)
}

fn binary(interp: &mut Interpreter, lhs: &Value, op: BinaryOp, rhs: &Value) -> Result<Value> {
    interp.eval_binary(lhs.clone(), op, rhs.clone())
}

fn binary_truthy(interp: &mut Interpreter, lhs: &Value, op: BinaryOp, rhs: &Value) -> Result<bool> {
    let res = binary(interp, lhs, op, rhs)?;
    interp.truthy_value(&res)
}

/// `value + delta` for the `order += 1` / `order -= 1` counters; `n` may be any
/// object accepted by `range()`, so fall back to the generic operators.
fn add_int(interp: &mut Interpreter, value: &Value, delta: i64) -> Result<Value> {
    if let Some(n) = value.as_int()
        && let Some(sum) = n.checked_add(delta)
    {
        return Ok(Value::int(sum));
    }
    let (op, magnitude) = if delta < 0 {
        (BinaryOp::Sub, -delta)
    } else {
        (BinaryOp::Add, delta)
    };
    interp.eval_binary_aug(value.clone(), op, Value::int(magnitude))
}

fn index_error(msg: &str) -> PyError {
    PyError::named("IndexError", msg)
}

// ── C-accelerator heap operations (`Modules/_heapqmodule.c`) ─────────────────

/// Resolve the `heap` argument: any `list` (including subclasses), whose
/// storage the C code mutates directly without consulting overridden methods.
fn heap_arg(value: &Value, fn_name: &str, positional: bool) -> Result<Value> {
    if value.is_list() {
        return Ok(value.clone());
    }
    if let Some(backing) = builtin_data_backing(value)
        && backing.is_list()
    {
        return Ok(backing);
    }
    let which = if positional { "1 " } else { "" };
    Err(PyError::named(
        "TypeError",
        format!(
            "{fn_name}() argument {which}must be list, not {}",
            value_type_name_str(value)
        ),
    ))
}

fn heap_len(heap: &Value) -> usize {
    heap.list_len().unwrap_or(0)
}

fn heap_get(heap: &Value, index: usize) -> Result<Value> {
    heap.list_with(|items| items.get(index).cloned())
        .flatten()
        .ok_or_else(|| index_error("list index out of range"))
}

fn heap_set(heap: &Value, index: usize, item: Value) {
    heap.list_with_mut(|items| {
        if let Some(slot) = items.get_mut(index) {
            *slot = item;
        }
    });
}

fn heap_swap(heap: &Value, a: usize, b: usize) {
    heap.list_with_mut(|items| {
        if a < items.len() && b < items.len() {
            items.swap(a, b);
        }
    });
}

fn size_changed() -> PyError {
    PyError::named("RuntimeError", "list changed size during iteration")
}

/// `siftdown` / `siftdown_max`: restore the heap invariant on the path from
/// `pos` up to `startpos`.
fn siftdown(
    interp: &mut Interpreter,
    heap: &Value,
    startpos: usize,
    mut pos: usize,
    max: bool,
) -> Result<()> {
    let size = heap_len(heap);
    if pos >= size {
        return Err(index_error("index out of range"));
    }
    let mut newitem = heap_get(heap, pos)?;
    while pos > startpos {
        let parentpos = (pos - 1) >> 1;
        let parent = heap_get(heap, parentpos)?;
        let cmp = if max {
            py_lt(interp, &parent, &newitem)?
        } else {
            py_lt(interp, &newitem, &parent)?
        };
        if size != heap_len(heap) {
            return Err(size_changed());
        }
        if !cmp {
            break;
        }
        let parent = heap_get(heap, parentpos)?;
        newitem = heap_get(heap, pos)?;
        heap_set(heap, parentpos, newitem.clone());
        heap_set(heap, pos, parent);
        pos = parentpos;
    }
    Ok(())
}

/// `siftup` / `siftup_max`: move the smaller (larger) child up until a leaf is
/// reached, then sift the original item back down.
fn siftup(interp: &mut Interpreter, heap: &Value, mut pos: usize, max: bool) -> Result<()> {
    let endpos = heap_len(heap);
    let startpos = pos;
    if pos >= endpos {
        return Err(index_error("index out of range"));
    }
    let limit = endpos >> 1;
    while pos < limit {
        let mut childpos = 2 * pos + 1;
        if childpos + 1 < endpos {
            let left = heap_get(heap, childpos)?;
            let right = heap_get(heap, childpos + 1)?;
            let cmp = if max {
                py_lt(interp, &right, &left)?
            } else {
                py_lt(interp, &left, &right)?
            };
            if !cmp {
                childpos += 1;
            }
            if endpos != heap_len(heap) {
                return Err(size_changed());
            }
        }
        heap_swap(heap, childpos, pos);
        pos = childpos;
    }
    siftdown(interp, heap, startpos, pos, max)
}

fn heappush_internal(interp: &mut Interpreter, heap: &Value, item: Value) -> Result<Value> {
    heap.list_push(item)?;
    siftdown(interp, heap, 0, heap_len(heap) - 1, false)?;
    Ok(Value::none())
}

fn heappop_internal(interp: &mut Interpreter, heap: &Value, max: bool) -> Result<Value> {
    let Some(lastelt) = heap.list_with_mut(|items| items.pop()).flatten() else {
        return Err(index_error("index out of range"));
    };
    if heap_len(heap) == 0 {
        return Ok(lastelt);
    }
    let returnitem = heap_get(heap, 0)?;
    heap_set(heap, 0, lastelt);
    siftup(interp, heap, 0, max)?;
    Ok(returnitem)
}

fn heapreplace_internal(
    interp: &mut Interpreter,
    heap: &Value,
    item: Value,
    max: bool,
) -> Result<Value> {
    if heap_len(heap) == 0 {
        return Err(index_error("index out of range"));
    }
    let returnitem = heap_get(heap, 0)?;
    heap_set(heap, 0, item);
    siftup(interp, heap, 0, max)?;
    Ok(returnitem)
}

fn heappushpop_internal(interp: &mut Interpreter, heap: &Value, item: Value) -> Result<Value> {
    if heap_len(heap) == 0 {
        return Ok(item);
    }
    let top = heap_get(heap, 0)?;
    if !py_lt(interp, &top, &item)? {
        return Ok(item);
    }
    // The comparison may have emptied the heap.
    if heap_len(heap) == 0 {
        return Err(index_error("index out of range"));
    }
    let returnitem = heap_get(heap, 0)?;
    heap_set(heap, 0, item);
    siftup(interp, heap, 0, false)?;
    Ok(returnitem)
}

fn heapify_internal(interp: &mut Interpreter, heap: &Value, max: bool) -> Result<Value> {
    let n = heap_len(heap);
    for i in (0..n >> 1).rev() {
        siftup(interp, heap, i, max)?;
    }
    Ok(Value::none())
}

// ── pure-Python sift helpers (`_siftdown`, `_siftup`, ...) ───────────────────

fn getitem(interp: &mut Interpreter, seq: &Value, index: &Value) -> Result<Value> {
    interp.eval_index(seq, index.clone())
}

fn setitem(interp: &mut Interpreter, seq: &Value, index: &Value, item: Value) -> Result<()> {
    let method = interp.get_attr(seq, "__setitem__")?;
    call_pos(interp, method, &[index.clone(), item])?;
    Ok(())
}

/// `heapq._siftdown` / `heapq._siftdown_max`.
fn py_siftdown(
    interp: &mut Interpreter,
    heap: &Value,
    startpos: &Value,
    pos: &Value,
    max: bool,
) -> Result<Value> {
    let one = Value::int(1);
    let mut pos = pos.clone();
    let newitem = getitem(interp, heap, &pos)?;
    // Follow the path to the root, moving parents down until finding a place
    // newitem fits.
    while binary_truthy(interp, &pos, BinaryOp::Gt, startpos)? {
        let pos_minus_one = binary(interp, &pos, BinaryOp::Sub, &one)?;
        let parentpos = binary(interp, &pos_minus_one, BinaryOp::RShift, &one)?;
        let parent = getitem(interp, heap, &parentpos)?;
        let move_down = if max {
            py_lt(interp, &parent, &newitem)?
        } else {
            py_lt(interp, &newitem, &parent)?
        };
        if !move_down {
            break;
        }
        setitem(interp, heap, &pos, parent)?;
        pos = parentpos;
    }
    setitem(interp, heap, &pos, newitem)?;
    Ok(Value::none())
}

/// `heapq._siftup` / `heapq._siftup_max`.
fn py_siftup(interp: &mut Interpreter, heap: &Value, pos: &Value, max: bool) -> Result<Value> {
    let one = Value::int(1);
    let two = Value::int(2);
    let endpos = call_builtin(interp, "len", std::slice::from_ref(heap))?;
    let startpos = pos.clone();
    let mut pos = pos.clone();
    let newitem = getitem(interp, heap, &pos)?;
    let child_of = |interp: &mut Interpreter, pos: &Value| -> Result<Value> {
        let doubled = binary(interp, &two, BinaryOp::Mul, pos)?;
        binary(interp, &doubled, BinaryOp::Add, &one)
    };
    // Bubble up the smaller (larger) child until hitting a leaf.
    let mut childpos = child_of(interp, &pos)?;
    while binary_truthy(interp, &childpos, BinaryOp::Lt, &endpos)? {
        let rightpos = binary(interp, &childpos, BinaryOp::Add, &one)?;
        if binary_truthy(interp, &rightpos, BinaryOp::Lt, &endpos)? {
            let left = getitem(interp, heap, &childpos)?;
            let right = getitem(interp, heap, &rightpos)?;
            let left_wins = if max {
                py_lt(interp, &right, &left)?
            } else {
                py_lt(interp, &left, &right)?
            };
            if !left_wins {
                childpos = rightpos;
            }
        }
        let child = getitem(interp, heap, &childpos)?;
        setitem(interp, heap, &pos, child)?;
        pos = childpos;
        childpos = child_of(interp, &pos)?;
    }
    // The leaf at pos is empty now.  Put newitem there, and bubble it up to
    // its final resting place (by sifting its parents down).
    setitem(interp, heap, &pos, newitem)?;
    py_siftdown(interp, heap, &startpos, &pos, max)
}

// ── nlargest / nsmallest ─────────────────────────────────────────────────────

/// Shared body of `nsmallest` (`largest == false`) and `nlargest`.
fn n_extreme(
    interp: &mut Interpreter,
    n: &Value,
    iterable: &Value,
    key: &Value,
    largest: bool,
) -> Result<Value> {
    let key_is_none = key.is_none();

    // Short-cut for n==1 is to use min() / max().
    if binary_truthy(interp, n, BinaryOp::Eq, &Value::int(1))? {
        let it = call_builtin(interp, "iter", std::slice::from_ref(iterable))?;
        let sentinel = Value::list(Vec::new());
        let result = call_kw(
            interp,
            Value::builtin_function(if largest { "max" } else { "min" }),
            &[it],
            &[("default", sentinel.clone()), ("key", key.clone())],
        )?;
        return Ok(if result.is_identical_to(&sentinel) {
            Value::list(Vec::new())
        } else {
            Value::list(vec![result])
        });
    }

    // When n>=size, it's faster to use sorted().
    match call_builtin(interp, "len", std::slice::from_ref(iterable)) {
        Ok(size) => {
            if binary_truthy(interp, n, BinaryOp::Ge, &size)? {
                let mut kwargs = vec![("key", key.clone())];
                if largest {
                    kwargs.push(("reverse", Value::bool_(true)));
                }
                let sorted = call_kw(
                    interp,
                    Value::builtin_function("sorted"),
                    std::slice::from_ref(iterable),
                    &kwargs,
                )?;
                let slice = call_builtin(interp, "slice", &[Value::none(), n.clone()])?;
                return interp.eval_index(&sorted, slice);
            }
        }
        Err(err) if err.class_name_is("TypeError") || err.class_name_is("AttributeError") => {}
        Err(err) => return Err(err),
    }

    let it = call_builtin(interp, "iter", std::slice::from_ref(iterable))?;
    // `nlargest` counts the order tie-breaker down from `-n`, `nsmallest`
    // counts it up from `n`.
    let (start, delta) = if largest {
        (interp.eval_unary(n.clone(), UnaryOp::Neg)?, -1)
    } else {
        (n.clone(), 1)
    };
    // Put the range(n) first so that zip() doesn't consume one too many
    // elements from the iterator.
    let range = if largest {
        call_builtin(
            interp,
            "range",
            &[Value::int(0), start.clone(), Value::int(-1)],
        )?
    } else {
        call_builtin(interp, "range", std::slice::from_ref(n))?
    };
    let zipped = call_builtin(interp, "zip", &[range, it.clone()])?;
    let mut decorated = Vec::new();
    loop {
        let pair = match interp.call_next(&zipped, None) {
            Ok(pair) => pair,
            Err(err) if is_stop_iteration_error(&err) => break,
            Err(err) => return Err(err),
        };
        let (i, elem) = match pair.as_tuple() {
            Some([i, elem]) => (i.clone(), elem.clone()),
            _ => {
                return Err(PyError::runtime(
                    "heapq: zip yielded a malformed pair".to_string(),
                ));
            }
        };
        decorated.push(if key_is_none {
            Value::tuple(vec![elem, i])
        } else {
            let k = call_pos(interp, key.clone(), std::slice::from_ref(&elem))?;
            Value::tuple(vec![k, i, elem])
        });
    }
    let result = Value::list(decorated);
    if heap_len(&result) == 0 {
        return Ok(result);
    }
    heapify_internal(interp, &result, !largest)?;
    let mut top = tuple_head(&result)?;
    let mut order = start;
    loop {
        let elem = match interp.call_next(&it, None) {
            Ok(elem) => elem,
            Err(err) if is_stop_iteration_error(&err) => break,
            Err(err) => return Err(err),
        };
        let probe = if key_is_none {
            elem.clone()
        } else {
            call_pos(interp, key.clone(), std::slice::from_ref(&elem))?
        };
        let displaces = if largest {
            py_lt(interp, &top, &probe)?
        } else {
            py_lt(interp, &probe, &top)?
        };
        if displaces {
            let entry = if key_is_none {
                Value::tuple(vec![probe, order.clone()])
            } else {
                Value::tuple(vec![probe, order.clone(), elem])
            };
            heapreplace_internal(interp, &result, entry, !largest)?;
            top = tuple_head(&result)?;
            order = add_int(interp, &order, delta)?;
        }
    }

    let sort = interp.get_attr(&result, "sort")?;
    if largest {
        call_kw(interp, sort, &[], &[("reverse", Value::bool_(true))])?;
    } else {
        call_pos(interp, sort, &[])?;
    }
    let payload = if key_is_none { 0 } else { 2 };
    let items = result
        .list_with(|items| {
            items
                .iter()
                .map(|entry| entry.as_tuple().and_then(|t| t.get(payload).cloned()))
                .collect::<Option<Vec<Value>>>()
        })
        .flatten()
        .ok_or_else(|| PyError::runtime("heapq: malformed result entry".to_string()))?;
    Ok(Value::list(items))
}

/// `result[0][0]` of the decorated heap.
fn tuple_head(result: &Value) -> Result<Value> {
    heap_get(result, 0)?
        .as_tuple()
        .and_then(|t| t.first().cloned())
        .ok_or_else(|| PyError::runtime("heapq: malformed heap entry".to_string()))
}

// ── merge ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergePhase {
    /// Nothing has run yet (generator-style lazy start).
    Start,
    /// At the `while len(h) > 1` check.
    Top,
    /// Inside the inner `while True`: yield the smallest stream's head.
    Inner,
    /// Resumed after a yield: advance the stream that was just consumed.
    Resume,
    /// Exactly one stream is left: yield its head.
    Tail,
    /// ...then drain it with `yield from`.
    Drain,
    Done,
}

struct MergeState {
    phase: MergePhase,
    iterables: Vec<Value>,
    key: Value,
    reverse: Value,
    use_key: bool,
    reverse_flag: bool,
    /// The heap: `[value, order, next, it]` entries (`[key(value), order,
    /// value, next, it]` with a key function).  Only the first two slots can
    /// ever take part in a comparison, because `order` is unique.
    heap: Value,
    /// The heap entry whose value was just yielded.
    pending: Option<Value>,
    /// The sole remaining stream, being drained by `yield from`.
    drain_src: Option<Value>,
    drain: Option<Value>,
}

impl MergeState {
    fn finish(&mut self) {
        self.phase = MergePhase::Done;
        self.iterables.clear();
        self.pending = None;
        self.drain_src = None;
        self.drain = None;
        self.heap = Value::list(Vec::new());
    }

    /// Slot of the stream iterator inside a heap entry.
    fn it_slot(&self) -> usize {
        if self.use_key { 4 } else { 3 }
    }

    /// Slot of the yielded value inside a heap entry.
    fn value_slot(&self) -> usize {
        if self.use_key { 2 } else { 0 }
    }
}

fn merge_start(interp: &mut Interpreter, st: &mut MergeState) -> Result<()> {
    st.reverse_flag = interp.truthy_value(&st.reverse)?;
    st.use_key = !st.key.is_none();
    let direction: i64 = if st.reverse_flag { -1 } else { 1 };
    let iterables = std::mem::take(&mut st.iterables);
    for (order, source) in iterables.iter().enumerate() {
        let it = make_iterator(interp, source)?;
        let entry = (|| -> Result<Value> {
            let next = interp.get_attr(&it, "__next__")?;
            let value = call_pos(interp, next.clone(), &[])?;
            let order = Value::int(order as i64 * direction);
            Ok(Value::list(if st.use_key {
                let k = call_pos(interp, st.key.clone(), std::slice::from_ref(&value))?;
                vec![k, order, value, next, it.clone()]
            } else {
                vec![value, order, next, it.clone()]
            }))
        })();
        match entry {
            Ok(entry) => st.heap.list_push(entry)?,
            Err(err) if is_stop_iteration_error(&err) => {}
            Err(err) => return Err(err),
        }
    }
    heapify_internal(interp, &st.heap, st.reverse_flag)?;
    Ok(())
}

/// Advance the stream whose head was just yielded, then restore the heap.
fn merge_resume(interp: &mut Interpreter, st: &mut MergeState, entry: &Value) -> Result<()> {
    let next_slot = st.it_slot() - 1;
    let next = heap_get(entry, next_slot)?;
    let value = call_pos(interp, next, &[])?;
    if st.use_key {
        let k = call_pos(interp, st.key.clone(), std::slice::from_ref(&value))?;
        heap_set(entry, 0, k);
        heap_set(entry, 2, value);
    } else {
        heap_set(entry, 0, value);
    }
    heapreplace_internal(interp, &st.heap, entry.clone(), st.reverse_flag)?;
    Ok(())
}

fn merge_step(interp: &mut Interpreter, st: &mut MergeState) -> Result<Option<Value>> {
    loop {
        match st.phase {
            MergePhase::Start => {
                merge_start(interp, st)?;
                st.phase = MergePhase::Top;
            }
            MergePhase::Top => {
                st.phase = match heap_len(&st.heap) {
                    0 => MergePhase::Done,
                    1 => MergePhase::Tail,
                    _ => MergePhase::Inner,
                };
            }
            MergePhase::Inner => {
                let entry = heap_get(&st.heap, 0)?;
                let value = heap_get(&entry, st.value_slot())?;
                st.pending = Some(entry);
                st.phase = MergePhase::Resume;
                return Ok(Some(value));
            }
            MergePhase::Resume => {
                let Some(entry) = st.pending.take() else {
                    st.phase = MergePhase::Done;
                    return Ok(None);
                };
                match merge_resume(interp, st, &entry) {
                    Ok(()) => st.phase = MergePhase::Inner,
                    Err(err) if is_stop_iteration_error(&err) => {
                        // Remove the exhausted iterator.
                        heappop_internal(interp, &st.heap, st.reverse_flag)?;
                        st.phase = MergePhase::Top;
                    }
                    Err(err) => return Err(err),
                }
            }
            MergePhase::Tail => {
                // Fast case when only a single iterator remains.
                let entry = heap_get(&st.heap, 0)?;
                let value = heap_get(&entry, st.value_slot())?;
                st.drain_src = Some(heap_get(&entry, st.it_slot())?);
                st.phase = MergePhase::Drain;
                return Ok(Some(value));
            }
            MergePhase::Drain => {
                if st.drain.is_none() {
                    let Some(source) = st.drain_src.take() else {
                        st.phase = MergePhase::Done;
                        return Ok(None);
                    };
                    st.drain = Some(make_iterator(interp, &source)?);
                }
                let drain = st.drain.clone().expect("drain iterator was just set");
                return match interp.call_next(&drain, None) {
                    Ok(value) => Ok(Some(value)),
                    Err(err) if is_stop_iteration_error(&err) => {
                        st.phase = MergePhase::Done;
                        Ok(None)
                    }
                    Err(err) => Err(err),
                };
            }
            MergePhase::Done => return Ok(None),
        }
    }
}

fn advance_merge(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    // The state is moved out while Python code runs, so a re-entrant `next()`
    // (e.g. from the key function) reports what a running generator would.
    let taken = state_rc
        .borrow_mut()
        .downcast_mut::<Option<MergeState>>()
        .map(Option::take);
    let mut st = match taken {
        Some(Some(st)) => st,
        Some(None) => {
            return Err(PyError::named("ValueError", "generator already executing"));
        }
        None => {
            return Err(PyError::runtime(
                "internal: heapq.merge iterator state corrupted".to_string(),
            ));
        }
    };
    let result = merge_step(interp, &mut st);
    if !matches!(result, Ok(Some(_))) {
        st.finish();
    }
    if let Some(slot) = state_rc.borrow_mut().downcast_mut::<Option<MergeState>>() {
        *slot = Some(st);
    }
    result
}

fn make_merge(iterables: &[Value], key: Value, reverse: Value) -> Value {
    let state = MergeState {
        phase: MergePhase::Start,
        iterables: iterables.to_vec(),
        key,
        reverse,
        use_key: false,
        reverse_flag: false,
        heap: Value::list(Vec::new()),
        pending: None,
        drain_src: None,
        drain: None,
    };
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(Some(state)),
        advance_merge,
        "generator",
        "generator",
        None,
    )))
}

// ── CPython-exact argument handling ──────────────────────────────────────────

type Kwargs = indexmap::IndexMap<String, Value>;

fn type_error(msg: String) -> PyError {
    PyError::named("TypeError", msg)
}

/// Positional-only C entry points of `_heapq` take neither keywords nor a
/// wrong number of arguments; `one` selects the `METH_O` wording, otherwise
/// the `METH_FASTCALL` clinic wording for a two-argument function is used.
fn c_args<'a>(name: &str, args: &'a [Value], kwargs: &Kwargs, one: bool) -> Result<&'a [Value]> {
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "_heapq.{name}() takes no keyword arguments"
        )));
    }
    if one {
        if args.len() != 1 {
            return Err(type_error(format!(
                "_heapq.{name}() takes exactly one argument ({} given)",
                args.len()
            )));
        }
    } else if args.len() != 2 {
        return Err(type_error(format!(
            "{name} expected 2 arguments, got {}",
            args.len()
        )));
    }
    Ok(args)
}

/// Bind `args` / `kwargs` to the parameters of a pure-Python function the way
/// CPython does, reproducing its `TypeError` wording. The first `required`
/// parameters have no default; an omitted optional parameter yields `None`.
fn bind_python_args<const N: usize>(
    fn_name: &str,
    params: &[&str; N],
    required: usize,
    args: &[Value],
    kwargs: &Kwargs,
) -> Result<[Option<Value>; N]> {
    if args.len() > N {
        let given = args.len();
        let takes = if required == N {
            format!("{N} positional argument{}", if N == 1 { "" } else { "s" })
        } else {
            format!("from {required} to {N} positional arguments")
        };
        let verb = if given == 1 { "was" } else { "were" };
        return Err(type_error(format!(
            "{fn_name}() takes {takes} but {given} {verb} given"
        )));
    }
    let mut bound: [Option<Value>; N] = std::array::from_fn(|index| args.get(index).cloned());
    for (key, value) in kwargs {
        let Some(index) = params.iter().position(|name| name == key) else {
            return Err(type_error(format!(
                "{fn_name}() got an unexpected keyword argument '{key}'"
            )));
        };
        if bound[index].is_some() {
            return Err(type_error(format!(
                "{fn_name}() got multiple values for argument '{key}'"
            )));
        }
        bound[index] = Some(value.clone());
    }
    let missing: Vec<String> = params
        .iter()
        .zip(&bound)
        .take(required)
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| format!("'{name}'"))
        .collect();
    if !missing.is_empty() {
        let list = match missing.as_slice() {
            [only] => only.clone(),
            [first, last] => format!("{first} and {last}"),
            [init @ .., last] => format!("{}, and {last}", init.join(", ")),
            [] => unreachable!(),
        };
        return Err(type_error(format!(
            "{fn_name}() missing {} required positional argument{}: {list}",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        )));
    }
    Ok(bound)
}

/// A bound parameter that the arity check guarantees is present.
fn arg(value: &Option<Value>) -> Value {
    value.clone().unwrap_or_else(Value::none)
}

fn optional_or(value: &Option<Value>, default: Value) -> Value {
    value.clone().unwrap_or(default)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(HEAPQ_DOC),
        "__about__" => Value::string(HEAPQ_ABOUT),
        "__all__" => Value::list(
            [
                "heappush",
                "heappop",
                "heapify",
                "heapreplace",
                "merge",
                "nlargest",
                "nsmallest",
                "heappushpop",
            ]
            .into_iter()
            .map(Value::string)
            .collect(),
        ),
    }

    /// Push item onto heap, maintaining the heap invariant.
    fn heappush(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("heappush", args, &kwargs, false)?;
        let heap = heap_arg(&args[0], "heappush", true)?;
        heappush_internal(_interp, &heap, args[1].clone())
    }

    /// Pop the smallest item off the heap, maintaining the heap invariant.
    fn heappop(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("heappop", args, &kwargs, true)?;
        let heap = heap_arg(&args[0], "heappop", false)?;
        heappop_internal(_interp, &heap, false)
    }

    /// Pop and return the current smallest value, and add the new item.
    ///
    /// This is more efficient than heappop() followed by heappush(), and can be
    /// more appropriate when using a fixed-size heap.  Note that the value
    /// returned may be larger than item!  That constrains reasonable uses of
    /// this routine unless written as part of a conditional replacement:
    ///
    ///     if item > heap[0]:
    ///         item = heapreplace(heap, item)
    fn heapreplace(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("heapreplace", args, &kwargs, false)?;
        let heap = heap_arg(&args[0], "heapreplace", true)?;
        heapreplace_internal(_interp, &heap, args[1].clone(), false)
    }

    /// Push item on the heap, then pop and return the smallest item from the heap.
    ///
    /// The combined action runs more efficiently than heappush() followed by
    /// a separate call to heappop().
    fn heappushpop(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("heappushpop", args, &kwargs, false)?;
        let heap = heap_arg(&args[0], "heappushpop", true)?;
        heappushpop_internal(_interp, &heap, args[1].clone())
    }

    /// Transform list into a heap, in-place, in O(len(heap)) time.
    fn heapify(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("heapify", args, &kwargs, true)?;
        let heap = heap_arg(&args[0], "heapify", false)?;
        heapify_internal(_interp, &heap, false)
    }

    /// Maxheap variant of heappop.
    fn _heappop_max(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("_heappop_max", args, &kwargs, true)?;
        let heap = heap_arg(&args[0], "_heappop_max", false)?;
        heappop_internal(_interp, &heap, true)
    }

    /// Maxheap variant of heapreplace.
    fn _heapreplace_max(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("_heapreplace_max", args, &kwargs, false)?;
        let heap = heap_arg(&args[0], "_heapreplace_max", true)?;
        heapreplace_internal(_interp, &heap, args[1].clone(), true)
    }

    /// Maxheap variant of heapify.
    fn _heapify_max(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let args = c_args("_heapify_max", args, &kwargs, true)?;
        let heap = heap_arg(&args[0], "_heapify_max", false)?;
        heapify_internal(_interp, &heap, true)
    }

    fn _siftdown(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let [heap, startpos, pos] =
            bind_python_args("_siftdown", &["heap", "startpos", "pos"], 3, args, &kwargs)?;
        py_siftdown(
            _interp,
            &arg(&heap),
            &arg(&startpos),
            &arg(&pos),
            false,
        )
    }

    fn _siftup(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let [heap, pos] = bind_python_args("_siftup", &["heap", "pos"], 2, args, &kwargs)?;
        py_siftup(_interp, &arg(&heap), &arg(&pos), false)
    }

    /// Maxheap variant of _siftdown
    fn _siftdown_max(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let [heap, startpos, pos] = bind_python_args(
            "_siftdown_max",
            &["heap", "startpos", "pos"],
            3,
            args,
            &kwargs,
        )?;
        py_siftdown(
            _interp,
            &arg(&heap),
            &arg(&startpos),
            &arg(&pos),
            true,
        )
    }

    /// Maxheap variant of _siftup
    fn _siftup_max(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let [heap, pos] = bind_python_args("_siftup_max", &["heap", "pos"], 2, args, &kwargs)?;
        py_siftup(_interp, &arg(&heap), &arg(&pos), true)
    }

    /// Merge multiple sorted inputs into a single sorted output.
    ///
    /// Similar to sorted(itertools.chain(*iterables)) but returns a generator,
    /// does not pull the data into memory all at once, and assumes that each of
    /// the input streams is already sorted (smallest to largest).
    ///
    /// >>> list(merge([1,3,5,7], [0,2,4,8], [5,10,15,20], [], [25]))
    /// [0, 1, 2, 3, 4, 5, 5, 7, 8, 10, 15, 20, 25]
    ///
    /// If *key* is not None, applies a key function to each element to determine
    /// its sort order.
    ///
    /// >>> list(merge(['dog', 'horse'], ['cat', 'fish', 'kangaroo'], key=len))
    /// ['dog', 'cat', 'fish', 'horse', 'kangaroo']
    fn merge(#[varargs] iterables: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let mut key = Value::none();
        let mut reverse = Value::bool_(false);
        for (name, value) in &kwargs {
            match name.as_str() {
                "key" => key = value.clone(),
                "reverse" => reverse = value.clone(),
                other => {
                    return Err(type_error(format!(
                        "merge() got an unexpected keyword argument '{other}'"
                    )));
                }
            }
        }
        Ok(make_merge(iterables, key, reverse))
    }

    /// Find the n smallest elements in a dataset.
    ///
    /// Equivalent to:  sorted(iterable, key=key)[:n]
    fn nsmallest(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let [n, iterable, key] =
            bind_python_args("nsmallest", &["n", "iterable", "key"], 2, args, &kwargs)?;
        n_extreme(
            _interp,
            &arg(&n),
            &arg(&iterable),
            &optional_or(&key, Value::none()),
            false,
        )
    }

    /// Find the n largest elements in a dataset.
    ///
    /// Equivalent to:  sorted(iterable, key=key, reverse=True)[:n]
    fn nlargest(#[varargs] args: &[Value], #[varkwargs] kwargs: Kwargs) -> Result<Value> {
        let [n, iterable, key] =
            bind_python_args("nlargest", &["n", "iterable", "key"], 2, args, &kwargs)?;
        n_extreme(
            _interp,
            &arg(&n),
            &arg(&iterable),
            &optional_or(&key, Value::none()),
            true,
        )
    }
}
