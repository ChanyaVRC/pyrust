/// Operation tag for set/frozenset binary operators.
#[derive(Clone, Copy)]
pub(super) enum SetOp {
    Or,  // union
    And, // intersection
    Sub, // difference
    Xor, // symmetric difference
}

/// True if `v` is a set-like dict view: `dict_keys` or `dict_items`
/// (issue #1891).  `dict_values` is deliberately *not* set-like.
pub(super) fn is_setlike_view(v: &Value) -> bool {
    matches!(
        pyrust_builtins::dict_views::view_kind(v),
        Some(
            pyrust_builtins::dict_views::DictViewKind::Keys
                | pyrust_builtins::dict_views::DictViewKind::Items
        )
    )
}

/// Extract a set's items and frozen flag from a value that is a `set`,
/// `frozenset`, or a `PyInstance` subclass backed by either.  Returns
/// `None` when the value is none of those.
pub(super) fn set_items_from_value(v: &Value) -> Option<(PySet, bool)> {
    if let ValueKind::Set(s) = v.kind() {
        return Some((s.clone(), false));
    }
    if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
        return Some(((*rc).clone(), true));
    }
    if let Some(backing) = builtin_data_backing(v) {
        return set_items_from_value(&backing);
    }
    None
}

/// Compute a binary set operation when both operands are set/frozenset (or
/// `PyInstance` subclasses thereof).  Returns `Set` if both backing stores are
/// mutable sets, otherwise `FrozenSet` (any frozenset operand promotes the
/// result, matching CPython).
///
/// Returns `None` when the left operand is not a set/frozenset (caller should
/// fall through to the next handler).  Returns `Some(Err(...))` when the left
/// operand is a set/frozenset but the right operand is not — CPython raises
/// `TypeError: unsupported operand type(s) for OP: 'X' and 'Y'` in that case.
/// True if the set holds any `PyKey::Object` element, i.e. a user instance
/// whose membership/equality requires `__hash__`/`__eq__` dispatch rather than
/// raw `IndexSet` identity comparison (issue #1907).  All-primitive sets take
/// the fast raw path.
pub(super) fn set_has_object_key(s: &PySet) -> bool {
    // Recurse into Tuple/FrozenSet element keys so a user object nested inside
    // a tuple key also forces the eq-aware path (issue #2059); a set of
    // primitive (or primitive-tuple) keys stays on the raw fast path.
    s.iter().any(key_contains_object)
}

/// Subtract `rhs` from a live mutable set backing in place.
///
/// This is the canonical implementation for `set -= other`, including set
/// subclasses after their backing set has been selected by expression
/// dispatch.  Primitive keys use a single `retain` pass: repeatedly calling
/// `IndexSet::shift_remove` preserves order by shifting the tail on every
/// removal and turns a full subtraction into O(n²).
///
/// Object keys need Python equality dispatch.  Probe a stable LHS snapshot so
/// the stored LHS key owns the `__eq__` call, matching CPython's
/// `stored_key == rhs_probe` direction.  Successful removals are accumulated
/// and applied in one linear retain pass.  If a later comparison raises, apply
/// the removals found before the error first; CPython exposes that partial
/// in-place progress.
pub(super) fn set_subtract_in_place(
    interp: &mut Interpreter,
    receiver: &Value,
    rhs: &PySet,
) -> Result<()> {
    if rhs.is_empty() {
        return Ok(());
    }

    let (lhs_is_empty, lhs_has_object) = receiver
        .set_with(|lhs| (lhs.is_empty(), set_has_object_key(lhs)))
        .ok_or_else(|| pyrust_core::PyError::Runtime("internal: expected set".to_string()))?;
    if lhs_is_empty {
        return Ok(());
    }

    if !lhs_has_object && !set_has_object_key(rhs) {
        receiver
            .set_with_mut(|lhs| lhs.retain(|key| !rhs.contains(key)))
            .ok_or_else(|| pyrust_core::PyError::Runtime("internal: expected set".to_string()))?;
        return Ok(());
    }

    let lhs_snapshot = receiver
        .set_with(|lhs| lhs.clone())
        .ok_or_else(|| pyrust_core::PyError::Runtime("internal: expected set".to_string()))?;
    let mut remove_mask = vec![false; lhs_snapshot.len()];
    let apply_removals = |receiver: &Value, remove_mask: &[bool]| -> Result<()> {
        let removals: PySet = lhs_snapshot
            .iter()
            .zip(remove_mask)
            .filter(|(_, remove)| **remove)
            .map(|(key, _)| key.clone())
            .collect();
        if removals.is_empty() {
            return Ok(());
        }
        receiver
            .set_with_mut(|lhs| lhs.retain(|key| !removals.contains(key)))
            .ok_or_else(|| pyrust_core::PyError::Runtime("internal: expected set".to_string()))?;
        Ok(())
    };

    for probe in rhs {
        match interp.set_lookup_in(&lhs_snapshot, probe) {
            Ok(Some(index)) => remove_mask[index] = true,
            Ok(None) => {}
            Err(err) => {
                apply_removals(receiver, &remove_mask)?;
                return Err(err);
            }
        }
    }
    apply_removals(receiver, &remove_mask)
}

