// `graphlib` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/graphlib.py` (issues #3208, #3462); there is no Python-source
// injection.
//
// The port follows the CPython source statement by statement:
//
//   * `TopologicalSorter` keeps its state in the same ordinary instance
//     attributes (`_node2info`, `_ready_nodes`, `_npassedout`, `_nfinished`)
//     so `vars(ts)` and subclasses see what they see on CPython.  The node
//     table is a real Python `dict`, so nodes are looked up with Python
//     hashing / equality (`__hash__` / `__eq__`, unhashable nodes raise
//     `TypeError`) rather than any Rust-side key type.
//   * Collaborating methods (`add`, `_get_nodeinfo`, `prepare`,
//     `_find_cycle`, `get_ready`, `is_active`, `done`) are resolved through
//     `self`, so subclass overrides are honoured exactly as on CPython.
//   * `_NodeInfo` is a `__slots__` class with `node`, `npredecessors` and
//     `successors`.
//   * `static_order()` returns a lazy iterator: nothing (not even
//     `prepare()`) runs until the first `next()`, and `done(*group)` runs
//     when the consumer asks for the item after the last one of a group,
//     exactly like the CPython generator.
//   * `CycleError(ValueError)` is raised as
//     `CycleError("nodes are in a cycle", cycle)` with `cycle` a list whose
//     first and last elements are the same node.
//
// The `MODULE_NAME` constant is injected by the `pyrust_builtin_modules!`
// plumbing; no module-name literal appears in this file.
//
// Reference: <https://docs.python.org/3.12/library/graphlib.html>

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::json::support::unpack2;
use crate::builtin_modules::operator::{getitem_value, setitem_value};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, ProviderIterator, render_value_repr};
use crate::value::{PyDict, PyModule, Value, ValueKind};

/// `_NODE_OUT`: set once `get_ready()` has passed the node out.
const NODE_OUT: i64 = -1;
/// `_NODE_DONE`: set once `done()` has marked the node processed.
const NODE_DONE: i64 = -2;

const TOPOLOGICAL_SORTER_DOC: &str =
    "Provides functionality to topologically sort a graph of hashable nodes";

const CYCLE_ERROR_DOC: &str = "Subclass of ValueError raised by TopologicalSorter.prepare if cycles\n    \
exist in the working graph.\n\n    \
If multiple cycles exist, only one undefined choice among them will be reported\n    \
and included in the exception. The detected cycle can be accessed via the second\n    \
element in the *args* attribute of the exception instance and consists in a list\n    \
of nodes, such that each node is, in the graph, an immediate predecessor of the\n    \
next node in the list. In the reported list, the first and the last node will be\n    \
the same, to make it clear that it is cyclic.\n    ";

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

const SIG_NODEINFO_INIT: Sig = sig("_NodeInfo.__init__", &["self", "node"], 0);
const SIG_INIT: Sig = sig("TopologicalSorter.__init__", &["self", "graph"], 1);
const SIG_GET_NODEINFO: Sig = sig("TopologicalSorter._get_nodeinfo", &["self", "node"], 0);
const SIG_PREPARE: Sig = sig("TopologicalSorter.prepare", &["self"], 0);
const SIG_GET_READY: Sig = sig("TopologicalSorter.get_ready", &["self"], 0);
const SIG_IS_ACTIVE: Sig = sig("TopologicalSorter.is_active", &["self"], 0);
const SIG_BOOL: Sig = sig("TopologicalSorter.__bool__", &["self"], 0);
const SIG_FIND_CYCLE: Sig = sig("TopologicalSorter._find_cycle", &["self"], 0);
const SIG_STATIC_ORDER: Sig = sig("TopologicalSorter.static_order", &["self"], 0);

fn value_error(message: impl Into<String>) -> PyError {
    PyError::named("ValueError", message.into())
}

fn type_error(message: impl Into<String>) -> PyError {
    PyError::named("TypeError", message.into())
}

