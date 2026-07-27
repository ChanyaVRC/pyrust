// Cache storage, wrapper construction, counters, and CacheInfo creation.

fn lru_cache_value(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Value> {
    let cache = inst
        .borrow()
        .attrs
        .get("_cache")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    if !cache.is_dict() {
        return Err(internal(fn_name));
    }
    Ok(cache)
}

fn decode_cache_entry(
    entry: Value,
    bounded: bool,
    fn_name: &str,
) -> Result<(Value, Option<usize>)> {
    if !bounded {
        return Ok((entry, None));
    }
    let ValueKind::Tuple(items) = entry.kind() else {
        return Err(internal(fn_name));
    };
    if items.len() != 2 {
        return Err(internal(fn_name));
    }
    let node_id = match items[1].kind() {
        ValueKind::Int(index) if index >= 0 => index as usize,
        _ => return Err(internal(fn_name)),
    };
    Ok((items[0].clone(), Some(node_id)))
}

/// Insert a miss result and, for a bounded cache, evict the LRU entry.
///
/// The wrapped function may recursively call the wrapper or clear its cache,
/// so the live cache is re-fetched and the key rechecked before mutation.
fn insert_cache(
    interp: &mut Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    key: PyKey,
    value: Value,
    maxsize: Option<i64>,
    fn_name: &str,
) -> Result<()> {
    let cache = lru_cache_value(inst, fn_name)?;
    if interp.dict_lookup(&cache, &key)?.is_some() {
        return Ok(());
    }

    let Some(maxsize) = maxsize else {
        cache
            .dict_with_mut(|dict| {
                dict.insert(key, value);
            })
            .ok_or_else(|| internal(fn_name))?;
        return Ok(());
    };
    debug_assert!(maxsize > 0);

    let current_len = cache.as_dict().map(|dict| dict.len()).unwrap_or(0);
    if current_len >= maxsize as usize {
        let evicted = with_lru_links(inst, fn_name, LruLinks::pop_lru)?;
        if let Some(evicted) = evicted {
            cache
                .dict_with_mut(|dict| {
                    // Cache order is private; swap removal keeps eviction O(1).
                    dict.swap_remove(&evicted);
                })
                .ok_or_else(|| internal(fn_name))?;
        }
    }

    let node_id = with_lru_links(inst, fn_name, |links| links.insert_mru(key.clone()))?;
    let entry = Value::tuple(vec![value, Value::int(node_id as i64)]);
    cache
        .dict_with_mut(|dict| {
            dict.insert(key, entry);
        })
        .ok_or_else(|| internal(fn_name))?;
    Ok(())
}

/// Construct a cache wrapper with its private state initialized.
fn make_lru_wrapper(
    wrapper_class: Rc<RefCell<PyClass>>,
    cache_info_class: Value,
    func: Value,
    maxsize: Option<i64>,
    typed: bool,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__wrapped__", func.clone());
    attrs.insert("_func", func);
    attrs.insert("_maxsize", maxsize.map_or_else(Value::none, Value::int));
    attrs.insert("_typed", Value::bool_(typed));
    attrs.insert("_cache", Value::dict(PyDict::default()));
    attrs.insert("_links", lru_links_value());
    attrs.insert("_hits", Value::int(0));
    attrs.insert("_misses", Value::int(0));
    attrs.insert("_cache_info_class", cache_info_class);
    make_instance_with_class(wrapper_class, "_lru_cache_wrapper", attrs)
}

/// Construct the decorator returned by `lru_cache(maxsize=N)`.
fn make_lru_factory(
    generation: &Value,
    factory_class: Rc<RefCell<PyClass>>,
    maxsize: Option<i64>,
    typed: bool,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_maxsize", maxsize.map_or_else(Value::none, Value::int));
    attrs.insert("_typed", Value::bool_(typed));
    attrs.insert("_generation", generation.clone());
    make_instance_with_class(factory_class, "_lru_cache_factory", attrs)
}

fn bump_counter(inst: &Rc<RefCell<PyInstance>>, key: &str) {
    let mut borrow = inst.borrow_mut();
    let current = match borrow.attrs.get(key).map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => value,
        _ => 0,
    };
    borrow
        .attrs
        .insert(key, Value::int(current.wrapping_add(1)));
}

fn counter_value(attrs: &InstanceAttrs, key: &str) -> i64 {
    match attrs.get(key).map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => value,
        _ => 0,
    }
}

/// Build a `CacheInfo(hits, misses, maxsize, currsize)` tuple subclass.
fn make_cache_info(
    interp: &mut Interpreter,
    class: Value,
    hits: i64,
    misses: i64,
    maxsize: Value,
    currsize: i64,
) -> Result<Value> {
    interp.call_function_expanded(
        class,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::int(hits),
            },
            ExpandedCallArg {
                name: None,
                value: Value::int(misses),
            },
            ExpandedCallArg {
                name: None,
                value: maxsize,
            },
            ExpandedCallArg {
                name: None,
                value: Value::int(currsize),
            },
        ],
    )
}