/// Return whether a value that is about to become one set key transitively
/// needs interpreter-owned conversion or equality.
///
/// Only hashable aggregate shapes are descended into: tuple, frozenset and
/// slice.  Lists/dicts/sets nested as an element are unhashable and will fail
/// before equality is relevant, so deliberately not recursing into them also
/// means a self-referential list cannot make this scan recurse forever.
///
/// The descended shapes cannot form a cycle without first crossing a mutable
/// or user-object edge: tuple/frozenset/slice are immutable after construction,
/// and `PyInstance` is a terminal `true`.  A visited-set guard is therefore
/// unnecessary.
fn hashable_value_needs_runtime_key_semantics(v: &Value) -> bool {
    // Hashing owns concrete aggregate classification.  Its typed decision
    // includes both user-object dispatch and slices, whose key conversion is
    // interpreter-owned even when all three fields are primitive.
    if value_needs_slow_hash(v) {
        return true;
    }
    match v.kind() {
        ValueKind::PyInstance(_) => true,
        ValueKind::Tuple(items) => items.iter().any(hashable_value_needs_runtime_key_semantics),
        _ => {
            if let Some(items) = pyrust_builtins::frozenset::as_items(v) {
                return set_has_object_key(&items);
            }
            false
        }
    }
}

/// Cheap, borrow-only check for whether an iterable operand to a set-algebra
/// method form needs interpreter-owned key conversion/equality.  Used to keep
/// ordinary all-primitive operands on the fast path without materialising
/// `PyKey`s (issue #1907).  Conservatively returns `true` for user-object or
/// slice keys and for iterables whose contents cannot be cheaply inspected
/// (Generators, custom `BuiltinObject`s), so correctness is never sacrificed
/// for speed.
pub(super) fn value_iterable_needs_runtime_key_semantics(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Set(s) => set_has_object_key(&s),
        ValueKind::List(items) => items.iter().any(hashable_value_needs_runtime_key_semantics),
        ValueKind::Tuple(items) => items.iter().any(hashable_value_needs_runtime_key_semantics),
        ValueKind::Dict(d) => d.keys().any(key_contains_object),
        // Primitive flat iterables can never hold user instances.
        ValueKind::Str(_) | ValueKind::Bytes(_) | ValueKind::Range { .. } => false,
        _ => {
            if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
                set_has_object_key(&rc)
            } else {
                // Unknown / opaque iterable: be conservative and dispatch
                // `__eq__` (still correct for primitive elements — just slower).
                true
            }
        }
    }
}

/// Split the operands of `a & b` into `(scanned, probed)` the way CPython's
/// `set_intersection` does (issue #2955).
///
/// CPython swaps the two tables so the *smaller* one is walked, then inserts
/// the walked side's elements into the result.  With `__eq__`-equal but
/// distinguishable elements (`1 == 1.0 == True`, or user instances carrying a
/// payload) the surviving object is therefore the smaller operand's, and the
/// right operand wins ties.  Walking the smaller table is also the cheaper
/// direction, so this costs nothing.
pub(super) fn intersection_scan_order<'a>(a: &'a PySet, b: &'a PySet) -> (&'a PySet, &'a PySet) {
    if b.len() <= a.len() { (b, a) } else { (a, b) }
}

/// [`intersection_scan_order`] for [`set_binary_op_from_items`], with the
/// dict-view operators held at their existing left-to-right scan.
///
/// `d.keys() & other` is not `set.__and__`: CPython routes it through
/// `_PyDictView_Intersect`, whose own swap rule is stated in terms of the view
/// (even when the view is the *right* operand, since `set & view` reaches it
/// via `__rand__`).  Applying the plain-set rule here would change which object
/// a view intersection keeps in cases where the current behaviour already
/// matches CPython, so views keep scanning the left operand.
#[inline]
fn intersection_scan<'a>(a: &'a PySet, b: &'a PySet, view_operands: bool) -> (&'a PySet, &'a PySet) {
    if view_operands {
        (a, b)
    } else {
        intersection_scan_order(a, b)
    }
}

