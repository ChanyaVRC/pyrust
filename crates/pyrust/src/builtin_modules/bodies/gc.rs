// `gc` module — 100% native Rust implementation of Python 3.12's `gc`
// (garbage collector interface) module (issue #3246).
//
// Reference: <https://docs.python.org/3.12/library/gc.html>

use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, key_to_value, object_class_singleton};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const DEBUG_STATS: i64 = 1;
const DEBUG_COLLECTABLE: i64 = 2;
const DEBUG_UNCOLLECTABLE: i64 = 4;
const DEBUG_SAVEALL: i64 = 32;
const DEBUG_LEAK: i64 = DEBUG_COLLECTABLE | DEBUG_UNCOLLECTABLE | DEBUG_SAVEALL;

/// Maximum number of live objects walked by `gc.get_objects()` /
/// `gc.get_referrers()` to guarantee bounded traversal time.
const MAX_GC_WALK_OBJECTS: usize = 50_000;

/// Bind the per-interpreter mutable `garbage` and `callbacks` lists onto a
/// freshly imported `gc` module instance.
pub(crate) fn prepare_module(interp: &Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let (garbage, callbacks) = {
        let state = interp.features().gc();
        (state.garbage.clone(), state.callbacks.clone())
    };
    let mut m = mod_rc.borrow_mut();
    m.insert_attr("garbage".to_string(), garbage);
    m.insert_attr("callbacks".to_string(), callbacks);
}

