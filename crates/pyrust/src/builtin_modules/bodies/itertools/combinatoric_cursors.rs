//! Rust-owned cursor state for itertools' positional combinatorics.
//!
//! Pools are materialised exactly once by the public constructors in the
//! parent module. Mutable indices remain behind one opaque builtin value for
//! the iterator's lifetime, so `__next__` never decodes or re-encodes Python
//! lists. All potentially large native allocations are fallible.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::value::{PyInstance, Value, ValueKind};
use pyrust_core::BuiltinTypeOps;

fn memory_error() -> PyError {
    PyError::named("MemoryError", String::new())
}

fn repeat_too_large() -> PyError {
    PyError::named("OverflowError", "repeat argument too large".to_string())
}

fn internal(fn_name: &str) -> PyError {
    PyError::Runtime(format!("internal: {fn_name}() instance state corrupted"))
}

fn try_value_vec(capacity: usize) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| memory_error())?;
    Ok(values)
}

fn try_usize_vec(capacity: usize) -> Result<Vec<usize>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| memory_error())?;
    Ok(values)
}

/// Reserve the repeated product pool before any user iterable is consumed.
/// `checked_mul` handles dimension-count overflow; `try_reserve_exact` turns
/// capacity overflow/allocation failure into a Python `MemoryError`.
pub(super) fn reserve_product_pools(
    distinct_pool_count: usize,
    repeat: usize,
) -> Result<Vec<Value>> {
    let total = distinct_pool_count
        .checked_mul(repeat)
        .ok_or_else(repeat_too_large)?;
    // CPython stores one PyObject pointer per repeated pool and reports this
    // representational overflow separately from an allocator failure.
    if total > isize::MAX as usize / std::mem::size_of::<*const ()>() {
        return Err(repeat_too_large());
    }
    try_value_vec(total)
}

/// Fallibly reserve storage used while materialising each distinct product
/// input once.
pub(super) fn reserve_distinct_pools(distinct_pool_count: usize) -> Result<Vec<Value>> {
    try_value_vec(distinct_pool_count)
}

enum Cursor {
    Product(ProductCursor),
    Combinations(CombinationsCursor),
    Permutations(PermutationsCursor),
}

struct CursorOps;
const CURSOR_OPS: &CursorOps = &CursorOps;

impl BuiltinTypeOps for CursorOps {
    fn type_name(&self) -> &'static str {
        "_itertools_combinatoric_cursor"
    }
}

fn cursor_value(cursor: Cursor) -> Value {
    Value::builtin_object(CURSOR_OPS, Box::new(cursor))
}

fn with_cursor<R>(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
    action: impl FnOnce(&mut Cursor) -> Result<R>,
) -> Result<R> {
    let cursor_value = inst
        .borrow()
        .attrs
        .get("_cursor")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let ValueKind::BuiltinObject { ops, state } = cursor_value.kind() else {
        return Err(internal(fn_name));
    };
    if !pyrust_core::builtin_ops_is::<CursorOps>(ops) {
        return Err(internal(fn_name));
    }
    let mut state = state.borrow_mut();
    let cursor = state
        .downcast_mut::<Cursor>()
        .ok_or_else(|| internal(fn_name))?;
    action(cursor)
}

struct ProductCursor {
    pools: Vec<Value>,
    indices: Vec<usize>,
    started: bool,
    exhausted: bool,
}

impl ProductCursor {
    fn try_new(pools: Vec<Value>) -> Result<Self> {
        let exhausted = pools.iter().any(|pool| match pool.kind() {
            ValueKind::List(items) => items.is_empty(),
            _ => false,
        });
        let mut indices = if exhausted {
            Vec::new()
        } else {
            try_usize_vec(pools.len())?
        };
        if !exhausted {
            indices.resize(pools.len(), 0);
        }
        Ok(Self {
            pools,
            indices,
            started: false,
            exhausted,
        })
    }

    fn next_tuple(&mut self, fn_name: &str) -> Result<Option<Vec<Value>>> {
        if self.exhausted {
            return Ok(None);
        }
        if self.started {
            let mut position = self.pools.len();
            loop {
                if position == 0 {
                    self.exhausted = true;
                    return Ok(None);
                }
                position -= 1;
                let pool_len = match self.pools[position].kind() {
                    ValueKind::List(items) => items.len(),
                    _ => return Err(internal(fn_name)),
                };
                self.indices[position] += 1;
                if self.indices[position] < pool_len {
                    break;
                }
                self.indices[position] = 0;
            }
        } else {
            self.started = true;
        }

        let mut tuple = try_value_vec(self.indices.len())?;
        for (pool, &index) in self.pools.iter().zip(&self.indices) {
            let ValueKind::List(items) = pool.kind() else {
                return Err(internal(fn_name));
            };
            tuple.push(items.get(index).cloned().ok_or_else(|| internal(fn_name))?);
        }
        Ok(Some(tuple))
    }
}

pub(super) fn product_cursor_value(pools: Vec<Value>) -> Result<Value> {
    Ok(cursor_value(Cursor::Product(ProductCursor::try_new(
        pools,
    )?)))
}