pub(super) fn set_binary_op(
    interp: &mut Interpreter,
    left: &Value,
    right: &Value,
    op: SetOp,
    op_sym: &str,
) -> Option<Result<Value>> {
    // CPython's dict-view set operators (`&`/`|`/`-`/`^`) accept *any* iterable
    // on the other side, not just a set — `d.keys() & ['a']`, `['a'] & d.keys()`,
    // `d.keys() | 'ab'`, `d.keys() - (g for g in …)` all work and return a plain
    // `set` (issue #1891).  Real `set`/`frozenset` operators keep the strict
    // "set operand required" rule, so only relax it when a view is involved.
    let view_involved = is_setlike_view(left) || is_setlike_view(right);
    if view_involved {
        let lhs_items = match interp.coerce_setop_operand(left, true) {
            Some(Ok(items)) => items,
            Some(Err(e)) => return Some(Err(e)),
            None => return None,
        };
        let rhs_items = match interp.coerce_setop_operand(right, true) {
            Some(Ok(items)) => items,
            Some(Err(e)) => return Some(Err(e)),
            None => return None,
        };
        return set_binary_op_from_items(interp, lhs_items, rhs_items, op, true);
    }
    // Fast path: both operands are plain `set` / `frozenset` (or `PyInstance`
    // subclasses backed by either) whose elements are all primitive (no
    // `PyKey::Object` user instances).  Borrow the backing `IndexSet`s in place
    // and clone only the elements that land in the result, instead of cloning
    // both whole operands up front (issue #1978).
    //
    // Sets with object keys take the eq-aware path below: there, user
    // `__hash__`/`__eq__` runs during the algebra and could re-enter and mutate
    // an operand, so we must not hold a live borrow of the backing `RefCell`
    // across it — the existing clone-then-compute path stays correct there.
    // Dict views and other set-like shapes also fall through.
    if let (Some((lhs_val, l_frozen)), Some((rhs_val, _r_frozen))) =
        (set_direct_value(left), set_direct_value(right))
    {
        let primitive = !with_set_items(&lhs_val, set_has_object_key)
            && !with_set_items(&rhs_val, set_has_object_key);
        if primitive {
            let out = with_set_items(&lhs_val, |a| {
                with_set_items(&rhs_val, |b| set_algebra_fast(a, b, op))
            });
            // Result type follows the LEFT operand (CPython 3.12): `set &
            // frozenset` → `set`, `frozenset & set` → `frozenset` (issue #2042).
            return Some(Ok(if l_frozen {
                pyrust_builtins::frozenset::frozenset(out)
            } else {
                Value::set(out)
            }));
        }
    }
    // LHS must be set-like (set/frozenset/subclass or a set-like dict view,
    // issue #1891); otherwise this isn't a set op and the caller falls through.
    let lhs_items = match interp.coerce_set_operand(left)? {
        Ok(items) => items,
        Err(e) => return Some(Err(e)),
    };
    // LHS is set-like; if RHS is not, emit the CPython-format TypeError.
    let rhs_items = match interp.coerce_set_operand(right) {
        Some(Ok(items)) => items,
        Some(Err(e)) => return Some(Err(e)),
        None => {
            // Only `|` delegates to dict's PEP 584 slots, so a mappingproxy
            // operand reports as `dict` for `|` but keeps its own name for the
            // set-only operators `&` / `-` / `^` (CPython 3.12).
            let (lt, rt) = if op_sym == "|" {
                (
                    bitor_operand_type_name(left),
                    bitor_operand_type_name(right),
                )
            } else {
                (value_type_name_str(left), value_type_name_str(right))
            };
            return Some(Err(pyrust_core::type_err!(
                "unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"
            )));
        }
    };
    set_binary_op_from_items(interp, lhs_items, rhs_items, op, false)
}