fn all_list() -> Value {
    Value::list(
        ["TopologicalSorter", "CycleError"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

fn call_pos(interp: &mut Interpreter, callee: Value, values: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = values
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    values: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, values)
}

fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    render_value_repr(interp, value)
}

/// The integer held by attribute `name` of `obj` (always an `int` unless the
/// program tampers with the private bookkeeping attributes).
fn int_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<i64> {
    let value = interp.get_attr(obj, name)?;
    value.as_int().ok_or_else(|| {
        type_error(format!(
            "graphlib: attribute '{name}' must be an int, not {}",
            pyrust_core::error_type_name(&value)
        ))
    })
}

/// A snapshot of the elements of a `list` / `tuple` value (any other value is
/// iterated through the interpreter).
fn snapshot(interp: &mut Interpreter, value: &Value) -> Result<Vec<Value>> {
    match value.kind() {
        ValueKind::List(items) => Ok(items.to_vec()),
        ValueKind::Tuple(items) => Ok(items.to_vec()),
        _ => interp.collect_iterable(value),
    }
}

/// The identity of a `_NodeInfo` instance.
fn identity(value: &Value) -> usize {
    match value.kind() {
        ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as *const () as usize,
        _ => 0,
    }
}

fn self_of(args: &[ExpandedCallArg]) -> Value {
    args.first()
        .map(|arg| arg.value.clone())
        .unwrap_or_else(Value::none)
}

/// `self._ready_nodes`, or the `ValueError` every method but `add` /
/// `prepare` raises while the sorter is not prepared.
fn ready_nodes(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let ready = interp.get_attr(this, "_ready_nodes")?;
    if ready.is_none() {
        return Err(value_error("prepare() must be called first"));
    }
    Ok(ready)
}

/// `self._node2info`.
fn node2info(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    interp.get_attr(this, "_node2info")
}

/// Bind `(self, node, *predecessors)` / `(self, *nodes)` style signatures:
/// keywords are only accepted for the named leading parameters.
fn bind_variadic(
    qualname: &str,
    named: &[&str],
    args: &[ExpandedCallArg],
) -> Result<(Value, Vec<Option<Value>>, Vec<Value>)> {
    let positional: Vec<&Value> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    // `self` is always the first positional argument.
    let this = positional.first().map(|v| (*v).clone());
    let mut slots: Vec<Option<Value>> = vec![None; named.len()];
    for (i, slot) in slots.iter_mut().enumerate() {
        if let Some(value) = positional.get(i + 1) {
            *slot = Some((*value).clone());
        }
    }
    for arg in args {
        let Some(name) = &arg.name else { continue };
        match named.iter().position(|p| p == name) {
            Some(i) if slots[i].is_some() => {
                return Err(type_error(format!(
                    "{qualname}() got multiple values for argument '{name}'"
                )));
            }
            Some(i) => slots[i] = Some(arg.value.clone()),
            None => {
                return Err(type_error(format!(
                    "{qualname}() got an unexpected keyword argument '{name}'"
                )));
            }
        }
    }
    let mut missing: Vec<&str> = Vec::new();
    if this.is_none() {
        missing.push("self");
    }
    missing.extend(
        named
            .iter()
            .zip(&slots)
            .filter(|(_, slot)| slot.is_none())
            .map(|(name, _)| *name),
    );
    let this = match this {
        Some(this) if missing.is_empty() => this,
        _ => {
            let quoted: Vec<String> = missing.iter().map(|n| format!("'{n}'")).collect();
            let shown = match quoted.len() {
                1 => quoted[0].clone(),
                2 => format!("{} and {}", quoted[0], quoted[1]),
                n => format!("{}, and {}", quoted[..n - 1].join(", "), quoted[n - 1]),
            };
            return Err(type_error(format!(
                "{qualname}() missing {} required positional argument{}: {shown}",
                quoted.len(),
                if quoted.len() == 1 { "" } else { "s" }
            )));
        }
    };
    let rest: Vec<Value> = positional
        .iter()
        .skip(1 + named.len())
        .map(|v| (*v).clone())
        .collect();
    Ok((this, slots, rest))
}

// ── `_NodeInfo` / `TopologicalSorter` ────────────────────────────────────────

/// `TopologicalSorter._get_nodeinfo`.
fn get_nodeinfo(interp: &mut Interpreter, this: &Value, node: Value) -> Result<Value> {
    let n2i = node2info(interp, this)?;
    let existing = call_method(interp, &n2i, "get", std::slice::from_ref(&node))?;
    if !existing.is_none() {
        return Ok(existing);
    }
    let module = interp.load_module(MODULE_NAME)?;
    let class = interp.get_attr(&module, "_NodeInfo")?;
    let result = call_pos(interp, class, std::slice::from_ref(&node))?;
    // `self._node2info[node] = result = _NodeInfo(node)`; re-read the table
    // like CPython's attribute load.
    let n2i = node2info(interp, this)?;
    setitem_value(interp, n2i, node, result.clone())?;
    Ok(result)
}

/// `self.add(node, *predecessors)` for every `node, predecessors in
/// graph.items()`.
fn add_graph(interp: &mut Interpreter, this: &Value, graph: &Value) -> Result<()> {
    let items = call_method(interp, graph, "items", &[])?;
    for entry in interp.collect_iterable(&items)? {
        let (node, predecessors) = unpack2(interp, &entry)?;
        let preds = match interp.collect_iterable(&predecessors) {
            Ok(values) => values,
            Err(err) if err.class_name_is("TypeError") => {
                let type_name = pyrust_core::error_type_name(&predecessors);
                let generic = format!("'{type_name}' object is not iterable");
                if matches!(
                    err.kind(),
                    pyrust_core::PyErrorKind::Named(class_name, message)
                        if class_name.as_ref() == "TypeError" && message == &generic
                ) {
                    return Err(type_error(format!(
                        "Value after * must be an iterable, not {type_name}"
                    )));
                }
                return Err(err);
            }
            Err(err) => return Err(err),
        };
        let add = interp.get_attr(this, "add")?;
        let mut call = Vec::with_capacity(preds.len() + 1);
        call.push(node);
        call.extend(preds);
        call_pos(interp, add, &call)?;
    }
    Ok(())
}

/// `TopologicalSorter.add`.
fn sorter_add(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, mut slots, predecessors) = bind_variadic("TopologicalSorter.add", &["node"], args)?;
    let Some(node) = slots[0].take() else {
        return Err(type_error(
            "TopologicalSorter.add() missing 1 required positional argument: 'node'",
        ));
    };
    if !interp.get_attr(&this, "_ready_nodes")?.is_none() {
        return Err(value_error(
            "Nodes cannot be added after a call to prepare()",
        ));
    }
    // Create the node -> predecessor edges
    let get = interp.get_attr(&this, "_get_nodeinfo")?;
    let nodeinfo = call_pos(interp, get, std::slice::from_ref(&node))?;
    let count = int_attr(interp, &nodeinfo, "npredecessors")?;
    interp.assign_attr(
        nodeinfo,
        "npredecessors",
        Value::int(count + predecessors.len() as i64),
    )?;
    // Create the predecessor -> node edges
    for pred in predecessors {
        let get = interp.get_attr(&this, "_get_nodeinfo")?;
        let pred_info = call_pos(interp, get, &[pred])?;
        let successors = interp.get_attr(&pred_info, "successors")?;
        call_method(interp, &successors, "append", std::slice::from_ref(&node))?;
    }
    Ok(Value::none())
}