pub(super) fn next_product(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<Option<Vec<Value>>> {
    with_cursor(inst, fn_name, |cursor| match cursor {
        Cursor::Product(cursor) => cursor.next_tuple(fn_name),
        _ => Err(internal(fn_name)),
    })
}

struct CombinationsCursor {
    pool: Vec<Value>,
    indices: Vec<usize>,
    with_replacement: bool,
    started: bool,
    exhausted: bool,
}

impl CombinationsCursor {
    fn try_new(pool: Vec<Value>, r: usize, with_replacement: bool) -> Result<Self> {
        let pool_len = pool.len();
        let exhausted = if with_replacement {
            pool_len == 0 && r > 0
        } else {
            r > pool_len
        };

        // An already-empty iterator never reads indices. In particular,
        // combinations([x], huge_r) and CWR([], huge_r) stay O(1) after the
        // one required pool materialisation.
        let mut indices = if exhausted || r == 0 {
            Vec::new()
        } else {
            try_usize_vec(r)?
        };
        if !exhausted && r > 0 {
            if with_replacement {
                indices.resize(r, 0);
            } else {
                indices.extend(0..r);
            }
        }
        Ok(Self {
            pool,
            indices,
            with_replacement,
            started: false,
            exhausted,
        })
    }

    fn next_tuple(&mut self, fn_name: &str) -> Result<Option<Vec<Value>>> {
        if self.exhausted {
            return Ok(None);
        }
        let r = self.indices.len();
        if self.started {
            if r == 0 {
                self.exhausted = true;
                return Ok(None);
            }
            let n = self.pool.len();
            let mut position = r;
            loop {
                if position == 0 {
                    self.exhausted = true;
                    return Ok(None);
                }
                position -= 1;
                let max_value = if self.with_replacement {
                    n - 1
                } else {
                    n - r + position
                };
                if self.indices[position] < max_value {
                    self.indices[position] += 1;
                    for index in (position + 1)..r {
                        self.indices[index] = if self.with_replacement {
                            self.indices[position]
                        } else {
                            self.indices[index - 1] + 1
                        };
                    }
                    break;
                }
            }
        } else {
            self.started = true;
        }
        tuple_from_pool(&self.pool, &self.indices, fn_name).map(Some)
    }
}

pub(super) fn combinations_cursor_value(
    pool: Vec<Value>,
    r: usize,
    with_replacement: bool,
) -> Result<Value> {
    Ok(cursor_value(Cursor::Combinations(
        CombinationsCursor::try_new(pool, r, with_replacement)?,
    )))
}

pub(super) fn next_combinations(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
    with_replacement: bool,
) -> Result<Option<Vec<Value>>> {
    with_cursor(inst, fn_name, |cursor| match cursor {
        Cursor::Combinations(cursor) if cursor.with_replacement == with_replacement => {
            cursor.next_tuple(fn_name)
        }
        _ => Err(internal(fn_name)),
    })
}

struct PermutationsCursor {
    pool: Vec<Value>,
    r: usize,
    indices: Vec<usize>,
    cycles: Vec<usize>,
    started: bool,
    exhausted: bool,
}

impl PermutationsCursor {
    fn try_new(pool: Vec<Value>, r: usize) -> Result<Self> {
        let exhausted = r > pool.len();
        let mut indices = if exhausted || r == 0 {
            Vec::new()
        } else {
            try_usize_vec(pool.len())?
        };
        let mut cycles = if exhausted || r == 0 {
            Vec::new()
        } else {
            try_usize_vec(r)?
        };
        if !exhausted && r > 0 {
            indices.extend(0..pool.len());
            cycles.extend((0..r).map(|index| pool.len() - index));
        }
        Ok(Self {
            pool,
            r,
            indices,
            cycles,
            started: false,
            exhausted,
        })
    }

    fn next_tuple(&mut self, fn_name: &str) -> Result<Option<Vec<Value>>> {
        if self.exhausted {
            return Ok(None);
        }
        if !self.started {
            self.started = true;
            return tuple_from_pool(&self.pool, &self.indices[..self.r], fn_name).map(Some);
        }

        let n = self.pool.len();
        for position in (0..self.r).rev() {
            self.cycles[position] -= 1;
            if self.cycles[position] == 0 {
                self.indices[position..].rotate_left(1);
                self.cycles[position] = n - position;
            } else {
                let swap_index = n - self.cycles[position];
                self.indices.swap(position, swap_index);
                return tuple_from_pool(&self.pool, &self.indices[..self.r], fn_name).map(Some);
            }
        }

        self.exhausted = true;
        Ok(None)
    }
}

pub(super) fn permutations_cursor_value(pool: Vec<Value>, r: usize) -> Result<Value> {
    Ok(cursor_value(Cursor::Permutations(
        PermutationsCursor::try_new(pool, r)?,
    )))
}

pub(super) fn next_permutations(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<Option<Vec<Value>>> {
    with_cursor(inst, fn_name, |cursor| match cursor {
        Cursor::Permutations(cursor) => cursor.next_tuple(fn_name),
        _ => Err(internal(fn_name)),
    })
}

fn tuple_from_pool(pool: &[Value], indices: &[usize], fn_name: &str) -> Result<Vec<Value>> {
    let mut tuple = try_value_vec(indices.len())?;
    for &index in indices {
        tuple.push(pool.get(index).cloned().ok_or_else(|| internal(fn_name))?);
    }
    Ok(tuple)
}