/// Shared set-algebra core for [`set_binary_op`].  Computes `lhs OP rhs` over
/// already-coerced `(PySet, frozen)` operands and packages the result.
///
/// `force_set` forces a plain `set` result regardless of the operands' frozen
/// flags — used for dict-view operators, which always return `set` (issue
/// #1891); otherwise the result type follows the LEFT operand (issue #2042).
fn set_binary_op_from_items(
    interp: &mut Interpreter,
    lhs_items: (PySet, bool),
    rhs_items: (PySet, bool),
    op: SetOp,
    force_set: bool,
) -> Option<Result<Value>> {
    let (a, l_frozen) = lhs_items;
    // RHS frozen-ness is irrelevant: the result type follows the LEFT operand
    // (issue #2042).
    let (b, _r_frozen) = rhs_items;
    // Fast path: neither operand contains user-instance keys, so raw
    // `IndexSet` identity comparison is exact (issue #1907).  Most sets are
    // primitive, so keep this allocation-cheap path with no `__eq__` dispatch.
    let needs_eq = set_has_object_key(&a) || set_has_object_key(&b);
    let result: Result<PySet> = if !needs_eq {
        let mut out: PySet = PySet::default();
        match op {
            SetOp::Or => {
                for k in a.iter().chain(b.iter()) {
                    out.insert(k.clone());
                }
            }
            SetOp::And => {
                let (scan, probe) = intersection_scan(&a, &b, force_set);
                for k in scan.iter() {
                    if probe.contains(k) {
                        out.insert(k.clone());
                    }
                }
            }
            SetOp::Sub => {
                for k in a.iter() {
                    if !b.contains(k) {
                        out.insert(k.clone());
                    }
                }
            }
            SetOp::Xor => {
                for k in a.iter() {
                    if !b.contains(k) {
                        out.insert(k.clone());
                    }
                }
                for k in b.iter() {
                    if !a.contains(k) {
                        out.insert(k.clone());
                    }
                }
            }
        }
        Ok(out)
    } else {
        // Slow path: at least one operand holds user instances.  Membership
        // (`contains`) and insertion (`insert`) go through `set_lookup_in` /
        // `set_insert`, which dispatch user `__hash__`-then-`__eq__`.
        (|| -> Result<PySet> {
            let mut out: PySet = PySet::default();
            match op {
                SetOp::Or => {
                    for k in a.iter().chain(b.iter()) {
                        interp.set_insert(&mut out, k.clone())?;
                    }
                }
                SetOp::And => {
                    let (scan, probe) = intersection_scan(&a, &b, force_set);
                    for k in scan.iter() {
                        if interp.set_lookup_in(probe, k)?.is_some() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                }
                SetOp::Sub => {
                    for k in a.iter() {
                        if interp.set_lookup_in(&b, k)?.is_none() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                }
                SetOp::Xor => {
                    for k in a.iter() {
                        if interp.set_lookup_in(&b, k)?.is_none() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                    for k in b.iter() {
                        if interp.set_lookup_in(&a, k)?.is_none() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                }
            }
            Ok(out)
        })()
    };
    let out = match result {
        Ok(out) => out,
        Err(e) => return Some(Err(e)),
    };
    // Result type follows the LEFT operand (CPython 3.12, issue #2042): `set &
    // frozenset` → `set`, `frozenset & set` → `frozenset`. The RHS frozen-ness
    // never affects the result type. Dict-view operators always yield `set`
    // (`force_set`).
    Some(Ok(if l_frozen && !force_set {
        pyrust_builtins::frozenset::frozenset(out)
    } else {
        Value::set(out)
    }))
}

/// Set/frozenset subset-relation comparison.
///
/// Returns `Some(Ok(bool))` when both `left` and `right` are set/frozenset
/// (or subclasses thereof), `None` otherwise (caller should fall through to
/// a TypeError).
///
/// Semantics match CPython 3.12:
/// - `a < b`  — proper subset: every element of `a` is in `b` and `a != b`
/// - `a <= b` — subset: every element of `a` is in `b`
/// - `a > b`  — proper superset: every element of `b` is in `a` and `a != b`
/// - `a >= b` — superset: every element of `b` is in `a`
///
/// Mixed `set`/`frozenset` comparisons are supported, as in CPython.
pub(super) fn set_subset_cmp(
    interp: &mut Interpreter,
    left: &Value,
    right: &Value,
    op: BinaryOp,
) -> Option<Result<Value>> {
    // Both operands must be set-like (set/frozenset/subclass or a set-like dict
    // view, issue #1891); otherwise fall through to the normal comparison path
    // so it raises the `'<=' not supported between …` TypeError.
    let (a, _) = match interp.coerce_set_operand(left)? {
        Ok(items) => items,
        Err(e) => return Some(Err(e)),
    };
    let (b, _) = match interp.coerce_set_operand(right)? {
        Ok(items) => items,
        Err(e) => return Some(Err(e)),
    };
    // Fast path: all-primitive operands — raw `contains` is exact (issue #1907).
    let needs_eq = set_has_object_key(&a) || set_has_object_key(&b);
    let (is_subset, is_superset) = if !needs_eq {
        (
            a.iter().all(|k| b.contains(k)),
            b.iter().all(|k| a.contains(k)),
        )
    } else {
        // Slow path: membership via `set_lookup_in` so user `__eq__` decides.
        let compute = (|| -> Result<(bool, bool)> {
            let mut subset = true;
            for k in a.iter() {
                if interp.set_lookup_in(&b, k)?.is_none() {
                    subset = false;
                    break;
                }
            }
            let mut superset = true;
            for k in b.iter() {
                if interp.set_lookup_in(&a, k)?.is_none() {
                    superset = false;
                    break;
                }
            }
            Ok((subset, superset))
        })();
        match compute {
            Ok(pair) => pair,
            Err(e) => return Some(Err(e)),
        }
    };
    let result = match op {
        BinaryOp::Lt => is_subset && !is_superset,
        BinaryOp::Le => is_subset,
        BinaryOp::Gt => is_superset && !is_subset,
        BinaryOp::Ge => is_superset,
        _ => unreachable!("set_subset_cmp called with non-comparison op"),
    };
    Some(Ok(Value::bool_(result)))
}