/// Return `true` when `value` matches CPython 3.12's `gc.is_tracked(obj)`.
fn is_tracked_value(value: &Value) -> bool {
    match value.kind() {
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Range { .. }
        | ValueKind::BigRange { .. }
        | ValueKind::NotImplemented
        | ValueKind::Ellipsis
        | ValueKind::BuiltinFunction(_) => false,
        ValueKind::Tuple(items) => !items.is_empty(),
        ValueKind::Dict(d) => !d.is_empty(),
        ValueKind::List(_)
        | ValueKind::Set(_)
        | ValueKind::UserFunction(_)
        | ValueKind::PyClass(_)
        | ValueKind::PyModule(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::SuperProxy { .. }
        | ValueKind::SuperProxyClass { .. }
        | ValueKind::SuperProxyUnbound { .. }
        | ValueKind::Generator(_) => true,
        ValueKind::PyInstance(inst) => {
            let obj_cls = object_class_singleton();
            !Rc::ptr_eq(&inst.borrow().class, &obj_cls)
        }
        ValueKind::BuiltinObject { .. } => {
            pyrust_builtins::frozenset::as_items(value).is_some()
                || pyrust_builtins::frame::is_frame(value)
                || pyrust_builtins::code::is_code(value)
        }
    }
}

/// Collect the direct Python referents of `value` into `out`.
fn collect_direct_referents(value: &Value, out: &mut Vec<Value>) {
    match value.kind() {
        ValueKind::List(items) => {
            for item in items.iter().rev() {
                out.push(item.clone());
            }
        }
        ValueKind::Tuple(items) => {
            for item in items.iter().rev() {
                out.push(item.clone());
            }
        }
        ValueKind::Dict(dict) => {
            for (k, v) in dict.iter() {
                out.push(v.clone());
                out.push(key_to_value(k.clone()));
            }
        }
        ValueKind::Set(set) => {
            for k in set.iter() {
                out.push(key_to_value(k.clone()));
            }
        }
        ValueKind::PyInstance(inst) => {
            let b = inst.borrow();
            out.push(Value::py_class(Rc::clone(&b.class)));
            if let Some(live_dict) = b.attrs.dict_ref() {
                out.push(live_dict.clone());
            } else {
                for (_, v) in b.attrs.items_snapshot() {
                    out.push(v);
                }
            }
            if let Some(bd) = b.attrs.builtin_data() {
                out.push(bd.clone());
            }
        }
        ValueKind::PyClass(cls) => {
            let b = cls.borrow();
            if let Some(ref base) = b.base {
                out.push(Value::py_class(Rc::clone(base)));
            }
            for extra in &b.extra_bases {
                out.push(Value::py_class(Rc::clone(extra)));
            }
            for (_, v) in &b.attrs {
                out.push(v.clone());
            }
        }
        ValueKind::PyModule(m) => {
            let b = m.borrow();
            if let Some(live_dict) = b.live_namespace() {
                out.push(live_dict);
            } else {
                // `attrs_snapshot` also yields deferred (not yet materialized)
                // attributes such as lazily-built `builtins` classes.
                for (_, v) in b.attrs_snapshot() {
                    out.push(v);
                }
            }
        }
        ValueKind::UserFunction(f) => {
            let defs = f.defaults_value();
            if !defs.is_none() {
                out.push(defs);
            }
            let kwdefs = f.kwdefaults_value();
            if !kwdefs.is_none() {
                out.push(kwdefs);
            }
        }
        ValueKind::BoundMethod { function, receiver } => {
            out.push(Value::user_function(Rc::clone(function)));
            out.push(Value::py_instance(Rc::clone(receiver)));
        }
        ValueKind::ClassBoundMethod { function, class } => {
            out.push(Value::user_function(Rc::clone(function)));
            out.push(Value::py_class(Rc::clone(class)));
        }
        ValueKind::SuperProxy { class, instance } => {
            out.push(Value::py_class(Rc::clone(class)));
            out.push(Value::py_instance(Rc::clone(instance)));
        }
        ValueKind::SuperProxyClass { class, obj_class } => {
            out.push(Value::py_class(Rc::clone(class)));
            out.push(Value::py_class(Rc::clone(obj_class)));
        }
        ValueKind::SuperProxyUnbound { class } => {
            out.push(Value::py_class(Rc::clone(class)));
        }
        ValueKind::BuiltinObject { .. } => {
            if let Some(items) = pyrust_builtins::frozenset::as_items(value) {
                for k in items.iter() {
                    out.push(key_to_value(k.clone()));
                }
            }
        }
        _ => {}
    }
}

/// Walk reachable live tracked objects from interpreter roots (module globals,
/// active VM frames, module cache, and `sys.modules`).
fn collect_live_tracked_objects(interp: &Interpreter) -> Vec<Value> {
    let mut visited: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<Value> = VecDeque::new();
    let mut tracked: Vec<Value> = Vec::new();

    let enqueue = |val: Value, visited: &mut HashSet<i64>, queue: &mut VecDeque<Value>| {
        if val.is_unset() {
            return;
        }
        if !is_tracked_value(&val) {
            return;
        }
        let oid = val.object_id().as_int().unwrap_or(0);
        if oid != 0 && visited.insert(oid) {
            queue.push_back(val);
        }
    };

    // 1. Active VM frame registers.
    for view in &interp.vm_frame_views {
        // SAFETY: active frame views on `vm_frame_views` point to live register slices.
        let regs = unsafe { std::slice::from_raw_parts(view.regs_ptr.as_ptr(), view.regs_len) };
        for v in regs {
            if !v.is_unset() {
                enqueue(v.clone(), &mut visited, &mut queue);
            }
        }
    }

    // 2. Interpreter environment chain.
    let mut env_cursor = Some(Rc::clone(&interp.env));
    while let Some(env_rc) = env_cursor {
        let env_ref = env_rc.borrow();
        if let Some(dict_val) = env_ref.namespace_explicit_globals() {
            enqueue(dict_val, &mut visited, &mut queue);
        }
        for v in env_ref.namespace_values().values() {
            enqueue(v.clone(), &mut visited, &mut queue);
        }
        env_cursor = env_ref.parent.as_ref().map(Rc::clone);
    }

    // 3. Cached modules.
    for m in interp.cold.module_cache.borrow().values() {
        enqueue(m.clone(), &mut visited, &mut queue);
    }

    let mut referents = Vec::new();
    while let Some(obj) = queue.pop_front() {
        if tracked.len() >= MAX_GC_WALK_OBJECTS {
            break;
        }
        referents.clear();
        collect_direct_referents(&obj, &mut referents);
        tracked.push(obj);
        for child in referents.drain(..) {
            if tracked.len() + queue.len() >= MAX_GC_WALK_OBJECTS {
                break;
            }
            enqueue(child, &mut visited, &mut queue);
        }
    }

    tracked
}

fn current_callbacks_snapshot(interp: &mut Interpreter) -> Vec<Value> {
    let cb_list = interp
        .cold
        .module_cache
        .borrow()
        .get("gc")
        .and_then(|m| match m.kind() {
            ValueKind::PyModule(mod_rc) => mod_rc.borrow().get_attr_value("callbacks"),
            _ => None,
        })
        .unwrap_or_else(|| interp.features().gc().callbacks.clone());
    interp.collect_iterable(&cb_list).unwrap_or_default()
}

fn invoke_gc_callbacks(
    interp: &mut Interpreter,
    callbacks: &[Value],
    phase: &str,
    generation: i64,
    collected: i64,
    uncollectable: i64,
) {
    for cb in callbacks {
        let mut info = PyDict::default();
        info.insert(PyKey::str_from("generation"), Value::int(generation));
        info.insert(PyKey::str_from("collected"), Value::int(collected));
        info.insert(PyKey::str_from("uncollectable"), Value::int(uncollectable));
        let args = [
            ExpandedCallArg {
                name: None,
                value: Value::string(phase),
            },
            ExpandedCallArg {
                name: None,
                value: Value::dict(info),
            },
        ];
        if let Err(err) = interp.call_function_expanded(cb.clone(), &args) {
            eprintln!("Exception ignored in gc callback: {err}");
        }
    }
}

pyrust_module! {
    constants {
        "DEBUG_STATS"         => Value::int(DEBUG_STATS),
        "DEBUG_COLLECTABLE"   => Value::int(DEBUG_COLLECTABLE),
        "DEBUG_UNCOLLECTABLE" => Value::int(DEBUG_UNCOLLECTABLE),
        "DEBUG_SAVEALL"       => Value::int(DEBUG_SAVEALL),
        "DEBUG_LEAK"          => Value::int(DEBUG_LEAK),
        // Replaced per-interpreter in `gc::prepare_module`.
        "garbage"             => Value::list(Vec::new()),
        "callbacks"           => Value::list(Vec::new()),
    }

    fn enable() -> Result<Value> {
        _interp.features().gc_mut().enabled = true;
        Ok(Value::none())
    }

    fn disable() -> Result<Value> {
        _interp.features().gc_mut().enabled = false;
        Ok(Value::none())
    }

    fn isenabled() -> Result<Value> {
        Ok(Value::bool_(_interp.features().gc().enabled))
    }

    fn collect(#[default(OptionalArg::Omitted)] generation: OptValueRef<'_>) -> Result<Value> {
        let generation_num = match generation.as_value() {
            None => 2i64,
            Some(v) => {
                use crate::interpreter::builtin_args::FromValue;
                PyIndex::try_from_value_with_interp(_interp, v, "collect", "generation")?
                    .expect_i64("collect", "generation")?
            }
        };
        if !(0..=2).contains(&generation_num) {
            return Err(PyError::named("ValueError", "invalid generation"));
        }

        let callbacks = current_callbacks_snapshot(_interp);
        invoke_gc_callbacks(_interp, &callbacks, "start", generation_num, 0, 0);

        {
            let mut state = _interp.features().gc_mut();
            let gen_idx = generation_num as usize;
            state.stats[gen_idx].0 += 1;
            match gen_idx {
                0 => {
                    state.counts.0 = 0;
                    state.counts.1 += 1;
                }
                1 => {
                    state.counts.0 = 0;
                    state.counts.1 = 0;
                    state.counts.2 += 1;
                }
                _ => {
                    state.counts = (0, 0, 0);
                }
            }
        }

        invoke_gc_callbacks(_interp, &callbacks, "stop", generation_num, 0, 0);
        Ok(Value::int(0))
    }

    fn get_count() -> Result<Value> {
        let (c0, c1, c2) = _interp.features().gc().counts;
        Ok(Value::tuple(vec![
            Value::int(c0),
            Value::int(c1),
            Value::int(c2),
        ]))
    }

    fn get_threshold() -> Result<Value> {
        let (t0, t1, t2) = _interp.features().gc().thresholds;
        Ok(Value::tuple(vec![
            Value::int(t0),
            Value::int(t1),
            Value::int(t2),
        ]))
    }

    fn set_threshold(
        #[positional_only] threshold0: PyIndex,
        #[positional_only] #[default(None)] threshold1: Option<PyIndex>,
        #[positional_only] #[default(None)] threshold2: Option<PyIndex>,
    ) -> Result<Value> {
        let t0 = threshold0.expect_i64("set_threshold", "threshold0")?;
        let t1 = match threshold1 {
            Some(v) => Some(v.expect_i64("set_threshold", "threshold1")?),
            None => None,
        };
        let t2 = match threshold2 {
            Some(v) => Some(v.expect_i64("set_threshold", "threshold2")?),
            None => None,
        };
        let mut state = _interp.features().gc_mut();
        state.thresholds.0 = t0;
        if let Some(v1) = t1 {
            state.thresholds.1 = v1;
        }
        if let Some(v2) = t2 {
            state.thresholds.2 = v2;
        }
        Ok(Value::none())
    }

    fn get_debug() -> Result<Value> {
        Ok(Value::int(_interp.features().gc().debug))
    }

    fn set_debug(#[positional_only] flags: PyIndex) -> Result<Value> {
        let f = flags.expect_i64("set_debug", "flags")?;
        _interp.features().gc_mut().debug = f;
        Ok(Value::none())
    }

    fn get_objects(#[default(OptionalArg::Omitted)] generation: OptValueRef<'_>) -> Result<Value> {
        let gen_opt = match generation.as_value() {
            None => None,
            Some(v) if v.is_none() => None,
            Some(v) => {
                use crate::interpreter::builtin_args::FromValue;
                let g = PyIndex::try_from_value_with_interp(_interp, v, "get_objects", "generation")?
                    .expect_i64("get_objects", "generation")?;
                if !(0..=2).contains(&g) {
                    return Err(PyError::named(
                        "ValueError",
                        "generation must be between 0 and 2",
                    ));
                }
                Some(g)
            }
        };
        match gen_opt {
            None | Some(0) => Ok(Value::list(collect_live_tracked_objects(_interp))),
            Some(_) => Ok(Value::list(Vec::new())),
        }
    }

    fn get_referents(#[varargs] objs: &[Value]) -> Result<Value> {
        let mut out = Vec::new();
        for obj in objs {
            collect_direct_referents(obj, &mut out);
        }
        Ok(Value::list(out))
    }

    fn get_referrers(#[varargs] objs: &[Value]) -> Result<Value> {
        if objs.is_empty() {
            return Ok(Value::list(Vec::new()));
        }
        let universe = collect_live_tracked_objects(_interp);
        let mut referrers = Vec::new();
        let mut refs = Vec::new();
        for candidate in universe {
            refs.clear();
            collect_direct_referents(&candidate, &mut refs);
            if refs
                .iter()
                .any(|r| objs.iter().any(|target| r.is_identical_to(target)))
            {
                referrers.push(candidate);
            }
        }
        Ok(Value::list(referrers))
    }

    fn get_stats() -> Result<Value> {
        let stats = _interp.features().gc().stats;
        let mut out = Vec::with_capacity(3);
        for (collections, collected, uncollectable) in stats {
            let mut d = PyDict::default();
            d.insert(PyKey::str_from("collections"), Value::int(collections));
            d.insert(PyKey::str_from("collected"), Value::int(collected));
            d.insert(PyKey::str_from("uncollectable"), Value::int(uncollectable));
            out.push(Value::dict(d));
        }
        Ok(Value::list(out))
    }

    fn is_tracked(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_tracked_value(&obj.0)))
    }

    fn is_finalized(#[positional_only] _obj: PyValue) -> Result<Value> {
        Ok(Value::bool_(false))
    }

    fn freeze() -> Result<Value> {
        let count = collect_live_tracked_objects(_interp).len().max(1) as i64;
        _interp.features().gc_mut().freeze_count += count;
        Ok(Value::none())
    }

    fn unfreeze() -> Result<Value> {
        _interp.features().gc_mut().freeze_count = 0;
        Ok(Value::none())
    }

    fn get_freeze_count() -> Result<Value> {
        Ok(Value::int(_interp.features().gc().freeze_count))
    }
}