/// `TopologicalSorter.prepare` (3.12.3: a second call is an error).
fn sorter_prepare(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    if !interp.get_attr(this, "_ready_nodes")?.is_none() {
        return Err(value_error("cannot prepare() more than once"));
    }
    let n2i = node2info(interp, this)?;
    let infos = call_method(interp, &n2i, "values", &[])?;
    let mut ready = Vec::new();
    for info in interp.collect_iterable(&infos)? {
        if int_attr(interp, &info, "npredecessors")? == 0 {
            ready.push(interp.get_attr(&info, "node")?);
        }
    }
    interp.assign_attr(this.clone(), "_ready_nodes", Value::list(ready))?;
    // `_ready_nodes` is set before we look for cycles on purpose: a caller
    // that catches the `CycleError` can keep using the instance.
    let cycle = call_method(interp, this, "_find_cycle", &[])?;
    if interp.truthy_value(&cycle)? {
        let module = interp.load_module(MODULE_NAME)?;
        let class = interp.get_attr(&module, "CycleError")?;
        let exc = call_pos(
            interp,
            class,
            &[Value::string("nodes are in a cycle"), cycle],
        )?;
        return Err(PyError::raised(exc));
    }
    Ok(Value::none())
}

/// `TopologicalSorter.get_ready`.
fn sorter_get_ready(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let ready = ready_nodes(interp, this)?;
    // Get the nodes that are ready and mark them
    let result = snapshot(interp, &ready)?;
    let n2i = node2info(interp, this)?;
    for node in &result {
        let info = getitem_value(interp, n2i.clone(), node.clone())?;
        interp.assign_attr(info, "npredecessors", Value::int(NODE_OUT))?;
    }
    // Clean the list of nodes that are ready and update the counter of nodes
    // that we have returned.
    let ready = interp.get_attr(this, "_ready_nodes")?;
    call_method(interp, &ready, "clear", &[])?;
    let passed = int_attr(interp, this, "_npassedout")?;
    interp.assign_attr(
        this.clone(),
        "_npassedout",
        Value::int(passed + result.len() as i64),
    )?;
    Ok(Value::tuple(result))
}

