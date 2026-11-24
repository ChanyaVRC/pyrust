// Live collection backing accessors and ordered-mapping guards.

enum LiveCollectionRef<'a> {
    Dict(&'a pyrust_core::PyDict),
    Set(&'a pyrust_core::PySet),
}

/// Borrow the underlying `PyDict` or `PySet` of any guarded collection carrier
/// (plain dict/set, builtin subclass via `__builtin_data__`, live dict view,
/// or dict-backed mappingproxy).
///
/// Each accessor holds a `RefCell` read guard only for the duration of the
/// callback and drops it before any element is yielded or dispatched.
#[inline(always)]
fn with_live_collection<R>(
    container: &Value,
    mut visit: impl FnMut(LiveCollectionRef<'_>) -> R,
) -> Option<R> {
    if let Some(result) = container.dict_with(|dict| visit(LiveCollectionRef::Dict(dict))) {
        return Some(result);
    }
    if let Some(result) = container.set_with(|set| visit(LiveCollectionRef::Set(set))) {
        return Some(result);
    }
    // dict/set-subclass instances (including provider-tagged ordered mappings
    // and primitive set subclasses): re-resolve `__builtin_data__` each step so
    // mutations rewriting the backing in place (`store_backing`, #2447) or
    // replacing the whole `Value` are both observed.
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(result) = backing.dict_with(|dict| visit(LiveCollectionRef::Dict(dict))) {
            return Some(result);
        }
        if let Some(result) = backing.set_with(|set| visit(LiveCollectionRef::Set(set))) {
            return Some(result);
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(visit(LiveCollectionRef::Dict(&dict.borrow())));
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .map(|dict| visit(LiveCollectionRef::Dict(&dict.borrow())))
}

/// Borrow the underlying `PyDict` of any dict-like guarded carrier.
#[inline(always)]
fn with_live_dict<R>(
    container: &Value,
    mut on_dict: impl FnMut(&pyrust_core::PyDict) -> R,
) -> Option<R> {
    if let Some(result) = container.dict_with(&mut on_dict) {
        return Some(result);
    }
    if let Some(backing) = builtin_data_backing(container)
        && let Some(result) = backing.dict_with(&mut on_dict)
    {
        return Some(result);
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(on_dict(&dict.borrow()));
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container).map(|dict| on_dict(&dict.borrow()))
}

/// Read the live element count of a `dict` / `set` / dict-view `container`,
/// used by the size-mutation guard (#1988). Returns `None` if `container` is
/// not one of those types (in which case the guard treats the snapshot as
/// non-mutating, since no other guarded source reaches this path).
pub(crate) fn live_collection_len(container: &Value) -> Option<usize> {
    if let Some(len) = with_live_collection(container, |coll| match coll {
        LiveCollectionRef::Dict(dict) => dict.len(),
        LiveCollectionRef::Set(set) => set.len(),
    }) {
        return Some(len);
    }
    // mappingproxy (issue #2728): class-backed (`vars(C)`) proxies are not
    // dict-Rc backed, so fall back to `mapping_proxy::live_len`.
    pyrust_builtins::mapping_proxy::live_len(container)
}

/// Register a shared mutation generation for the live backing store reached by
/// `container`. Dict views and mapping proxies use the same generation as
/// their source dict, while builtin subclasses resolve `__builtin_data__`.
pub(crate) fn live_collection_mutation_state(
    container: &Value,
) -> Option<pyrust_core::CollectionMutationState> {
    if let Some(state) = container.dict_iteration_mutation_state() {
        return Some(state);
    }
    if let Some(state) = container.set_iteration_mutation_state() {
        return Some(state);
    }
    if let Some(backing) = builtin_data_backing(container) {
        if let Some(state) = backing.dict_iteration_mutation_state() {
            return Some(state);
        }
        if let Some(state) = backing.set_iteration_mutation_state() {
            return Some(state);
        }
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(pyrust_core::dict_iteration_mutation_state(&dict));
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
        .map(|dict| pyrust_core::dict_iteration_mutation_state(&dict))
}

fn live_collection_key_at(container: &Value, index: usize) -> Option<PyKey> {
    with_live_collection(container, |coll| match coll {
        LiveCollectionRef::Dict(dict) => dict.get_index(index).map(|(key, _)| key.clone()),
        LiveCollectionRef::Set(set) => set.get_index(index).cloned(),
    })
    .flatten()
}

fn live_collection_keys(container: &Value) -> Option<Vec<PyKey>> {
    let mut keys = Vec::new();
    extend_live_collection_keys(container, &mut keys).then_some(keys)
}

/// Append the live key order of `container` to `keys`.
///
/// Written as an append so a cursor can refill a recycled buffer instead of
/// allocating one per loop entry.
fn extend_live_collection_keys(container: &Value, keys: &mut Vec<PyKey>) -> bool {
    with_live_collection(container, |coll| match coll {
        LiveCollectionRef::Dict(dict) => keys.extend(dict.keys().cloned()),
        LiveCollectionRef::Set(set) => keys.extend(set.iter().cloned()),
    })
    .is_some()
}

fn live_collection_key_index(container: &Value, key: &PyKey) -> Option<usize> {
    with_live_collection(container, |coll| match coll {
        LiveCollectionRef::Dict(dict) => dict.get_index_of(key),
        LiveCollectionRef::Set(set) => set.get_index_of(key),
    })
    .flatten()
}

fn live_dict_value(container: &Value, key: &PyKey) -> Option<Value> {
    with_live_dict(container, |dict| dict.get(key).cloned()).flatten()
}

/// Resolve the backing mapping a live cursor may read positionally.
///
/// Only containers whose backing identity is fixed qualify: a builtin subclass
/// can replace `__builtin_data__` under an active iterator, so those cursors
/// keep re-probing the carrier on every step.
pub(crate) fn stable_cursor_backing(
    container: &Value,
    dynamic_backing: bool,
) -> Option<pyrust_builtins::dict_views::DictRc> {
    if dynamic_backing {
        return None;
    }
    if let Some(dict) = container.get_dict_rc() {
        return Some(Rc::clone(dict));
    }
    if let Some(dict) = pyrust_builtins::dict_views::as_dict_rc(container) {
        return Some(dict);
    }
    pyrust_builtins::mapping_proxy::as_dict_rc(container)
}

/// Resolve the backing identity required by a provider-supplied ordered
/// mapping guard. Generic iteration knows only the supported storage shapes.
fn ordered_mapping_backing_id(container: &Value) -> Option<i64> {
    if container.is_dict() {
        return container.value_id();
    }
    if let Some(backing) = builtin_data_backing(container) {
        return backing.value_id();
    }
    pyrust_builtins::dict_views::as_dict_rc(container).map(|rc| Rc::as_ptr(&rc) as i64)
}

pub(crate) fn ordered_mapping_guard_outcome(
    container: &Value,
    recorded_len: usize,
    watch: &OrderedIterationWatch,
) -> pyrust_builtins::ordered_mapping::GuardOutcome {
    pyrust_builtins::ordered_mapping::guard_outcome(
        ordered_mapping_backing_id(container),
        recorded_len,
        live_collection_len(container),
        watch,
    )
}

/// The provider-owned guard state a tagged ordered-mapping iterator carries
/// for life. Named here so consumers — including the fast-path domain, which
/// must not reach into the builtins crate — see one typed decision surface.
pub(crate) type OrderedIterationWatch = pyrust_builtins::ordered_mapping::IterationWatch;

/// Snapshot the provider-owned guard state a new ordered-mapping iterator
/// carries, registering it against the backing mapping so entry-order
/// relinking is tracked for as long as the iterator lives (#2931).
///
/// Registration is what makes the generation advance at all, so holding it
/// for the guard's lifetime is load-bearing rather than incidental — and it
/// resolves every carrier shape (the mapping, a builtin-subclass instance's
/// `__builtin_data__`, a live dict view) to the one backing store the clear
/// registry is also keyed by.
pub(crate) fn ordered_iteration_watch(container: &Value) -> OrderedIterationWatch {
    pyrust_builtins::ordered_mapping::watch_iteration(live_collection_mutation_state(container))
}