/// `TopologicalSorter.is_active`.
fn sorter_is_active(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let ready = ready_nodes(interp, this)?;
    let finished = int_attr(interp, this, "_nfinished")?;
    let passed = int_attr(interp, this, "_npassedout")?;
    if finished < passed {
        return Ok(Value::bool_(true));
    }
    Ok(Value::bool_(interp.truthy_value(&ready)?))
}

/// `TopologicalSorter.done`.
fn sorter_done(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, _, nodes) = bind_variadic("TopologicalSorter.done", &[], args)?;
    ready_nodes(interp, &this)?;
    let n2i = node2info(interp, &this)?;
    for node in nodes {
        // Check if we know about this node (it was added previously using
        // add()).
        let nodeinfo = call_method(interp, &n2i, "get", std::slice::from_ref(&node))?;
        if nodeinfo.is_none() {
            let shown = repr_of(interp, &node)?;
            return Err(value_error(format!(
                "node {shown} was not added using add()"
            )));
        }
        // If the node has not being returned (marked as ready) previously,
        // inform the user.
        let stat = int_attr(interp, &nodeinfo, "npredecessors")?;
        if stat != NODE_OUT {
            let shown = repr_of(interp, &node)?;
            if stat >= 0 {
                return Err(value_error(format!(
                    "node {shown} was not passed out (still not ready)"
                )));
            } else if stat == NODE_DONE {
                return Err(value_error(format!("node {shown} was already marked done")));
            }
            return Err(PyError::named(
                "AssertionError",
                format!("node {shown}: unknown status {stat}"),
            ));
        }
        // Mark the node as processed
        interp.assign_attr(nodeinfo.clone(), "npredecessors", Value::int(NODE_DONE))?;
        // Go to all the successors and reduce the number of predecessors,
        // collecting all the ones that are ready to be returned in the next
        // get_ready() call.
        let successors = interp.get_attr(&nodeinfo, "successors")?;
        for successor in snapshot(interp, &successors)? {
            let successor_info = getitem_value(interp, n2i.clone(), successor.clone())?;
            let remaining = int_attr(interp, &successor_info, "npredecessors")? - 1;
            interp.assign_attr(successor_info, "npredecessors", Value::int(remaining))?;
            if remaining == 0 {
                let ready = interp.get_attr(&this, "_ready_nodes")?;
                call_method(interp, &ready, "append", &[successor])?;
            }
        }
        let finished = int_attr(interp, &this, "_nfinished")?;
        interp.assign_attr(this.clone(), "_nfinished", Value::int(finished + 1))?;
    }
    Ok(Value::none())
}

/// `TopologicalSorter._find_cycle`: an iterative depth-first search that
/// reports the first back edge as `stack[i:] + [node]`.
///
/// `seen` / `node2stacki` are keyed by the identity of the node's
/// `_NodeInfo`: the node table maps every equal node to the same record, so
/// this is the same equivalence relation as the Python-level `set` / `dict`.
fn sorter_find_cycle(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let n2i = node2info(interp, this)?;
    let mut stack: Vec<Value> = Vec::new();
    let mut stack_ids: Vec<usize> = Vec::new();
    let mut itstack: Vec<(Vec<Value>, usize)> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut node2stacki: HashMap<usize, usize> = HashMap::new();

    for key in interp.collect_iterable(&n2i)? {
        let mut node = key;
        loop {
            let info = getitem_value(interp, n2i.clone(), node.clone())?;
            let id = identity(&info);
            if seen.contains(&id) {
                // If we have seen already the node and is in the current
                // stack we have found a cycle.
                if let Some(&start) = node2stacki.get(&id) {
                    let mut cycle = stack[start..].to_vec();
                    cycle.push(node);
                    return Ok(Value::list(cycle));
                }
                // else go on to get next successor
            } else {
                seen.insert(id);
                let successors = interp.get_attr(&info, "successors")?;
                itstack.push((snapshot(interp, &successors)?, 0));
                node2stacki.insert(id, stack.len());
                stack.push(node.clone());
                stack_ids.push(id);
            }

            // Backtrack to the topmost stack entry with at least another
            // successor.
            let mut advanced = false;
            while !stack.is_empty() {
                let (successors, position) = itstack.last_mut().expect("parallel stacks");
                if let Some(next) = successors.get(*position) {
                    node = next.clone();
                    *position += 1;
                    advanced = true;
                    break;
                }
                stack.pop();
                if let Some(id) = stack_ids.pop() {
                    node2stacki.remove(&id);
                }
                itstack.pop();
            }
            if !advanced {
                break;
            }
        }
    }
    Ok(Value::none())
}

// ── `static_order()` ─────────────────────────────────────────────────────────

enum StaticPhase {
    /// Body not entered yet: `prepare()` is still to run.
    Start,
    /// Between groups: `is_active()` is to be asked next.
    Between,
    /// Yielding the items of a group; `done(*group)` follows its last item.
    Group,
    Finished,
}

struct StaticOrderState {
    this: Value,
    phase: StaticPhase,
    group: Vec<Value>,
    position: usize,
}

fn static_order_step(interp: &mut Interpreter, st: &mut StaticOrderState) -> Result<Option<Value>> {
    loop {
        match st.phase {
            StaticPhase::Start => {
                call_method(interp, &st.this, "prepare", &[])?;
                st.phase = StaticPhase::Between;
            }
            StaticPhase::Between => {
                let active = call_method(interp, &st.this, "is_active", &[])?;
                if !interp.truthy_value(&active)? {
                    st.phase = StaticPhase::Finished;
                    return Ok(None);
                }
                let node_group = call_method(interp, &st.this, "get_ready", &[])?;
                // `yield from node_group`
                st.group = interp.collect_iterable(&node_group)?;
                st.position = 0;
                st.phase = StaticPhase::Group;
            }
            StaticPhase::Group => {
                if let Some(node) = st.group.get(st.position) {
                    let node = node.clone();
                    st.position += 1;
                    return Ok(Some(node));
                }
                // `self.done(*node_group)`
                let group = std::mem::take(&mut st.group);
                let done = interp.get_attr(&st.this, "done")?;
                st.phase = StaticPhase::Between;
                call_pos(interp, done, &group)?;
            }
            StaticPhase::Finished => return Ok(None),
        }
    }
}

fn advance_static_order(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    // The state is moved out while Python code runs, so a re-entrant `next()`
    // reports what a running generator would.
    let taken = state_rc
        .borrow_mut()
        .downcast_mut::<Option<StaticOrderState>>()
        .map(Option::take);
    let mut st = match taken {
        Some(Some(st)) => st,
        Some(None) => return Err(value_error("generator already executing")),
        None => {
            return Err(PyError::runtime(
                "internal: graphlib.static_order iterator state corrupted".to_string(),
            ));
        }
    };
    let result = static_order_step(interp, &mut st);
    if !matches!(result, Ok(Some(_))) {
        // An exhausted generator and one that raised are both finished.
        st.phase = StaticPhase::Finished;
        st.group.clear();
    }
    if let Some(slot) = state_rc
        .borrow_mut()
        .downcast_mut::<Option<StaticOrderState>>()
    {
        *slot = Some(st);
    }
    result
}

fn make_static_order(this: Value) -> Value {
    let state = StaticOrderState {
        this,
        phase: StaticPhase::Start,
        group: Vec::new(),
        position: 0,
    };
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(Some(state)),
        advance_static_order,
        "generator",
        "generator",
        None,
    )))
}

/// `TopologicalSorter.__class_getitem__` (`classmethod(GenericAlias)`).
fn class_getitem(args: &[ExpandedCallArg]) -> Result<Value> {
    let (cls, rest) = args.split_first().ok_or_else(|| {
        type_error("TopologicalSorter.__class_getitem__() takes exactly one argument (0 given)")
    })?;
    if rest.iter().any(|arg| arg.name.is_some()) || rest.len() != 1 {
        return Err(type_error(format!(
            "TopologicalSorter.__class_getitem__() takes exactly one argument ({} given)",
            rest.len()
        )));
    }
    let item = &rest[0].value;
    let type_arguments = if matches!(item.kind(), ValueKind::Tuple(_)) {
        item.clone()
    } else {
        Value::tuple(vec![item.clone()])
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        cls.value.clone(),
        type_arguments,
    ))
}

/// Post-import wiring: `_NodeInfo` is a `__slots__` class and `CycleError`
/// carries CPython's docstring.
pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let class = module.borrow().get_attr_value("_NodeInfo");
    let Some(class) = class else {
        return Err(PyError::runtime("graphlib: _NodeInfo missing".into()));
    };
    if let Some(cycle_error) = module.borrow().get_attr_value("CycleError")
        && let ValueKind::PyClass(cycle_error) = cycle_error.kind()
    {
        cycle_error
            .borrow_mut()
            .attrs
            .insert("__doc__".to_string(), Value::string(CYCLE_ERROR_DOC));
    }
    if let ValueKind::PyClass(class) = class.kind() {
        class.borrow_mut().slots = Some(
            ["node", "npredecessors", "successors"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => all_list(),
        "GenericAlias" => Value::py_class(crate::interpreter::generic_alias_class_singleton()),
        "_NODE_OUT" => Value::int(NODE_OUT),
        "_NODE_DONE" => Value::int(NODE_DONE),
    }

    class _NodeInfo {
        constants {
            "__slots__" => Value::tuple(vec![
                Value::string("node"),
                Value::string("npredecessors"),
                Value::string("successors"),
            ]),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_NODEINFO_INIT, args)?;
            let this = bound.take_or_none(0);
            let node = bound.take_or_none(1);
            // The node this class is augmenting.
            _interp.assign_attr(this.clone(), "node", node)?;
            // Number of predecessors, generally >= 0.  When this value falls
            // to 0, and is returned by get_ready(), this is set to _NODE_OUT
            // and when the node is marked done by a call to done(), set to
            // _NODE_DONE.
            _interp.assign_attr(this.clone(), "npredecessors", Value::int(0))?;
            // List of successor nodes.  The list can contain duplicated
            // elements as long as they're all reflected in the successor's
            // npredecessors attribute.
            _interp.assign_attr(this, "successors", Value::list(Vec::new()))?;
            Ok(Value::none())
        }
    }

    exception CycleError(ValueError);

    class TopologicalSorter {
        constants {
            "__doc__" => Value::string(TOPOLOGICAL_SORTER_DOC),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_INIT, args)?;
            let this = bound.take_or_none(0);
            let graph = bound.take_or_none(1);
            _interp.assign_attr(this.clone(), "_node2info", Value::dict(PyDict::default()))?;
            _interp.assign_attr(this.clone(), "_ready_nodes", Value::none())?;
            _interp.assign_attr(this.clone(), "_npassedout", Value::int(0))?;
            _interp.assign_attr(this.clone(), "_nfinished", Value::int(0))?;
            if !graph.is_none() {
                add_graph(_interp, &this, &graph)?;
            }
            Ok(Value::none())
        }

        fn _get_nodeinfo(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GET_NODEINFO, args)?;
            let this = bound.take_or_none(0);
            let node = bound.take_or_none(1);
            get_nodeinfo(_interp, &this, node)
        }

        fn add(args) -> Result<Value> {
            sorter_add(_interp, args)
        }

        fn prepare(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_PREPARE, args)?;
            let this = bound.take_or_none(0);
            sorter_prepare(_interp, &this)
        }

        fn get_ready(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_GET_READY, args)?;
            let this = bound.take_or_none(0);
            sorter_get_ready(_interp, &this)
        }

        fn is_active(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IS_ACTIVE, args)?;
            let this = bound.take_or_none(0);
            sorter_is_active(_interp, &this)
        }

        fn __bool__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_BOOL, args)?;
            let this = bound.take_or_none(0);
            call_method(_interp, &this, "is_active", &[])
        }

        fn done(args) -> Result<Value> {
            sorter_done(_interp, args)
        }

        fn _find_cycle(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_FIND_CYCLE, args)?;
            let this = bound.take_or_none(0);
            sorter_find_cycle(_interp, &this)
        }

        fn static_order(args) -> Result<Value> {
            args::bind(&SIG_STATIC_ORDER, args)?;
            Ok(make_static_order(self_of(args)))
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            class_getitem(args)
        }
    }
}
