// Thread-local call depth counter. Using thread_local avoids the split-borrow
// problem: a guard that holds &mut self.call_depth cannot coexist with a &mut self
// method call. The thread_local is safe because the interpreter is single-threaded.

thread_local! {
    static CALL_DEPTH: Cell<usize> = const { Cell::new(0) };
    // Recursion limit, configurable via sys.setrecursionlimit(). Default matches CPython.
    static RECURSION_LIMIT: Cell<usize> = const { Cell::new(1000) };
}

/// Resolved class bases: `(primary_base, extra_bases)` as produced by
/// `make_class_resolve_bases`.
type ResolvedBases = (Option<Rc<RefCell<PyClass>>>, Vec<Rc<RefCell<PyClass>>>);

pub(crate) fn get_recursion_limit() -> usize {
    RECURSION_LIMIT.with(|l| l.get())
}

pub(crate) fn set_recursion_limit(n: usize) {
    RECURSION_LIMIT.with(|l| l.set(n));
}

fn max_call_depth() -> usize {
    get_recursion_limit()
}

fn call_depth() -> usize {
    CALL_DEPTH.with(|d| d.get())
}

pub(crate) fn get_call_depth() -> usize {
    call_depth()
}

// RAII guard: increments the thread-local call depth on creation and decrements
// it on drop, including on panic, without any unsafe code.
struct CallDepthGuard;

impl CallDepthGuard {
    fn enter() -> Self {
        CALL_DEPTH.with(|d| d.set(d.get() + 1));
        Self
    }
}

impl Drop for CallDepthGuard {
    fn drop(&mut self) {
        CALL_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

/// Reject keyword arguments on a builtin method that accepts none, raising the
/// CPython-matching `"<label>() takes no keyword arguments"` `TypeError`.
///
/// `label` is the method's display name and may be a literal or a `format!`
/// pattern (e.g. `reject_kwargs!(kw, "{}.fromkeys", class_name)`).  The label
/// is only built when `kw` is non-empty, so the empty-kw fast path (the common
/// case) pays no allocation.  Expands to an early `return Err(...)`, so call it
/// from a function returning `Result<_>`.
macro_rules! reject_kwargs {
    ($kw:expr, $($label:tt)+) => {
        if !$kw.is_empty() {
            return Err(pyrust_core::type_err!("{}() takes no keyword arguments", format_args!($($label)+)));
        }
    };
}

/// Build the per-type `TypeError` message for sequence item access with a
/// non-integer non-`__index__` index, matching CPython 3.12.
///
/// CPython uses different message formats per type:
/// - list / tuple / bytes: `"X indices must be integers or slices, not Y"`
/// - string: `"string indices must be integers, not 'Y'"` (different wording
///   and the type name is quoted).
fn seq_index_type_error(label: &str, type_name: &str) -> String {
    if label == "string" {
        format!("string indices must be integers, not '{type_name}'")
    } else {
        format!("{label} indices must be integers or slices, not {type_name}")
    }
}

/// Narrow an already-resolved index `Value` (guaranteed `Int`/`Bool`/`BigInt`
/// by [`Interpreter::value_to_index`]) to `i64`, raising `OverflowError` with
/// the caller's context message when a `BigInt` doesn't fit.
fn index_value_to_i64(v: &Value, overflow_msg: &str) -> Result<i64> {
    use crate::value::PyToPrimitive;
    match v.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(b as i64),
        ValueKind::BigInt(b) => b.to_i64().ok_or_else(|| pyrust_core::overflow_err!("{}", overflow_msg)),
        _ => unreachable!("index_value_to_i64: value_to_index guarantees an integer"),
    }
}

impl Interpreter {
    /// Shared constructor for the `GeneratorFrame` wrapped in a
    /// `Value::generator`. Both call-site branches in
    /// `call_user_function_expanded` (simple-arity and variadic) bind
    /// params into `regs`, capture the active env, then need to short-
    /// circuit before `run_bytecode_for_fn`. The two branches must keep
    /// the frame initialisation identical — factored here so issue #488
    /// (variadic) can't drift from the simple-path baseline again.
    fn build_generator_value(
        code: &Rc<crate::bytecode::FnCode>,
        regs: RegsBuf,
        saved_env: EnvRef,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        fn_name: std::sync::Arc<str>,
        qualname: std::sync::Arc<str>,
    ) -> Value {
        let num_iters = code.num_iters as usize;
        let is_coroutine = code.is_coroutine;
        let frame = GeneratorFrame {
            code: Rc::clone(code),
            // Tighten to an exactly-sized `Vec` for the generator's lifetime
            // (#2257): `into_vec` reuses the buffer when the param-binding
            // `RegsBuf` already spilled to the heap, and otherwise allocates a
            // snug `num_regs`-length `Vec` (vs the 128-byte inline buffer).
            regs: regs.into_vec(),
            iters: smallvec![None; num_iters],
            exc_handlers: ExcHandlersBuf::new(),
            pc: 0,
            done: false,
            saved_env,
            // PEP 3134 per-generator exception state; all empty until
            // the body actually pushes handlers and yields inside one.
            handled_exc_slice: HandledExcBuf::new(),
            active_exception: None,
            exc_saved_active_slice: Vec::new(),
            local_index,
            // Meaningless until the first yield; initialised to 0 as a safe
            // default (the Yield opcode always overwrites this before resumption).
            yield_dst: 0,
            // Set by resume_generator_with_exc when FrameOutcome::Returned(val)
            // is received; read by Insn::YieldFrom to retrieve the sub-iterator's
            // StopIteration.value (PEP 380).
            last_return_value: None,
            // Stored so resume_generator_with_exc can name the generator's
            // traceback frame when an exception propagates out (issue #908).
            fn_name,
            qualname,
            // Coroutine tag (issue #1039): distinguishes an `async def` frame
            // from a plain generator for type/repr/iterability purposes.
            is_coroutine,
        };
        Value::generator(Box::new(frame))
    }

    pub(crate) fn call_function_expanded(
        &mut self,
        function: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Registry-driven dispatch: built-in callables declared via
        // `pyrust_module! { … fn name(args) … }` (or the per-fn fallback
        // `#[pyfunction(name = …)]`) are collected into
        // `crate::builtin_registry`.  We probe it first; the only arms left
        // in the match cascade below are pattern-guarded (bound-method
        // dispatch, `str.*` method-name prefix matching, the `property`
        // accessor partial-slot guard) — these key on `Value` state rather
        // than on a registered name, so they can't live in the registry.
        // See `crates/pyrust/src/builtin_modules/` for the per-module bodies.

        // Fast path (#frame-setup-trim): a plain user function — the most
        // common callee — can never be a `super_bound_builtin`, a registered
        // `BuiltinFunction`, a bound method, or any of the pattern-guarded
        // arms below.  Dispatch straight to `call_user_function_expanded`,
        // skipping the `as_super_bound_builtin` probe, the registry lookup,
        // and entry into the large `match function.kind()` cascade.  Exactly
        // equivalent to the `ValueKind::UserFunction` arm of that match.
        if let ValueKind::UserFunction(f) = function.kind() {
            let f = Rc::clone(f);
            return self.call_user_function_expanded(f, args, &[]);
        }

        // Issue #988: `super().__init__(args)` on a dict/list/set subclass.
        // The SuperProxy wraps the resolved BuiltinFunction sentinel together
        // with the instance in a `super_bound_builtin` object so that `self`
        // is available here.  Prepend the instance and call the registry
        // dispatch — mirroring what `invoke_class_method` does for the normal
        // `__init__` path (called from `call_class_expanded`).
        //
        // Issue #1771: if the method is not in the registry (e.g. `dict.update`,
        // `list.append`, `set.add` — dispatched via the bound-method table rather
        // than the builtin registry), fall back to constructing a bound_method
        // value and re-entering call_function_expanded.  This covers all
        // type-qualified names of the form "<type>.<method>".
        //
        // When the instance is a PyInstance (subclass of a builtin), use its
        // `__builtin_data__` backing value as the bound-method receiver so
        // that the method operates on the underlying native container.  This
        // is the same mechanism used in the normal instance-attribute dispatch
        // (the `BuiltinFunction` + `instance_builtin_data` path in
        // `call_function_expanded`'s `Kind::Other` arm).
        if let Some((fn_name, instance)) =
            pyrust_builtins::super_bound_builtin::as_super_bound_builtin(&function)
        {
            if let Some(dispatch) = crate::builtin_registry::lookup(&fn_name) {
                let mut combined: ExpandedArgBuf = ExpandedArgBuf::with_capacity(args.len() + 1);
                combined.push(ExpandedCallArg { name: None, value: instance });
                combined.extend(args.iter().cloned());
                return dispatch(self, &combined);
            }
            // Bound-method fallback: construct a bound_method carrying the
            // bare method name (after the '.') bound to the native backing
            // value.  For a PyInstance subclass (`class MyDict(dict)`), the
            // `__builtin_data__` attribute holds the live dict/list/set that
            // the bound-method dispatch operates on.  For a plain native value
            // (e.g. `super()` from a class whose instance IS the native type),
            // use the instance directly.
            if let Some(method_name) = fn_name.split_once('.').map(|(_, m)| m) {
                // Drop the kind() borrow before we may move `instance`.
                let backing_opt = match instance.kind() {
                    ValueKind::PyInstance(inst) => instance_builtin_data(inst),
                    _ => None,
                };
                let receiver = backing_opt.unwrap_or(instance);
                let bound = pyrust_builtins::bound_method::bound_method(method_name, receiver);
                return self.call_function_expanded(bound, args);
            }
        }

        // Issue #2096: calling the `type.__call__` method-wrapper returned by
        // `C.__call__` (for a class that defines no `__call__` of its own) is
        // equivalent to calling the class itself — `C.__call__(args)` ==
        // `C(args)`.  Re-dispatch onto the bound class so both the primitive
        // (`int.__call__("5")`) and user-class construction paths are reused.
        if let Some(class_val) =
            pyrust_builtins::type_call_wrapper::as_type_call_wrapper(&function)
        {
            return self.call_function_expanded(class_val, args);
        }

        // Issue #1909: type-level unbound container protocol dunders
        // (`list.__getitem__([1,2], 0)`, `list.__setitem__(l, 0, 9)`,
        // `list.__add__([1], [2])`, …).  Route through the shared dispatcher so
        // the result matches the bound form and the operator behaviour.  A
        // `PyInstance` first argument (a `dict`/`list`/… *subclass* instance,
        // or a `super().__getitem__(...)` call) is left to the registry bodies
        // below, which unwrap `__builtin_data__` and support `super()`.
        if let ValueKind::BuiltinFunction(name) = function.kind()
            && let Some((type_name, method)) = name.split_once('.')
            && method.starts_with("__")
            && builtin_protocol_dunders(type_name).contains(&method)
            && let Some(self_arg) = args.first() {
                let recv = self_arg.value.clone();
                let recv_is_match = !matches!(recv.kind(), ValueKind::PyInstance(_))
                    && pyrust_core::builtin_type_name(&recv) == type_name;
                if recv_is_match {
                    let rest: Vec<Value> = args[1..]
                        .iter()
                        .filter(|a| a.name.is_none())
                        .map(|a| a.value.clone())
                        .collect();
                    if args[1..].iter().any(|a| a.name.is_some()) {
                        // CPython's keyword-rejection wording depends on whether
                        // the slot is a named method-wrapper or an anonymous slot
                        // wrapper (issue #2291).
                        return Err(if is_named_protocol_wrapper(method, type_name) {
                            pyrust_core::type_err!("{type_name}.{method}() takes no keyword arguments")
                        } else {
                            pyrust_core::type_err!("wrapper {method}() takes no keyword arguments")
                        });
                    }
                    // `method` borrows `name` from the matched `function.kind()`
                    // Ref; clone to a small owned str so the dispatcher can run
                    // without holding that borrow.
                    let method = method.to_string();
                    return self.dispatch_builtin_protocol_dunder(&method, recv, rest);
                }
            }

        if let ValueKind::BuiltinFunction(name) = function.kind()
            && let Some(dispatch) = crate::builtin_registry::lookup(name)
        {
            return dispatch(self, args);
        }
        match function.kind() {
            // Migrated to `crate::builtin_modules::builtins`:
            //   print, range, open, __vcall__, plus all the simple top-level
            //   builtins (abs, len, …) — dispatched via the registry probe
            //   at the top of this fn.  `math.*` and `sys.exit` similarly
            //   live in their per-module bodies.
            _ if pyrust_builtins::bound_method::as_bound_method(&function).is_some() => {
                let (name_rc, receiver_owned) =
                    pyrust_builtins::bound_method::as_bound_method(&function).unwrap();
                self.call_bound_method_dispatch(name_rc, receiver_owned, args)
            }
            // PEP 654: BaseExceptionGroup.derive / subgroup / split.  These need
            // interpreter access (to call a user predicate or a subclass's
            // overridden `derive`), so they are intercepted here rather than
            // declared as registry builtins.
            ValueKind::BuiltinFunction("BaseExceptionGroup.derive") => {
                self.exception_group_derive(args)
            }
            ValueKind::BuiltinFunction("BaseExceptionGroup.subgroup") => {
                self.exception_group_subgroup_or_split(args, false)
            }
            ValueKind::BuiltinFunction("BaseExceptionGroup.split") => {
                self.exception_group_subgroup_or_split(args, true)
            }
            // Issue #2276: the unbound type-qualified object-level dunders
            // (`str.__hash__` / `int.__repr__` / `str.__str__` /
            // `int.__format__` / …) synthesised by `get_attr_class` for the
            // primitives that override them in CPython.  CPython attributes the
            // receiver guard to the *called* type (not `object`) and dispatches
            // through the type's own slot; emulate that by validating the
            // receiver here (slot-wrapper wording for `__hash__`/`__repr__`/
            // `__str__`, method_descriptor wording for `__format__`) and then
            // delegating to the shared `object.__X__` body / `apply_format_spec`
            // — the implementations that already render a bare primitive and a
            // subclass `PyInstance` correctly.
            ValueKind::BuiltinFunction(name)
                if name
                    .split_once('.')
                    .is_some_and(|(_, m)| matches!(m, "__hash__" | "__repr__" | "__str__" | "__format__"))
                    && primitive_object_dunder_owner(name).is_some() =>
            {
                let (type_name, method) = name.split_once('.').unwrap();
                self.call_primitive_object_dunder(type_name, method, args)
            }
            ValueKind::BuiltinFunction("str.format") => {
                let self_val = args
                    .first()
                    .map(|a| &a.value)
                    .ok_or_else(|| pyrust_core::descriptor_needs_arg!("format", "str", method))?;
                let template = match self_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        let actual = pyrust_core::builtin_type_name(self_val);
                        return Err(pyrust_core::descriptor_requires!(
                            "format", "str", actual, method
                        ));
                    }
                };
                let mut positional: Vec<Value> = Vec::new();
                let mut keyword: Vec<(String, Value)> = Vec::new();
                for a in &args[1..] {
                    match &a.name {
                        Some(n) => keyword.push((n.clone(), a.value.clone())),
                        None => positional.push(a.value.clone()),
                    }
                }
                self.format_str_template(&template, &positional, &keyword)
            }
            // `float.fromhex` is a classmethod: the first positional arg is the
            // string to parse.  It must be dispatched before the generic
            // `"float.*"` arm below so that the arg-0-is-receiver assumption
            // in that arm is not applied here.
            ValueKind::BuiltinFunction("float.fromhex") => {
                // Accept both `float.fromhex(s)` and `(1.0).fromhex(s)`.
                // Filter out a leading float or class receiver if present, then
                // enforce exactly one remaining positional argument.
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::Float(_) | ValueKind::PyClass(_)
                            )
                    })
                    .collect();
                // If every arg was a float/class receiver (e.g. pure class call
                // with no string) fall back to requiring exactly one total arg.
                let n_payload = if positional_args.is_empty() {
                    args.len()
                } else {
                    positional_args.len()
                };
                if n_payload == 0 {
                    return Err(pyrust_core::type_err!("float.fromhex() takes exactly one argument (0 given)"));
                }
                if n_payload > 1 {
                    return Err(pyrust_core::type_err!("float.fromhex() takes exactly one argument ({} given)",
                            n_payload));
                }
                let s_val = if positional_args.is_empty() {
                    args.first().map(|a| a.value.clone())
                } else {
                    positional_args.first().map(|a| a.value.clone())
                }
                .ok_or_else(|| {
                    pyrust_core::type_err!("float.fromhex() takes exactly one argument (0 given)")
                })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(pyrust_core::type_err!("bad argument type for built-in operation"))
                    }
                };
                pyrust_builtins::float::fromhex(&s).map(Value::float)
            }
            // Issue #1413: generator type descriptor methods.
            // `type(gen).__iter__(g)`, `type(gen).__next__(g)`, etc.
            // These are unbound BuiltinFunction sentinels returned by
            // get_attr on BuiltinFunction("generator").  Dispatch to
            // call_generator_method with args[0] as the receiver.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(t, _)| t == "generator") =>
            {
                let (_, method) = name.split_once('.').unwrap();
                // `__next__`/`__iter__` are slot wrappers; `send`/`close`/`throw`
                // are method_descriptors — distinct receiver-guard wording (#2266).
                let is_dunder = method.starts_with("__") && method.ends_with("__");
                let self_val = args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        if is_dunder {
                            pyrust_core::descriptor_needs_arg!(method, "generator")
                        } else {
                            pyrust_core::descriptor_needs_arg!(method, "generator", method)
                        }
                    })?;
                if !matches!(self_val.kind(), ValueKind::Generator(_)) {
                    let actual = pyrust_core::builtin_type_name(&self_val);
                    return Err(if is_dunder {
                        pyrust_core::descriptor_requires!(method, "generator", actual)
                    } else {
                        pyrust_core::descriptor_requires!(method, "generator", actual, method)
                    });
                }
                let pos: Vec<Value> = args[1..].iter().filter(|a| a.name.is_none()).map(|a| a.value.clone()).collect();
                self.call_generator_method(self_val, method, pos)
            }
            // float instance methods via descriptor call: `float.is_integer(x)`.
            // The float call fn takes an `f64` receiver directly, so this arm
            // is separate from the generic str/list/… arm below.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(t, _)| t == "float") =>
            {
                let (_, method) = name.split_once('.').unwrap();
                let self_val = args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        pyrust_core::descriptor_needs_arg!(method, "float", method)
                    })?;
                let f = match self_val.kind() {
                    ValueKind::Float(f) => f,
                    _ => {
                        let actual = pyrust_core::builtin_type_name(&self_val);
                        return Err(pyrust_core::type_err!("descriptor '{method}' for 'float' objects doesn't apply to a '{actual}' object",));
                    }
                };
                let pos: Vec<Value> = args[1..]
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::float::call(method, f, &pos)
            }
            // PEP 585: `__class_getitem__` classmethods on built-in collection
            // types.  Sentinel names follow the pattern
            // `"<type>.__class_getitem__"` (registered by
            // `build_primitive_classes` in `helpers.rs`).
            // This arm handles the direct-call path:
            //   `list.__class_getitem__(int)` → GenericAlias(list, (int,))
            // The `list[int]` subscript path is handled directly in
            // `eval_index` without passing through here.
            // This arm must come before the instance-method arm below because
            // `"list.__class_getitem__"` would otherwise match that arm's
            // `"list"` prefix guard and raise a spurious descriptor TypeError.
            ValueKind::BuiltinFunction(name)
                if name
                    .split_once('.')
                    .is_some_and(|(_, m)| m == "__class_getitem__") =>
            {
                let type_name = name.split_once('.').unwrap().0;
                // Recover the origin class value from the per-thread singleton.
                let origin_class = primitive_class_by_name(type_name).ok_or_else(|| {
                    PyError::Runtime(format!(
                        "internal: unknown primitive class for __class_getitem__: {type_name}"
                    ))
                })?;
                // Accept one positional argument: the type parameter(s).
                //   `list.__class_getitem__(int)`       → args[0] = int
                //   `list.__class_getitem__((str, int))` → args[0] = (str, int)
                let index = args
                    .iter()
                    .find(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        pyrust_core::type_err!("descriptor '__class_getitem__' of '{type_name}' object \
                                 needs an argument")
                    })?;
                let is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                let type_args = if is_tuple { index } else { Value::tuple(vec![index]) };
                Ok(pyrust_builtins::generic_alias::generic_alias(
                    Value::py_class(origin_class),
                    type_args,
                ))
            }
            ValueKind::BuiltinFunction("str.format_map") => {
                let self_val = args
                    .first()
                    .map(|a| &a.value)
                    .ok_or_else(|| pyrust_core::descriptor_needs_arg!("format_map", "str"))?;
                let template = match self_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(pyrust_core::descriptor_requires!("format_map", "str")),
                };
                // format_map takes exactly one positional argument (the mapping).
                let rest = &args[1..];
                let kw_count = rest.iter().filter(|a| a.name.is_some()).count();
                let pos_count = rest.iter().filter(|a| a.name.is_none()).count();
                if pos_count != 1 || kw_count != 0 {
                    return Err(pyrust_core::type_err!("str.format_map() takes exactly one argument ({} given)",
                            pos_count + kw_count));
                }
                let mapping = rest[0].value.clone();
                self.format_str_template_map(&template, mapping)
            }
            // `bytes.fromhex` is a classmethod: the first positional arg is the
            // hex string to decode.  Must appear before the generic `bytes.*`
            // arm so that the arg-0-is-receiver assumption there is not applied.
            ValueKind::BuiltinFunction("bytes.fromhex") => {
                // Strip any leading bytes receiver or PyClass arg (from instance
                // calls like `b''.fromhex(s)` or `bytes.fromhex(s)`), then
                // enforce exactly one remaining positional argument.
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::Bytes(_) | ValueKind::PyClass(_)
                            )
                    })
                    .collect();
                let n_payload = if positional_args.is_empty() {
                    args.len()
                } else {
                    positional_args.len()
                };
                if n_payload == 0 {
                    return Err(pyrust_core::type_err!("bytes.fromhex() takes exactly one argument (0 given)"));
                }
                if n_payload > 1 {
                    return Err(pyrust_core::type_err!("bytes.fromhex() takes exactly one argument ({n_payload} given)"));
                }
                let s_val = if positional_args.is_empty() {
                    args.first().map(|a| a.value.clone())
                } else {
                    positional_args.first().map(|a| a.value.clone())
                }
                .ok_or_else(|| {
                    pyrust_core::type_err!("bytes.fromhex() takes exactly one argument (0 given)")
                })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(pyrust_core::type_err!("fromhex() argument must be str, not {}",
                                pyrust_core::builtin_type_name(&s_val)));
                    }
                };
                pyrust_builtins::bytes::bytes_fromhex(&s).map(Value::bytes)
            }
            // `bytearray.fromhex` is a classmethod, same pattern as `bytes.fromhex`.
            ValueKind::BuiltinFunction("bytearray.fromhex") => {
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::BuiltinObject { ops, .. }
                                    if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
                            )
                            && !matches!(a.value.kind(), ValueKind::PyClass(_))
                    })
                    .collect();
                let n_payload = if positional_args.is_empty() {
                    args.len()
                } else {
                    positional_args.len()
                };
                if n_payload == 0 {
                    return Err(pyrust_core::type_err!("bytearray.fromhex() takes exactly one argument (0 given)"));
                }
                if n_payload > 1 {
                    return Err(pyrust_core::type_err!("bytearray.fromhex() takes exactly one argument ({n_payload} given)"));
                }
                let s_val = if positional_args.is_empty() {
                    args.first().map(|a| a.value.clone())
                } else {
                    positional_args.first().map(|a| a.value.clone())
                }
                .ok_or_else(|| {
                    pyrust_core::type_err!("bytearray.fromhex() takes exactly one argument (0 given)")
                })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(pyrust_core::type_err!("fromhex() argument must be str, not {}",
                                pyrust_core::builtin_type_name(&s_val)));
                    }
                };
                pyrust_builtins::bytes::bytes_fromhex(&s)
                    .map(pyrust_builtins::bytearray::bytearray)
            }
            // `bytes.maketrans` is a staticmethod: args contains only the two
            // from/to bytes arguments (no implicit receiver).  Both `bytes.maketrans(f, t)`
            // and `b''.maketrans(f, t)` resolve to the same unbound BuiltinFunction,
            // so args is always exactly [from, to] without a prepended receiver.
            // Must appear before the generic `bytes.*` arm, which expects args[0]
            // to be the bytes receiver.
            ValueKind::BuiltinFunction("bytes.maketrans") => {
                let positional: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::bytes::bytes_maketrans(&positional)
            }
            // `str.maketrans` is a staticmethod: same pattern as `bytes.maketrans`.
            // Must appear before the generic `str.*` arm.
            ValueKind::BuiltinFunction("str.maketrans") => {
                let positional: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::string::str_maketrans(&positional)
            }
            // `int.from_bytes` is a classmethod.  Must appear before the generic
            // `int.*` arm so that the arg-0-is-receiver assumption there is not
            // applied here.  Both `int.from_bytes(b, 'big')` and
            // `(5).from_bytes(b, 'big')` arrive here with `args` containing only
            // the explicit positional arguments — the receiver is never injected
            // into `args` for this dispatch path (unlike the generic `int.*` arm).
            ValueKind::BuiltinFunction("int.from_bytes") => {
                let pos: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                let mut kw: PyDict = PyDict::default();
                for a in args {
                    if let Some(name) = &a.name {
                        kw.insert(PyKey::str_from(name.as_str()), a.value.clone());
                    }
                }
                pyrust_builtins::int::int_from_bytes(&pos, &kw)
            }
            // #462: class-method-of-primitive dispatch.  When a primitive
            // class's attr is `BuiltinFunction("<type>.<method>")` — populated
            // by `populate_*_methods` in `helpers.rs` — calling it dispatches
            // like a bound method with `args[0]` as the receiver.  Mirrors the
            // bound-method arm above so `str.upper(s)` and `s.upper()` go
            // through the same per-type `call` fn.  `str.format` is handled
            // by the preceding arm because it threads kwargs into the template.
            ValueKind::BuiltinFunction(name)
                if name
                    .split_once('.')
                    .is_some_and(|(t, _)| matches!(t, "int" | "bytes" | "str" | "list" | "tuple" | "dict" | "set" | "complex" | "frozenset")) =>
            {
                let (type_name, method) = name.split_once('.').unwrap();
                // CPython exposes most dunders (`str.__getitem__`, `list.__add__`,
                // …) as *slot wrappers* and regular methods (`str.upper`, …) as
                // *method_descriptors*; the two raise differently worded receiver
                // guards.  See issue #2266.  A handful of dunders are *per-type*
                // method_descriptors rather than slot wrappers — e.g.
                // `dict`/`set`/`frozenset`.__contains__ (but `str`/`list`/`tuple`/
                // `bytes`.__contains__ stay slot wrappers).  Treat those as
                // method_descriptors so they get the "unbound method …" / "doesn't
                // apply to …" wording.
                let is_method_descriptor_dunder = matches!(
                    (type_name, method),
                    ("dict" | "set" | "frozenset", "__contains__")
                );
                let is_dunder =
                    method.starts_with("__") && method.ends_with("__") && !is_method_descriptor_dunder;
                let self_val = args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        if is_dunder {
                            pyrust_core::descriptor_needs_arg!(method, type_name)
                        } else {
                            pyrust_core::descriptor_needs_arg!(method, type_name, method)
                        }
                    })?;
                let mut pos: Vec<Value> = Vec::with_capacity(args.len().saturating_sub(1));
                let mut kw: PyDict = PyDict::default();
                for a in &args[1..] {
                    match &a.name {
                        Some(n) => { kw.insert(PyKey::str_from(n.as_str()), a.value.clone()); }
                        None => pos.push(a.value.clone()),
                    }
                }
                // Issue #976/#994: if `self_val` is a PyInstance with a
                // `__builtin_data__` backing value (set at construction for
                // subclasses of dict/list/set/frozenset/tuple), use that
                // backing value as the effective receiver so the kind_ok
                // check and dispatch below see the expected primitive type.
                // Issue #1204: same for str/int/float/bytes subclasses.
                let self_val =
                    if matches!(type_name, "dict" | "list" | "set" | "frozenset" | "tuple"
                        | "str" | "int" | "float" | "bytes") {
                        // Extract the Rc before kind() drops its borrow.
                        let maybe_inst = if let ValueKind::PyInstance(inst) = self_val.kind() {
                            Some(Rc::clone(inst))
                        } else {
                            None
                        };
                        if let Some(inst) = maybe_inst {
                            instance_builtin_data(&inst).unwrap_or(self_val)
                        } else {
                            self_val
                        }
                    } else {
                        self_val
                    };
                // Receiver-type guard: validate `self_val`'s kind matches
                // `type_name` before dispatch — otherwise the per-type call fn
                // surfaces an internal `"expected dict"` / `"receiver is not a
                // list"` runtime error instead of the descriptor TypeError that
                // CPython raises.  See Copilot review on #463.
                let kind_ok = match (type_name, self_val.kind()) {
                    ("int", ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)) => true,
                    ("bytes", ValueKind::Bytes(_)) => true,
                    ("str", ValueKind::Str(_)) => true,
                    ("list", ValueKind::List(_)) => true,
                    ("tuple", ValueKind::Tuple(_)) => true,
                    ("dict", ValueKind::Dict(_)) => true,
                    ("set", ValueKind::Set(_)) => true,
                    ("complex", ValueKind::Complex(_, _)) => true,
                    ("frozenset", ValueKind::BuiltinObject { ops, .. })
                        if ops.type_name() == "frozenset" => true,
                    _ => false,
                };
                if !kind_ok {
                    let actual = pyrust_core::builtin_type_name(&self_val);
                    return Err(if is_dunder {
                        pyrust_core::descriptor_requires!(method, type_name, actual)
                    } else {
                        pyrust_core::descriptor_requires!(method, type_name, actual, method)
                    });
                }
                match type_name {
                    "int" => {
                        self.resolve_to_bytes_length(method, &mut pos, &mut kw)?;
                        pyrust_builtins::int::call(method, &self_val, &pos, &kw)
                    }
                    "bytes" => {
                        // Accept bytes-subclass / bytearray args (#1928).
                        let pos = coerce_bytes_subclass_method_args(method, pos);
                        pyrust_builtins::bytes::call(method, &self_val, &pos, &kw)
                    }
                    "str" => {
                        if kw.is_empty() || method == "format" {
                            self.call_str_method(method, self_val, pos)
                        } else {
                            str_merge_kwargs(method, &mut pos, kw)?;
                            self.call_str_method(method, self_val, pos)
                        }
                    }
                    "list" => {
                        if method == "sort" {
                            // Interpreter-aware sort so user `key=` and no-key
                            // user `__lt__` dispatch correctly (#1925).
                            self.list_sort_with_kwargs(&self_val, pos, &kw)
                        } else if method == "remove" {
                            self.call_seq_remove(&self_val, pos)
                        } else if method == "index" || method == "count" {
                            let needs_dispatch = pos.first().map(|t| {
                                self_val
                                    .list_with(|items| Self::seq_search_needs_dispatch(t, items))
                                    .unwrap_or(true)
                            }).unwrap_or(false);
                            let pos = if method == "index" {
                                self.resolve_seq_index_pos(pos)?
                            } else {
                                pos
                            };
                            if needs_dispatch {
                                let snapshot = self_val
                                    .list_with(|items| items.clone())
                                    .ok_or_else(|| {
                                        pyrust_core::type_err!("list.index receiver is not a list")
                                    })?;
                                if method == "index" {
                                    self.call_seq_index(snapshot, &pos, "list")
                                } else {
                                    self.call_seq_count(snapshot, &pos, "list")
                                }
                            } else {
                                pyrust_builtins::list::call(method, &self_val, pos, &kw)
                            }
                        } else {
                            pyrust_builtins::list::call(method, &self_val, pos, &kw)
                        }
                    }
                    "tuple" => {
                        if method == "index" || method == "count" {
                            match self_val.kind() {
                                ValueKind::Tuple(items) => {
                                    let needs_dispatch = pos
                                        .first()
                                        .map(|t| Self::seq_search_needs_dispatch(t, items))
                                        .unwrap_or(false);
                                    let pos = if method == "index" {
                                        self.resolve_seq_index_pos(pos)?
                                    } else {
                                        pos
                                    };
                                    if needs_dispatch {
                                        let snapshot = items.to_vec();
                                        if method == "index" {
                                            self.call_seq_index(snapshot, &pos, "tuple")
                                        } else {
                                            self.call_seq_count(snapshot, &pos, "tuple")
                                        }
                                    } else {
                                        pyrust_builtins::tuple::call(method, items, pos)
                                    }
                                }
                                _ => unreachable!("kind_ok guard above"),
                            }
                        } else {
                            match self_val.kind() {
                                ValueKind::Tuple(items) => {
                                    pyrust_builtins::tuple::call(method, items, pos)
                                }
                                _ => unreachable!("kind_ok guard above"),
                            }
                        }
                    }
                    "dict" => self.call_dict_method(method, self_val, pos, &kw),
                    "set" => self.call_set_method(method, self_val, pos),
                    "complex" => pyrust_builtins::complex::call(method, &self_val, pos),
                    "frozenset" => self.call_frozenset_method(method, self_val, pos),
                    _ => unreachable!("guard matched type_name above"),
                }
            }
            ValueKind::UserFunction(function) => {
                let function = Rc::clone(function);
                self.call_user_function_expanded(function, args, &[])
            }
            ValueKind::PyClass(class) => {
                // #462 perf: primitive classes (`int`, `str`, …) bypass
                // `call_class_expanded` entirely and dispatch straight
                // to the same registry fn that `BuiltinFunction("int")`
                // would.  One `HashMap` lookup + fn pointer call vs.
                // the three-layer Pyinstance-alloc / lookup / re-call
                // chain.
                if let Some(dispatch) = primitive_class_dispatch(class) {
                    return dispatch(self, args);
                }
                let class = Rc::clone(class);
                self.call_class_expanded(class, args)
            }
            ValueKind::BoundMethod { function, receiver } => {
                let function = Rc::clone(function);
                let receiver = Rc::clone(receiver);
                self.call_user_function_expanded(function, args, &[Value::py_instance(receiver)])
            }
            ValueKind::ClassBoundMethod { function, class } => {
                let function = Rc::clone(function);
                let class = Rc::clone(class);
                // First argument is the class itself (not an instance)
                self.call_user_function_expanded(function, args, &[Value::py_class(class)])
            }
            // `format`, `classmethod`, `staticmethod` migrated to
            // `crate::builtin_modules::builtins`.

            // Bound property descriptor-method (`f = p.__get__; f(obj, owner)`).
            // Sits with the property arms, after the hot user-function arm, so
            // the common call fast path is untouched.
            _ if pyrust_builtins::property::as_property_method(&function).is_some() => {
                let (prop, kind) =
                    pyrust_builtins::property::as_property_method(&function).unwrap();
                // CPython's descriptor slot wrappers are positional-only; a
                // keyword argument raises TypeError rather than being bound.
                if args.iter().any(|a| a.name.is_some()) {
                    return Err(pyrust_core::type_err!("this method takes no keyword arguments"));
                }
                let pos: Vec<Value> = args.iter().map(|a| a.value.clone()).collect();
                dispatch_property_method(self, &prop, kind, &pos)
            }

            // Calling prop.setter(fn), prop.deleter(fn), or prop.getter(fn).
            // Returns a new Property with the respective slot replaced.
            _ if pyrust_builtins::property::property_partial_slot(&function)
                .is_some_and(|s| s.is_some()) =>
            {
                let (fget, fset, fdel, slot) =
                    pyrust_builtins::property::with_property(&function, |s| {
                        (
                            Rc::clone(&s.fget),
                            Rc::clone(&s.fset),
                            Rc::clone(&s.fdel),
                            s.partial_slot.expect("guard ensured Some"),
                        )
                    })
                    .expect("guard ensured property");
                let accessor_name = match slot {
                    0 => "property.getter",
                    1 => "property.setter",
                    _ => "property.deleter",
                };
                if args.iter().any(|a| a.name.is_some()) {
                    return Err(pyrust_core::type_err!("{accessor_name}() takes no keyword arguments"));
                }
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!("{accessor_name}() takes exactly one argument ({} given)",
                            args.len()));
                }
                let new_fn = args[0].value.clone();
                let (fget_val, fset_val, fdel_val) = match slot {
                    0 => (new_fn, (*fset).clone(), (*fdel).clone()),
                    1 => ((*fget).clone(), new_fn, (*fdel).clone()),
                    2 => ((*fget).clone(), (*fset).clone(), new_fn),
                    _ => unreachable!(),
                };
                Ok(pyrust_builtins::property::property(
                    fget_val, fset_val, fdel_val,
                ))
            }

            // `property` (named-arm form) and `super` migrated to
            // `crate::builtin_modules::builtins`.  `super` reaches the
            // registry through `#[py_name = "super"]` on its Rust fn
            // (named `super_fn`) — `super` is a strict Rust keyword that
            // rejects even the raw-ident form, so the explicit override is
            // the only way to give it its Python-level name.

            // Calling `classmethod.__get__(instance, owner)` where the
            // classmethod wraps a `UserFunction`.  Returns a ClassBoundMethod
            // with `owner` as the bound class, or the plain function when
            // `owner` is not a recognisable class value.
            _ if pyrust_builtins::classmethod::as_class_method_get_binder(&function)
                .is_some() =>
            {
                let func = pyrust_builtins::classmethod::as_class_method_get_binder(&function)
                    .expect("guard checked above");
                // args[0] = instance, args[1] = owner class.
                // CPython 3.12: __get__(None, None) is invalid.
                let instance = args.first().map(|a| a.value.clone()).unwrap_or_else(Value::none);
                let owner = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
                if matches!(instance.kind(), ValueKind::None)
                    && matches!(owner.kind(), ValueKind::None)
                {
                    return Err(pyrust_core::type_err!("__get__(None, None) is invalid"));
                }
                let class_rc = match owner.kind() {
                    ValueKind::PyClass(c) => Some(Rc::clone(c)),
                    _ => None,
                };
                match class_rc {
                    Some(class_rc) => Ok(Value::class_bound_method(func, class_rc)),
                    None => Ok(Value::user_function(func)),
                }
            }

            // Calling `staticmethod.__get__(instance, owner)` where the
            // staticmethod wraps a `UserFunction`.  Returns the underlying
            // plain function, ignoring both arguments.
            _ if pyrust_builtins::classmethod::as_static_method_get_binder(&function)
                .is_some() =>
            {
                let func = pyrust_builtins::classmethod::as_static_method_get_binder(&function)
                    .expect("guard checked above");
                // CPython 3.12: __get__(None, None) is invalid.
                let instance = args.first().map(|a| a.value.clone()).unwrap_or_else(Value::none);
                let owner = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
                if matches!(instance.kind(), ValueKind::None)
                    && matches!(owner.kind(), ValueKind::None)
                {
                    return Err(pyrust_core::type_err!("__get__(None, None) is invalid"));
                }
                // Prefer wrapped_func to preserve object identity when
                // `sm = staticmethod(fn)` and `sm.__get__(obj, cls) is fn`.
                Ok(if let Some(inner) = func.wrapped_func.as_ref() {
                    Value::user_function(Rc::clone(inner))
                } else {
                    Value::with_function_kind(func, pyrust_core::UserFunctionKind::Regular)
                })
            }

            // Issue #1617: `method_descriptor` objects (e.g. `int.conjugate`)
            // are callable with an explicit instance argument, just like
            // CPython's method_descriptor (`int.conjugate(5)` → 5).
            // Dispatch by calling the named method on the first positional arg.
            _ if pyrust_builtins::numeric_attrs_descriptor::as_method_descriptor(&function)
                .is_some() =>
            {
                let (attr_name, _class_name) =
                    pyrust_builtins::numeric_attrs_descriptor::as_method_descriptor(&function)
                        .expect("guard checked above");
                // Reject keyword arguments — CPython's method_descriptor does not
                // accept them.
                if args.iter().any(|a| a.name.is_some()) {
                    return Err(pyrust_core::type_err!("{}() takes no keyword arguments",
                            attr_name));
                }
                if args.is_empty() {
                    return Err(pyrust_core::descriptor_needs_arg!(attr_name, _class_name, method));
                }
                // Re-dispatch as attribute access on the first argument.
                let remaining = &args[1..];
                let method_val = self.get_attr(&args[0].value, attr_name)?;
                let expanded: Vec<ExpandedCallArg> = remaining
                    .iter()
                    .map(|a| ExpandedCallArg {
                        name: a.name.clone(),
                        value: a.value.clone(),
                    })
                    .collect();
                self.call_function_expanded(method_val, &expanded)
            }

            // Issue #2133: a `GenericAlias` (`list[int]`) is callable and
            // delegates construction to its `__origin__`, so `list[int]([1,2,3])`
            // behaves like `list([1,2,3])` (returning a plain `list`).  The
            // origin's constructor may run user code, so we re-dispatch the call
            // through the interpreter rather than via the receiver-only ops.
            _ if pyrust_builtins::generic_alias::as_generic_alias_origin(&function).is_some() => {
                let origin =
                    pyrust_builtins::generic_alias::as_generic_alias_origin(&function)
                        .expect("guard checked above");
                self.call_function_expanded(origin, args)
            }

            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__call__") {
                    return invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(inst_rc),
                        args,
                    );
                }
                Err(pyrust_core::type_err!("'{}' object is not callable", class.borrow().name))
            }
            _ => Err(pyrust_core::type_err!("'{}' object is not callable",
                    pyrust_core::builtin_type_name(&function))),
        }
    }

    /// Dispatch a call on a bound-method value (`x.append(...)`, `s.upper()`,
    /// …).  Extracted verbatim from `call_function_expanded`'s bound-method
    /// match arm (issue #458 / size reduction); behaviour is identical.  The
    /// caller has already confirmed `function` is a bound method and passes the
    /// unwrapped `(name, receiver)` pair so this helper holds no `function`
    /// coupling.
    /// Dispatch a call on a bound-method value (`x.append(...)`, `s.upper()`,
    /// …).  Thin wrapper that borrows the interpreter's reusable
    /// positional-args buffer and hands it back on every exit path, so the
    /// inner method has a single owner-restore point instead of ~20 scattered
    /// `self.bound_method_pos_buf = pos;` lines.
    fn call_bound_method_dispatch(
        &mut self,
        name_rc: std::rc::Rc<String>,
        receiver_owned: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        let mut pos = std::mem::take(&mut self.bound_method_pos_buf);
        let result =
            self.bound_method_dispatch_inner(name_rc, receiver_owned, args, &mut pos);
        self.bound_method_pos_buf = pos;
        result
    }

    fn bound_method_dispatch_inner(
        &mut self,
        name_rc: std::rc::Rc<String>,
        receiver_owned: Value,
        args: &[ExpandedCallArg],
        pos: &mut Vec<Value>,
    ) -> Result<Value> {
        // Borrow the method name as `&str` from the `Rc<String>` rather
        // than cloning into a fresh `String` — the dispatch helpers
        // below all accept `&str`. See issue #276 item #1.
        let method: &str = name_rc.as_str();
        let receiver = receiver_owned;
        // Reuse the interpreter-level positional-args buffer so that
        // tight loops calling a bound method pay zero allocation once
        // the buffer has grown to the high-watermark arg count (issue
        // #276 item #3).  The buffer is taken with `std::mem::take` so
        // no borrow-checker split is needed; it is restored at the end
        // of this arm (via `bound_method_pos_buf = pos` after the match).
        pos.clear();
        // Keyword-args fast path (issue #276 item #4): skip IndexMap
        // construction entirely when all arguments are positional —
        // the common case for builtin bound methods.
        let has_kw = args.iter().any(|a| a.name.is_some());
        let mut kw: PyDict = PyDict::default();
        if has_kw {
            for a in args {
                match &a.name {
                    Some(n) => { kw.insert(PyKey::str_from(n.as_str()), a.value.clone()); }
                    None => pos.push(a.value.clone()),
                }
            }
        } else {
            for a in args {
                pos.push(a.value.clone());
            }
        }
        // #2151: object-protocol method-wrappers (`__sizeof__`, `__dir__`,
        // `__reduce__`, `__reduce_ex__`, and `None.__bool__`) bound on a
        // built-in data value.  Intercept here — the receiver is already bound,
        // so dispatch directly rather than threading these through every
        // per-type arm below.
        if method.starts_with("__") {
            if crate::interpreter::is_object_protocol_method(&receiver, method) {
                return Ok(self.object_protocol_method_result(method, &receiver));
            }
            // #2191: `__format__` bound on a built-in data value
            // (`(5).__format__('x')`, `"hi".__format__('>5')`,
            // `None.__format__('')`, …).  Route through the same
            // `apply_format_spec` machinery the `format()` builtin uses, so that
            // `x.__format__(spec)` and `format(x, spec)` agree byte-for-byte.
            // Primitives reach here as a bound `__format__` (see
            // `builtin_has_method`); a built-in *subclass* instance
            // (`class I(int)`) that inherits `object.__format__` also reaches
            // here as a bound wrapper (issue #2214) — for it, delegate to
            // `dispatch_dunder_format` so the backing primitive formats the
            // value, matching `format(inst, spec)`.  User `__format__` overrides
            // resolve to a PyFunction, not this wrapper.  Gated under the `__`
            // prefix so the common method-name path is untouched.
            if method == "__format__" {
                if has_kw {
                    return Err(pyrust_core::type_err!(
                        "{}.__format__() takes no keyword arguments",
                        format_dunder_owner(&receiver)
                    ));
                }
                let spec = format_dunder_spec_arg(&receiver, pos)?;
                if matches!(receiver.kind(), ValueKind::PyInstance(_)) {
                    return self.dispatch_dunder_format(&receiver, spec);
                }
                return apply_format_spec(&receiver, spec);
            }
        }
        // `__slots__` member_descriptor's descriptor-protocol methods, invoked
        // directly (`S.x.__get__(inst)`, `S.x.__set__(inst, v)`,
        // `S.x.__delete__(inst)`).  Issue #2084.
        if matches!(method, "__get__" | "__set__" | "__delete__")
            && pyrust_builtins::member_descriptor::as_member_descriptor(&receiver).is_some()
        {
            let args_vec: Vec<Value> = std::mem::take(pos);
            return self.member_descriptor_protocol_call(receiver, method, args_vec);
        }
        // Bound-method dispatch: each builtin takes `&Value`
        // and scopes its own `RefCell::borrow_mut()` for the
        // duration of the operation (#448).  No `&mut Vec /
        // Map / Set` crosses the crate boundary, so the
        // previous `unalias_args_for_mutation` dance is
        // unnecessary — aliasing-safety is structural now,
        // not a discipline the caller has to remember.
        // Probe the receiver's kind via a scoped block so the
        // `kind()` Ref guards drop before we may move
        // `receiver` into call_dict_method / call_set_method
        // (#450).  Variants that read the kind's payload
        // (Tuple, Complex, BuiltinObject, PyInstance) keep
        // working off the live Ref because they don't move
        // the receiver.
        // #1907: frozenset set-algebra method forms must dispatch user
        // `__eq__`.  Intercept here (before the `match receiver.kind()` borrow
        // scrutinee, which would otherwise block moving `receiver`) so a
        // frozenset receiver routes to the `__eq__`-aware interpreter path.
        // `call_frozenset_method` keeps the raw fast path for primitive keys.
        if !has_kw
            && matches!(
                method,
                "union" | "intersection" | "difference" | "symmetric_difference"
                    | "issubset"
                    | "issuperset"
                    | "isdisjoint"
            )
            && pyrust_builtins::frozenset::as_items(&receiver).is_some()
        {
            let args_vec: Vec<Value> = std::mem::take(pos);
            return self.call_frozenset_method(method, receiver, args_vec);
        }
        // #1891: set-like dict views (`dict_keys` / `dict_items`) expose
        // `isdisjoint`, which accepts any iterable and returns True when no
        // element of the argument is in the view.  Iterating the argument (and
        // probing the view's `__contains__`) means a `dict_items` view whose
        // own values are unhashable still works, matching CPython.
        if !has_kw && method == "isdisjoint"
            && let Some(kind) = pyrust_builtins::dict_views::view_kind(&receiver)
                && (kind == 0 || kind == 2) {
                    let args_vec: Vec<Value> = std::mem::take(pos);
                    return self.dict_view_isdisjoint(receiver, args_vec);
                }
        // #2093: dict views are reversible by insertion order; expose
        // `view.__reversed__()` so the dunder is callable (and `reversed(view)`
        // works through the builtin).  Routes back through the `reversed`
        // builtin, which materialises the view in reverse.
        if !has_kw
            && method == "__reversed__"
            && pyrust_builtins::dict_views::view_kind(&receiver).is_some()
        {
            if !pos.is_empty() {
                let view_name = value_type_name_str(&receiver);
                return Err(pyrust_core::type_err!(
                    "{view_name}.__reversed__() takes no arguments ({} given)",
                    pos.len()
                ));
            }
            let arg = ExpandedCallArg { name: None, value: receiver };
            let dispatch = crate::builtin_registry::lookup("reversed")
                .expect("reversed must be in the registry");
            return dispatch(self, &[arg]);
        }
        enum Kind {
            Int,
            Float,
            Bytes,
            Str,
            List,
            Dict,
            Set,
            Other,
        }
        let kind_tag = match receiver.kind() {
            // bool is a subclass of int in CPython; route to the int
            // dispatch so True.bit_length() / True.is_integer() work.
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => Kind::Int,
            ValueKind::Float(_) => Kind::Float,
            ValueKind::Bytes(_) => Kind::Bytes,
            ValueKind::Str(_) => Kind::Str,
            ValueKind::List(_) => Kind::List,
            ValueKind::Dict(_) => Kind::Dict,
            ValueKind::Set(_) => Kind::Set,
            _ => Kind::Other,
        };
        // __iter__ on any iterable built-in type delegates to the same
        // logic as the `iter()` built-in: wrap the receiver in a
        // NativeIterFrame generator.  Intercept here before the per-type
        // arms so that none of the crate-level `call` functions need to
        // handle "__iter__" (they would error on an unknown method name).
        if method == "__iter__" {
            let is_iterable_builtin = matches!(
                receiver.kind(),
                ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Str(_)
                    | ValueKind::Bytes(_)
                    | ValueKind::Dict(_)
                    | ValueKind::Set(_)
                    | ValueKind::Range { .. }
            ) || matches!(receiver.kind(), ValueKind::BuiltinObject { ops, .. }
                if ops.has_method("__iter__"));
            if is_iterable_builtin {
                reject_kwargs!(kw, "wrapper __iter__");
                if !pos.is_empty() {
                    let n = pos.len();
                    return Err(pyrust_core::type_err!("expected 0 arguments, got {n}"));
                }
                let iter_arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup("iter")
                    .expect("iter must be in the registry");
                return dispatch(self, &[iter_arg]);
            }
        }
        // Issue #1909: container/sequence protocol dunders exposed as bound
        // method-wrappers (`obj.__getitem__(i)`, `obj.__add__(o)`, …).  The
        // receiver here is a built-in primitive (the bound-method wrapper was
        // constructed by `get_attr` only for `builtin_protocol_dunders`
        // names), so dispatch straight through the operator machinery.
        if method.starts_with("__")
            && builtin_protocol_dunders(&pyrust_core::builtin_type_name(&receiver))
                .contains(&method)
        {
            reject_kwargs!(kw, "{}", method);
            let args_vec: Vec<Value> = std::mem::take(pos);
            return self.dispatch_builtin_protocol_dunder(method, receiver, args_vec);
        }
        // Arms that accept `&[Value]` (Int, Float, Bytes) borrow `pos`
        // directly — the buf's capacity is fully preserved on return.
        // Arms that need `Vec<Value>` ownership hand the buffer to the
        // callee via `std::mem::take(pos)` (issue #1863): a single
        // pointer swap — no allocation and no element move (vs the old
        // `pos.drain(..).collect()`, which allocated a fresh Vec and
        // copied every element).  `pos` is left as an empty zero-cap Vec
        // and re-grows from the next call's `pos.push(...)`; either way
        // exactly one allocation happens per call.  The buf is restored
        // to `bound_method_pos_buf` below.  Error paths also restore it.
        let result = match kind_tag {
            Kind::Int => {
                self.resolve_to_bytes_length(method, pos, &mut kw)?;
                pyrust_builtins::int::call(method, &receiver, &pos[..], &kw)
            }
            Kind::Float => {
                let f = match receiver.kind() {
                    ValueKind::Float(f) => f,
                    _ => unreachable!("kind_tag guard above"),
                };
                pyrust_builtins::float::call(method, f, pos)
            }
            Kind::Bytes => {
                if method == "join" {
                    reject_kwargs!(kw, "bytes.join");
                    let args_vec: Vec<Value> = std::mem::take(pos);
                    self.call_bytes_join(receiver, args_vec)
                } else {
                    // Accept bytes-subclass / bytearray args (#1928).
                    let args_vec = coerce_bytes_subclass_method_args(
                        method,
                        std::mem::take(pos),
                    );
                    pyrust_builtins::bytes::call(method, &receiver, &args_vec, &kw)
                }
            }
            Kind::Str => {
                // `format` needs kwargs threaded into the template.
                // Intercept before `call_str_method`, which only receives
                // positional args and would silently drop keyword arguments.
                if method == "format" {
                    let template = match receiver.kind() {
                        ValueKind::Str(s) => s.to_string(),
                        _ => {
                            let actual = pyrust_core::builtin_type_name(&receiver);
                            return Err(pyrust_core::descriptor_requires!(
                                "format", "str", actual, method
                            ));
                        }
                    };
                    let keyword: Vec<(String, Value)> = kw
                        .into_iter()
                        .filter_map(|(k, v)| {
                            if let PyKey::Str(name) = k {
                                Some((name.as_str().unwrap_or("").to_owned(), v))
                            } else {
                                None
                            }
                        })
                        .collect();
                    // Borrow pos; capacity retained in the buf below.
                    self.format_str_template(&template, pos, &keyword)
                } else if !kw.is_empty() {
                    // Resolve kwargs for str methods before passing to
                    // call_str_method, which only accepts positional args.
                    match str_merge_kwargs(method, pos, kw) {
                        Ok(()) => {
                            let args_vec: Vec<Value> = std::mem::take(pos);
                            self.call_str_method(method, receiver, args_vec)
                        }
                        Err(e) => {
                            return Err(e);
                        }
                    }
                } else {
                    // call_str_method takes Vec<Value> by value; hand
                    // the buffer over with mem::take (see #1863).
                    let args_vec: Vec<Value> = std::mem::take(pos);
                    self.call_str_method(method, receiver, args_vec)
                }
            }
            Kind::List => {
                let args_vec: Vec<Value> = std::mem::take(pos);
                if method == "sort" {
                    // Route through the interpreter-aware sort so a user
                    // `key=` callable and user `__lt__` (no-key) both dispatch
                    // correctly — `pyrust_builtins::list::call` can reach
                    // neither (#1925).
                    self.list_sort_with_kwargs(&receiver, args_vec, &kw)
                } else if method == "remove" {
                    self.call_seq_remove(&receiver, args_vec)
                } else if method == "index" || method == "count" {
                    let needs_dispatch = args_vec.first().map(|t| {
                        receiver
                            .list_with(|items| Self::seq_search_needs_dispatch(t, items))
                            .unwrap_or(true)
                    }).unwrap_or(false);
                    let args_vec = if method == "index" {
                        match self.resolve_seq_index_pos(args_vec) {
                            Ok(v) => v,
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    } else {
                        args_vec
                    };
                    if needs_dispatch {
                        // Snapshot so we can release the list borrow
                        // before `values_user_eq` may re-enter user code.
                        let snapshot = receiver
                            .list_with(|items| items.clone())
                            .ok_or_else(|| {
                                pyrust_core::type_err!("list.index receiver is not a list")
                            })?;
                        if method == "index" {
                            self.call_seq_index(snapshot, &args_vec, "list")
                        } else {
                            self.call_seq_count(snapshot, &args_vec, "list")
                        }
                    } else {
                        pyrust_builtins::list::call(method, &receiver, args_vec, &kw)
                    }
                } else {
                    pyrust_builtins::list::call(method, &receiver, args_vec, &kw)
                }
            }
            Kind::Dict => {
                let args_vec: Vec<Value> = std::mem::take(pos);
                self.call_dict_method(method, receiver, args_vec, &kw)
            }
            Kind::Set => {
                let args_vec: Vec<Value> = std::mem::take(pos);
                self.call_set_method(method, receiver, args_vec)
            }
            Kind::Other => match receiver.kind() {
            ValueKind::Tuple(items) => {
                let args_vec: Vec<Value> = std::mem::take(pos);
                if method == "index" || method == "count" {
                    let needs_dispatch = args_vec
                        .first()
                        .map(|t| Self::seq_search_needs_dispatch(t, items))
                        .unwrap_or(false);
                    let args_vec = if method == "index" {
                        match self.resolve_seq_index_pos(args_vec) {
                            Ok(v) => v,
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    } else {
                        args_vec
                    };
                    if needs_dispatch {
                        // Snapshot to release the tuple Ref before
                        // `values_user_eq` may re-enter user code.
                        let snapshot = items.to_vec();
                        if method == "index" {
                            self.call_seq_index(snapshot, &args_vec, "tuple")
                        } else {
                            self.call_seq_count(snapshot, &args_vec, "tuple")
                        }
                    } else {
                        pyrust_builtins::tuple::call(method, items, args_vec)
                    }
                } else {
                    pyrust_builtins::tuple::call(method, items, args_vec)
                }
            }
            ValueKind::Complex(_, _) => {
                let args_vec: Vec<Value> = std::mem::take(pos);
                pyrust_builtins::complex::call(method, &receiver, args_vec)
            }
            ValueKind::BuiltinObject { ops, state } => {
                let mut args_vec: Vec<Value> = std::mem::take(pos);
                // bytearray methods accept bytes-subclass / bytearray args
                // (#1928); coerce them to a real `Bytes` value before the
                // receiver-only ops extractors (which match exact `Bytes`) see
                // them.  Other BuiltinObject types (frozenset) are untouched.
                if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME {
                    if method == "join" {
                        // join's single iterable arg holds the items to join;
                        // coerce its bytes-subclass / bytearray elements.
                        args_vec = args_vec
                            .into_iter()
                            .map(coerce_bytes_subclass_join_iterable)
                            .collect();
                    } else {
                        args_vec = coerce_bytes_subclass_method_args(method, args_vec);
                    }
                }
                // Thread any keyword arguments through to the builtin object
                // (e.g. `bytearray.split(maxsplit=1)`); `call_method` keeps its
                // kwargs `String`-keyed.
                let kw_str: indexmap::IndexMap<String, Value> = kw
                    .iter()
                    .map(|(k, v)| {
                        let key = match k {
                            PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                            _ => String::new(),
                        };
                        (key, v.clone())
                    })
                    .collect();
                ops.call_method(state, method, args_vec, &kw_str)
            }
            ValueKind::PyInstance(inst) => {
                // Class method backed by a `BuiltinFunction` — emitted
                // by `pyrust_module!`'s `class { … }` block.  `get_attr`
                // wrapped the method-name-on-instance pair as a
                // bound_method; here we re-resolve the method on the
                // class and dispatch with `self` prepended through the
                // unified helper.
                let class = Rc::clone(&inst.borrow().class);
                let method_val = match lookup_class_attr(&class, method) {
                    Some(v) => v,
                    None => {
                        let class_name = class.borrow().name.clone();
                        return Err(PyError::attribute_error(
                            format!("'{class_name}' object has no attribute '{method}'"),
                            Some(method.to_string()),
                            Some(Value::py_instance(Rc::clone(inst))),
                        ));
                    }
                };
                // Issue #976/#994: if the resolved method is a primitive
                // builtin (e.g. `dict.keys`, `tuple.count`), and the
                // instance has a `__builtin_data__` backing value (set at
                // construction time for subclasses of dict/list/set/
                // frozenset/tuple), dispatch on the backing value directly.
                // This avoids the `kind_ok` type guard in
                // `call_function_expanded` rejecting the PyInstance receiver.
                // Issue #1204: same mechanism for str/int/float/bytes subclasses.
                if let ValueKind::BuiltinFunction(fn_name) = method_val.kind()
                    && fn_name.split_once('.').is_some_and(|(t, _)| {
                        matches!(t, "dict" | "list" | "set" | "frozenset" | "tuple"
                            | "str" | "int" | "float" | "bytes")
                    })
                        && let Some(backing) = instance_builtin_data(inst) {
                            // Issue #1909: container protocol dunders
                            // (`MyList().__len__()`, `MyDict().__getitem__(k)`)
                            // operate on the backing primitive — route through
                            // the shared dispatcher so they match the
                            // plain-primitive form instead of leaking a
                            // `RuntimeError` from the per-type `call`.
                            if method.starts_with("__")
                                && builtin_protocol_dunders(
                                    &pyrust_core::builtin_type_name(&backing),
                                )
                                .contains(&method)
                            {
                                reject_kwargs!(kw, "{}", method);
                                let args_vec: Vec<Value> = std::mem::take(pos);
                                return self.dispatch_builtin_protocol_dunder(
                                    method, backing, args_vec,
                                );
                            }
                            enum BkKind {
                                Dict, List, Set, Frozenset, Tuple,
                                Str, Int, Float, Bytes, Other,
                            }
                            let bk_kind = match backing.kind() {
                                ValueKind::Dict(_) => BkKind::Dict,
                                ValueKind::List(_) => BkKind::List,
                                ValueKind::Set(_) => BkKind::Set,
                                ValueKind::BuiltinObject { ops, .. }
                                    if ops.type_name() == "frozenset" =>
                                {
                                    BkKind::Frozenset
                                }
                                ValueKind::Tuple(_) => BkKind::Tuple,
                                ValueKind::Str(_) => BkKind::Str,
                                ValueKind::Int(_)
                                | ValueKind::BigInt(_)
                                | ValueKind::Bool(_) => BkKind::Int,
                                ValueKind::Float(_) => BkKind::Float,
                                ValueKind::Bytes(_) => BkKind::Bytes,
                                _ => BkKind::Other,
                            };
                            let args_vec: Vec<Value> = std::mem::take(pos);
                            return match bk_kind {
                                BkKind::Dict => {
                                    // Issue #1563: `fromkeys` is a classmethod; when
                                    // called on a subclass instance (`MyDict().fromkeys`)
                                    // CPython uses `type(self)` as `cls`, so the result
                                    // is a `MyDict`, not a plain `dict`.  Route through
                                    // the same class-dispatch path used for
                                    // `MyDict.fromkeys` (bound-method on PyClass).
                                    if method == "fromkeys" {
                                        let bound = pyrust_builtins::bound_method::bound_method(
                                            "fromkeys",
                                            Value::py_class(Rc::clone(&class)),
                                        );
                                        let mut expanded: Vec<ExpandedCallArg> =
                                            args_vec
                                                .into_iter()
                                                .map(|v| ExpandedCallArg {
                                                    name: None,
                                                    value: v,
                                                })
                                                .collect();
                                        for (k, v) in &kw {
                                            if let PyKey::Str(s) = k {
                                                expanded.push(ExpandedCallArg {
                                                    name: Some(
                                                        s.as_str()
                                                            .unwrap_or("")
                                                            .to_owned(),
                                                    ),
                                                    value: v.clone(),
                                                });
                                            }
                                        }
                                        return self.call_function_expanded(
                                            bound, &expanded,
                                        );
                                    }
                                    self.call_dict_method(method, backing, args_vec, &kw)
                                }
                                BkKind::List => {
                                    if method == "sort" {
                                        // Interpreter-aware sort so user `key=`
                                        // and no-key user `__lt__` dispatch
                                        // (#1925).
                                        self.list_sort_with_kwargs(
                                            &backing, args_vec, &kw,
                                        )
                                    } else if method == "remove" {
                                        self.call_seq_remove(&backing, args_vec)
                                    } else if method == "index" || method == "count" {
                                        let needs_dispatch =
                                            args_vec.first().map(|t| {
                                                backing
                                                    .list_with(|items| {
                                                        Self::seq_search_needs_dispatch(
                                                            t, items,
                                                        )
                                                    })
                                                    .unwrap_or(true)
                                            }).unwrap_or(false);
                                        if needs_dispatch {
                                            let snapshot = backing
                                                .list_with(|items| items.clone())
                                                .ok_or_else(|| {
                                                    pyrust_core::type_err!("list.index receiver is not a list"
                                                            .to_string())
                                                })?;
                                            if method == "index" {
                                                self.call_seq_index(
                                                    snapshot, &args_vec, "list",
                                                )
                                            } else {
                                                self.call_seq_count(
                                                    snapshot, &args_vec, "list",
                                                )
                                            }
                                        } else {
                                            pyrust_builtins::list::call(
                                                method,
                                                &backing,
                                                args_vec,
                                                &kw,
                                            )
                                        }
                                    } else {
                                        pyrust_builtins::list::call(
                                            method,
                                            &backing,
                                            args_vec,
                                            &kw,
                                        )
                                    }
                                }
                                BkKind::Set => {
                                    self.call_set_method(method, backing, args_vec)
                                }
                                BkKind::Frozenset => {
                                    self.call_frozenset_method(method, backing, args_vec)
                                }
                                BkKind::Tuple => match backing.kind() {
                                    ValueKind::Tuple(items) => {
                                        if method == "index" || method == "count" {
                                            let needs_dispatch = args_vec
                                                .first()
                                                .map(|t| {
                                                    Self::seq_search_needs_dispatch(
                                                        t, items,
                                                    )
                                                })
                                                .unwrap_or(false);
                                            if needs_dispatch {
                                                let snapshot = items.to_vec();
                                                if method == "index" {
                                                    self.call_seq_index(
                                                        snapshot, &args_vec, "tuple",
                                                    )
                                                } else {
                                                    self.call_seq_count(
                                                        snapshot, &args_vec, "tuple",
                                                    )
                                                }
                                            } else {
                                                pyrust_builtins::tuple::call(
                                                    method,
                                                    items,
                                                    args_vec,
                                                )
                                            }
                                        } else {
                                            pyrust_builtins::tuple::call(
                                                method,
                                                items,
                                                args_vec,
                                            )
                                        }
                                    }
                                    _ => unreachable!("BkKind::Tuple guard above"),
                                },
                                BkKind::Str => {
                                    // `str.format` on a subclass receiver: thread
                                    // kwargs into the template, mirroring the
                                    // plain-str bound-method path above.
                                    // `call_str_method` only accepts positional
                                    // args, so it can't reach `format` (it would
                                    // hit the drift-guard sentinel in
                                    // pyrust_builtins::string) and would silently
                                    // drop keyword fields (#2376).
                                    if method == "format" {
                                        let template = match backing.kind() {
                                            ValueKind::Str(s) => s.to_string(),
                                            _ => unreachable!(
                                                "BkKind::Str guard above"
                                            ),
                                        };
                                        let keyword: Vec<(String, Value)> = kw
                                            .into_iter()
                                            .filter_map(|(k, v)| {
                                                if let PyKey::Str(name) = k {
                                                    Some((
                                                        name.as_str()
                                                            .unwrap_or("")
                                                            .to_owned(),
                                                        v,
                                                    ))
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect();
                                        self.format_str_template(
                                            &template, &args_vec, &keyword,
                                        )
                                    } else {
                                        self.call_str_method(method, backing, args_vec)
                                    }
                                }
                                BkKind::Int => {
                                    let mut int_args = args_vec;
                                    self.resolve_to_bytes_length(
                                        method,
                                        &mut int_args,
                                        &mut kw,
                                    )?;
                                    pyrust_builtins::int::call(
                                        method,
                                        &backing,
                                        &int_args,
                                        &kw,
                                    )
                                }
                                BkKind::Float => {
                                    let f = match backing.kind() {
                                        ValueKind::Float(f) => f,
                                        _ => unreachable!("BkKind::Float guard above"),
                                    };
                                    pyrust_builtins::float::call(method, f, &args_vec)
                                }
                                BkKind::Bytes => {
                                    // Accept bytes-subclass / bytearray args
                                    // (#1928).
                                    let args_vec = coerce_bytes_subclass_method_args(
                                        method, args_vec,
                                    );
                                    pyrust_builtins::bytes::call(
                                        method,
                                        &backing,
                                        &args_vec,
                                        &kw,
                                    )
                                }
                                BkKind::Other => Err(PyError::Runtime(format!(
                                    "internal: unexpected builtin_data kind for method '{method}'"
                                ))),
                            };
                        }
                // Reconstitute kwargs as ExpandedCallArgs (the
                // bound_method dispatch split them into pos+kw maps).
                // Drain pos so its capacity is preserved in the buf.
                let mut combined: ExpandedArgBuf =
                    ExpandedArgBuf::with_capacity(pos.len() + kw.len());
                for v in pos.drain(..) {
                    combined.push(ExpandedCallArg { name: None, value: v });
                }
                for (k, v) in kw {
                    if let PyKey::Str(name) = k {
                        combined.push(ExpandedCallArg {
                            name: Some(name.as_str().unwrap_or("").to_owned()),
                            value: v,
                        });
                    }
                }
                invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst.clone()),
                    &combined,
                )
            }
            ValueKind::PyClass(class) => {
                // `type.mro()` — returns the MRO as a list (same entries
                // as `__mro__` tuple).  CPython's `type.mro(self)` is a
                // C slot that returns `list(self.__mro__)`.
                //
                // Two call forms:
                //   B.mro()          → bound receiver is B, pos is empty
                //   type.mro(B)      → bound receiver is `type`, pos[0] is B
                let class = Rc::clone(class);
                match method {
                    "mro" => {
                        // Determine the target class.  When the bound receiver
                        // IS the `type` metaclass, the first positional arg is
                        // the self argument (unbound descriptor form).  When
                        // the receiver is any other class, mro() takes no
                        // extra arguments.
                        let receiver_is_type =
                            Rc::ptr_eq(&class, &type_class_singleton());
                        let target_class: Rc<RefCell<PyClass>> = if receiver_is_type {
                            // Unbound descriptor call: type.mro(B).
                            // Requires exactly one positional arg that is a type,
                            // with no extra positional or keyword arguments.
                            if pos.is_empty() {
                                return Err(pyrust_core::descriptor_needs_arg!("mro", "type", method));
                            }
                            // pos[0] is the self (type) argument.
                            let maybe_class = match pos[0].kind() {
                                ValueKind::PyClass(c) => Some(Rc::clone(c)),
                                _ => None,
                            };
                            let target = match maybe_class {
                                Some(c) => c,
                                None => {
                                    let type_name = pyrust_core::builtin_type_name(&pos[0]).to_string();
                                    return Err(pyrust_core::type_err!("descriptor 'mro' for 'type' objects doesn't apply to a '{type_name}' object",));
                                }
                            };
                            // After resolving self, no extra positional args or kwargs allowed.
                            reject_kwargs!(kw, "type.mro");
                            if pos.len() > 1 {
                                let extra = pos.len() - 1;
                                return Err(pyrust_core::type_err!("type.mro() takes no arguments ({extra} given)",));
                            }
                            target
                        } else if pos.is_empty() && kw.is_empty() {
                            // Bound call: B.mro()
                            class
                        } else if !kw.is_empty() {
                            // Keyword arguments are never accepted.
                            let class_name = class.borrow().name.clone();
                            return Err(pyrust_core::type_err!("{class_name}.mro() takes no keyword arguments"));
                        } else {
                            // Too many positional arguments.
                            let n = pos.len();
                            let class_name = class.borrow().name.clone();
                            return Err(pyrust_core::type_err!("{class_name}.mro() takes no arguments ({n} given)",));
                        };
                        Ok(Value::list(class_mro_items(&target_class)?))
                    }
                    "__subclasses__" => {
                        // CPython: type.__subclasses__(self) → list of direct subclasses.
                        // Takes no arguments. Prunes stale weak refs lazily.
                        // CPython distinguishes kw vs positional:
                        //   A.__subclasses__(1)       → "takes no arguments (1 given)"
                        //   A.__subclasses__(x=1)     → "takes no keyword arguments"
                        //   A.__subclasses__(1, x=2)  → "takes no keyword arguments"
                        let class_name = class.borrow().name.clone();
                        reject_kwargs!(kw, "{class_name}.__subclasses__");
                        let n_pos = pos.len();
                        if n_pos > 0 {
                            return Err(pyrust_core::type_err!("{class_name}.__subclasses__() takes no arguments ({n_pos} given)",));
                        }
                        Ok(Value::list(class_direct_subclasses(&class)))
                    }
                    // Issue #1563: `dict.fromkeys` classmethod on a dict subclass.
                    // `env.rs` binds the subclass as the receiver when `fromkeys` is
                    // looked up on a non-primitive dict subclass.  Collect the iterable
                    // and optional default, call `cls()` to construct an empty instance,
                    // then replace its `__builtin_data__` with the populated dict.
                    "fromkeys" => {
                        reject_kwargs!(kw, "{}.fromkeys", class.borrow().name);
                        if pos.is_empty() {
                            return Err(pyrust_core::type_err!("fromkeys expected at least 1 argument, got 0"));
                        }
                        if pos.len() > 2 {
                            let n = pos.len();
                            return Err(pyrust_core::type_err!("fromkeys expected at most 2 arguments, got {n}",));
                        }
                        let default_val =
                            pos.get(1).cloned().unwrap_or_else(Value::none);
                        // Collect keys before moving pos into bound_method_pos_buf
                        // so we can borrow &pos[0] without an extra clone.
                        let keys = self.collect_iterable(&pos[0])?;
                        let mut map: PyDict =
                            PyDict::with_capacity_and_hasher(keys.len(), Default::default());
                        for key in &keys {
                            let py_key = self.value_to_pykey(key)?;
                            map.entry(py_key).or_insert_with(|| default_val.clone());
                        }
                        // Construct `cls()` with no arguments to get a subclass instance
                        // (matching CPython's `dict.fromkeys` classmethod semantics).
                        let instance =
                            self.call_class_expanded(Rc::clone(&class), &[])?;
                        // Replace the empty `__builtin_data__` backing set by
                        // `call_class_expanded` with the populated dict.
                        if let ValueKind::PyInstance(inst_rc) = instance.kind() {
                            inst_rc.borrow_mut().attrs.insert(
                                BUILTIN_DATA_ATTR,
                                Value::dict(map),
                            );
                        }
                        return Ok(instance);
                    }
                    _ => {
                        let class_name = class.borrow().name.clone();
                        return Err(PyError::attribute_error(
                            format!("type object '{class_name}' has no attribute '{method}'"),
                            Some(method.to_string()),
                            Some(Value::py_class(Rc::clone(&class))),
                        ));
                    }
                }
            }
            // Range methods: count, index, __len__ (issue #1807).
            // These are dispatched directly here rather than through
            // pyrust_builtins because range is not a BuiltinObject.
            ValueKind::Range { start, stop, step } => {
                let args_vec: Vec<Value> = std::mem::take(pos);
                match method {
                    "__len__" => {
                        let extra = args_vec.len() + kw.len();
                        if extra != 0 {
                            Err(pyrust_core::type_err!("expected 0 arguments, got {extra}"))
                        } else {
                            use crate::value::range_len;
                            Ok(Value::int(range_len(start, stop, step)))
                        }
                    }
                    "count" => {
                        if args_vec.len() != 1 || !kw.is_empty() {
                            Err(pyrust_core::type_err!("range.count() takes exactly one argument ({} given)",
                                    args_vec.len() + kw.len()))
                        } else {
                            let contained = self.range_contains_value(
                                start, stop, step, &args_vec[0],
                            )?;
                            Ok(Value::int(if contained { 1 } else { 0 }))
                        }
                    }
                    "index" => {
                        if args_vec.len() != 1 || !kw.is_empty() {
                            Err(pyrust_core::type_err!("range.index() takes exactly one argument ({} given)",
                                    args_vec.len() + kw.len()))
                        } else {
                            use crate::value::PyToPrimitive;
                            let v = &args_vec[0];
                            // Convert v to i64 if possible; non-integer types
                            // can never be in a range (CPython returns the
                            // "x not in sequence" message for those).
                            let vi_opt: Option<i64> = match v.kind() {
                                ValueKind::Int(x) => Some(x),
                                ValueKind::Bool(b) => Some(b as i64),
                                ValueKind::BigInt(n) => n.to_i64(),
                                ValueKind::Float(f) => {
                                    const I64_MIN_F: f64 = i64::MIN as f64;
                                    const I64_MAX_PLUS1_F: f64 =
                                        9_223_372_036_854_775_808.0_f64;
                                    if f.is_finite()
                                        && f.fract() == 0.0
                                        && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&f)
                                    {
                                        Some(f as i64)
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            match vi_opt {
                                None => Err(pyrust_core::value_err!("sequence.index(x): x not in sequence")),
                                Some(vi) => {
                                    let contained = self.range_contains_value(
                                        start, stop, step, v,
                                    )?;
                                    if contained {
                                        Ok(Value::int((vi - start) / step))
                                    } else {
                                        Err(pyrust_core::value_err!("{} is not in range", v.repr()))
                                    }
                                }
                            }
                        }
                    }
                    _ => Err(pyrust_core::py_err!("AttributeError", "'range' object has no attribute '{method}'")),
                }
            }
            // Arbitrary-precision range methods (#2118): __len__ / count / index
            // in BigInt arithmetic.  start/stop/step are cloned out of the borrow
            // first so the helpers can take &mut self if needed.
            ValueKind::BigRange { start, stop, step } => {
                let (start, stop, step) = (start.clone(), stop.clone(), step.clone());
                let args_vec: Vec<Value> = std::mem::take(pos);
                match method {
                    "__len__" => {
                        let extra = args_vec.len() + kw.len();
                        if extra != 0 {
                            Err(pyrust_core::type_err!("expected 0 arguments, got {extra}"))
                        } else {
                            match pyrust_core::bigrange_len(&start, &stop, &step).to_i64() {
                                Some(n) => Ok(Value::int(n)),
                                None => Err(pyrust_core::py_err!(
                                    "OverflowError",
                                    "Python int too large to convert to C ssize_t".to_string()
                                )),
                            }
                        }
                    }
                    "count" => {
                        if args_vec.len() != 1 || !kw.is_empty() {
                            Err(pyrust_core::type_err!("range.count() takes exactly one argument ({} given)",
                                    args_vec.len() + kw.len()))
                        } else {
                            let contained =
                                Self::bigrange_member(&start, &stop, &step, &args_vec[0]).is_some();
                            Ok(Value::int(if contained { 1 } else { 0 }))
                        }
                    }
                    "index" => {
                        if args_vec.len() != 1 || !kw.is_empty() {
                            Err(pyrust_core::type_err!("range.index() takes exactly one argument ({} given)",
                                    args_vec.len() + kw.len()))
                        } else {
                            let v = &args_vec[0];
                            // CPython distinguishes a non-integer subscript (which
                            // never has an index → "x not in sequence") from an
                            // integer that simply isn't a member of this range
                            // ("{} is not in range").
                            let is_int_valued = matches!(
                                v.kind(),
                                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                            ) || matches!(v.kind(), ValueKind::Float(f) if f.is_finite() && f.fract() == 0.0);
                            match Self::bigrange_member(&start, &stop, &step, v) {
                                Some(x) => Ok(value_from_bigint((x - &start) / &step)),
                                None if is_int_valued => {
                                    Err(pyrust_core::value_err!("{} is not in range", v.repr()))
                                }
                                None => Err(pyrust_core::value_err!(
                                    "sequence.index(x): x not in sequence"
                                )),
                            }
                        }
                    }
                    _ => Err(pyrust_core::py_err!("AttributeError", "'range' object has no attribute '{method}'")),
                }
            }
            // Issue #1413: generator bound-methods (__iter__, __next__,
            // send, close, throw) captured via get_attr route here.
            // Delegate to call_generator_method which holds all the VM
            // logic for these operations.  Clone the receiver to release
            // the borrow held by the outer `match receiver.kind()`.
            ValueKind::Generator(_) => {
                let gen_val = receiver.clone();
                let args_vec: Vec<Value> = std::mem::take(pos);
                self.call_generator_method(gen_val, method, args_vec)
            }
            _ => Err(pyrust_core::type_err!("'{}' object has no method '{method}'", pyrust_core::builtin_type_name(&receiver))),
            },
        };
        // Restore the positional-args buffer.  For borrow arms (Int,
        // Float, Str::format) pos still holds all elements with full
        // capacity.  For mem::take arms (Bytes, Str, List, …) pos is an
        // empty zero-cap Vec (its old buffer went to the callee); it
        // re-grows on next call.
        result
    }

    /// Collect all values from an iterable (including generators) into a Vec.
    pub(crate) fn collect_iterable(&mut self, val: &Value) -> Result<Vec<Value>> {
        // A coroutine (`async def`, issue #1039) is not iterable — `list(coro)`,
        // `tuple(coro)`, unpacking, etc. all raise TypeError, matching CPython.
        if crate::builtin_modules::builtins::is_coroutine_value(val) {
            return Err(pyrust_core::type_err!("'coroutine' object is not iterable"));
        }
        if let ValueKind::Generator(state_rc) = val.kind() {
            let state_rc = Rc::clone(state_rc);

            // Fast path: NativeIterFrame — drain remaining items in one shot.
            //
            // `try_borrow_mut`: the cell is mutably checked out while the
            // generator's own body executes, so a re-entrant `list(g)` /
            // `sum(g)` / unpack from inside `g` raises CPython's
            // `ValueError: generator already executing` instead of panicking
            // on the re-borrow (issue #2285).
            let mut probe = state_rc.try_borrow_mut().map_err(|_| {
                pyrust_core::value_err!("generator already executing")
            })?;
            if let Some(native) = probe.downcast_mut::<NativeIterFrame>() {
                let remaining: Vec<Value> = native.items[native.pos..].to_vec();
                native.pos = native.items.len();
                return Ok(remaining);
            }

            // Single-probe dispatch on the concrete iterator-state type for
            // everything else; see `call_next` for the rationale (#2315).
            let tid = {
                let any_ref: &dyn std::any::Any = &**probe;
                any_ref.type_id()
            };
            drop(probe);
            use std::any::TypeId;

            // A `GenDriving` placeholder means the frame is checked out by
            // the gen-drive trampoline (#2253) — the generator is executing,
            // so collecting it re-entrantly raises the already-executing
            // error (issue #2285).
            if tid == TypeId::of::<GenDriving>() {
                return Err(pyrust_core::value_err!("generator already executing"));
            }

            // GeneratorFrame path: drive the generator to exhaustion.
            if tid == TypeId::of::<GeneratorFrame>() {
                let mut items = Vec::new();
                loop {
                    let mut borrow = state_rc.try_borrow_mut().map_err(|_| {
                        pyrust_core::value_err!("generator already executing")
                    })?;
                    if borrow.is::<GenDriving>() {
                        return Err(pyrust_core::value_err!("generator already executing"));
                    }
                    let frame = borrow
                        .downcast_mut::<GeneratorFrame>()
                        .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
                    if frame.done {
                        break;
                    }
                    match self.resume_generator(frame) {
                        Ok(yielded) => {
                            drop(borrow);
                            items.push(yielded);
                        }
                        Err(ref e) if e.class_name_is("StopIteration") => {
                            break;
                        }
                        Err(e) => return Err(e),
                    }
                }
                return Ok(items);
            }

            // CallableIter path: drive iter(callable, sentinel) to exhaustion.
            if tid == TypeId::of::<CallableIter>() {
                let mut items = Vec::new();
                loop {
                    match self.step_callable_iter(&state_rc) {
                        Ok(Some(v)) => items.push(v),
                        Ok(None) => break,
                        // StopIteration raised by the callable stops the
                        // iteration gracefully — CPython treats it the same
                        // as hitting the sentinel (list/tuple exhaustion).
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    }
                }
                return Ok(items);
            }

            // GetItemIter path: drive __getitem__(0), __getitem__(1), … to exhaustion.
            if tid == TypeId::of::<GetItemIter>() {
                let mut items = Vec::new();
                while let Some(v) = self.step_getitem_iter(&state_rc)? {
                    items.push(v);
                }
                return Ok(items);
            }

            // MapIter path: drive map() to exhaustion.
            if tid == TypeId::of::<MapIter>() {
                let mut items = Vec::new();
                while let Some(v) = self.step_map_iter(&state_rc)? {
                    items.push(v);
                }
                return Ok(items);
            }

            // FilterIter path: drive filter() to exhaustion.
            if tid == TypeId::of::<FilterIter>() {
                let mut items = Vec::new();
                while let Some(v) = self.step_filter_iter(&state_rc)? {
                    items.push(v);
                }
                return Ok(items);
            }

            // EnumerateIter path: drive enumerate() to exhaustion.
            if tid == TypeId::of::<EnumerateIter>() {
                let mut items = Vec::new();
                while let Some(v) = self.step_enumerate_iter(&state_rc)? {
                    items.push(v);
                }
                return Ok(items);
            }

            // ZipIter path: drive zip() to exhaustion.
            if tid == TypeId::of::<ZipIter>() {
                let mut items = Vec::new();
                while let Some(v) = self.step_zip_iter(&state_rc)? {
                    items.push(v);
                }
                return Ok(items);
            }

            // BigRangeIter path: drive a lazy big range to exhaustion (#2118).
            if tid == TypeId::of::<BigRangeIter>() {
                let mut items = Vec::new();
                while let Some(v) = self.step_bigrange_iter(&state_rc)? {
                    items.push(v);
                }
                return Ok(items);
            }

            Err(PyError::Runtime("invalid generator state".to_string()))
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            // CPython fallback order is `__iter__` first, then the legacy
            // sequence-iter protocol via `__getitem__`.  Having only
            // `__next__` does *not* make a class iterable — that property
            // belongs to iterator objects, not iterables (#416 Copilot
            // review).
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Check __builtin_data__ before __getitem__: list/dict/set subclasses
            // with no user-defined __iter__ should iterate the backing primitive.
            if lookup_class_attr(&class, "__iter__").is_none()
                && let Some(backing) = instance_builtin_data(&inst_rc) {
                    return self.collect_iterable(&backing);
                }
            let iterator = if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?
            } else if lookup_class_attr(&class, "__getitem__").is_some() {
                // Legacy sequence-iter protocol — lazy iterator that
                // drives `__getitem__(0)`, `__getitem__(1)`, … on
                // demand and terminates on IndexError/StopIteration.
                self.make_getitem_iter(Rc::clone(&inst_rc))?
            } else {
                return Err(pyrust_core::type_err!("'{}' object is not iterable", class.borrow().name));
            };
            let mut items = Vec::new();
            // Cache the __next__ method once for PyInstance iterators to avoid a
            // per-iteration class-walk (lookup_class_attr traverses the full MRO).
            if let ValueKind::PyInstance(iter_inst) = iterator.kind() {
                let iter_class = Rc::clone(&iter_inst.borrow().class);
                let Some(next_method) = lookup_class_attr(&iter_class, "__next__") else {
                    return Err(pyrust_core::type_err!("'{}' object is not an iterator", iter_class.borrow().name));
                };
                loop {
                    match invoke_class_method(self, next_method.clone(), iterator.clone(), &[]) {
                        Ok(v) => items.push(v),
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    }
                }
            } else {
                loop {
                    match self.call_next(&iterator, None) {
                        Ok(item) => items.push(item),
                        // class_name_is now walks the hierarchy for Raised variants,
                        // so StopIteration subclasses raised by __next__ are caught here.
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(items)
        } else {
            iter_values(val)
        }
    }

    /// Issue #2276: dispatch an unbound type-qualified object-level dunder
    /// (`str.__hash__(x)`, `int.__format__(5, 'x')`, …) whose owner is the
    /// primitive `type_name`.  Validates the receiver against the called type
    /// (CPython names the *called* type, not `object`, and picks slot-wrapper vs
    /// method_descriptor wording by descriptor kind), then delegates:
    ///   * `__hash__`/`__repr__`/`__str__` (slot wrappers) → the shared
    ///     `object.__X__` registry body, which renders both a bare primitive and
    ///     a subclass `PyInstance` (backing form, or set/frozenset type name).
    ///   * `__format__` (method_descriptor; int/str/float) → `apply_format_spec`
    ///     on the backing value, matching the bound form and `format()`.
    fn call_primitive_object_dunder(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        let is_format = method == "__format__";
        // Receiver presence guard.  Slot wrappers use "descriptor '<m>' of
        // '<type>' object needs an argument"; `__format__` is a method_descriptor
        // ("unbound method <type>.__format__() needs an argument").
        let self_arg = args.first().ok_or_else(|| {
            if is_format {
                pyrust_core::descriptor_needs_arg!(method, type_name, method)
            } else {
                pyrust_core::descriptor_needs_arg!(method, type_name)
            }
        })?;
        // Receiver-type guard: accept a bare primitive of `type_name` or a
        // subclass `PyInstance` whose backing data is that type.
        let recv_ok = match (type_name, self_arg.value.kind()) {
            ("int" | "bool", ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)) => true,
            ("float", ValueKind::Float(_)) => true,
            ("complex", ValueKind::Complex(_, _)) => true,
            ("str", ValueKind::Str(_)) => true,
            ("bytes", ValueKind::Bytes(_)) => true,
            ("list", ValueKind::List(_)) => true,
            ("tuple", ValueKind::Tuple(_)) => true,
            ("dict", ValueKind::Dict(_)) => true,
            ("set", ValueKind::Set(_)) => true,
            ("frozenset", ValueKind::BuiltinObject { ops, .. })
                if ops.type_name() == "frozenset" => true,
            (_, ValueKind::PyInstance(inst)) => instance_builtin_data(inst)
                .is_some_and(|b| pyrust_core::builtin_type_name(&b) == type_name),
            _ => false,
        };
        if !recv_ok {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(if is_format {
                pyrust_core::descriptor_requires!(method, type_name, actual, method)
            } else {
                pyrust_core::descriptor_requires!(method, type_name, actual)
            });
        }
        if is_format {
            if args.iter().any(|a| a.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "{type_name}.__format__() takes no keyword arguments"
                ));
            }
            if args.len() != 2 {
                return Err(pyrust_core::type_err!(
                    "{type_name}.__format__() takes exactly one argument ({} given)",
                    args.len() - 1
                ));
            }
            let spec = match args[1].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => {
                    return Err(pyrust_core::type_err!(
                        "__format__() argument must be str, not {}",
                        pyrust_core::builtin_type_name(&args[1].value)
                    ));
                }
            };
            return apply_format_spec(&self_arg.value, &spec);
        }
        // __hash__/__repr__/__str__ take no further positional/keyword args.
        // These are slot wrappers, so CPython's keyword-rejection message uses
        // the anonymous "wrapper <name>() takes no keyword arguments" form
        // (issue #2291), not the type-qualified `<type>.<name>()` form used by
        // the `__format__` method_descriptor above.
        if args.iter().skip(1).any(|a| a.name.is_some()) {
            return Err(pyrust_core::type_err!(
                "wrapper {method}() takes no keyword arguments"
            ));
        }
        // `__hash__` routes through `hash_value_with_interp` (the same path the
        // `hash()` builtin uses) rather than `object.__hash__`, so a scalar
        // subclass instance (`int.__hash__(MyInt(5))`) hashes its backing value
        // to `5` as CPython does — `object.__hash__` returns the identity hash.
        if method == "__hash__" {
            let h = crate::builtin_modules::builtins::hash_value_with_interp(
                self,
                &self_arg.value,
            )?;
            return Ok(Value::int(h));
        }
        let object_fn: &'static str = if method == "__repr__" {
            "object.__repr__"
        } else {
            "object.__str__"
        };
        let dispatch = crate::builtin_registry::lookup(object_fn)
            .unwrap_or_else(|| panic!("{object_fn} must be in the registry"));
        dispatch(self, args)
    }

    /// Issue #1909: execute a container/sequence protocol dunder on a built-in
    /// primitive receiver, routing through the same operator machinery the
    /// implicit operators use so results and error messages match CPython 3.12
    /// exactly.  `method` must be one of the names in
    /// [`builtin_protocol_dunders`] for `receiver`'s type; `args` are the
    /// positional arguments after the receiver (so `l.__getitem__(0)` arrives
    /// as `["__getitem__", l, [0]]`).  `__iter__` is handled separately by the
    /// callers (it is in each type's `METHODS` slice, not the dunder set).
    pub(crate) fn dispatch_builtin_protocol_dunder(
        &mut self,
        method: &str,
        receiver: Value,
        mut args: Vec<Value>,
    ) -> Result<Value> {
        let type_name = pyrust_core::builtin_type_name(&receiver);
        // Arity check up front so the error matches CPython 3.12's slot-wrapper
        // messages rather than a downstream operator error.  CPython's wording
        // is slot-dependent (verified against `python3.12`):
        //   - `mp_subscript` (dict/list `__getitem__`) and `sq_contains`
        //     (dict/set/frozenset `__contains__`) are *named* method-wrappers:
        //     `{type}.{name}() takes exactly one argument ({n} given)`.
        //   - the anonymous sequence slots (`sq_item`/`sq_concat`/`sq_ass_item`
        //     /…) use `expected N argument(s), got M`; `sq_repeat` (`__mul__`)
        //     and `sq_ass_item` (`__setitem__`) carry a leading space.
        let want: usize = match method {
            "__len__" | "__neg__" | "__pos__" | "__abs__" | "__invert__" | "__hash__"
            | "__str__" | "__repr__" | "__bool__" | "__reversed__" => 0,
            "__setitem__" => 2,
            _ => 1,
        };
        if args.len() != want {
            if is_named_protocol_wrapper(method, &type_name) {
                return Err(pyrust_core::type_err!("{type_name}.{method}() takes exactly one argument ({} given)",
                        args.len()));
            }
            // `dict.__reversed__()` is a named no-arg method-wrapper in CPython
            // 3.12: `dict.__reversed__() takes no arguments (N given)` (#2093).
            if method == "__reversed__" {
                return Err(pyrust_core::type_err!(
                    "{type_name}.__reversed__() takes no arguments ({} given)",
                    args.len()
                ));
            }
            // `__mul__` (sq_repeat), `__imul__` (sq_inplace_repeat) and
            // `__setitem__` (sq_ass_item) print a leading space before
            // "expected" in CPython 3.12.
            let lead =
                if matches!(method, "__mul__" | "__imul__" | "__setitem__") { " " } else { "" };
            let plural = if want == 1 { "argument" } else { "arguments" };
            return Err(pyrust_core::type_err!("{lead}expected {want} {plural}, got {}",
                    args.len()));
        }
        // Issue #2070: scalar/object dunders that route to a registry builtin or
        // an interpreter helper, exposed on every primitive type.  These never
        // collide with the container `__add__`/`__mul__`/… arms below.
        match method {
            "__hash__" => {
                let h =
                    crate::builtin_modules::builtins::hash_value_with_interp(self, &receiver)?;
                return Ok(Value::int(h));
            }
            "__str__" | "__repr__" => {
                let name = if method == "__str__" { "str" } else { "repr" };
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup(name)
                    .unwrap_or_else(|| panic!("{name} must be in the registry"));
                return dispatch(self, &[arg]);
            }
            "__bool__" => {
                let b = self.truthy_value(&receiver)?;
                return Ok(Value::bool_(b));
            }
            // #2093: `dict.__reversed__()` yields keys in reverse insertion
            // order.  Routes through the `reversed` builtin, which handles the
            // dict case directly.
            "__reversed__" => {
                let arg = ExpandedCallArg { name: None, value: receiver };
                let dispatch = crate::builtin_registry::lookup("reversed")
                    .expect("reversed must be in the registry");
                return dispatch(self, &[arg]);
            }
            // Rich-comparison dunders (issue #2070): exposed on every primitive
            // type.  The forward slot returns `NotImplemented` for operand types
            // it does not accept (`(5).__eq__('x')`, `(1,).__lt__([1])`), exactly
            // as CPython does — the `==`/`<` *operators* never reach here (they go
            // through `eval_binary`), so this is access-only and leaves the
            // operator hot paths untouched.
            "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__" => {
                let other = args.pop().unwrap();
                return self.primitive_richcmp_dunder(method, &receiver, &other);
            }
            _ => {}
        }
        // Issue #2070: scalar-numeric forward dunders (int/float/complex/bool).
        // Gated on the scalar types so the container `__add__`/`__mul__`/`__or__`
        // arms below keep their sequence/set semantics.  Returns `NotImplemented`
        // when the operand type is outside the receiver's accepted set.
        if matches!(&*type_name, "int" | "float" | "complex" | "bool")
            && let Some(result) =
                self.primitive_numeric_dunder(method, &receiver, &mut args)?
            {
                return Ok(result);
            }
        match method {
            "__len__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup("len")
                    .expect("len must be in the registry");
                dispatch(self, &[arg])
            }
            "__getitem__" => {
                let index = args.pop().unwrap();
                self.eval_index(&receiver, index)
            }
            "__contains__" => {
                let item = args.pop().unwrap();
                self.eval_in(receiver, item)
            }
            "__add__" => {
                let other = args.pop().unwrap();
                self.eval_binary(receiver, crate::ast::BinaryOp::Add, other)
            }
            "__mul__" => {
                // CPython's `sq_repeat` slot wrapper (`list.__mul__`,
                // `str.__mul__`, …) requires the repeat count to be int-like
                // and raises `'X' object cannot be interpreted as an integer`
                // for anything else — stricter than the `*` operator, which
                // says "can't multiply sequence by non-int".  Resolve the
                // count through `__index__` so the dunder matches CPython, then
                // delegate to the same repetition machinery as `*`.
                let other = args.pop().unwrap();
                // Resolve the count through the shared index protocol (#2022):
                // int/bool/bigint/int-subclass/`__index__` are accepted; float
                // and `__int__`-only objects raise the canonical TypeError.
                let count = self.value_to_index(&other, |v| {
                    pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                            pyrust_core::builtin_type_name(v))
                })?;
                self.eval_binary(receiver, crate::ast::BinaryOp::Mul, count)
            }
            "__setitem__" => {
                let value = args.pop().unwrap();
                let index = args.pop().unwrap();
                // Reuse the VM item-assign machinery (slice assignment, dict
                // key dedup, bytearray __index__ resolution) via a scratch
                // register file: obj@0, idx@1, val@2.  The receiver is not the
                // module globals dict, so the globals write-through in
                // `exec_set_item` stays inert.
                let mut scratch = vec![receiver, index, value];
                let mut regs = unsafe {
                    RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len())
                };
                self.exec_set_item(&mut regs, 0, 0, 1, 2)?;
                Ok(Value::none())
            }
            "__delitem__" => {
                let index = args.pop().unwrap();
                let mut scratch = vec![receiver, index];
                let mut regs = unsafe {
                    RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len())
                };
                self.exec_delete_item(&mut regs, 0, 0, 1)?;
                Ok(Value::none())
            }
            // list/bytearray in-place dunders (#2119): identical semantics to
            // the `+=`/`*=` operators — mutate the receiver in place and return
            // it.  `try_inplace_op(..., is_augmented_assign = true)` routes
            // through the same machinery the operators use, including the
            // operator-form TypeErrors (`'int' object is not iterable`,
            // `'float' object cannot be interpreted as an integer`, …).
            "__iadd__" if matches!(&*type_name, "list" | "bytearray") => {
                let other = args.pop().unwrap();
                match self.try_inplace_op(&receiver, crate::ast::BinaryOp::Add, &other, true)? {
                    Some(v) => Ok(v),
                    // The fast paths in `try_inplace_op` always handle list /
                    // bytearray `+=`, so this fallback is defensive only — it
                    // surfaces the operator's TypeError for a bad operand.
                    None => self.eval_binary(receiver, crate::ast::BinaryOp::Add, other),
                }
            }
            "__imul__" if matches!(&*type_name, "list" | "bytearray") => {
                // The `sq_inplace_repeat` slot wrapper resolves the count
                // through `__index__` (like `__mul__`/`sq_repeat`), so a float
                // raises `'X' object cannot be interpreted as an integer` —
                // stricter than the `*=` operator's "can't multiply sequence by
                // non-int" message.  Resolve first, then mutate in place.
                let other = args.pop().unwrap();
                let count = self.value_to_index(&other, |v| {
                    pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                            pyrust_core::builtin_type_name(v))
                })?;
                match self.try_inplace_op(&receiver, crate::ast::BinaryOp::Mul, &count, true)? {
                    Some(v) => Ok(v),
                    // None only for an out-of-range count (e.g. a BigInt that
                    // can't fit an index): delegate so the canonical
                    // OverflowError is raised, matching CPython.
                    None => self.eval_binary(receiver, crate::ast::BinaryOp::Mul, count),
                }
            }
            // set/frozenset/dict forward algebra & merge dunders (#2122).
            // CPython returns `NotImplemented` (not TypeError) when the other
            // operand is not set-/dict-compatible, so guard the operand type
            // before delegating to the operator machinery.
            "__or__" | "__and__" | "__sub__" | "__xor__"
                if matches!(&*type_name, "set" | "frozenset") =>
            {
                let other = args.pop().unwrap();
                if set_items_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                let op = match method {
                    "__or__" => crate::ast::BinaryOp::BitOr,
                    "__and__" => crate::ast::BinaryOp::BitAnd,
                    "__sub__" => crate::ast::BinaryOp::Sub,
                    _ => crate::ast::BinaryOp::BitXor,
                };
                self.eval_binary(receiver, op, other)
            }
            // Reflected set dunders: `a.__rOP__(b)` computes `b OP a`.  Same
            // NotImplemented guard as the forward forms.
            "__ror__" | "__rand__" | "__rsub__" | "__rxor__"
                if matches!(&*type_name, "set" | "frozenset") =>
            {
                let other = args.pop().unwrap();
                if set_items_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                let op = match method {
                    "__ror__" => crate::ast::BinaryOp::BitOr,
                    "__rand__" => crate::ast::BinaryOp::BitAnd,
                    "__rsub__" => crate::ast::BinaryOp::Sub,
                    _ => crate::ast::BinaryOp::BitXor,
                };
                self.eval_binary(other, op, receiver)
            }
            // set in-place algebra dunders (#2122).  Unlike the `|=`/`&=`/…
            // operators (which raise TypeError on a non-set operand), the
            // dunder returns `NotImplemented`, so guard first and only then
            // route through the mutating operator machinery.
            "__ior__" | "__iand__" | "__isub__" | "__ixor__" if &*type_name == "set" => {
                let other = args.pop().unwrap();
                if set_items_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                let op = match method {
                    "__ior__" => crate::ast::BinaryOp::BitOr,
                    "__iand__" => crate::ast::BinaryOp::BitAnd,
                    "__isub__" => crate::ast::BinaryOp::Sub,
                    _ => crate::ast::BinaryOp::BitXor,
                };
                match self.try_inplace_op(&receiver, op, &other, true)? {
                    Some(v) => Ok(v),
                    None => Ok(receiver),
                }
            }
            // dict PEP-584 forward / reflected merge dunders (#2122).  Returns
            // `NotImplemented` when the other operand is not a mapping (matching
            // `dict.__or__`/`__ror__`, which only accept dicts — not arbitrary
            // iterables of pairs, unlike `__ior__`).
            "__or__" | "__ror__" if &*type_name == "dict" => {
                let other = args.pop().unwrap();
                if dict_entries_from_value(&other).is_none() {
                    return Ok(Value::not_implemented());
                }
                if method == "__or__" {
                    self.eval_binary(receiver, crate::ast::BinaryOp::BitOr, other)
                } else {
                    self.eval_binary(other, crate::ast::BinaryOp::BitOr, receiver)
                }
            }
            // dict `__ior__` (#2122): identical to `|=` — accepts dicts *and*
            // iterables of (key, value) pairs, mutates in place, returns self.
            "__ior__" if &*type_name == "dict" => {
                let other = args.pop().unwrap();
                match self.try_inplace_op(&receiver, crate::ast::BinaryOp::BitOr, &other, true)? {
                    Some(v) => Ok(v),
                    None => Ok(receiver),
                }
            }
            other => Err(PyError::Runtime(format!(
                "internal: unhandled builtin protocol dunder '{other}'"
            ))),
        }
    }

    /// Dispatch a rich-comparison dunder (`__eq__`/`__ne__`/`__lt__`/…) accessed
    /// directly on a primitive instance (issue #2070).  Returns the boolean
    /// result of the comparison, or `NotImplemented` when the forward slot does
    /// not accept the operand type — matching CPython's per-type slot semantics
    /// exactly (`(5).__eq__('x')` → `NotImplemented`, `(5).__eq__(5.0)` →
    /// `NotImplemented`, `(5.0).__eq__(5)` → `True`, `(1,).__lt__([1])` →
    /// `NotImplemented`).  The `==`/`<` *operators* never call this — they go
    /// through `eval_binary` — so the operator fast paths are untouched.
    fn primitive_richcmp_dunder(
        &mut self,
        method: &str,
        recv: &Value,
        other: &Value,
    ) -> Result<Value> {
        let is_equality = matches!(method, "__eq__" | "__ne__");
        if !richcmp_operand_accepted(recv, other, is_equality) {
            return Ok(Value::not_implemented());
        }
        let op = match method {
            "__eq__" => crate::ast::BinaryOp::Eq,
            "__ne__" => crate::ast::BinaryOp::Ne,
            "__lt__" => crate::ast::BinaryOp::Lt,
            "__le__" => crate::ast::BinaryOp::Le,
            "__gt__" => crate::ast::BinaryOp::Gt,
            _ => crate::ast::BinaryOp::Ge,
        };
        self.eval_binary(recv.clone(), op, other.clone())
    }

    /// Dispatch a scalar-numeric forward dunder (int/float/complex/bool) accessed
    /// directly on a primitive instance (issue #2070): the binary arithmetic /
    /// bitwise slots and the unary `__neg__`/`__pos__`/`__abs__`/`__invert__`.
    ///
    /// Returns:
    /// - `Some(Ok(NotImplemented))` when a *binary* slot's operand is outside the
    ///   receiver's accepted numeric set (`(5).__add__(5.0)` → `NotImplemented`);
    /// - `Some(Ok(result))` / `Some(Err(..))` when the slot computed / raised;
    /// - `None` when `method` is not a scalar-numeric dunder (the caller then
    ///   falls through to the container/other arms).
    fn primitive_numeric_dunder(
        &mut self,
        method: &str,
        recv: &Value,
        args: &mut Vec<Value>,
    ) -> Result<Option<Value>> {
        use crate::ast::BinaryOp;
        // Unary slots first — no operand-acceptance check needed.  `__neg__` /
        // `__pos__` / `__invert__` route through the canonical `vm_eval_unary`;
        // `__abs__` through the `abs` registry builtin (complex `abs` returns a
        // float, which `vm_eval_unary` does not cover).
        match method {
            "__neg__" => {
                return Ok(Some(crate::interpreter::vm_eval_unary(
                    crate::ast::UnaryOp::Neg,
                    recv.clone(),
                )?));
            }
            "__pos__" => {
                return Ok(Some(crate::interpreter::vm_eval_unary(
                    crate::ast::UnaryOp::Pos,
                    recv.clone(),
                )?));
            }
            "__invert__" => {
                return Ok(Some(crate::interpreter::vm_eval_unary(
                    crate::ast::UnaryOp::BitNot,
                    recv.clone(),
                )?));
            }
            "__abs__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
                let dispatch = crate::builtin_registry::lookup("abs")
                    .expect("abs must be in the registry");
                return Ok(Some(dispatch(self, &[arg])?));
            }
            _ => {}
        }
        // `__divmod__` / `__rdivmod__` route through the `divmod` builtin
        // (which returns a 2-tuple) rather than a single `BinaryOp`.  The
        // reflected form swaps the operand order (`b.__rdivmod__(a)` →
        // `divmod(a, b)`), issue #2215.
        match method {
            "__divmod__" => {
                let other = args.pop().unwrap();
                if !numeric_operand_accepted(recv, &other) {
                    return Ok(Some(Value::not_implemented()));
                }
                let dispatch = crate::builtin_registry::lookup("divmod")
                    .expect("divmod must be in the registry");
                let a = ExpandedCallArg { name: None, value: recv.clone() };
                let b = ExpandedCallArg { name: None, value: other };
                return Ok(Some(dispatch(self, &[a, b])?));
            }
            "__rdivmod__" => {
                let other = args.pop().unwrap();
                if !numeric_operand_accepted(recv, &other) {
                    return Ok(Some(Value::not_implemented()));
                }
                let dispatch = crate::builtin_registry::lookup("divmod")
                    .expect("divmod must be in the registry");
                let a = ExpandedCallArg { name: None, value: other };
                let b = ExpandedCallArg { name: None, value: recv.clone() };
                return Ok(Some(dispatch(self, &[a, b])?));
            }
            _ => {}
        }
        // Reflected binary slots (issue #2215): same op table as forward, but
        // with operands swapped (`eval_binary(other, op, recv)` computes
        // `other OP recv`).  Acceptance is identical to the forward direction
        // (see `numeric_operand_accepted`): `(5).__radd__(2.5)` →
        // `NotImplemented` because float outranks int.
        if let Some(rop) = match method {
            "__radd__" => Some(BinaryOp::Add),
            "__rsub__" => Some(BinaryOp::Sub),
            "__rmul__" => Some(BinaryOp::Mul),
            "__rtruediv__" => Some(BinaryOp::Div),
            "__rfloordiv__" => Some(BinaryOp::FloorDiv),
            "__rmod__" => Some(BinaryOp::Mod),
            "__rpow__" => Some(BinaryOp::Pow),
            "__rand__" => Some(BinaryOp::BitAnd),
            "__ror__" => Some(BinaryOp::BitOr),
            "__rxor__" => Some(BinaryOp::BitXor),
            "__rlshift__" => Some(BinaryOp::LShift),
            "__rrshift__" => Some(BinaryOp::RShift),
            _ => None,
        } {
            let other = args.pop().unwrap();
            if !numeric_operand_accepted(recv, &other) {
                return Ok(Some(Value::not_implemented()));
            }
            return Ok(Some(self.eval_binary(other, rop, recv.clone())?));
        }
        let op = match method {
            "__add__" => BinaryOp::Add,
            "__sub__" => BinaryOp::Sub,
            "__mul__" => BinaryOp::Mul,
            "__truediv__" => BinaryOp::Div,
            "__floordiv__" => BinaryOp::FloorDiv,
            "__mod__" => BinaryOp::Mod,
            "__pow__" => BinaryOp::Pow,
            "__and__" => BinaryOp::BitAnd,
            "__or__" => BinaryOp::BitOr,
            "__xor__" => BinaryOp::BitXor,
            "__lshift__" => BinaryOp::LShift,
            "__rshift__" => BinaryOp::RShift,
            _ => return Ok(None),
        };
        let other = args.pop().unwrap();
        if !numeric_operand_accepted(recv, &other) {
            return Ok(Some(Value::not_implemented()));
        }
        Ok(Some(self.eval_binary(recv.clone(), op, other)?))
    }

    /// Build a lazy iterator wrapping the legacy `__getitem__`
    /// sequence-iter protocol for `inst_rc`.  Returns a
    /// `Value::generator(...)` that downcasts to [`GetItemIter`].
    /// Each `next()` call invokes `inst.__getitem__(i)` once; the
    /// caller's `break`/early-return correctly stops at the first
    /// unused index (CPython semantics, #394).  Issue #416 Copilot
    /// review: switched from eager materialise to lazy after the
    /// reviewer flagged the observable break/short-circuit gap.
    pub(crate) fn make_getitem_iter(&self, inst_rc: Rc<RefCell<PyInstance>>) -> Result<Value> {
        let class = Rc::clone(&inst_rc.borrow().class);
        let method_val = lookup_class_attr(&class, "__getitem__").ok_or_else(|| {
            pyrust_core::type_err!("'{}' object is not iterable", class.borrow().name)
        })?;
        let obj = Value::py_instance(inst_rc);
        Ok(Value::generator(Box::new(GetItemIter {
            obj,
            method: method_val,
            index: 0,
            exhausted: false,
        })))
    }

    /// Resolve one `next()` step: yield the produced value, or fall back to
    /// `default` if the iterator is exhausted, or raise a bare `StopIteration`
    /// if there is no default.  Shared by every built-in iterator arm in
    /// [`call_next`], which previously inlined this same match.
    fn step_or_stop(item: Option<Value>, default: Option<Value>) -> Result<Value> {
        match item {
            Some(v) => Ok(v),
            None => default.ok_or_else(|| pyrust_core::py_err!("StopIteration", String::new())),
        }
    }

    /// Call next() on a generator or any object with __next__.
    pub(crate) fn call_next(&mut self, val: &Value, default: Option<Value>) -> Result<Value> {
        use std::any::TypeId;
        if let ValueKind::Generator(state_rc) = val.kind() {
            let state_rc = Rc::clone(state_rc);

            // Fast path: NativeIterFrame (no VM required) — probed exactly
            // as before so this arm pays no extra cost.
            //
            // `try_borrow_mut`: the cell is mutably checked out while the
            // generator's own body executes (a native `resume_generator` up the
            // stack), so a re-entrant `next(g)` from inside `g` must raise
            // CPython's `ValueError: generator already executing` rather than
            // panicking on the re-borrow (issue #2285).
            let mut borrow = state_rc.try_borrow_mut().map_err(|_| {
                pyrust_core::value_err!("generator already executing")
            })?;
            if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                return match native.advance()? {
                    Some(v) => Ok(v),
                    None => {
                        if let Some(d) = default {
                            Ok(d)
                        } else {
                            Err(pyrust_core::py_err!("StopIteration", String::new()))
                        }
                    }
                };
            }

            // Single-probe dispatch on the concrete iterator-state type for
            // everything else (issue #2315).  The previous probe cascade
            // re-borrowed the RefCell and ran a failed `downcast` per
            // variant, so a real generator — the most common kind in hot
            // consumer loops — paid 8 failed probes per element, which
            // dominated `sum(genexpr)`-style consumers.  `TypeId` equality
            // is exactly the check `downcast` performs internally, so the
            // per-arm downcasts below cannot fail.  The borrow is reused by
            // the GeneratorFrame arm and dropped before the `step_*` arms,
            // which re-borrow internally.
            let tid = {
                let any_ref: &dyn std::any::Any = &**borrow;
                any_ref.type_id()
            };

            // A `GenDriving` placeholder means the frame is checked out by
            // the gen-drive trampoline (#2253) up the stack — the generator
            // is executing, so a re-entrant `next(g)` raises CPython's
            // already-executing error (issue #2285).
            if tid == TypeId::of::<GenDriving>() {
                return Err(pyrust_core::value_err!("generator already executing"));
            }

            // GeneratorFrame path (hottest: every generator / genexpr).
            if tid == TypeId::of::<GeneratorFrame>() {
                let frame = borrow
                    .downcast_mut::<GeneratorFrame>()
                    .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
                // Async generators (#2280) are not synchronous iterators: `next(g)`
                // raises TypeError, matching CPython.  They are consumed via
                // `async for` / `__anext__`.
                if frame.is_async_generator() {
                    return Err(pyrust_core::type_err!(
                        "'async_generator' object is not an iterator"
                    ));
                }
                // Coroutines (#2314) are awaitable, not iterable: `next(coro)` raises
                // TypeError instead of driving the coroutine.  They are consumed via
                // `await` / `.send()`.
                if frame.is_coroutine && !frame.code.is_generator {
                    return Err(pyrust_core::type_err!(
                        "'coroutine' object is not an iterator"
                    ));
                }
                if frame.done {
                    drop(borrow);
                    return if let Some(d) = default {
                        Ok(d)
                    } else {
                        // Exhausted generator: StopIteration() with no args → .value is None.
                        let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                            PyError::Raised(instantiate_exception(cls, vec![]))
                        } else {
                            pyrust_core::py_err!("StopIteration", String::new())
                        };
                        Err(exc)
                    };
                }
                return match self.resume_generator(frame) {
                    Ok(yielded) => Ok(yielded),
                    Err(e) if e.class_name_is("StopIteration") => {
                        drop(borrow);
                        if let Some(d) = default {
                            Ok(d)
                        } else {
                            // Propagate the original error so StopIteration.value
                            // is preserved (PEP 380 / issue #600).
                            Err(e)
                        }
                    }
                    Err(e) => Err(e),
                };
            }

            // The step_* helpers below re-borrow the state internally.
            drop(borrow);

            // GetItemIter path: drive one `__getitem__(i)` call lazily.
            // Borrow released by step_getitem_iter before invoking the
            // method (it would otherwise re-entrantly re-borrow).
            if tid == TypeId::of::<GetItemIter>() {
                return Self::step_or_stop(self.step_getitem_iter(&state_rc)?, default);
            }

            // CallableIter path: invoke callable(), stop when result == sentinel.
            if tid == TypeId::of::<CallableIter>() {
                return Self::step_or_stop(self.step_callable_iter(&state_rc)?, default);
            }

            // MapIter path: apply func to one row of columns per step.
            if tid == TypeId::of::<MapIter>() {
                return Self::step_or_stop(self.step_map_iter(&state_rc)?, default);
            }

            // FilterIter path: scan forward for next passing element.
            if tid == TypeId::of::<FilterIter>() {
                return Self::step_or_stop(self.step_filter_iter(&state_rc)?, default);
            }

            // BigRangeIter path: lazy arbitrary-precision range iteration (#2118).
            if tid == TypeId::of::<BigRangeIter>() {
                return Self::step_or_stop(self.step_bigrange_iter(&state_rc)?, default);
            }

            // EnumerateIter path: (counter, element) pair per step.
            if tid == TypeId::of::<EnumerateIter>() {
                return Self::step_or_stop(self.step_enumerate_iter(&state_rc)?, default);
            }

            // ZipIter path: one row tuple per step.
            if tid == TypeId::of::<ZipIter>() {
                return Self::step_or_stop(self.step_zip_iter(&state_rc)?, default);
            }

            Err(PyError::Runtime("invalid generator state".to_string()))
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__next__") {
                match invoke_class_method(self, method_val, Value::py_instance(inst_rc), &[]) {
                    Ok(v) => Ok(v),
                    Err(PyError::Raised(exc)) => {
                        // Use class_name_is so StopIteration subclasses are
                        // correctly detected (hierarchy walk, not exact name).
                        if PyError::Raised(exc.clone()).class_name_is("StopIteration") {
                            if let Some(d) = default {
                                Ok(d)
                            } else {
                                Err(PyError::Raised(exc))
                            }
                        } else {
                            Err(PyError::Raised(exc))
                        }
                    }
                    Err(e) => Err(e),
                }
            } else {
                Err(pyrust_core::type_err!("'{}' object is not an iterator",
                        class.borrow().name))
            }
        } else if let ValueKind::BuiltinObject { ops, state } = val.kind()
            && ops.is_iterable()
        {
            Self::step_or_stop(ops.iter_next(state)?, default)
        } else {
            Err(pyrust_core::type_err!("'{}' object is not an iterator", value_type_name_str(val)))
        }
    }

    pub(crate) fn parse_print_options_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<PrintOptions> {
        // First pass: reject any unknown keyword name before type-checking valid ones.
        // CPython 3.12 raises the unknown-keyword error first regardless of argument order.
        for arg in args {
            if let Some(name) = arg.name.as_deref()
                && !matches!(name, "sep" | "end" | "file" | "flush") {
                    return Err(pyrust_core::type_err!("'{}' is an invalid keyword argument for print()", name));
                }
        }

        // Second pass: extract and validate known keyword arguments.
        let mut values = Vec::new();
        let mut sep = String::from(" ");
        let mut end = String::from("\n");
        let mut file: Option<Value> = None;
        let mut flush = false;

        for arg in args {
            let value = arg.value.clone();
            match arg.name.as_deref() {
                None => values.push(value),
                Some("sep") => {
                    sep = extract_optional_string(value, "sep")?.unwrap_or_else(|| " ".to_string());
                }
                Some("end") => {
                    end =
                        extract_optional_string(value, "end")?.unwrap_or_else(|| "\n".to_string());
                }
                Some("file") => {
                    if !value.is_none() {
                        file = Some(value);
                    }
                }
                Some("flush") => {
                    flush = self.truthy_value(&value)?;
                }
                Some(_) => unreachable!("unknown keywords already rejected in first pass"),
            }
        }

        Ok(PrintOptions { values, sep, end, file, flush })
    }

    pub(crate) fn call_user_function_expanded(
        &mut self,
        function: Rc<UserFunction>,
        args: &[ExpandedCallArg],
        bound_prefix: &[Value],
    ) -> Result<Value> {
        // Single pass over the parameter list (replaces three separate
        // `.iter().any()` scans on the hot call path): `*args` / `**kwargs`
        // divert to the variadic path, and whether any parameter is
        // keyword-only gates the exact-positional fast bind below.
        let mut has_args_param = false;
        let mut has_kwargs_param = false;
        let mut has_kwonly_param = false;
        for p in &function.params {
            has_args_param |= p.is_args;
            has_kwargs_param |= p.is_kwargs;
            has_kwonly_param |= p.is_keyword_only;
        }

        if !has_args_param && !has_kwargs_param {
            // Fast path: no variadic params.
            // Tier-0: register-VM path — fetch compiled bytecode up front so we
            // can bind arguments *directly* into the callee's new frame register
            // file (like CPython's fastlocals), skipping the per-call
            // `Vec<Option<Value>>` allocation + option-wrapping (#2123).
            let Some(code) = self.get_or_compile_bytecode(&function) else {
                // All user functions must have precompiled bytecode.
                return Err(PyError::Runtime(format!("no bytecode for '{}'", function.name)));
            };
            let num_regs = code.num_regs as usize;
            let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

            // Create a local env when the function uses globals, nonlocals, or
            // cell vars.  Determined here (before arg binding) so cell-var
            // parameters can be written straight into the env rather than
            // staged through an intermediate buffer.
            let needs_local_env = !function.global_names.is_empty()
                || !function.nonlocal_names.is_empty()
                || !code.cell_vars.is_empty();
            let local_env = if needs_local_env {
                let env = self.alloc_env(Some(Rc::clone(&function.env)));
                {
                    let mut e = env.borrow_mut();
                    e.local_names = Rc::clone(&function.local_names);
                    e.global_names = Rc::clone(&function.global_names);
                    e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                }
                Some(env)
            } else {
                None
            };

            let nparams = function.params.len();
            // Exact-arity positional fast bind: the dominant call shape — every
            // argument supplied positionally, exactly filling the parameters,
            // with no keyword-only parameters and no bound prefix (e.g. a plain
            // `fib(n - 1)`).  Binds each argument straight into its precomputed
            // destination register/cell, skipping the per-param bound-flag
            // tracking, keyword matching, defaults resolution, and the
            // posonly/missing-argument diagnostics the general path needs.
            if bound_prefix.is_empty()
                && !has_kwonly_param
                && args.len() == nparams
                && args.iter().all(|a| a.name.is_none())
            {
                for (pi, arg) in args.iter().enumerate() {
                    bind_param_direct(
                        &function,
                        num_regs,
                        &mut regs,
                        &local_env,
                        pi,
                        arg.value.clone(),
                    )?;
                }
            } else {
                // General argument binding (CPython 3.12 parity): keyword
                // matching, positional-only / unexpected-keyword diagnostics,
                // defaults, and missing-argument collection.
                let positional_count =
                    args.iter().filter(|arg| arg.name.is_none()).count();
                // Number of params that can accept positional arguments
                // (excludes keyword-only).
                let positional_param_count =
                    function.params.iter().filter(|p| !p.is_keyword_only).count();
                let required_positional_count = function
                    .params
                    .iter()
                    .filter(|p| !p.is_keyword_only && p.default.is_none())
                    .count();
                let total_positional_given = positional_count + bound_prefix.len();
                if total_positional_given > positional_param_count {
                    let given_word = if total_positional_given == 1 { "was" } else { "were" };
                    let (takes_str, arg_word) = if required_positional_count == positional_param_count {
                        let arg_word =
                            if positional_param_count == 1 { "argument" } else { "arguments" };
                        (format!("{positional_param_count}"), arg_word)
                    } else {
                        (
                            format!("from {required_positional_count} to {positional_param_count}"),
                            "arguments",
                        )
                    };
                    return Err(pyrust_core::type_err!("{}() takes {takes_str} positional {arg_word} but {} {given_word} given",
                            function.name,
                            total_positional_given,));
                }
                // Per-param "already bound" flags (stack-allocated for typical
                // arity — replaces the heap `Vec<Option<Value>>` whose `Some`
                // discriminant previously doubled as this flag).
                let mut bound: smallvec::SmallVec<[bool; 16]> = smallvec![false; nparams];
                // Routes one bound value to its compile-time destination: a frame
                // register (the common case) or — for cell-var params under a local
                // env — an env entry by name.  Marks the param bound.  The body lives
                // in `fast_path.rs::bind_param` (the frame-binding fast path, #2123),
                // taking the frame-local state by reference so the file boundary stays
                // zero-cost (the helper is `#[inline]`).
                for (index, value) in bound_prefix.iter().enumerate() {
                    bind_param(&mut bound, &function, num_regs, &mut regs, &local_env, index, value.clone())?;
                }
                let mut positional_index = bound_prefix.len();
                let mut posonly_violations: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
                // Deferred unknown-keyword: CPython raises posonly error before
                // unexpected-keyword error when both are present in the same call.
                let mut first_unknown_keyword: Option<&str> = None;
                for arg in args {
                    let value = arg.value.clone();
                    if let Some(name) = &arg.name {
                        let Some(param_index) =
                            function.params.iter().position(|param| param.name == *name)
                        else {
                            // Don't return immediately — a posonly violation earlier
                            // in the arg list must still take priority (CPython 3.12).
                            if first_unknown_keyword.is_none() {
                                first_unknown_keyword = Some(name.as_str());
                            }
                            continue;
                        };
                        if function.params[param_index].is_positional_only {
                            // The fast path only runs when the function has neither
                            // *args nor **kwargs (see the `if !has_args_param &&
                            // !has_kwargs_param` guard above), so there is no
                            // **kwargs to absorb this name — TypeError is correct.
                            // The variadic path (`compute_kw_pos` below) handles
                            // the "absorb into **kwargs" case separately.
                            // Collect all violations so the error lists all names,
                            // matching CPython 3.12: foo() got some positional-only
                            // arguments passed as keyword arguments: 'a, b'
                            posonly_violations.push(name.as_str());
                            continue;
                        }
                        if bound[param_index] {
                            return Err(pyrust_core::type_err!("{}() got multiple values for argument '{}'",
                                    function.name, name));
                        }
                        bind_param(&mut bound, &function, num_regs, &mut regs, &local_env, param_index, value)?;
                    } else {
                        // Skip already-bound slots and keyword-only params.
                        while positional_index < nparams
                            && (bound[positional_index]
                                || function.params[positional_index].is_keyword_only)
                        {
                            positional_index += 1;
                        }
                        if positional_index >= nparams
                            || function.params[positional_index].is_keyword_only
                        {
                            let given_word =
                                if total_positional_given == 1 { "was" } else { "were" };
                            let (takes_str, arg_word) =
                                if required_positional_count == positional_param_count {
                                    let arg_word = if positional_param_count == 1 {
                                        "argument"
                                    } else {
                                        "arguments"
                                    };
                                    (format!("{positional_param_count}"), arg_word)
                                } else {
                                    (
                                        format!(
                                            "from {required_positional_count} to {positional_param_count}"
                                        ),
                                        "arguments",
                                    )
                                };
                            return Err(pyrust_core::type_err!("{}() takes {takes_str} positional {arg_word} but {} {given_word} given",
                                    function.name,
                                    total_positional_given,));
                        }
                        bind_param(&mut bound, &function, num_regs, &mut regs, &local_env, positional_index, value)?;
                        positional_index += 1;
                    }
                }
                if !posonly_violations.is_empty() {
                    return Err(pyrust_core::type_err!("{}() got some positional-only arguments passed as keyword arguments: '{}'",
                            function.name,
                            posonly_violations.join(", ")));
                }
                if let Some(name) = first_unknown_keyword {
                    return Err(pyrust_core::type_err!("{}() got an unexpected keyword argument '{}'", function.name, name));
                }
                // Resolve defaults: bind any still-unbound params straight into their
                // destination register/cell.
                // Collect all missing required positional and keyword-only args before
                // raising, so the error groups them all (CPython 3.12 parity).
                let mut missing_positional: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
                let mut missing_kwonly: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
                for index in 0..nparams {
                    if !bound[index] {
                        if let Some(default) = function.params[index].default.clone() {
                            bind_param(&mut bound, &function, num_regs, &mut regs, &local_env, index, default)?;
                        } else if function.params[index].is_keyword_only {
                            missing_kwonly.push(&function.params[index].name);
                        } else {
                            missing_positional.push(&function.params[index].name);
                        }
                    }
                }
                // Use the qualified name (e.g. "Foo.__new__") so the error message
                // matches CPython 3.12: "Foo.__new__() missing 1 required positional
                // argument: 'x'" rather than the bare "__new__()".
                check_missing_args(&function.qualname, &missing_positional, &missing_kwonly)?;
            }

            // Arguments are already bound directly into `regs` / `local_env`
            // above (#2123).  Run the register-VM path.
            {
                let _depth_guard = CallDepthGuard::enter();
                if call_depth() > max_call_depth() {
                    let exc = if let Some(cls) = self.exc_classes.get("RecursionError") {
                        instantiate_exception(
                            cls,
                            vec![Value::string("maximum recursion depth exceeded")],
                        )
                    } else {
                        self.instantiate_named_exception(
                            "RecursionError",
                            "maximum recursion depth exceeded".to_string(),
                        )?
                    };
                    return Err(PyError::Raised(exc));
                }

                // Swap in the callee's env (the local env built above, or the
                // function's captured env when no local env is needed).
                let previous_env = match local_env {
                    Some(env) => std::mem::replace(&mut self.env, env),
                    None => std::mem::replace(&mut self.env, Rc::clone(&function.env)),
                };

                // Self-reference for recursive calls (only if not a cell var) —
                // bind slot precomputed at compile time (#1918).
                if let Some(slot) = function.self_bind {
                    if slot as usize >= num_regs {
                        return Err(pyrust_core::py_err!("SystemError", "self-reference register index {} out of range (num_regs={})",
                                slot, num_regs));
                    }
                    regs[slot as usize] = Value::user_function(Rc::clone(&function));
                }

                // Generator or coroutine function: create a frame rather than
                // executing.  An `async def` body (issue #1039) is always a
                // suspendable frame — even with no `await` — so it returns a
                // coroutine object instead of running synchronously.
                if code.is_generator || code.is_coroutine {
                    // Restore env before capturing it into the frame.
                    // (When `needs_local_env` is false, `gen_env` ==
                    // `function.env` — the GeneratorFrame keeps it alive.)
                    let gen_env = std::mem::replace(&mut self.env, previous_env);
                    let gen_qualname =
                        std::sync::Arc::from(function.effective_qualname().as_str());
                    return Ok(Self::build_generator_value(
                        &code,
                        regs,
                        gen_env,
                        Rc::clone(&function.local_index),
                        std::sync::Arc::from(&function.name[..]),
                        gen_qualname,
                    ));
                }

                // Issue #389: publish a view of this function frame so
                // `locals()` can surface its fastlocal registers
                // mid-call.  Popped immediately after `run_bytecode`
                // returns so the raw pointer never outlives `regs`.
                // Issue #486: also capture nonlocal_names and the
                // current env so `snapshot_current_locals` can resolve
                // nonlocal bindings that live in enclosing envs.
                let nonlocal_names_opt = if function.nonlocal_names.is_empty() {
                    None
                } else {
                    Some(Rc::clone(&function.nonlocal_names))
                };
                let env_opt = if function.nonlocal_names.is_empty() {
                    None
                } else {
                    Some(Rc::clone(&self.env))
                };
                // Capture the raw pointer and length BEFORE constructing RegSlice
                // so both the VmFrameView and the dispatch loop share the same raw
                // pointer with no &mut [Value] in scope (issue #547 / PR #646).
                let regs_ptr = unsafe {
                    std::ptr::NonNull::new_unchecked(regs.as_mut_ptr())
                };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Function,
                    // SAFETY: SmallVec / Vec allocation is always non-null.
                    // Popped before `regs` is dropped (see above).
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&function.local_index),
                    nonlocal_names: nonlocal_names_opt,
                    env: env_opt,
                    is_class_method: code.is_class_method,
                    function: Some(Rc::clone(&function)),
                });
                // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
                // of `regs` (a local RegsBuf that outlives this call).  No
                // &mut [Value] referencing `regs` is held while the dispatch loop
                // runs; RegSlice (raw pointer + len) is used instead, removing
                // the LLVM noalias constraint (issue #547).
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                let vm_result = self.run_bytecode_for_fn(&code, regs_slice, function.id);
                // Lazy traceback: only build + record this frame's `FrameInfo`
                // when the body actually errored.  The no-exception common path
                // does no allocation and touches no traceback thread-local.
                if vm_result.is_err() {
                    let tb_filename = self
                        .script_filename
                        .clone()
                        .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                    // Capture the source line in this callee where execution
                    // stopped (the callee published it via `set_current_vm_line`
                    // on the way out).  Surfaced to Python as `tb_lineno` /
                    // `f_lineno`; 0 means "no line table" (kept as `None`).
                    let tb_lineno = match pyrust_core::get_current_vm_line() {
                        0 => None,
                        n => Some(n),
                    };
                    pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                        filename: tb_filename,
                        lineno: tb_lineno,
                        source_line: None,
                        funcname: std::sync::Arc::from(&function.name[..]),
                    });
                }
                self.vm_frame_views.pop();

                let used_env = std::mem::replace(&mut self.env, previous_env);
                if needs_local_env {
                    self.free_env(used_env);
                }
                let value = vm_result?;
                return Ok(value);
            }
        }

        // Variadic path (*args / **kwargs) lives in a helper to keep this
        // function focused on the common no-variadic fast path above.
        self.call_user_function_variadic(function, args, bound_prefix, has_args_param)
    }

    /// Variadic argument-binding + execution path for
    /// `call_user_function_expanded` — the `*args` / `**kwargs` case.
    /// Extracted verbatim (size reduction); the fast no-variadic path stays
    /// inline in the caller and returns before reaching this helper.
    fn call_user_function_variadic(
        &mut self,
        function: Rc<UserFunction>,
        args: &[ExpandedCallArg],
        bound_prefix: &[Value],
        has_args_param: bool,
    ) -> Result<Value> {
        // Variadic path: handle *args and **kwargs
        // Gather positional and keyword args
        let mut positional_vals: Vec<Value> = bound_prefix.to_vec();
        let mut keyword_vals: Vec<(String, Value)> = Vec::new();
        for arg in args {
            if let Some(name) = &arg.name {
                keyword_vals.push((name.clone(), arg.value.clone()));
            } else {
                positional_vals.push(arg.value.clone());
            }
        }

        // Pre-check: reject excess positional arguments before binding when
        // there is no *args to absorb them. This matches CPython's error ordering.
        if !has_args_param {
            let positional_param_count = function
                .params
                .iter()
                .filter(|p| !p.is_keyword_only && !p.is_args && !p.is_kwargs)
                .count();
            let required_positional_count = function
                .params
                .iter()
                .filter(|p| !p.is_keyword_only && !p.is_args && !p.is_kwargs && p.default.is_none())
                .count();
            if positional_vals.len() > positional_param_count {
                let given_word = if positional_vals.len() == 1 { "was" } else { "were" };
                let (takes_str, arg_word) = if required_positional_count == positional_param_count {
                    let arg_word =
                        if positional_param_count == 1 { "argument" } else { "arguments" };
                    (format!("{positional_param_count}"), arg_word)
                } else {
                    (
                        format!("from {required_positional_count} to {positional_param_count}"),
                        "arguments",
                    )
                };
                return Err(pyrust_core::type_err!("{}() takes {takes_str} positional {arg_word} but {} {given_word} given",
                        function.name, positional_vals.len(),));
            }
        }

        let has_kwargs = function.params.iter().any(|p| p.is_kwargs);
        let mut consumed_keywords = std::collections::HashSet::new();
        let mut pos_idx = 0;
        let mut param_vals: Vec<Value> = Vec::with_capacity(function.params.len());
        // Collect all missing required args before raising, so the error groups
        // them all (CPython 3.12 parity).
        let mut missing_positional: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();
        let mut missing_kwonly: smallvec::SmallVec<[&str; 4]> = smallvec::SmallVec::new();

        for param in function.params.iter() {
            let value = if param.is_args {
                let rest = positional_vals[pos_idx..].to_vec();
                pos_idx = positional_vals.len();
                Value::tuple(rest)
            } else if param.is_kwargs {
                let mut dict: PyDict = PyDict::default();
                for (k, v) in &keyword_vals {
                    if !consumed_keywords.contains(k)
                        && let Some(key) = Value::string(k.clone()).to_key() {
                            dict.insert(key, v.clone());
                        }
                }
                Value::dict(dict)
            } else {
                let kw_pos = if param.is_positional_only {
                    None
                } else {
                    keyword_vals.iter().position(|(k, _)| k == &param.name)
                };
                if let Some(ki) = kw_pos {
                    consumed_keywords.insert(keyword_vals[ki].0.clone());
                    keyword_vals[ki].1.clone()
                } else if !param.is_keyword_only && pos_idx < positional_vals.len() {
                    let v = positional_vals[pos_idx].clone();
                    pos_idx += 1;
                    v
                } else if let Some(d) = &param.default {
                    d.clone()
                } else if param.is_keyword_only {
                    missing_kwonly.push(&param.name);
                    Value::unset()
                } else {
                    missing_positional.push(&param.name);
                    Value::unset()
                }
            };
            param_vals.push(value);
        }

        // Report positional missing args first; only report kwonly if all
        // positional params were satisfied (matching CPython 3.12 behaviour).
        check_missing_args(&function.qualname, &missing_positional, &missing_kwonly)?;

        if !has_kwargs {
            // First pass: collect all positional-only violations so the error
            // lists every offending name, matching CPython 3.12 parity.
            let posonly_violations: smallvec::SmallVec<[&str; 4]> = keyword_vals
                .iter()
                .filter(|(name, _)| {
                    !consumed_keywords.contains(name)
                        && function
                            .params
                            .iter()
                            .any(|p| p.is_positional_only && &p.name == name)
                })
                .map(|(name, _)| name.as_str())
                .collect();
            if !posonly_violations.is_empty() {
                return Err(pyrust_core::type_err!("{}() got some positional-only arguments passed as keyword arguments: '{}'",
                        function.name,
                        posonly_violations.join(", ")));
            }
            // Second pass: check for entirely unexpected keyword arguments.
            for (name, _) in &keyword_vals {
                if !consumed_keywords.contains(name) {
                    return Err(pyrust_core::type_err!("{}() got an unexpected keyword argument '{}'",
                            function.name, name));
                }
            }
        }

        // Now run via VM (same as non-variadic Tier-0 path)
        if let Some(code) = self.get_or_compile_bytecode(&function) {
            let num_regs = code.num_regs as usize;
            let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

            // Bind non-cell params into register file using precomputed slots
            // (#1918).  Cell-var params are inserted into the env below.
            for ((param, bind), val) in function
                .params
                .iter()
                .zip(function.param_binds.iter())
                .zip(param_vals.iter())
            {
                if let pyrust_core::ParamBind::Reg(slot) = *bind {
                    if (slot as usize) >= num_regs {
                        return Err(pyrust_core::py_err!("SystemError", "parameter '{}' register index {} out of range (num_regs={})",
                                param.name, slot, num_regs));
                    }
                    regs[slot as usize] = val.clone();
                }
            }
            // Self-reference for recursive calls (only if not a cell var).
            if let Some(slot) = function.self_bind {
                if (slot as usize) >= num_regs {
                    return Err(pyrust_core::py_err!("SystemError", "self-reference register index {} out of range (num_regs={})",
                            slot, num_regs));
                }
                regs[slot as usize] = Value::user_function(Rc::clone(&function));
            }

            let _depth_guard = CallDepthGuard::enter();
            if call_depth() > max_call_depth() {
                let exc = if let Some(cls) = self.exc_classes.get("RecursionError") {
                    instantiate_exception(
                        cls,
                        vec![Value::string("maximum recursion depth exceeded")],
                    )
                } else {
                    self.instantiate_named_exception(
                        "RecursionError",
                        "maximum recursion depth exceeded".to_string(),
                    )?
                };
                return Err(PyError::Raised(exc));
            }

            // Create a local env when the function uses globals, nonlocals, or cell vars.
            let needs_local_env = !function.global_names.is_empty()
                || !function.nonlocal_names.is_empty()
                || !code.cell_vars.is_empty();

            let previous_env = if needs_local_env {
                let local_env = self.alloc_env(Some(Rc::clone(&function.env)));
                {
                    let mut e = local_env.borrow_mut();
                    e.local_names = Rc::clone(&function.local_names);
                    e.global_names = Rc::clone(&function.global_names);
                    e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                    // Store cell var params in the env so inner closures can capture them.
                    for ((param, bind), val) in function
                        .params
                        .iter()
                        .zip(function.param_binds.iter())
                        .zip(param_vals.iter())
                    {
                        if *bind == pyrust_core::ParamBind::Cell {
                            e.values.insert(&param.name, val.clone());
                        }
                    }
                }
                std::mem::replace(&mut self.env, local_env)
            } else {
                std::mem::replace(&mut self.env, Rc::clone(&function.env))
            };

            // Issue #488: variadic generator functions (`def g(*args):
            // yield ...` and friends) must also be wrapped in a
            // GeneratorFrame instead of executed synchronously — the
            // simple-path branch already does this above; mirror it here
            // so the body's `yield` isn't observed as a runtime error.
            // Coroutines (`async def`, issue #1039) take the same path.
            if code.is_generator || code.is_coroutine {
                let gen_env = std::mem::replace(&mut self.env, previous_env);
                let gen_qualname =
                    std::sync::Arc::from(function.effective_qualname().as_str());
                return Ok(Self::build_generator_value(
                    &code,
                    regs,
                    gen_env,
                    Rc::clone(&function.local_index),
                    std::sync::Arc::from(&function.name[..]),
                    gen_qualname,
                ));
            }

            // Issue #389: publish a function frame view (see the
            // matching push in the simple-path branch above).
            // Issue #486: nonlocal_names + env for nonlocal resolution.
            let nonlocal_names_opt = if function.nonlocal_names.is_empty() {
                None
            } else {
                Some(Rc::clone(&function.nonlocal_names))
            };
            let env_opt = if function.nonlocal_names.is_empty() {
                None
            } else {
                Some(Rc::clone(&self.env))
            };
            // Capture the raw pointer and length BEFORE constructing RegSlice
            // so both the VmFrameView and the dispatch loop share the same raw
            // pointer with no &mut [Value] in scope (issue #547 / PR #646).
            let regs_ptr = unsafe {
                std::ptr::NonNull::new_unchecked(regs.as_mut_ptr())
            };
            let regs_len = regs.len();
            self.vm_frame_views.push(VmFrameView {
                kind: FrameKind::Function,
                // SAFETY: SmallVec / Vec allocation is always non-null.
                // Popped before `regs` is dropped (see above).
                regs_ptr,
                regs_len,
                local_index: Rc::clone(&function.local_index),
                nonlocal_names: nonlocal_names_opt,
                env: env_opt,
                is_class_method: code.is_class_method,
                function: Some(Rc::clone(&function)),
            });
            // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
            // of `regs` (a local RegsBuf that outlives this call).  No
            // &mut [Value] referencing `regs` is held while the dispatch loop
            // runs; RegSlice (raw pointer + len) is used instead, removing
            // the LLVM noalias constraint (issue #547).
            let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
            let vm_result = self.run_bytecode_for_fn(&code, regs_slice, function.id);
            // Lazy traceback: only build + record this frame's `FrameInfo`
            // when the body actually errored (see the simple-call path above).
            if vm_result.is_err() {
                let tb_filename = self
                    .script_filename
                    .clone()
                    .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                let tb_lineno = match pyrust_core::get_current_vm_line() {
                    0 => None,
                    n => Some(n),
                };
                pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
                    filename: tb_filename,
                    lineno: tb_lineno,
                    source_line: None,
                    funcname: std::sync::Arc::from(&function.name[..]),
                });
            }
            self.vm_frame_views.pop();

            let used_env = std::mem::replace(&mut self.env, previous_env);
            if needs_local_env {
                self.free_env(used_env);
            }
            return vm_result;
        }

        // All user functions must have precompiled bytecode
        Err(PyError::Runtime(format!("no bytecode for '{}'", function.name)))
    }

    /// Resolve a `Unicode*Error` `start`/`end` positional through the shared
    /// index protocol (#2022), storing the resolved int back into `args[idx]`.
    /// CPython 3.12 accepts any `__index__` object here and stores the resulting
    /// int as the `.start`/`.end` attribute; a non-integer raises the canonical
    /// `'X' object cannot be interpreted as an integer` TypeError.
    fn resolve_unicode_pos_arg(&mut self, args: &mut [Value], idx: usize) -> Result<()> {
        let resolved = self.value_to_index(&args[idx], |v| {
            pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(v))
        })?;
        args[idx] = resolved;
        Ok(())
    }

    /// Validate arguments for `UnicodeDecodeError(encoding, object, start, end, reason)`.
    /// Matches CPython 3.12's `UnicodeDecodeError_init` checks.
    fn validate_unicode_decode_args(&mut self, args: &mut [Value]) -> Result<()> {
        if args.len() != 5 {
            return Err(pyrust_core::type_err!("function takes exactly 5 arguments ({} given)", args.len()));
        }
        if !matches!(args[0].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 1 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[0])));
        }
        if !matches!(args[1].kind(), ValueKind::Bytes(_)) {
            return Err(pyrust_core::type_err!("a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[1])));
        }
        for idx in [2usize, 3usize] {
            self.resolve_unicode_pos_arg(args, idx)?;
        }
        if !matches!(args[4].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 5 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[4])));
        }
        Ok(())
    }

    /// Validate arguments for `UnicodeEncodeError(encoding, object, start, end, reason)`.
    /// Matches CPython 3.12's `UnicodeEncodeError_init` checks.
    fn validate_unicode_encode_args(&mut self, args: &mut [Value]) -> Result<()> {
        if args.len() != 5 {
            return Err(pyrust_core::type_err!("function takes exactly 5 arguments ({} given)", args.len()));
        }
        if !matches!(args[0].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 1 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[0])));
        }
        if !matches!(args[1].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 2 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1])));
        }
        for idx in [2usize, 3usize] {
            self.resolve_unicode_pos_arg(args, idx)?;
        }
        if !matches!(args[4].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 5 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[4])));
        }
        Ok(())
    }

    /// Validate arguments for `UnicodeTranslateError(object, start, end, reason)`.
    /// Matches CPython 3.12's `UnicodeTranslateError_init` checks.
    fn validate_unicode_translate_args(&mut self, args: &mut [Value]) -> Result<()> {
        if args.len() != 4 {
            return Err(pyrust_core::type_err!("function takes exactly 4 arguments ({} given)", args.len()));
        }
        if !matches!(args[0].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 1 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[0])));
        }
        for idx in [1usize, 2usize] {
            self.resolve_unicode_pos_arg(args, idx)?;
        }
        if !matches!(args[3].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!("argument 4 must be str, not {}",
                    pyrust_core::builtin_type_name(&args[3])));
        }
        Ok(())
    }

    fn call_class_expanded(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Issue #1956: `Cls(*args)` is uniformly `type(Cls).__call__(Cls, *args)`.
        // If the metaclass defines a *user* `__call__` override, route through
        // it; its `super().__call__(*args)` chains back to the default
        // `type.__call__` (see `default_construct`).  Ordinary classes (metatype
        // is the built-in `type`) return `None` here and fall through to the
        // existing default construct, preserving the fast path.
        if let Some(call_fn) = crate::interpreter::metaclass_dunder(&class, "__call__")
            && let ValueKind::UserFunction(f) = call_fn.kind() {
                let func = Rc::clone(f);
                return self.call_user_function_expanded(
                    func,
                    args,
                    &[Value::py_class(Rc::clone(&class))],
                );
            }

        self.default_construct(class, args)
    }

    /// The default `type.__call__` behaviour: allocate via `__new__` and run
    /// `__init__`.  This is the single construction site reached both by a
    /// plain `Cls(*args)` (when the metaclass does not override `__call__`) and
    /// by `super().__call__(*args)` chaining from a metaclass `__call__`
    /// override.  Issue #1956.
    pub(crate) fn default_construct(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        if is_exception_class(&class) {
            return self.construct_exception_instance(class, args);
        }

        self.instantiate_normal_instance(class, args)
    }

    /// Construct an instance of a built-in or user-defined exception class.
    /// Handles the user `__new__`/`__init__` dispatch for exception subclasses
    /// plus the special keyword/positional argument shapes CPython uses for
    /// `NameError`, `ImportError`, `SyntaxError`, the `Unicode*Error` family,
    /// and `BaseExceptionGroup`/`ExceptionGroup`.
    fn construct_exception_instance(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Classify the class once up front: a single non-cloning MRO walk
        // (issue #1967) that yields both the special-exception flags reused
        // throughout this function (and threaded into `instantiate_exception`)
        // and `has_user_new`/`has_user_init`.  The latter let the hot
        // `raise ValueError("x")` path skip the dedicated `__new__`/`__init__`
        // MRO lookups entirely — plain built-in exceptions have neither.
        let kinds = classify_exception_class(&class);

        // Issue #1420: if the class has a user-defined __new__ (UserFunction in
        // the MRO), call it with `cls` as the first argument before falling
        // through to instantiate_exception.  This mirrors the non-exception
        // __new__ dispatch below (issue #1143).
        let user_new = if kinds.has_user_new {
            lookup_class_attr(&class, "__new__")
                .filter(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        } else {
            None
        };
        if let Some(new_val) = user_new {
            let func = match new_val.kind() {
                ValueKind::UserFunction(f) => Rc::clone(f),
                _ => unreachable!(),
            };
            let new_result = self.call_user_function_expanded(
                func,
                args,
                &[Value::py_class(Rc::clone(&class))],
            )?;
            // After __new__, call __init__ only if the result is an instance
            // of cls (CPython parity).
            if let ValueKind::PyInstance(inst_rc) = new_result.kind() {
                let inst_class = inst_rc.borrow().class.clone();
                if class_is_subclass_of(&inst_class, &class) {
                    let init = lookup_class_attr(&inst_class, "__init__");
                    if let Some(init_val) = init
                        && matches!(
                            init_val.kind(),
                            ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                        ) {
                            let result = invoke_class_method(
                                self,
                                init_val,
                                Value::py_instance(Rc::clone(inst_rc)),
                                args,
                            )?;
                            if !result.is_none() {
                                return Err(pyrust_core::type_err!(&format!(
                                        "__init__() should return None, not '{}'",
                                        pyrust_core::builtin_type_name(&result),
                                    )));
                            }
                        }
                }
            }
            return Ok(new_result);
        }

        // Issue #1112: if the class has a user-defined __init__ (UserFunction in
        // the MRO), create the instance via instantiate_exception first (which sets
        // .args and any special attrs like StopIteration.value from the constructor
        // args), then call the user's __init__ so it can override .args via
        // super().__init__(...) and set its own instance attributes.
        let user_init = if kinds.has_user_init {
            lookup_class_attr(&class, "__init__")
                .filter(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        } else {
            None
        };
        if let Some(init_val) = user_init {
            let values: Vec<Value> = args
                .iter()
                .filter(|a| a.name.is_none())
                .map(|a| a.value.clone())
                .collect();
            let instance = instantiate_exception(Rc::clone(&class), values);
            let result = invoke_class_method(self, init_val, instance.clone(), args)?;
            if !result.is_none() {
                return Err(pyrust_core::type_err!(&format!(
                        "__init__() should return None, not '{}'",
                        pyrust_core::builtin_type_name(&result),
                    )));
            }
            return Ok(instance);
        }

        // CPython 3.12: NameError.__init__ accepts exactly one keyword argument
        // (`name=`); ImportError.__init__ accepts two (`name=` and `path=`).
        // Extract any recognised keyword arguments before building the positional
        // values list; reject unrecognised keywords with the class-specific
        // error message CPython uses.
        //
        // IMPORTANT: CPython's error messages always use the *base* class name
        // ("NameError()" / "ImportError()"), even when the actual class is a
        // subclass like UnboundLocalError or ModuleNotFoundError.
        let class_name = class.borrow().name.clone();
        let is_name_error_class = kinds.name_error;
        let is_import_error_class = kinds.import_error;
        let mut kw_name: Option<Value> = None;
        let mut kw_path: Option<Value> = None;
        let mut values = Vec::with_capacity(args.len());
        if is_name_error_class {
            // CPython 3.12: NameError accepts at most 1 keyword argument (`name=`).
            // If total kwarg count > 1, raises "takes at most 1 keyword argument".
            // If total kwarg count == 1 and it is not `name=`, raises "invalid keyword".
            // Error messages always say "NameError()" regardless of the actual subclass.
            let kw_count = args.iter().filter(|a| a.name.is_some()).count();
            if kw_count > 1 {
                return Err(pyrust_core::type_err!("NameError() takes at most 1 keyword argument ({kw_count} given)"));
            }
            for arg in args {
                match arg.name.as_deref() {
                    None => values.push(arg.value.clone()),
                    Some("name") => kw_name = Some(arg.value.clone()),
                    Some(other) => {
                        return Err(pyrust_core::type_err!("'{other}' is an invalid keyword argument for NameError()"));
                    }
                }
            }
        } else if is_import_error_class {
            // CPython 3.12: ImportError accepts `name=` and `path=`; any other
            // keyword raises "'X' is an invalid keyword argument for ImportError()".
            // Error messages always say "ImportError()" regardless of the actual subclass.
            for arg in args {
                match arg.name.as_deref() {
                    None => values.push(arg.value.clone()),
                    Some("name") => kw_name = Some(arg.value.clone()),
                    Some("path") => kw_path = Some(arg.value.clone()),
                    Some(other) => {
                        return Err(pyrust_core::type_err!("'{other}' is an invalid keyword argument for ImportError()"));
                    }
                }
            }
        } else {
            reject_keyword_args_expanded(&class_name, args)?;
            for arg in args {
                values.push(arg.value.clone());
            }
        }
        // CPython 3.12 SyntaxError.__init__ validates args[1] if present:
        // it must be an iterable that yields exactly 4 or 6 elements.
        // Non-iterables raise TypeError; the wrong number raises TypeError.
        if kinds.syntax_error && values.len() >= 2 {
            let second = &values[1];
            let items_opt: Option<Vec<Value>> = second
                .as_tuple()
                .map(|s| s.to_vec())
                .or_else(|| second.as_list().map(|s| s.to_vec()));
            match items_opt {
                None => {
                    // args[1] is not a sequence — CPython raises TypeError
                    return Err(pyrust_core::type_err!(&format!(
                            "'{}' object is not iterable",
                            pyrust_core::builtin_type_name(second)
                        )));
                }
                Some(ref items) if items.len() < 4 => {
                    return Err(pyrust_core::type_err!(&format!(
                            "function takes at least 4 arguments ({} given)",
                            items.len()
                        )));
                }
                Some(ref items) if items.len() == 5 => {
                    return Err(pyrust_core::type_err!("end_offset must be provided when end_lineno is provided"));
                }
                Some(ref items) if items.len() > 6 => {
                    return Err(pyrust_core::type_err!(&format!(
                            "function takes at most 6 arguments ({} given)",
                            items.len()
                        )));
                }
                _ => {}
            }
        }
        // CPython 3.12: UnicodeDecodeError and UnicodeEncodeError require
        // exactly 5 positional arguments; UnicodeTranslateError requires 4.
        // Also validate argument types (encoding must be str, object must be
        // bytes for Decode / str for Encode, start/end must be int-like,
        // reason must be str).
        if kinds.unicode_decode_error {
            self.validate_unicode_decode_args(&mut values)?;
        } else if kinds.unicode_encode_error {
            self.validate_unicode_encode_args(&mut values)?;
        } else if kinds.unicode_translate_error {
            self.validate_unicode_translate_args(&mut values)?;
        }
        // PEP 654 (Python 3.11+): BaseExceptionGroup and ExceptionGroup validation.
        // CPython validates in BaseExceptionGroup.__new__:
        //  - message must be a str
        //  - exceptions must be a non-empty sequence of BaseException instances
        //  - If calling ExceptionGroup, all exceptions must be Exception subclasses
        //  - If calling BaseExceptionGroup and all exceptions are Exception subclasses,
        //    the returned type is silently promoted to ExceptionGroup.
        let is_base_exception_group = kinds.base_exception_group;
        if is_base_exception_group {
            // Validate arg count.
            if values.len() != 2 {
                return Err(pyrust_core::type_err!(&format!(
                        "BaseExceptionGroup.__new__() takes exactly 2 arguments ({} given)",
                        values.len()
                    )));
            }
            // Validate message is a str.
            // CPython: "BaseExceptionGroup.__new__() argument 1 must be str, not <type>"
            if !matches!(values[0].kind(), ValueKind::Str(_)) {
                return Err(pyrust_core::type_err!(&format!(
                        "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
                        pyrust_core::builtin_type_name(&values[0])
                    )));
            }
            // Validate exceptions is a non-empty sequence.
            let exc_items: Option<Vec<Value>> = values[1]
                .as_tuple()
                .map(|s| s.to_vec())
                .or_else(|| values[1].as_list().map(|s| s.to_vec()));
            let exc_items = if let Some(items) = exc_items {
                items
            } else {
                // CPython raises TypeError for non-sequence second argument
                // (e.g. an integer, a generator/iterator), and ValueError
                // for a sequence whose items are not exceptions (e.g. a string
                // whose characters are not exceptions).
                // Match CPython: str is a sequence, so each character is
                // checked and produces ValueError; everything else is TypeError.
                if let ValueKind::Str(s) = values[1].kind() {
                    if s.is_empty() {
                        return Err(pyrust_core::value_err!("second argument (exceptions) must be a non-empty sequence"));
                    }
                    return Err(pyrust_core::value_err!("Item 0 of second argument (exceptions) is not an exception"));
                }
                return Err(pyrust_core::type_err!("second argument (exceptions) must be a sequence"));
            };
            if exc_items.is_empty() {
                return Err(pyrust_core::value_err!("second argument (exceptions) must be a non-empty sequence"));
            }
            // Validate each exception is a BaseException instance.
            for (i, exc_val) in exc_items.iter().enumerate() {
                let ok = if let ValueKind::PyInstance(inst_rc) = exc_val.kind() {
                    class_chain_contains_name(&inst_rc.borrow().class, "BaseException")
                } else {
                    false
                };
                if !ok {
                    return Err(pyrust_core::value_err!(&format!(
                            "Item {} of second argument (exceptions) is not an exception",
                            i
                        )));
                }
            }
            // If ExceptionGroup, all exceptions must be Exception (not just BaseException).
            let actual_class_name = class.borrow().name.clone();
            let is_eg = actual_class_name.as_str() == "ExceptionGroup";
            if is_eg {
                for exc_val in &exc_items {
                    if let ValueKind::PyInstance(inst_rc) = exc_val.kind()
                        && !class_chain_contains_name(&inst_rc.borrow().class, "Exception") {
                            return Err(pyrust_core::type_err!("Cannot nest BaseExceptions in an ExceptionGroup"));
                        }
                }
            }
            // CPython: if calling BaseExceptionGroup and all exceptions are Exception
            // subclasses, the returned type is ExceptionGroup.
            let is_beg = actual_class_name.as_str() == "BaseExceptionGroup";
            let actual_class = if is_beg {
                let all_exceptions = exc_items.iter().all(|exc_val| {
                    if let ValueKind::PyInstance(inst_rc) = exc_val.kind() {
                        class_chain_contains_name(&inst_rc.borrow().class, "Exception")
                    } else {
                        false
                    }
                });
                if all_exceptions {
                    // Promote to ExceptionGroup.
                    lookup_exc_class("ExceptionGroup").unwrap_or(class)
                } else {
                    class
                }
            } else {
                class
            };
            let instance = instantiate_exception(actual_class, values);
            return Ok(instance);
        }
        // Reuse the `kinds` classification computed above instead of running a
        // second MRO walk inside `instantiate_exception` (perf: one classify per
        // raise instead of two).
        let instance = instantiate_exception_with_kinds(class, values, &kinds);
        // Apply keyword arguments extracted above for NameError and ImportError.
        // `instantiate_exception` already initialised `.name` (and `.path`) to
        // `None`; override them with the caller-supplied values when provided.
        // CPython 3.12: keyword values are NOT included in `.args`.
        if let Some(name_val) = kw_name
            && let ValueKind::PyInstance(inst_rc) = instance.kind() {
                inst_rc.borrow_mut().attrs.insert("name", name_val);
            }
        if let Some(path_val) = kw_path
            && let ValueKind::PyInstance(inst_rc) = instance.kind() {
                inst_rc.borrow_mut().attrs.insert("path", path_val);
            }
        Ok(instance)
    }

    /// Instantiate a normal (non-exception) class: walk the MRO for a
    /// user-defined `__new__`, then call `__init__`, falling back to the
    /// default allocation + primitive-backing path when neither is defined.
    fn instantiate_normal_instance(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Primitive classes never reach this fn — the `PyClass` arm in
        // `call_function_expanded` short-circuits them via
        // `PRIMITIVE_CLASS_DISPATCH` (issue #462).  Subclasses of
        // primitives (`class S(int): pass`) DO reach here but without an
        // inherited `__init__` (helpers.rs deliberately leaves primitive
        // class attrs empty so the BuiltinFunction constructor isn't
        // exposed to PyInstance-based subclass dispatch — see #463
        // Copilot review).  They land in the `None` arm of the
        // init match below.

        // Resolve `__new__`, `__init__`, and the primitive-base classification
        // in a *single* base-chain walk (the common single-inheritance case).
        // Previously each of these did its own full MRO traversal on every
        // instantiation, so the per-construction cost scaled with MRO depth.
        // `resolve_construction_plan` returns `None` for multiply-inherited
        // classes, where attribute resolution must follow the C3 MRO — those
        // fall back to the byte-identical per-attr `lookup_class_attr` walks.
        let plan = resolve_construction_plan_cached(&class);
        let prim = match &plan {
            Some(p) => p.prim,
            None => classify_primitive_base(&class),
        };

        // `__new__` protocol: walk the MRO for `__new__`, skipping the
        // default `object.__new__` (which the normal allocation path below
        // already implements).  If a user-defined `__new__` (UserFunction) or
        // a non-object BuiltinFunction `__new__` is found, call it with `cls`
        // as the first argument, then call `__init__` on the result if it is
        // an instance of `cls` (CPython parity, issue #1143).
        let mro_new = match &plan {
            Some(p) => p.new_val.clone(),
            None => lookup_class_attr(&class, "__new__"),
        };
        let has_user_new = mro_new.as_ref().is_some_and(|v| {
            !matches!(v.kind(), ValueKind::BuiltinFunction("object.__new__"))
        });
        if has_user_new {
            let new_val = mro_new.unwrap();
            let new_result = match new_val.kind() {
                ValueKind::UserFunction(f) => {
                    let func = Rc::clone(f);
                    // Prepend `cls` as the first positional arg.
                    self.call_user_function_expanded(
                        func,
                        args,
                        &[Value::py_class(Rc::clone(&class))],
                    )?
                }
                ValueKind::BuiltinFunction(name) => {
                    let dispatch = crate::builtin_registry::lookup(name).ok_or_else(|| {
                        PyError::Runtime(format!(
                            "internal: __new__ builtin '{name}' not in registry"
                        ))
                    })?;
                    let mut combined: ExpandedArgBuf = ExpandedArgBuf::with_capacity(args.len() + 1);
                    combined.push(ExpandedCallArg {
                        name: None,
                        value: Value::py_class(Rc::clone(&class)),
                    });
                    combined.extend(args.iter().cloned());
                    dispatch(self, &combined)?
                }
                _ => {
                    return Err(pyrust_core::type_err!("__new__ must be a callable, not '{}'",
                            pyrust_core::builtin_type_name(&new_val)));
                }
            };

            // After `__new__` succeeds, call `__init__` on the result if it
            // is a PyInstance whose class is equal to or a subclass of `cls`.
            if let ValueKind::PyInstance(inst_rc) = new_result.kind() {
                let inst_class = inst_rc.borrow().class.clone();
                if class_is_subclass_of(&inst_class, &class) {
                    let init = lookup_class_attr(&inst_class, "__init__");
                    if let Some(init_val) = init
                        && matches!(
                            init_val.kind(),
                            ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                        ) {
                            let result = invoke_class_method(
                                self,
                                init_val,
                                Value::py_instance(Rc::clone(inst_rc)),
                                args,
                            )?;
                            if !result.is_none() {
                                return Err(pyrust_core::type_err!(&format!(
                                        "__init__() should return None, not '{}'",
                                        pyrust_core::builtin_type_name(&result),
                                    )));
                            }
                        }
                }
            }
            // Issue #1385: metaclass protocol — if __new__ returned a PyClass
            // (i.e. a class object was constructed) and the calling class is a
            // metaclass (subclass of type), call __init__ on the new class with
            // the same arguments.  This mirrors type.__call__'s two-phase
            // __new__ + __init__ protocol applied at the metaclass level.
            if let ValueKind::PyClass(new_class_rc) = new_result.kind() {
                let new_class_rc = Rc::clone(new_class_rc);
                let type_cls = type_class_singleton();
                if class_is_subclass_of(&class, &type_cls) {
                    let init = lookup_class_attr(&class, "__init__");
                    if let Some(init_val) = init
                        && matches!(
                            init_val.kind(),
                            ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                        ) {
                            // Skip type.__init__ (the no-op sentinel) to avoid
                            // double-calling when there is no user __init__.
                            let is_type_init = matches!(
                                init_val.kind(),
                                ValueKind::BuiltinFunction("type.__init__")
                            );
                            if !is_type_init {
                                let result = invoke_class_method(
                                    self,
                                    init_val,
                                    Value::py_class(Rc::clone(&new_class_rc)),
                                    args,
                                )?;
                                if !result.is_none() {
                                    return Err(pyrust_core::type_err!(&format!(
                                            "__init__() should return None, not '{}'",
                                            pyrust_core::builtin_type_name(&result),
                                        )));
                                }
                            }
                        }
                }
            }
            return Ok(new_result);
        }

        let instance = Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(&class),
            attrs: InstanceAttrs::new(),
        }));

        // `prim` (the primitive-base classification) was resolved together with
        // `__new__` / `__init__` in the single construction-plan walk above.
        // The three primitive categories are mutually exclusive — a class cannot
        // inherit two incompatible primitive storage layouts.
        match prim {
            // Issue #976: dict/list/set — pre-initialise an *empty* backing
            // store *before* calling __init__, so that `self[k] = v` (or any
            // method call on `self`) inside a user-defined __init__ sees a valid
            // __builtin_data__ entry to delegate to.  When there is no __init__,
            // the None arm below replaces it with the args-populated value.
            PrimitiveBase::Mutable(prim_name) => {
                if let Some(dispatch) = crate::builtin_registry::lookup(prim_name) {
                    let backing = dispatch(self, &[])?;
                    instance
                        .borrow_mut()
                        .attrs
                        .insert(BUILTIN_DATA_ATTR, backing);
                }
            }
            // Issue #994 (frozenset/tuple) and #1204 (str/int/float/bytes/…):
            // immutable / scalar primitives build the backing from the
            // constructor args immediately — the content is fixed at
            // construction and __init__ cannot change it.
            PrimitiveBase::Immutable(prim_name) | PrimitiveBase::Scalar(prim_name) => {
                if let Some(dispatch) = crate::builtin_registry::lookup(prim_name) {
                    let backing = dispatch(self, args)?;
                    instance
                        .borrow_mut()
                        .attrs
                        .insert(BUILTIN_DATA_ATTR, backing);
                }
            }
            PrimitiveBase::None => {}
        }

        let init = match plan {
            Some(p) => p.init_val,
            None => lookup_class_attr(&class, "__init__"),
        };

        // CPython parity (issue #2323): for a plain (non-primitive) class that
        // overrides neither `__new__` nor `__init__`, excess construction
        // arguments are rejected by `object.__new__` *before* `object.__init__`
        // runs, with the wording `<Cls>() takes no arguments` — not the
        // `__init__`-arity wording.  We are past the `has_user_new` early return
        // above, so reaching here means `__new__` is `object.__new__`; if the
        // resolved `__init__` is also the `object.__init__` sentinel (or absent),
        // neither slot is user-defined and the bare-class message applies.
        // Primitive bases (`prim != None`) legitimately consume args via their
        // backing constructor and must not be caught here.  The check reads the
        // already-resolved `init` (from the cached construction plan) and adds no
        // work to the steady-state path with a user `__init__`.
        let init_is_object = matches!(
            init.as_ref().map(|v| v.kind()),
            None | Some(ValueKind::BuiltinFunction("object.__init__"))
        );
        if prim == PrimitiveBase::None && init_is_object && !args.is_empty() {
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!("{class_name}() takes no arguments"));
        }

        match init {
            Some(method_val)
                if matches!(
                    method_val.kind(),
                    ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                ) =>
            {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&instance)),
                    args,
                )?;
                if !result.is_none() {
                    return Err(pyrust_core::type_err!(&format!(
                            "__init__() should return None, not '{}'",
                            pyrust_core::builtin_type_name(&result),
                        )));
                }
            }
            Some(_) => {
                return Err(PyError::Runtime(
                    "__init__ attribute is not callable".to_string(),
                ));
            }
            None => {
                // No `__init__` in the MRO.  If the class inherits from a
                // mutable primitive (dict / list / set), call that base's
                // constructor with the provided args and store the result as
                // `__builtin_data__` so subscript / method dispatch can
                // delegate to the backing value (issue #976).
                // NOTE: the empty backing was already inserted above; replace it
                // with the args-populated value now.
                match prim {
                    PrimitiveBase::Mutable(prim_name) => {
                        if let Some(dispatch) = crate::builtin_registry::lookup(prim_name) {
                            let backing = dispatch(self, args)?;
                            instance
                                .borrow_mut()
                                .attrs
                                .insert(BUILTIN_DATA_ATTR, backing);
                        }
                    }
                    PrimitiveBase::None => {
                        // Excess args with no `__init__`/`__new__` are already
                        // rejected (as `TypeError`) by the bare-class guard above
                        // (issue #2323); reaching here means `args` is empty.
                    }
                    // Immutable / scalar primitives already populated their
                    // args-based backing above; nothing more to do here.
                    PrimitiveBase::Immutable(_) | PrimitiveBase::Scalar(_) => {}
                }
            }
        }

        Ok(Value::py_instance(instance))
    }


    pub(crate) fn call_range_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<Value> {
        reject_keyword_args_expanded("range", args)?;
        if args.is_empty() {
            return Err(pyrust_core::type_err!("range expected at least 1 argument, got {}",
                    args.len()));
        }
        if args.len() > 3 {
            return Err(pyrust_core::type_err!("range expected at most 3 arguments, got {}",
                    args.len()));
        }

        // Resolve each argument to an arbitrary-precision int through the
        // `__index__` protocol (#2118).  Bounds beyond i64 are kept as BigInt;
        // `Value::range_big` collapses the all-i64 case back to the cheap
        // i64-backed range so the common path is unchanged.
        let mut ints: Vec<PyBigInt> = Vec::with_capacity(args.len());
        for arg in args {
            ints.push(self.coerce_range_arg_big(arg.value.clone())?);
        }

        let (start, stop, step) = match ints.as_slice() {
            [stop] => (PyBigInt::from(0), stop.clone(), PyBigInt::from(1)),
            [start, stop] => (start.clone(), stop.clone(), PyBigInt::from(1)),
            [start, stop, step] => (start.clone(), stop.clone(), step.clone()),
            _ => unreachable!("validated by length"),
        };

        if step == PyBigInt::from(0) {
            return Err(pyrust_core::value_err!("range() arg 3 must not be zero"));
        }

        Ok(Value::range_big(start, stop, step))
    }

    /// Return the compiled `FnCode` for `function`.
    /// Returns `None` only if `precompiled_code` is absent.
    fn get_or_compile_bytecode(&mut self, function: &Rc<UserFunction>) -> Option<Rc<FnCode>> {
        function
            .precompiled_code
            .as_ref()
            .and_then(|rc| Rc::clone(rc).downcast::<FnCode>().ok())
    }

    /// Implements `str.format_map(mapping)`.  Parses `{name}` replacement
    /// fields and looks up each name via `mapping[name]` (`__getitem__`).
    ///
    /// Unlike `format_str_template`, positional fields (`{}` or `{0}`) raise
    /// `ValueError: Format string contains positional fields`, matching CPython
    /// 3.12.  Conversion (`!r`/`!s`/`!a`), format specs (`:…`), and field
    /// accessors (`.attr` / `[key]`) are supported identically to `format`.
    pub(crate) fn format_str_template_map(
        &mut self,
        template: &str,
        mapping: Value,
    ) -> Result<Value> {
        let bytes = template.as_bytes();
        let mut out = String::with_capacity(template.len());
        let mut i = 0;

        while i < bytes.len() {
            let c = bytes[i];
            if c == b'{' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                    out.push('{');
                    i += 2;
                    continue;
                }
                // Find matching '}'.
                let mut depth = 1;
                let mut j = i + 1;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                    j += 1;
                }
                if depth != 0 {
                    return Err(pyrust_core::value_err!("Single '{' encountered in format string".to_string()));
                }
                let field = &template[i + 1..j];
                i = j + 1;

                let (field_name_full, spec) = split_field_and_spec(field);
                let (field_name, conversion) = match field_name_full.rsplit_once('!') {
                    Some((name, conv)) if conv.len() == 1 => {
                        (name, Some(conv.chars().next().unwrap()))
                    }
                    _ => (field_name_full, None),
                };

                let (head, rest) = split_head_and_accessors(field_name);
                // format_map does not support positional fields.
                if head.is_empty() || head.parse::<usize>().is_ok() {
                    return Err(pyrust_core::value_err!("Format string contains positional fields"));
                }
                // Look up the named key in the mapping via __getitem__.
                let base =
                    self.eval_index(&mapping, Value::string(head))?;

                let value = apply_field_accessors(self, base, rest)?;
                let value = match conversion {
                    Some('r') => Value::string(render_instance_repr(self, &value)?),
                    Some('s') => Value::string(self.render_value_as_str(&value)?),
                    Some('a') => Value::string(ascii_repr_interp(self, &value)?),
                    Some(c) => {
                        return Err(pyrust_core::value_err!("Unknown conversion specifier {c}"));
                    }
                    None => value,
                };
                // Expand any `{name}` references inside the format spec
                // (PEP 3101 one-level nesting).  Only named fields are allowed;
                // positional fields in the spec raise ValueError.
                let expanded_spec;
                let spec = if spec.contains('{') {
                    let mut spec_out = String::new();
                    let sbytes = spec.as_bytes();
                    let mut si = 0;
                    while si < sbytes.len() {
                        match sbytes[si] {
                            b'{' if si + 1 < sbytes.len() && sbytes[si + 1] == b'{' => {
                                spec_out.push('{');
                                si += 2;
                            }
                            b'}' if si + 1 < sbytes.len() && sbytes[si + 1] == b'}' => {
                                spec_out.push('}');
                                si += 2;
                            }
                            b'{' => {
                                let ss = si + 1;
                                let se = sbytes[ss..]
                                    .iter()
                                    .position(|&b| b == b'}')
                                    .ok_or_else(|| {
                                        pyrust_core::value_err!("Single '{' encountered in format string".to_string())
                                    })?
                                    + ss;
                                // PEP 3101: inner fields cannot have a nested spec; if the
                                // user wrote `{name:spec}` inside a format spec, CPython
                                // treats everything before `:` as the field name.
                                let inner_raw = &spec[ss..se];
                                si = se + 1;
                                let inner = inner_raw
                                    .split_once(':')
                                    .map(|(name, _)| name)
                                    .unwrap_or(inner_raw);
                                if inner.is_empty() || inner.parse::<usize>().is_ok() {
                                    return Err(pyrust_core::value_err!("Format string contains positional fields"));
                                }
                                let sv = self.eval_index(
                                    &mapping,
                                    Value::string(inner),
                                )?;
                                spec_out.push_str(&sv.to_py_str());
                            }
                            b'}' => {
                                return Err(pyrust_core::value_err!("Single '}' encountered in format string".to_string()));
                            }
                            _ => {
                                let ch_s = si;
                                si += 1;
                                while si < sbytes.len() && (sbytes[si] & 0xC0) == 0x80 {
                                    si += 1;
                                }
                                spec_out.push_str(&spec[ch_s..si]);
                            }
                        }
                    }
                    expanded_spec = spec_out;
                    expanded_spec.as_str()
                } else {
                    spec
                };
                let formatted = self.dispatch_dunder_format(&value, spec)?;
                out.push_str(&extract_str_value(&formatted));
            } else if c == b'}' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                    out.push('}');
                    i += 2;
                } else {
                    return Err(pyrust_core::value_err!("Single '}' encountered in format string".to_string()));
                }
            } else {
                let ch_start = i;
                i += 1;
                while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                    i += 1;
                }
                out.push_str(&template[ch_start..i]);
            }
        }
        Ok(Value::string(out))
    }

    /// Resolve a single `range()` argument to an arbitrary-precision `BigInt`
    /// through the `__index__` protocol (#2118).  Unlike [`Self::coerce_range_arg`]
    /// this does not clamp to `i64`, so `range(10**20)` no longer raises
    /// `OverflowError`.  A non-integer (no `__index__`) raises
    /// `'X' object cannot be interpreted as an integer`, matching CPython 3.12.
    fn coerce_range_arg_big(&mut self, val: Value) -> Result<PyBigInt> {
        let resolved = self.value_to_index(&val, |v| {
            pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(v))
        })?;
        Ok(value_to_bigint(&resolved).expect("value_to_index returns an int-like value"))
    }

    /// Resolve a single start/stop argument for `list.index` / `tuple.index`
    /// through the `__index__` protocol, matching CPython 3.12 semantics.
    ///
    /// Thin wrapper over [`Interpreter::value_to_index`]; on a non-integer
    /// non-`__index__` value it raises `TypeError: slice indices must be
    /// integers or have an __index__ method`.
    fn resolve_index_arg(&mut self, val: Value) -> Result<Value> {
        self.value_to_index(&val, |_| {
            pyrust_core::type_err!("slice indices must be integers or have an __index__ method")
        })
    }

    /// For `list.index` / `tuple.index`, resolve start (pos[1]) and stop
    /// (pos[2]) through `resolve_index_arg`.  pos[0] is the search target
    /// and is left unchanged.  Returns a new `Vec<Value>` with the resolved
    /// slice-boundary arguments in place.
    fn resolve_seq_index_pos(&mut self, mut pos: Vec<Value>) -> Result<Vec<Value>> {
        if pos.len() >= 2 {
            let start = pos.remove(1);
            let resolved = self.resolve_index_arg(start)?;
            pos.insert(1, resolved);
        }
        if pos.len() >= 3 {
            let stop = pos.remove(2);
            let resolved = self.resolve_index_arg(stop)?;
            pos.insert(2, resolved);
        }
        Ok(pos)
    }

    /// Returns `true` if searching `target` in `items` requires user `__eq__`
    /// dispatch (i.e. `values_user_eq`), rather than the primitive `Value::eq`
    /// (which uses `Rc::ptr_eq` for `PyInstance` and recursion for containers).
    ///
    /// Matches the same gate used by the `in` operator fix in PR #1638.
    fn seq_search_needs_dispatch(target: &Value, items: &[Value]) -> bool {
        matches!(
            target.kind(),
            ValueKind::PyInstance(_)
                | ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
                | ValueKind::BuiltinObject { .. }
        ) || items.iter().any(|e| {
            matches!(
                e.kind(),
                ValueKind::PyInstance(_)
                    | ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Dict(_)
                    | ValueKind::Set(_)
                    | ValueKind::BuiltinObject { .. }
            )
        })
    }

    /// O(1) membership test for range values, matching `eval_in` for Range.
    /// Returns `true` iff `v` is an element of `range(start, stop, step)`.
    fn range_contains_value(&self, start: i64, stop: i64, step: i64, v: &Value) -> Result<bool> {
        use crate::value::PyToPrimitive;
        let range_contains_i64 = |x: i64| -> bool {
            if step > 0 {
                x >= start && x < stop && (x - start) % step == 0
            } else if step < 0 {
                x <= start && x > stop && (x - start) % step == 0
            } else {
                false
            }
        };
        Ok(match v.kind() {
            ValueKind::Int(x) => range_contains_i64(x),
            ValueKind::Bool(b) => range_contains_i64(b as i64),
            ValueKind::BigInt(n) => n.to_i64().is_some_and(range_contains_i64),
            ValueKind::Float(f) => {
                const I64_MIN_F: f64 = i64::MIN as f64;
                const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                f.is_finite()
                    && f.fract() == 0.0
                    && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&f)
                    && range_contains_i64(f as i64)
            }
            _ => false,
        })
    }

    /// Arbitrary-precision analogue of [`Self::range_contains_value`] (#2118):
    /// resolves `v` to an integer value (int/bool/bigint or an integer-valued
    /// finite float) and applies the BigInt membership formula.  Returns the
    /// resolved `BigInt` when `v` is a member, else `None`.
    fn bigrange_member(
        start: &PyBigInt,
        stop: &PyBigInt,
        step: &PyBigInt,
        v: &Value,
    ) -> Option<PyBigInt> {
        use num_traits::FromPrimitive;
        let x: PyBigInt = match v.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => value_to_bigint(v)?,
            ValueKind::Float(f) if f.is_finite() && f.fract() == 0.0 => PyBigInt::from_f64(f)?,
            _ => return None,
        };
        let sgn = step.sign();
        let in_bounds = if sgn == pyrust_core::PyBigIntSign::Plus {
            x >= *start && x < *stop
        } else {
            x <= *start && x > *stop
        };
        if in_bounds && ((&x - start) % step).sign() == pyrust_core::PyBigIntSign::NoSign {
            Some(x)
        } else {
            None
        }
    }

    /// `list.index(target[, start[, stop]])` / `tuple.index(...)` with correct
    /// `__eq__` dispatch.
    ///
    /// The `args` slice must already have had its start/stop arguments resolved
    /// through `resolve_seq_index_pos` (so they are `Int`/`BigInt`/`Bool`).
    /// The `items` snapshot is taken by the caller (snapshot required to release
    /// the list/tuple borrow before calling `values_user_eq`, which may re-enter
    /// user code that mutates the receiver).
    fn call_seq_index(
        &mut self,
        items: Vec<Value>,
        args: &[Value],
        type_name: &'static str,
    ) -> Result<Value> {
        let target = args.first().ok_or_else(|| {
            pyrust_core::type_err!("index expected at least 1 argument, got 0")
        })?;
        let len = items.len();
        let start = match args.get(1).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => pyrust_builtins::sequence::normalise_index(i, len).min(len),
            Some(ValueKind::Bool(b)) => {
                pyrust_builtins::sequence::normalise_index(b as i64, len).min(len)
            }
            Some(ValueKind::BigInt(b)) => {
                pyrust_builtins::sequence::normalise_bigint_index(b, len).min(len)
            }
            None => 0,
            // Other types were already rejected by resolve_seq_index_pos.
            _ => 0,
        };
        let stop = match args.get(2).map(|v| v.kind()) {
            Some(ValueKind::Int(i)) => pyrust_builtins::sequence::normalise_index(i, len).min(len),
            Some(ValueKind::Bool(b)) => {
                pyrust_builtins::sequence::normalise_index(b as i64, len).min(len)
            }
            Some(ValueKind::BigInt(b)) => {
                pyrust_builtins::sequence::normalise_bigint_index(b, len).min(len)
            }
            None => len,
            _ => len,
        };
        let stop = stop.max(start);
        let window = &items[start..stop];
        if Self::seq_search_needs_dispatch(target, window) {
            for (i, item) in window.iter().enumerate() {
                if self.values_user_eq(item, target)? {
                    return Ok(Value::int((start + i) as i64));
                }
            }
        } else {
            for (i, item) in window.iter().enumerate() {
                if item == target {
                    return Ok(Value::int((start + i) as i64));
                }
            }
        }
        let msg = if type_name == "tuple" {
            "tuple.index(x): x not in tuple".to_string()
        } else {
            let repr_str = render_instance_repr(self, target)?;
            format!("{repr_str} is not in {type_name}")
        };
        Err(pyrust_core::value_err!(msg))
    }

    /// `list.count(target)` / `tuple.count(target)` with correct `__eq__` dispatch.
    ///
    /// The `items` snapshot is taken by the caller for the same borrow-safety
    /// reason as `call_seq_index`.
    fn call_seq_count(
        &mut self,
        items: Vec<Value>,
        args: &[Value],
        type_name: &'static str,
    ) -> Result<Value> {
        let target = args.first().ok_or_else(|| {
            pyrust_core::type_err!("{type_name}.count() takes exactly one argument (0 given)")
        })?;
        if Self::seq_search_needs_dispatch(target, &items) {
            let mut n: i64 = 0;
            for item in &items {
                if self.values_user_eq(item, target)? {
                    n += 1;
                }
            }
            Ok(Value::int(n))
        } else {
            let n = items.iter().filter(|v| *v == target).count();
            Ok(Value::int(n as i64))
        }
    }

    /// `list.remove(target)` with correct `__eq__` dispatch.
    ///
    /// Mirrors CPython's `list_remove`: walk the live list by index and remove
    /// the **first** element equal to `target`, in place, on a no match raise
    /// `ValueError` with CPython's fixed wording.
    ///
    /// The per-element comparison fuses two cases in a single scan so the common
    /// all-primitive case never pays for user-dispatch machinery (no second
    /// pass, no `values_user_eq` call):
    ///
    /// - When neither `target` nor the element can fire user `__eq__`
    ///   (`value_search_dispatches` is false for both), compare with
    ///   `Value::eq` — the same primitive equality the interpreter-free fast
    ///   path uses.
    /// - Otherwise dispatch through `values_user_eq`, which checks identity
    ///   before `__eq__` (matching `PyObject_RichCompareBool(item, x, Py_EQ)`)
    ///   and may re-enter user code.
    ///
    /// The element is read fresh from the receiver each iteration and the length
    /// is rechecked before removal, so a user `__eq__` that mutates the list
    /// mid-search cannot index out of range.
    fn call_seq_remove(&mut self, receiver: &Value, args: Vec<Value>) -> Result<Value> {
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "list.remove() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        // Outcome of the single-borrow primitive fast scan below.
        enum SeqRemoveScan {
            Found(usize),
            NotFound,
            NeedsDispatch,
        }

        let target = &args[0];
        // Whether `target` itself can fire user `__eq__` (container / instance).
        // Cheap, kind-only check — does not scan the list.
        let target_dispatches = Self::value_search_dispatches(target);

        // Fast path: when `target` is primitive, attempt a single-borrow scan
        // using `Value::eq` (no re-entry into user code).  We can resolve the
        // whole search this way *only* while every element seen is also
        // primitive — a dispatching element (PyInstance / container) might
        // match `target` through its own `__eq__`, so as soon as we reach one
        // we abandon the fast scan and restart the slow per-index walk from the
        // front (preserving first-match semantics).  When no dispatching
        // element exists, this is one borrow and one pass — matching the
        // interpreter-free `ms::remove` cost.
        if !target_dispatches {
            let outcome = receiver.list_with(|items| {
                for (i, item) in items.iter().enumerate() {
                    // `cannot_user_eq` resolves scalar elements (the common
                    // all-int/str list) from `top16` alone — a single tag
                    // compare, no `ValueKind` build and no `RefCell` borrow —
                    // so the hot scan pays only this plus the `Value::eq`
                    // below, matching the interpreter-free `ms::remove` cost.
                    // A non-scalar element might match `target` through its own
                    // `__eq__`, so we abandon the fast scan and restart the slow
                    // per-index walk from the front (preserving first-match).
                    if !item.cannot_user_eq() {
                        return SeqRemoveScan::NeedsDispatch;
                    }
                    if item == target {
                        return SeqRemoveScan::Found(i);
                    }
                }
                SeqRemoveScan::NotFound
            });
            match outcome {
                Some(SeqRemoveScan::Found(i)) => {
                    receiver.list_pop_at(i)?;
                    return Ok(Value::none());
                }
                Some(SeqRemoveScan::NotFound) => {
                    return Err(pyrust_core::value_err!("list.remove(x): x not in list"));
                }
                // NeedsDispatch, or receiver is no longer a list — fall through
                // to the slow per-index walk below.
                _ => {}
            }
        }

        // Slow path: at least one operand can fire user `__eq__`.  Read each
        // element fresh and recheck length before removal so a user `__eq__`
        // that mutates the list mid-search cannot index out of range.
        let mut i = 0usize;
        loop {
            let item = match receiver.list_with(|items| items.get(i).cloned()) {
                Some(Some(item)) => item,
                _ => break,
            };
            if self.values_user_eq(&item, target)? {
                if receiver.list_with(|items| i < items.len()).unwrap_or(false) {
                    receiver.list_pop_at(i)?;
                }
                return Ok(Value::none());
            }
            i += 1;
        }
        Err(pyrust_core::value_err!("list.remove(x): x not in list"))
    }

    /// `true` if a single value can fire user `__eq__` during a membership
    /// search (a `PyInstance`, or a container that may transitively hold one).
    /// The per-value half of `seq_search_needs_dispatch`.
    fn value_search_dispatches(v: &Value) -> bool {
        matches!(
            v.kind(),
            ValueKind::PyInstance(_)
                | ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
                | ValueKind::BuiltinObject { .. }
        )
    }

    /// **The single source of truth for CPython's index protocol** (`operator.index`
    /// / `PyNumber_Index`).  Resolve `val` to an integer `Value` (guaranteed
    /// `Int`, `Bool`, or `BigInt`), honoring `__index__` uniformly:
    ///
    /// - `Int` / `Bool` / `BigInt`: returned unchanged (the common, branch-cheap
    ///   path — checked first so plain-int indexing has no extra work).
    /// - `PyInstance` that is an `int`/`bool` subclass (#1929): its primitive
    ///   backing is returned directly (the object already *is* an int, so the
    ///   backing wins even over a user `__index__` override, matching CPython).
    /// - `PyInstance` with a user `__index__`: the method is called; its result
    ///   must be `Int`/`Bool`/`BigInt`, else `TypeError: __index__ returned
    ///   non-int (type X)`.
    /// - Anything else (incl. `float`, `__int__`-only objects, instances without
    ///   `__index__`): the caller-supplied `not_index_err` closure produces the
    ///   context-specific `TypeError` (CPython varies the message per context:
    ///   `"'X' object cannot be interpreted as an integer"`, `"list indices must
    ///   be integers or slices, not X"`, etc.).
    ///
    /// Replaced ~40 open-coded coercions (issue #2022); the thin wrappers below
    /// (`call_index_protocol`, `coerce_range_arg_big`, `resolve_index_arg`,
    /// `try_resolve_index_value`, `try_index_for_seq_repeat`) all route here.
    pub(crate) fn value_to_index(
        &mut self,
        val: &Value,
        not_index_err: impl FnOnce(&Value) -> PyError,
    ) -> Result<Value> {
        // Fast path: a primitive integer is already its own index.  Checked
        // before any class/dunder probe so plain `a[i]` stays branch-cheap.
        match val.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => return Ok(val.clone()),
            ValueKind::PyInstance(_) => {}
            _ => return Err(not_index_err(val)),
        }
        // Issue #1929: an int/bool subclass *is* the int it backs, so the
        // backing value is used directly (it wins over a user `__index__`
        // override, matching CPython's C-level int reuse).
        if let Some(backing) = coerce_subclass_backing(val, &[])
            && matches!(
                backing.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ) {
                return Ok(backing);
            }
        let inst_rc = match val.kind() {
            ValueKind::PyInstance(inst) => Rc::clone(inst),
            _ => unreachable!("value_to_index: kind() changed under us"),
        };
        let class = Rc::clone(&inst_rc.borrow().class);
        let Some(method_val) = lookup_class_attr(&class, "__index__") else {
            return Err(not_index_err(val));
        };
        let result = invoke_class_method(self, method_val, val.clone(), &[])?;
        if matches!(
            result.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        ) {
            Ok(result)
        } else {
            Err(pyrust_core::type_err!("__index__ returned non-int (type {})",
                    value_type_name_str(&result),))
        }
    }

    /// Resolve an index argument for **sequence item access** (`a[i]`, `a[i] = v`,
    /// `del a[i]`) through the `__index__` protocol, matching CPython 3.12.
    ///
    /// Thin wrapper over [`Interpreter::value_to_index`] that supplies the
    /// per-type error message via [`seq_index_type_error`]:
    /// - list/tuple/bytes: `"X indices must be integers or slices, not Y"`
    /// - string: `"string indices must be integers, not 'Y'"` (different!)
    pub(crate) fn call_index_protocol(&mut self, val: &Value, label: &str) -> Result<Value> {
        self.value_to_index(val, |v| {
            pyrust_core::type_err!(seq_index_type_error(label, &value_type_name_str(v)))
        })
    }

    /// Resolve `val` to an `i64` (a Py_ssize_t-sized count/index) through the
    /// shared index protocol ([`Interpreter::value_to_index`]).  A non-integer
    /// raises `'X' object cannot be interpreted as an integer`; a `BigInt` that
    /// doesn't fit raises `OverflowError(overflow_msg)`.  Used by counted APIs
    /// (`range`, `itertools.repeat`, …) that need a concrete `i64` (#2022).
    pub(crate) fn value_to_isize(&mut self, val: &Value, overflow_msg: &str) -> Result<i64> {
        let resolved = self.value_to_index(val, |v| {
            pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(v))
        })?;
        index_value_to_i64(&resolved, overflow_msg)
    }

    /// Resolve the integer `length` argument of `int.to_bytes` through the
    /// `__index__` protocol when it is a `PyInstance` (issue #1929: an int
    /// subclass, e.g. `(255).to_bytes(I(2), "big")`, or a custom `__index__`
    /// object).  Other args (`byteorder` str, `signed` bool) and the
    /// no-instance fast path are left untouched so the receiver-side
    /// `pyrust_builtins::int::to_bytes` validation is unchanged.
    fn resolve_to_bytes_length(
        &mut self,
        method: &str,
        pos: &mut [Value],
        kw: &mut PyDict,
    ) -> Result<()> {
        if method != "to_bytes" {
            return Ok(());
        }
        if let Some(first) = pos.first().cloned()
            && let Some(resolved) = self.try_resolve_index_value(&first)? {
                pos[0] = resolved;
            }
        // Keyword `length=` form.
        let length_key = PyKey::Str(Value::string("length"));
        if let Some(v) = kw.get(&length_key).cloned()
            && let Some(resolved) = self.try_resolve_index_value(&v)? {
                kw.insert(length_key, resolved);
            }
        Ok(())
    }

    /// If `v` is a `PyInstance` resolvable as an integer — either an int/bool
    /// subclass (use the backing int, which wins over any `__index__` since the
    /// object already *is* an int) or an object defining `__index__` (call it)
    /// — return the resolved integer.  Returns `Ok(None)` for any non-instance
    /// value and for instances that are not integer-like, so the caller leaves
    /// the original value in place and the receiver-side validation raises the
    /// canonical TypeError.
    ///
    /// Routes the actual `__index__` dispatch through
    /// [`Interpreter::value_to_index`]; the `NotIndex` sentinel distinguishes
    /// "this instance isn't integer-like" (→ `Ok(None)`) from a real error
    /// raised inside `__index__`.
    fn try_resolve_index_value(&mut self, v: &Value) -> Result<Option<Value>> {
        if !matches!(v.kind(), ValueKind::PyInstance(_)) {
            return Ok(None);
        }
        match self.value_to_index(v, |_| PyError::named("__pyrust_NotIndex__", String::new())) {
            Ok(resolved) => Ok(Some(resolved)),
            Err(PyError::Named(name, _)) if name == "__pyrust_NotIndex__" => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Apply a Python format spec string to a `Value` and return the formatted string.
///
/// Implements the [Python format-spec mini-language][docs]:
///
/// ```text
/// [[fill]align][sign][#][0][width][grouping][.precision][type]
/// ```
///
/// Supported components for the built-in numeric / string types:
/// - **fill / align** (`<`, `>`, `^`, `=`) with any single fill character
/// - **sign** (`+`, `-`, ` `)
/// - **alternate form** `#` for `b`, `o`, `x`, `X` (and float types)
/// - **zero-pad** `0` (implies sign-aware `=` alignment)
/// - **width** (decimal integer)
/// - **grouping** `,` (comma) or `_` (underscore)
/// - **precision** `.N` for floats and strings
/// - **type** `b`, `c`, `d`, `e`, `E`, `f`, `F`, `g`, `G`, `n`, `o`, `s`, `x`, `X`, `%`
///
/// Complex values support the bare width / fill / align spec (matching
/// CPython's `format(1+2j, ">10")` -> `"    (1+2j)"`) but do not yet accept
/// numeric type codes (`e`/`f`/`g`) or sign / precision / grouping / `#` /
/// `0` — those will raise ValueError.
///
/// Not yet implemented: locale-aware grouping (`n` and float `n` types),
/// Complex with explicit numeric type codes, and non-ASCII fill characters
/// in nested f-string specs round-trip through `str.format` as bytes rather
/// than chars.  These gaps mirror documented pyrust limitations.
///
/// [docs]: https://docs.python.org/3/library/string.html#format-specification-mini-language
pub(crate) fn apply_format_spec(value: &Value, spec: &str) -> Result<Value> {
    apply_format_spec_named(value, spec, None)
}

/// Like [`apply_format_spec`], but lets the caller name the type reported in the
/// "unsupported format string passed to <type>.__format__" `TypeError`.  When
/// `owner` is `Some`, that name is used verbatim (the actual subclass, e.g. `B`
/// for `class B(bytes)`); when `None`, the value's own builtin type name is
/// used.  CPython names the *actual* type the spec was passed to, not the
/// backing primitive a subclass wraps (issue #2212).
pub(crate) fn apply_format_spec_named(
    value: &Value,
    spec: &str,
    owner: Option<&str>,
) -> Result<Value> {
    if spec.is_empty() {
        // gh-95778: an empty spec renders an int in base 10 — enforce the
        // int_max_str_digits limit.  The `value_may_exceed_int_str_limit` tag
        // test keeps the hot `f"{int}"` / `format(int)` path free of the error
        // branch entirely; only BigInt/containers enter the checked path.
        if pyrust_core::value_may_exceed_int_str_limit(value) {
            pyrust_core::check_int_str_conversion(value)?;
        }
        return Ok(Value::string(value.to_py_str()));
    }

    // Types that inherit the default `object.__format__` (None, list, tuple,
    // dict, set, bytes, function, module, …) reject any non-empty format spec
    // with a TypeError, mirroring CPython 3.12.  Only the value kinds that
    // provide a real `__format__` (str / int / bool / float / complex) accept
    // a spec; everything else is rejected here.
    if !value_has_real_format(value) {
        let type_name = owner
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|| pyrust_core::builtin_type_name(value));
        return Err(pyrust_core::type_err!(
            "unsupported format string passed to {}.__format__",
            type_name
        ));
    }

    let parsed = parse_format_spec(spec)?;
    let formatted = render_format_spec(value, &parsed)?;
    Ok(Value::string(formatted))
}

/// True when `value`'s type provides a real `__format__` that honours a format
/// spec (`str`, `int`/`bool`/`BigInt`, `float`, `complex`).  Every other type
/// inherits the default `object.__format__`, which rejects non-empty specs.
fn value_has_real_format(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::Str(_)
            | ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Bool(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _)
    )
}

/// Validate the positional args of a builtin `obj.__format__(spec)` call and
/// return the borrowed spec string.  Mirrors CPython 3.12's error wording:
/// a non-`str` spec → `__format__() argument must be str, not <type>`; more
/// than one argument → `<owner>.__format__() takes exactly one argument (N
/// given)`, where `<owner>` is the receiver's own type when it defines a real
/// `__format__` (int/float/str/bool/complex) and `object` otherwise (the
/// inherited `object.__format__`).  Shared by the two `__format__` method-call
/// dispatch sites (bound-method wrapper + tagged-container opcode), #2191.
fn format_dunder_spec_arg<'a>(receiver: &Value, pos: &'a [Value]) -> Result<&'a str> {
    if pos.len() > 1 {
        return Err(pyrust_core::type_err!(
            "{}.__format__() takes exactly one argument ({} given)",
            format_dunder_owner(receiver),
            pos.len()
        ));
    }
    match pos.first() {
        None => Ok(""),
        Some(v) => v.as_str().ok_or_else(|| {
            pyrust_core::type_err!(
                "__format__() argument must be str, not {}",
                pyrust_core::builtin_type_name(v)
            )
        }),
    }
}

/// The owner type name CPython 3.12 names in a builtin `__format__` arg error:
/// the receiver's own type when it defines a real `__format__`
/// (int/float/str/bool/complex), `object` otherwise (the inherited
/// `object.__format__`).  Shared by the spec-arg and keyword-arg validators so
/// both `__format__` method-call dispatch sites use the same wording.
fn format_dunder_owner(receiver: &Value) -> std::borrow::Cow<'static, str> {
    // `bool` inherits `int.__format__`; CPython names the type that *defines*
    // `__format__` in the MRO, so `True.__format__(...)` errors name `int`.
    if matches!(receiver.kind(), ValueKind::Bool(_)) {
        return std::borrow::Cow::Borrowed("int");
    }
    // A built-in subclass instance (`class I(int)`) without a `__format__`
    // override resolves the method to the backing type's `__format__` in
    // CPython, so an arg error names that backing type (`int`), not `object`
    // (issue #2214).
    if let ValueKind::PyInstance(inst) = receiver.kind()
        && let Some(backing) = instance_builtin_data(inst)
    {
        return format_dunder_owner(&backing);
    }
    if value_has_real_format(receiver) {
        pyrust_core::builtin_type_name(receiver)
    } else {
        std::borrow::Cow::Borrowed("object")
    }
}

#[derive(Debug, Clone)]
struct FormatSpec {
    fill: char,
    align: Option<char>,
    /// True when the user explicitly supplied a fill character (the
    /// two-character `[fill]align` form).  When false, a subsequent `0`
    /// flag promotes the fill to `'0'`.
    fill_explicit: bool,
    sign: Option<char>,
    alt: bool,
    zero_pad: bool,
    width: usize,
    grouping: Option<char>,
    precision: Option<usize>,
    type_char: Option<char>,
}

fn parse_format_spec(spec: &str) -> Result<FormatSpec> {
    let chars: Vec<char> = spec.chars().collect();
    let len = chars.len();
    let mut pos = 0;

    // fill + align: the align character (one of <>=^) must be at index 1
    // when a fill is present.  A bare align character at index 0 means
    // fill defaults to space.  '{' and '}' are not legal fill characters
    // (they would terminate the replacement field) — guard explicitly.
    let (fill, align, fill_explicit) =
        if len >= 2 && matches!(chars[1], '<' | '>' | '=' | '^') && !matches!(chars[0], '{' | '}') {
            let f = chars[0];
            let a = chars[1];
            pos += 2;
            (f, Some(a), true)
        } else if len >= 1 && matches!(chars[0], '<' | '>' | '=' | '^') {
            let a = chars[0];
            pos += 1;
            (' ', Some(a), false)
        } else {
            (' ', None, false)
        };

    // sign
    let sign = if pos < len && matches!(chars[pos], '+' | '-' | ' ') {
        let s = chars[pos];
        pos += 1;
        Some(s)
    } else {
        None
    };

    // alternate form '#'
    let alt = if pos < len && chars[pos] == '#' {
        pos += 1;
        true
    } else {
        false
    };

    // zero-padding '0' — always consumed when present at this position.
    // Semantics depend on whether align/fill were explicit (see render).
    let zero_pad = if pos < len && chars[pos] == '0' {
        pos += 1;
        true
    } else {
        false
    };

    // width
    let width_start = pos;
    while pos < len && chars[pos].is_ascii_digit() {
        pos += 1;
    }
    let width: usize = if pos > width_start {
        let raw: String = chars[width_start..pos].iter().collect();
        raw.parse::<usize>().map_err(|_| {
            pyrust_core::value_err!("Too many decimal digits in format string")
        })?
    } else {
        0
    };

    // grouping option (',' or '_') — sits between width and precision.
    let grouping = if pos < len && (chars[pos] == ',' || chars[pos] == '_') {
        let g = chars[pos];
        pos += 1;
        Some(g)
    } else {
        None
    };

    // precision
    let precision = if pos < len && chars[pos] == '.' {
        pos += 1;
        let prec_start = pos;
        while pos < len && chars[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos > prec_start {
            let raw: String = chars[prec_start..pos].iter().collect();
            Some(raw.parse::<usize>().map_err(|_| {
                pyrust_core::value_err!("Too many decimal digits in format string")
            })?)
        } else {
            // '.' with no digits is a syntax error in CPython.
            return Err(pyrust_core::value_err!("Format specifier missing precision"));
        }
    } else {
        None
    };

    // type char (must be the last character if present)
    let type_char = if pos < len {
        let t = chars[pos];
        pos += 1;
        Some(t)
    } else {
        None
    };

    if pos != len {
        return Err(pyrust_core::value_err!("Invalid format specifier"));
    }

    Ok(FormatSpec {
        fill,
        align,
        fill_explicit,
        sign,
        alt,
        zero_pad,
        width,
        grouping,
        precision,
        type_char,
    })
}

/// Apply the parsed spec to the value.  Splits into a string-typed branch
/// and a numeric branch so type-specific validation stays close to formatting.
fn render_format_spec(value: &Value, fs: &FormatSpec) -> Result<String> {
    // Treat the value as a string when the type code is 's' (or absent and
    // the value is a string).  For non-string values with no type code, fall
    // back to numeric handling so width / zero-pad / sign still apply.
    let is_string_target = matches!(fs.type_char, Some('s'))
        || (fs.type_char.is_none() && matches!(value.kind(), ValueKind::Str(_)));

    if is_string_target {
        return format_as_string(value, fs);
    }

    // No type code and a non-string value: route by value kind.
    if fs.type_char.is_none() {
        match value.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                return format_int_value(value, fs, None)
            }
            ValueKind::Float(_) => return format_float_value(value, fs, None),
            // Complex with no explicit type code: render via complex_repr
            // (matching CPython's `format(1+2j)` -> "(1+2j)") and then apply
            // width / align / fill to the resulting string.  The float
            // format codes are rejected for Complex in format_float_value,
            // so we must short-circuit here for the bare-spec case.
            ValueKind::Complex(_, _) => return format_complex_value(value, fs),
            _ => {
                // Anything else: fall back to str() then pad like a string.
                return format_as_string(value, fs);
            }
        }
    }

    let t = fs.type_char.unwrap();
    // Complex supports the float presentation types f/F/e/E/g/G plus 'n'
    // (locale-as-'g'), applying the spec to both components.  It does NOT
    // support '%' or any integer/string code.
    if matches!(value.kind(), ValueKind::Complex(_, _)) {
        if matches!(t, 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'n') {
            return format_complex_value(value, fs);
        }
        return Err(pyrust_core::value_err!("Unknown format code '{t}' for object of type 'complex'"));
    }
    match t {
        // 'n' is locale-aware and supported on both integer and float values.
        // Route it by the value's type: int/bool to the integer formatter,
        // float to the float formatter (which treats 'n' as 'g' since pyrust
        // has no locale, matching CPython's C-locale behavior where n == g).
        'n' if matches!(value.kind(), ValueKind::Float(_)) => {
            format_float_value(value, fs, Some('n'))
        }
        'd' | 'b' | 'o' | 'x' | 'X' | 'c' | 'n' => format_int_value(value, fs, Some(t)),
        'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%' => format_float_value(value, fs, Some(t)),
        's' => format_as_string(value, fs),
        _ => Err(pyrust_core::value_err!("Unknown format code '{t}' for object of type '{}'",
                value_type_name_str(value))),
    }
}

fn format_as_string(value: &Value, fs: &FormatSpec) -> Result<String> {
    // Reject numeric-only options on strings, matching CPython.
    if matches!(fs.type_char, Some('s')) && !matches!(value.kind(), ValueKind::Str(_)) {
        return Err(pyrust_core::value_err!("Unknown format code 's' for object of type '{}'",
                value_type_name_str(value)));
    }
    if fs.sign.is_some() {
        return Err(pyrust_core::value_err!("Sign not allowed in string format specifier"));
    }
    if fs.alt {
        return Err(pyrust_core::value_err!("Alternate form (#) not allowed in string format specifier"));
    }
    if fs.grouping.is_some() {
        return Err(pyrust_core::value_err!("Cannot specify ',' or '_' with 's'."));
    }
    if matches!(fs.align, Some('=')) {
        return Err(pyrust_core::value_err!("'=' alignment not allowed in string format specifier"));
    }

    let raw = match value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => value.to_py_str(),
    };
    let raw = match fs.precision {
        Some(p) => raw.chars().take(p).collect::<String>(),
        None => raw,
    };
    // CPython accepts the `0` zero-pad flag on strings (with a
    // DeprecationWarning) — it promotes the fill character to '0' but keeps
    // the default left-alignment.  We just match the run-time behavior.
    let effective_fill = if fs.zero_pad && !fs.fill_explicit {
        '0'
    } else {
        fs.fill
    };
    Ok(pad_value(&raw, fs, '<', effective_fill))
}

/// Produce the digit-only "body" of an integer formatting (no sign / prefix).
fn int_body(magnitude: u64, type_char: char) -> String {
    match type_char {
        'd' => format!("{magnitude}"),
        'b' => format!("{magnitude:b}"),
        'o' => format!("{magnitude:o}"),
        'x' => format!("{magnitude:x}"),
        'X' => format!("{magnitude:X}"),
        _ => format!("{magnitude}"),
    }
}

/// Produce the digit-only body of a BigInt formatting (no sign / prefix).
fn bigint_body(magnitude: &PyBigInt, type_char: char) -> String {
    let radix = match type_char {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        _ => 10,
    };
    // magnitude is always non-negative here (callers strip the sign).
    let s = magnitude.to_str_radix(radix);
    if type_char == 'X' {
        s.to_uppercase()
    } else {
        s
    }
}

fn prefix_for(type_char: char, alt: bool) -> &'static str {
    if !alt {
        return "";
    }
    match type_char {
        'b' => "0b",
        'o' => "0o",
        'x' => "0x",
        'X' => "0X",
        _ => "",
    }
}

fn format_int_value(value: &Value, fs: &FormatSpec, type_char: Option<char>) -> Result<String> {
    // BigInt is handled via a separate path that avoids the i128 narrowing.
    if let ValueKind::BigInt(b) = value.kind() {
        return format_bigint_value(b, fs, type_char);
    }

    let n: i128 = match value.kind() {
        ValueKind::Int(n) => n as i128,
        ValueKind::Bool(b) => {
            if b {
                1
            } else {
                0
            }
        }
        _ => {
            let code = type_char.unwrap_or('d');
            return Err(pyrust_core::value_err!("Unknown format code '{code}' for object of type '{}'",
                    value_type_name_str(value)));
        }
    };

    if fs.precision.is_some() {
        return Err(pyrust_core::value_err!("Precision not allowed in integer format specifier"));
    }

    let t = type_char.unwrap_or('d');

    // 'c': render as the unicode character.
    if t == 'c' {
        if fs.sign.is_some() || fs.alt || fs.grouping.is_some() {
            return Err(pyrust_core::value_err!("Cannot specify ',' or '_', sign, or '#' with 'c'."));
        }
        if !(0..=0x10FFFF).contains(&n) {
            return Err(pyrust_core::overflow_err!("%c arg not in range(0x110000)"));
        }
        let ch = char::from_u32(n as u32).ok_or_else(|| {
            pyrust_core::overflow_err!("%c arg not in range(0x110000)")
        })?;
        let raw = ch.to_string();
        return Ok(pad_value(&raw, fs, '<', fs.fill));
    }

    // 'n' already implies locale-aware grouping, so CPython rejects an
    // explicit ',' / '_' combined with it (reported against the original 'n'
    // type, not the effective 'd' it maps to).
    if let Some(g) = fs.grouping
        && t == 'n' {
            return Err(pyrust_core::value_err!("Cannot specify '{g}' with 'n'."));
        }

    // 'n' = same as 'd' for now (no locale-aware grouping).
    let effective_t = if t == 'n' { 'd' } else { t };

    // Validate grouping vs type.
    if let Some(g) = fs.grouping {
        let ok = match (g, effective_t) {
            (',', 'd') => true,
            (',', _) => false,
            ('_', 'd' | 'b' | 'o' | 'x' | 'X') => true,
            _ => false,
        };
        if !ok {
            return Err(pyrust_core::value_err!("Cannot specify '{g}' with '{effective_t}'."));
        }
    }

    let negative = n < 0;
    let magnitude: u64 = if negative {
        // i64::MIN edge case: -(i128) fits in u64 via wrap.
        (-n) as u64
    } else {
        n as u64
    };

    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let alt_prefix = prefix_for(effective_t, fs.alt);
    let mut body = int_body(magnitude, effective_t);

    // Apply grouping to the digit body.  For non-decimal bases (b/o/x/X),
    // CPython groups every 4 digits with '_'.  For decimal, every 3 digits
    // with either ',' or '_'.
    let group_size = if effective_t == 'd' { 3 } else { 4 };
    if let Some(g) = fs.grouping {
        body = group_digits(&body, g, group_size);
    }

    // Apply zero-pad / width / alignment.  Pass `group_size` so that
    // zero-pad + grouping with non-decimal bases (e.g. `{:0_12x}`) re-groups
    // the zero-padded body every 4 digits rather than every 3.
    Ok(assemble_numeric(
        sign_prefix,
        alt_prefix,
        body,
        fs,
        // Numeric default alignment is right.
        '>',
        group_size,
    ))
}

/// Format a `BigInt` value according to an integer `FormatSpec`.
/// Mirrors `format_int_value` but uses `bigint_body` instead of the
/// `u64`-based `int_body` to avoid narrowing large values.
fn format_bigint_value(b: &PyBigInt, fs: &FormatSpec, type_char: Option<char>) -> Result<String> {
    if fs.precision.is_some() {
        return Err(pyrust_core::value_err!("Precision not allowed in integer format specifier"));
    }

    let t = type_char.unwrap_or('d');

    // 'c': a BigInt is almost certainly out of range, but check correctly.
    if t == 'c' {
        if fs.sign.is_some() || fs.alt || fs.grouping.is_some() {
            return Err(pyrust_core::value_err!("Cannot specify ',' or '_', sign, or '#' with 'c'."));
        }
        // A BigInt is by definition outside the C long range (> i64::MAX or
        // < i64::MIN), so it can never be a valid chr() argument.  CPython
        // raises "Python int too large to convert to C long" for such values
        // rather than the "%c arg not in range(0x110000)" it uses for
        // in-range negative integers.
        return Err(pyrust_core::overflow_err!("Python int too large to convert to C long"));
    }

    // 'n' already implies locale-aware grouping, so CPython rejects an
    // explicit ',' / '_' combined with it (reported against the original 'n'
    // type, not the effective 'd' it maps to).
    if let Some(g) = fs.grouping
        && t == 'n' {
            return Err(pyrust_core::value_err!("Cannot specify '{g}' with 'n'."));
        }

    // 'n' = same as 'd' for now (no locale-aware grouping).
    let effective_t = if t == 'n' { 'd' } else { t };

    // gh-95778: base-10 ('d'/'n') rendering of a BigInt is subject to the
    // int_max_str_digits limit; the power-of-two bases ('b'/'o'/'x'/'X') below
    // are exempt.
    if effective_t == 'd' && pyrust_core::bigint_str_digits_exceed_limit(b) {
        return Err(pyrust_core::int_max_str_digits_format_error());
    }

    // Validate grouping vs type.
    if let Some(g) = fs.grouping {
        let ok = match (g, effective_t) {
            (',', 'd') => true,
            (',', _) => false,
            ('_', 'd' | 'b' | 'o' | 'x' | 'X') => true,
            _ => false,
        };
        if !ok {
            return Err(pyrust_core::value_err!("Cannot specify '{g}' with '{effective_t}'."));
        }
    }

    use num_bigint::Sign;
    let negative = b.sign() == Sign::Minus;
    // magnitude: absolute value used for digit conversion.
    let magnitude = if negative { -b.clone() } else { b.clone() };

    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let alt_prefix = prefix_for(effective_t, fs.alt);
    let mut body = bigint_body(&magnitude, effective_t);

    let group_size = if effective_t == 'd' { 3 } else { 4 };
    if let Some(g) = fs.grouping {
        body = group_digits(&body, g, group_size);
    }

    Ok(assemble_numeric(
        sign_prefix,
        alt_prefix,
        body,
        fs,
        '>',
        group_size,
    ))
}

fn format_float_value(value: &Value, fs: &FormatSpec, type_char: Option<char>) -> Result<String> {
    // Complex numbers don't yet support the explicit float / int type codes
    // here.  The bare-spec (no type char) path routes Complex through
    // `format_complex_value` before reaching this function, so a Complex
    // value here means the user supplied an unsupported type code.
    if matches!(value.kind(), ValueKind::Complex(_, _)) {
        let code = type_char.unwrap_or('\0');
        return Err(pyrust_core::value_err!("Unknown format code '{code}' for object of type 'complex'"));
    }

    // str.__format__ rejects float format codes with ValueError (matching
    // CPython's "Unknown format code 'f' for object of type 'str'").  The
    // generic `fmt_value_to_float` would raise TypeError instead, so we
    // intercept str values here before the conversion attempt.
    if matches!(value.kind(), ValueKind::Str(_)) {
        let code = type_char.unwrap_or('\0');
        return Err(pyrust_core::value_err!("Unknown format code '{code}' for object of type '{}'",
                value_type_name_str(value)));
    }

    let f = fmt_value_to_float(value)?;
    let t = type_char.unwrap_or('\0'); // '\0' = no type, use shortest repr-ish

    let negative = f.is_sign_negative() && !f.is_nan();
    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let abs_f = f.abs();

    // Special values: inf / nan ignore precision / alt / grouping.
    if f.is_nan() {
        let mut body = if matches!(t, 'F' | 'G' | 'E') {
            "NAN".to_string()
        } else {
            "nan".to_string()
        };
        // The '%' presentation type still appends the percent sign for non-finite
        // values: format(nan, '%') -> 'nan%' (#2027).
        if t == '%' {
            body.push('%');
        }
        // nan has no sign, but the explicit sign flag ('+' / ' ') still applies
        // (CPython: format(nan, '+') -> '+nan').
        return Ok(assemble_numeric(sign_prefix, "", body, fs, '>', 3));
    }
    if f.is_infinite() {
        let mut body = if matches!(t, 'F' | 'G' | 'E') {
            "INF".to_string()
        } else {
            "inf".to_string()
        };
        // '%' still appends the percent sign: format(inf, '%') -> 'inf%' (#2027).
        if t == '%' {
            body.push('%');
        }
        return Ok(assemble_numeric(sign_prefix, "", body, fs, '>', 3));
    }

    // Validate grouping vs type.  Comma and '_' are allowed on all float
    // types except 'n', which already implies locale-aware grouping and so
    // CPython rejects an explicit ',' / '_' combined with it.
    if let Some(g) = fs.grouping
        && t == 'n' {
            return Err(pyrust_core::value_err!("Cannot specify '{g}' with 'n'."));
        }

    let (mut body, alt_prefix) = match t {
        'f' | 'F' => {
            let prec = fs.precision.unwrap_or(6);
            let s = format!("{:.prec$}", abs_f);
            let s = if t == 'F' { s.to_uppercase() } else { s };
            (ensure_alt_float(s, fs.alt, fs.precision), "")
        }
        'e' | 'E' => {
            let prec = fs.precision.unwrap_or(6);
            let s = if t == 'E' {
                format!("{:.prec$E}", abs_f)
            } else {
                format!("{:.prec$e}", abs_f)
            };
            let s = normalise_exp_digits(s);
            (ensure_alt_float(s, fs.alt, fs.precision), "")
        }
        // 'g'/'G' general format and 'n' (locale-aware general format). In
        // pyrust's locale-free C-locale behavior, 'n' is identical to 'g':
        // same default precision, same trailing-zero stripping, same exponent
        // threshold, and lowercase output (no uppercase 'N' variant exists).
        'g' | 'G' | 'n' => {
            let upper = t == 'G';
            let prec = fs.precision.unwrap_or(6);
            let prec = if prec == 0 { 1 } else { prec };
            // `format_g` keeps trailing zeros and the decimal point when the
            // alternate '#' form is requested (#1950).
            let s = format_g(abs_f, prec, upper, fs.alt);
            (s, "")
        }
        '%' => {
            let prec = fs.precision.unwrap_or(6);
            let s = format!("{:.prec$}", abs_f * 100.0);
            let s = ensure_alt_float(s, fs.alt, fs.precision);
            (format!("{s}%"), "")
        }
        _ => {
            // No explicit type char.  When precision is given, CPython's
            // no-type-char float format differs from 'g' in two ways:
            //
            //  1. Exponential threshold: use `exp >= max(prec - 1, 0)` (one
            //     step earlier than 'g' which uses `exp >= prec`).
            //  2. Fixed notation must preserve at least one decimal digit
            //     (e.g. `10.0` not `10`).
            //
            // Without precision, use a shortest-roundtrip-ish repr with at
            // least one digit after the decimal point.
            let s = if let Some(prec) = fs.precision {
                format_no_type_with_prec(abs_f, prec)
            } else {
                match value.kind() {
                    ValueKind::Float(_) => {
                        let raw = Value::float(abs_f).to_py_str();
                        if !raw.contains('.') && !raw.contains('e') && !raw.contains('n') {
                            format!("{raw}.0")
                        } else {
                            raw
                        }
                    }
                    ValueKind::Int(n) => {
                        let n = if n < 0 { -n } else { n };
                        format!("{n}.0")
                    }
                    ValueKind::Bool(b) => if b { "1.0" } else { "0.0" }.to_string(),
                    _ => format!("{abs_f}"),
                }
            };
            (s, "")
        }
    };

    // Apply grouping on the integer part of the float body.
    if let Some(g) = fs.grouping
        && (g == ',' || g == '_') {
            body = group_float_int_part(&body, g);
        }

    Ok(assemble_numeric(sign_prefix, alt_prefix, body, fs, '>', 3))
}

/// Format a Complex value.
///
/// CPython applies the float format mini-language to both the real and the
/// imaginary part, joining them as `<re><signed-im>j`.  The imaginary part
/// always carries an explicit sign; the real part follows the spec's sign
/// flag.  Width / fill / alignment then apply to the assembled string.
///
/// When no presentation type code is given, the components use the repr-style
/// float format (with the spec's precision, if any) and the result is wrapped
/// in parentheses unless the real part is positive zero — matching CPython's
/// `format(1+2j)` -> `"(1+2j)"` and `format(2j)` -> `"2j"`.  A presentation
/// type code (f/F/e/E/g/G/n) suppresses the parentheses.
///
/// Zero-padding and `=` alignment are always rejected for complex (the `j`
/// suffix / optional parens make interior padding ill-defined).
fn format_complex_value(value: &Value, fs: &FormatSpec) -> Result<String> {
    if fs.zero_pad && !fs.fill_explicit {
        return Err(pyrust_core::value_err!("Zero padding is not allowed in complex format specifier"));
    }
    if matches!(fs.align, Some('=')) {
        return Err(pyrust_core::value_err!("'=' alignment flag is not allowed in complex format specifier"));
    }

    let (re, im) = match value.kind() {
        ValueKind::Complex(re, im) => (re, im),
        // Non-complex values never reach this routine.
        _ => unreachable!("format_complex_value called on non-complex value"),
    };

    // The complex 'n' type maps to 'g' for the components (no locale grouping
    // here).  With no explicit type CPython uses a repr-style component: when
    // a precision is supplied it behaves like 'g' with that precision;
    // otherwise it is the shortest round-trip repr with integer-valued floats
    // rendered without the trailing `.0` (e.g. `3` not `3.0`).
    let no_type = fs.type_char.is_none();
    let component_type = match fs.type_char {
        Some('n') => Some('g'),
        None if fs.precision.is_some() => Some('g'),
        other => other,
    };

    // Per-component sub-spec: keep sign / alt / precision / grouping / type,
    // but strip width / zero-pad / fill / align (those apply to the whole
    // assembled string, not the individual components).
    let make_component = |part: f64, sign: Option<char>| -> Result<String> {
        let sub = FormatSpec {
            fill: ' ',
            align: None,
            fill_explicit: false,
            sign,
            alt: fs.alt,
            zero_pad: false,
            width: 0,
            grouping: fs.grouping,
            precision: fs.precision,
            type_char: component_type,
        };
        let mut s = format_float_value(&Value::float(part), &sub, component_type)?;
        // Repr-style (no type, no precision): drop the trailing `.0` that the
        // float formatter emits for integer-valued floats.  With the alternate
        // form the decimal point is retained (`3.` not `3.0`); CPython keeps
        // the point but not the zero.
        if no_type && component_type.is_none()
            && let Some(stripped) = s.strip_suffix(".0") {
                s = if fs.alt {
                    format!("{stripped}.")
                } else {
                    stripped.to_string()
                };
            }
        Ok(s)
    };

    // The imaginary part always carries an explicit sign separator.  The float
    // formatter drops the forced `+` for nan / inf (its special-value branch
    // ignores the sign flag), so re-assert it here to match CPython's
    // `inf+nanj` form.
    let imag_component = |part: f64| -> Result<String> {
        let s = make_component(part, Some('+'))?;
        if s.starts_with('+') || s.starts_with('-') {
            Ok(s)
        } else {
            Ok(format!("+{s}"))
        }
    };

    let body = if no_type {
        // No presentation type: repr-style components, parenthesised unless the
        // real part is positive zero (then only the imaginary part is shown).
        if re == 0.0 && (1.0_f64).copysign(re) > 0.0 {
            // Pure-imaginary form: the imaginary part follows the spec's sign
            // flag (no forced `+` separator, since no real part precedes it).
            let im_str = make_component(im, fs.sign)?;
            format!("{im_str}j")
        } else {
            let re_str = make_component(re, fs.sign)?;
            let im_str = imag_component(im)?;
            format!("({re_str}{im_str}j)")
        }
    } else {
        // Presentation type given: format both parts, no parentheses.
        let re_str = make_component(re, fs.sign)?;
        let im_str = imag_component(im)?;
        format!("{re_str}{im_str}j")
    };

    // CPython right-aligns complex on width (numeric default).
    Ok(pad_value(&body, fs, '>', fs.fill))
}

fn sign_prefix_for(negative: bool, sign: Option<char>) -> &'static str {
    if negative {
        return "-";
    }
    match sign {
        Some('+') => "+",
        Some(' ') => " ",
        _ => "",
    }
}

/// Insert grouping characters into a digit string (right-to-left).
fn group_digits(digits: &str, sep: char, group_size: usize) -> String {
    let bytes: Vec<char> = digits.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(bytes.len() + bytes.len() / group_size);
    for (i, c) in bytes.iter().rev().enumerate() {
        if i > 0 && i % group_size == 0 {
            out.push(sep);
        }
        out.push(*c);
    }
    out.iter().rev().collect()
}

/// Apply decimal grouping to the integer part of a float body (e.g. "1234.50"
/// → "1,234.50").  Leaves any exponent / suffix portion intact.
fn group_float_int_part(body: &str, sep: char) -> String {
    // Find the integer portion: up to the first '.' or 'e' / 'E' or '%'.
    let mut end = body.len();
    for (i, c) in body.char_indices() {
        if matches!(c, '.' | 'e' | 'E' | '%') {
            end = i;
            break;
        }
    }
    let (int_part, rest) = body.split_at(end);
    let grouped = group_digits(int_part, sep, 3);
    format!("{grouped}{rest}")
}

/// Assemble the final string with sign / alt-prefix / body and apply width
/// + alignment + zero-pad rules.
///
/// `group_size` controls how zero-pad interleaves with the grouping
/// separator: `3` for decimal / float grouping, `4` for `_` grouping with
/// non-decimal integer bases (`b`/`o`/`x`/`X`), matching CPython.
fn assemble_numeric(
    sign_prefix: &str,
    alt_prefix: &str,
    body: String,
    fs: &FormatSpec,
    default_align: char,
    group_size: usize,
) -> String {
    let raw_len = sign_prefix.chars().count() + alt_prefix.chars().count() + body.chars().count();

    // Determine effective alignment.  If zero-pad is set and no explicit
    // align was given, alignment becomes '=' (pad between sign/prefix and
    // digits).
    let effective_align = if let Some(a) = fs.align {
        a
    } else if fs.zero_pad {
        '='
    } else {
        default_align
    };
    // Zero-pad promotes fill to '0' unless the user explicitly supplied a
    // fill character via the two-character `[fill]align` form.
    let effective_fill = if fs.zero_pad && !fs.fill_explicit {
        '0'
    } else {
        fs.fill
    };

    if fs.width == 0 || raw_len >= fs.width {
        return format!("{sign_prefix}{alt_prefix}{body}");
    }
    let pad = fs.width - raw_len;
    let fill_str: String = std::iter::repeat_n(effective_fill, pad).collect();

    match effective_align {
        '=' => {
            // sign + prefix + fill + body
            let body_grouped = if let (true, Some(grouping)) = (effective_fill == '0', fs.grouping) {
                // CPython interleaves the grouping separator with the zero
                // pad characters so the resulting body still groups in
                // threes-or-fours.  Apply by left-padding the body with
                // zeros first, then re-grouping the integer portion.
                regroup_with_zero_pad(&body, pad, grouping, group_size, alt_prefix)
            } else {
                let mut s = String::with_capacity(pad + body.len());
                s.push_str(&fill_str);
                s.push_str(&body);
                s
            };
            format!("{sign_prefix}{alt_prefix}{body_grouped}")
        }
        '>' => format!("{fill_str}{sign_prefix}{alt_prefix}{body}"),
        '<' => format!("{sign_prefix}{alt_prefix}{body}{fill_str}"),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let left_fill: String = std::iter::repeat_n(effective_fill, left).collect();
            let right_fill: String = std::iter::repeat_n(effective_fill, right).collect();
            format!("{left_fill}{sign_prefix}{alt_prefix}{body}{right_fill}")
        }
        _ => format!("{sign_prefix}{alt_prefix}{body}"),
    }
}

/// When zero-padding combines with thousands grouping (e.g. `{-12345:08,}` ->
/// `-012,345`), CPython expands the integer portion with zeros then regroups
/// so the leading zeros are themselves separated by the group char.  The
/// final grouped string must be at least `current_int_len + pad` characters
/// long.
///
/// `group_size` is `3` for decimal grouping (`,` or `_` with `d`/`n`/no type
/// or with floats), and `4` for `_` grouping with non-decimal integer bases
/// (`b`/`o`/`x`/`X`), matching CPython's rules.
fn regroup_with_zero_pad(
    body: &str,
    pad: usize,
    sep: char,
    group_size: usize,
    _alt_prefix: &str,
) -> String {
    // Split body into integer / fractional / suffix parts.  For non-decimal
    // integer bases (group_size == 4) the body is hex/oct/bin digits only —
    // including `e`, which is a legitimate hex digit — so skip the split.
    // For decimal / float (group_size == 3) the body may contain `.`, `e`,
    // `E`, or `%` which mark the end of the integer portion.
    let (int_part, rest) = if group_size == 3 {
        match body.find(['.', 'e', 'E', '%']) {
            Some(i) => (&body[..i], &body[i..]),
            None => (body, ""),
        }
    } else {
        (body, "")
    };

    // Strip existing separators from the integer part.
    let bare_int: String = int_part.chars().filter(|c| *c != sep).collect();
    let target_int_len = int_part.chars().count() + pad;

    // Iteratively prepend zeros and regroup until length matches target.
    // Bounded by `target_int_len` iterations.
    let mut digits = bare_int;
    for _ in 0..=target_int_len {
        let grouped = group_digits(&digits, sep, group_size);
        if grouped.chars().count() >= target_int_len {
            return format!("{grouped}{rest}");
        }
        digits.insert(0, '0');
    }
    // Safety fallback (unreachable in practice).
    let grouped = group_digits(&digits, sep, group_size);
    format!("{grouped}{rest}")
}

/// When the alternate form '#' is given to f/e/E/%, force a decimal point in
/// the body even if precision was 0.
fn ensure_alt_float(s: String, alt: bool, precision: Option<usize>) -> String {
    if !alt {
        return s;
    }
    if precision == Some(0) && !s.contains('.') {
        // Insert '.' before exponent if present, else append.
        if let Some(e_pos) = s.find(['e', 'E']) {
            let (a, b) = s.split_at(e_pos);
            format!("{a}.{b}")
        } else {
            format!("{s}.")
        }
    } else {
        s
    }
}

/// Pad a string-typed value per the format spec.
fn pad_value(raw: &str, fs: &FormatSpec, default_align: char, fill: char) -> String {
    let raw_len = raw.chars().count();
    if fs.width == 0 || raw_len >= fs.width {
        return raw.to_string();
    }
    let pad = fs.width - raw_len;
    let align = fs.align.unwrap_or(default_align);
    let fill_str: String = std::iter::repeat_n(fill, pad).collect();
    match align {
        '>' => format!("{fill_str}{raw}"),
        '<' => format!("{raw}{fill_str}"),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let left_fill: String = std::iter::repeat_n(fill, left).collect();
            let right_fill: String = std::iter::repeat_n(fill, right).collect();
            format!("{left_fill}{raw}{right_fill}")
        }
        _ => format!("{raw}{fill_str}"),
    }
}

/// Normalise Rust's e-notation digits to Python's: always at least two
/// exponent digits and an explicit sign.
fn normalise_exp_digits(s: String) -> String {
    let e_pos = match s.find(['e', 'E']) {
        Some(p) => p,
        None => return s,
    };
    let (mantissa, exp_part) = s.split_at(e_pos);
    let e_char = &exp_part[..1];
    let exp_digits = &exp_part[1..];
    let (exp_sign, exp_num) = if exp_digits.starts_with('+') || exp_digits.starts_with('-') {
        (&exp_digits[..1], &exp_digits[1..])
    } else {
        ("+", exp_digits)
    };
    let exp_num_padded = if exp_num.len() < 2 {
        format!("0{exp_num}")
    } else {
        exp_num.to_string()
    };
    format!("{mantissa}{e_char}{exp_sign}{exp_num_padded}")
}

/// Coerce a `Value` to `f64` for format-spec (`format(x, ".2f")` / f-string)
/// numeric formatting.  Thin wrapper around [`try_value_to_float`] that
/// reports the format-path CPython-parity error message.
///
/// Raises `OverflowError` (not `TypeError`) when a `BigInt` argument overflows
/// f64 range, matching CPython's behaviour for `format(2**10000, ".2f")`.
fn fmt_value_to_float(value: &Value) -> Result<f64> {
    if let ValueKind::BigInt(b) = value.kind() {
        use crate::value::PyToPrimitive;
        let f = b.to_f64().unwrap_or(f64::INFINITY);
        return if f.is_finite() {
            Ok(f)
        } else {
            Err(pyrust_core::overflow_err!("int too large to convert to float"))
        };
    }
    try_value_to_float(value).ok_or_else(|| {
        pyrust_core::type_err!("must be real number, not {}", value_type_name_str(value))
    })
}

fn apply_sign_str(s: String, f: f64, sign: Option<char>) -> String {
    match sign {
        Some('+') if f >= 0.0 && !s.starts_with('-') => format!("+{s}"),
        Some(' ') if f >= 0.0 && !s.starts_with('-') => format!(" {s}"),
        _ => s,
    }
}

fn normalise_exp_str(s: String, f: f64, sign: Option<char>) -> String {
    // Rust: 1.23e5; Python: 1.23e+05 — adjust exponent format.
    let s = if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        let (mantissa, exp_part) = s.split_at(e_pos);
        let e_char = &exp_part[..1];
        let exp_digits = &exp_part[1..];
        // exp_digits starts with optional sign then digits
        let (exp_sign, exp_num) = if exp_digits.starts_with('+') || exp_digits.starts_with('-') {
            (&exp_digits[..1], &exp_digits[1..])
        } else {
            ("+", exp_digits)
        };
        // Python always uses at least 2 digits for the exponent (e.g. e+05 not e+5).
        let exp_num_padded = if exp_num.len() < 2 {
            format!("0{exp_num}")
        } else {
            exp_num.to_string()
        };
        format!("{mantissa}{e_char}{exp_sign}{exp_num_padded}")
    } else {
        s
    };
    apply_sign_str(s, f, sign)
}

/// Format a float with the no-type-char rule when a precision is given.
///
/// CPython's `{:.N}` (no explicit type) on a float differs from `{:.Ng}` in:
///
/// 1. Exponential threshold: switches to `e` notation when `exp >= max(N-1, 0)`
///    (one step earlier than `g`'s `exp >= N`).
/// 2. Fixed notation result must have at least one decimal digit (trailing `.0`
///    appended when the sig-fig trim would leave a bare integer like `10`).
///
/// `prec=0` is normalised to `prec=1` for the sig-fig computation but keeps
/// the `prec=0` threshold (i.e. `exp >= 0` triggers exponential).
///
/// Importantly, both threshold checks (`exp < -4` and `exp >= threshold`) use
/// the exponent of the *rounded* value, not the original.  For example,
/// `9.99` rounded to 1 sig fig is `10` (exp=1), so with prec=2 (threshold=1)
/// it triggers exponential notation even though the original exp was 0.
fn format_no_type_with_prec(f: f64, prec: usize) -> String {
    let sig_prec = if prec == 0 { 1 } else { prec };
    // Threshold: exp >= max(prec - 1, 0).  For prec=0 and prec=1 this is 0;
    // for prec >= 2 it is prec - 1.
    let threshold = if prec <= 1 { 0_i32 } else { (prec as i32) - 1 };

    if f == 0.0 {
        // Zero with prec <= 1 uses exponential: '0e+00'.
        // Zero with prec >= 2 uses fixed: '0.0'.
        return if threshold == 0 {
            "0e+00".to_string()
        } else {
            "0.0".to_string()
        };
    }

    // Format in exponential notation first to get the rounded exponent.
    // This correctly handles cases where rounding changes the order of
    // magnitude (e.g. 9.99 rounded to 1 sig fig => 10, exp becomes 1).
    let sig_digits = sig_prec.saturating_sub(1);
    let exp_str = format!("{:.sig_digits$e}", f);
    // Parse the exponent from Rust's exponential string (e.g. "1e1" -> 1).
    let rounded_exp = if let Some(e_pos) = exp_str.find('e') {
        exp_str[e_pos + 1..].parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    if rounded_exp < -(4_i32) || rounded_exp >= threshold {
        // Exponential notation: reuse the already-computed exp_str.
        trim_g_trailing_zeros(normalise_exp_str(exp_str, f, None))
    } else {
        // Fixed notation.  Compute decimal places for sig_prec sig figs
        // using the rounded exponent so the digit count is correct.
        let decimal_digits = if rounded_exp >= 0 {
            sig_prec.saturating_sub(rounded_exp as usize + 1)
        } else {
            sig_prec + (-rounded_exp - 1) as usize
        };
        let s = format!("{:.decimal_digits$}", f);
        let s = trim_g_trailing_zeros(s);
        // Ensure at least one digit after the decimal point.
        if s.contains('.') || s.contains('e') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

/// CPython's `%g` / `format(_, 'g')` algorithm, shared by `str.format`/`format()`
/// and `%`-printf (`expr.rs::format_general_float` delegates here).
///
/// Faithful port of CPython's general-format rounding rule:
///   1. Round the value to `prec` significant digits FIRST (via Rust's `{:.Ne}`,
///      which itself rounds).
///   2. Read the decimal exponent X of the *rounded* value — rounding can bump
///      the magnitude across a power of ten (e.g. `999999.5` -> `1e+06`), which
///      must change the fixed-vs-exponent decision (#2000).
///   3. Use fixed notation iff `-4 <= X < prec`, else exponential.
///   4. `alt` (the `#` form) keeps trailing zeros out to `prec` significant
///      figures and always keeps the decimal point; otherwise strip them (#1950).
///
/// `f` must be finite and non-NaN; callers handle inf/nan/sign separately.
fn format_g(f: f64, prec: usize, upper: bool, alt: bool) -> String {
    if f == 0.0 {
        // Zero's exponent is taken as 0, so it is always fixed notation.
        if alt {
            let mut out = String::from("0.");
            if prec > 1 {
                for _ in 0..prec - 1 {
                    out.push('0');
                }
            }
            return out;
        }
        return "0".to_string();
    }

    // Round to `prec` significant digits via exponential formatting, then read
    // the rounded exponent. This correctly handles rounding that crosses a
    // power of ten (#2000).
    let sig_digits = prec.saturating_sub(1);
    let exp_str = format!("{:.sig_digits$e}", f);
    let rounded_exp = if let Some(pos) = exp_str.find('e') {
        exp_str[pos + 1..].parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    if rounded_exp < -(4_i32) || rounded_exp >= prec as i32 {
        // Exponential notation. Reuse the already-rounded mantissa string.
        let s = if upper {
            format!("{:.sig_digits$E}", f)
        } else {
            exp_str
        };
        let s = normalise_exp_str(s, f, None);
        if alt {
            ensure_exp_alt_zeros(s, prec)
        } else {
            trim_g_trailing_zeros(s)
        }
    } else {
        // Fixed notation: `prec` significant figures means
        //   decimal_digits = prec - 1 - exp (clamped at 0).
        let decimal_digits = if rounded_exp >= 0 {
            prec.saturating_sub(rounded_exp as usize + 1)
        } else {
            prec + (-rounded_exp - 1) as usize
        };
        let s = format!("{:.decimal_digits$}", f);
        if alt {
            // The rounded fixed string already carries exactly `prec`
            // significant digits; just guarantee a trailing decimal point.
            if s.contains('.') {
                s
            } else {
                format!("{s}.")
            }
        } else {
            trim_g_trailing_zeros(s)
        }
    }
}

/// Pad an already-normalised exponential string (`mantissa` + `e[+-]NN`) so its
/// mantissa carries `prec` significant digits, for the `#g`/`#G` alternate
/// form.  CPython keeps trailing zeros and the decimal point in this mode.
fn ensure_exp_alt_zeros(s: String, prec: usize) -> String {
    let e_pos = match s.find(['e', 'E']) {
        Some(p) => p,
        None => return s,
    };
    let (mantissa, exp_part) = s.split_at(e_pos);
    let sig: usize = mantissa.chars().filter(|c| c.is_ascii_digit()).count();
    let mut m = mantissa.to_string();
    if !m.contains('.') {
        m.push('.');
    }
    for _ in sig..prec {
        m.push('0');
    }
    format!("{m}{exp_part}")
}

fn trim_g_trailing_zeros(s: String) -> String {
    // Trim trailing zeros after decimal point (but keep 'e' part intact).
    let (mantissa, exp_part) = if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        (&s[..e_pos], &s[e_pos..])
    } else {
        (s.as_str(), "")
    };
    if mantissa.contains('.') {
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{trimmed}{exp_part}")
    } else {
        s
    }
}

/// The set of names that CPython's `object` exposes via `dir(object)`.
/// These are included in `dir(instance)` for every user-defined class
/// instance because every class implicitly inherits from `object` (#1225).
static OBJECT_DUNDER_NAMES: &[&str] = &[
    "__class__",
    "__delattr__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__le__",
    "__lt__",
    "__ne__",
    "__new__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__subclasshook__",
];

/// Pre-allocated `Vec<String>` of `OBJECT_DUNDER_NAMES` so that each
/// `dir()` call can clone the cached vec rather than allocating 24
/// `String`s from scratch.
static OBJECT_DUNDER_NAMES_OWNED: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| {
        OBJECT_DUNDER_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    });

/// Append the universal `object` dunder names (#2151) to a built-in value's
/// method list, so `dir(x)` advertises exactly the names `hasattr(x, …)` /
/// `getattr(x, …)` resolve through the value's `object`-rooted class MRO.
/// The caller's dedup pass removes any duplicates from type-specific overrides.
fn with_object_dunders(mut names: Vec<String>) -> Vec<String> {
    names.extend_from_slice(&OBJECT_DUNDER_NAMES_OWNED);
    names
}

/// Returns the list of attribute/method names that `dir(obj)` should report.
pub(crate) fn dir_names(value: &Value) -> Vec<String> {
    /// Recursively collect all attribute names from a class and its entire
    /// MRO (primary base then extra_bases, depth-first).
    ///
    /// When the chain terminates (base == None and the class is not the
    /// object singleton itself), append the standard object dunder names
    /// so that inherited names from `object` appear in `dir()` output,
    /// matching CPython's behaviour (#1225).
    fn collect_class_names(class: &Rc<RefCell<PyClass>>, names: &mut Vec<String>) {
        let (own_keys, base, extra_bases): (Vec<String>, _, _) = {
            let borrowed = class.borrow();
            (
                borrowed.attrs.keys().cloned().collect(),
                borrowed.base.clone(),
                borrowed.extra_bases.clone(),
            )
        };
        names.extend(own_keys);
        if let Some(b) = base {
            collect_class_names(&b, names);
        } else {
            // Reached the top of the MRO chain.  Append the names that
            // CPython's `object` exposes; the caller's dedup pass removes
            // any that were already collected from a subclass override.
            // Clone from the pre-allocated static vec to avoid 24 per-call
            // String allocations.
            names.extend_from_slice(&OBJECT_DUNDER_NAMES_OWNED);
        }
        for eb in &extra_bases {
            collect_class_names(eb, names);
        }
    }
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            let mut names: Vec<String> =
                inst.borrow().attrs.keys().map(|k| k.to_string()).collect();
            let class = Rc::clone(&inst.borrow().class);
            collect_class_names(&class, &mut names);
            names
        }
        ValueKind::PyClass(class) => {
            let mut names: Vec<String> = Vec::new();
            collect_class_names(class, &mut names);
            names
        }
        ValueKind::PyModule(module) => {
            let mut names: Vec<String> = module.borrow().attrs.keys().cloned().collect();
            // Append the synthetic dunder attributes that are returned by
            // get_attr for all module objects (env.rs), mirroring CPython 3.12
            // which includes these in dir(m) even for built-in modules.
            for dunder in &[
                "__name__",
                "__package__",
                "__loader__",
                "__spec__",
                "__doc__",
            ] {
                if !names.iter().any(|n| n == dunder) {
                    names.push(dunder.to_string());
                }
            }
            names
        }
        // Built-in data values (int/str/list/.../None) now chain to `object`
        // in their class MRO (#2151), so `dir(x)` includes the universal object
        // dunders (`__class__`, `__doc__`, `__eq__`, `__sizeof__`, `__dir__`,
        // `__reduce__`, …) alongside the type-specific methods, matching
        // `dir(x)` under CPython.  `with_object_dunders` appends them; the
        // caller's dedup pass removes overlaps.
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => {
            with_object_dunders(builtin_method_names("int"))
        }
        ValueKind::Bytes(_) => with_object_dunders(builtin_method_names("bytes")),
        ValueKind::Str(_) => with_object_dunders(builtin_method_names("str")),
        ValueKind::List(_) => with_object_dunders(builtin_method_names("list")),
        ValueKind::Tuple(_) => with_object_dunders(builtin_method_names("tuple")),
        ValueKind::Dict(_) => with_object_dunders(builtin_method_names("dict")),
        ValueKind::Set(_) => with_object_dunders(builtin_method_names("set")),
        ValueKind::Float(_) | ValueKind::Complex(_, _) | ValueKind::None
        | ValueKind::NotImplemented | ValueKind::Ellipsis | ValueKind::Range { .. } => {
            with_object_dunders(Vec::new())
        }
        ValueKind::BuiltinObject { ops, .. } => {
            with_object_dunders(builtin_method_names(ops.type_name()))
        }
        ValueKind::Generator(state_rc) => {
            // CPython exposes a type-specific introspection surface per
            // generator kind (issue #2302): a plain generator advertises the
            // synchronous iteration protocol plus `gi_*`; an async generator
            // advertises the asynchronous protocol plus `ag_*`; a coroutine
            // advertises `send`/`throw`/`close` plus `cr_*` (but NOT
            // `__iter__`/`__next__`).
            let (is_async_gen, is_coroutine) = state_rc
                .try_borrow()
                .ok()
                .and_then(|b| {
                    b.downcast_ref::<GeneratorFrame>()
                        .map(|f| (f.is_async_generator(), f.is_coroutine))
                })
                .unwrap_or((false, false));
            let mut names = vec![
                "__class__".to_string(),
                "__name__".to_string(),
                "__qualname__".to_string(),
            ];
            if is_async_gen {
                names.extend(
                    [
                        "__aiter__",
                        "__anext__",
                        "asend",
                        "athrow",
                        "aclose",
                        "ag_code",
                        "ag_frame",
                        "ag_running",
                        "ag_await",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
            } else if is_coroutine {
                names.extend(
                    [
                        "send", "throw", "close", "cr_code", "cr_frame", "cr_running", "cr_await",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
            } else {
                names.extend(
                    [
                        "__iter__",
                        "__next__",
                        "send",
                        "throw",
                        "close",
                        "gi_code",
                        "gi_frame",
                        "gi_running",
                        "gi_yieldfrom",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
            }
            names
        }
        _ => Vec::new(),
    }
}

/// Sequence / mapping / container protocol dunders that CPython 3.12 exposes
/// as bound method-wrappers on each built-in type, beyond `__iter__` (which is
/// already listed in every type's `METHODS` slice).  Single source of truth for
/// issue #1909: `dir()` / `hasattr` advertise exactly these names, the instance
/// `get_attr` path returns a bound wrapper for each, and the bound-method /
/// unbound-descriptor call paths dispatch them through the matching operator
/// machinery (`eval_index`, `eval_in`, `eval_binary`, `len`, item-assign /
/// item-delete).  Mirrors `python3.12`'s `hasattr(obj, name)` answers exactly,
/// including the asymmetries (no `__setitem__` on str/tuple, no `__add__` on
/// dict/set, no `__getitem__` on set/frozenset).
/// `true` if `name` is one of the container/sequence protocol dunder names
/// managed by [`builtin_protocol_dunders`] (issue #1909).  Used to decide
/// whether a `__dunder__` method call on a tagged container should route
/// through the protocol dispatcher (or raise `AttributeError` when the name is
/// valid for other types but not the receiver's) — without disturbing the
/// dispatch of object-level dunders like `__repr__` / `__eq__`.
fn is_container_protocol_dunder_name(name: &str) -> bool {
    matches!(
        name,
        "__len__"
            | "__getitem__"
            | "__setitem__"
            | "__delitem__"
            | "__contains__"
            | "__add__"
            | "__mul__"
            // In-place sequence dunders (#2119): list/bytearray.
            | "__iadd__"
            | "__imul__"
            // set/frozenset/dict algebra & merge dunders (#2122), including
            // reflected (`__rOP__`) and in-place (`__iOP__`) forms.
            | "__or__"
            | "__ror__"
            | "__ior__"
            | "__and__"
            | "__rand__"
            | "__iand__"
            | "__sub__"
            | "__rsub__"
            | "__isub__"
            | "__xor__"
            | "__rxor__"
            | "__ixor__"
            // Rich-comparison / `__hash__` / `__str__` / `__repr__` exposed on
            // every primitive type (issue #2070).  Routed through the protocol
            // dispatcher so `[1].__lt__([2])`, `'a'.__eq__('a')`,
            // `{1}.__hash__()` … work via the method-call opcode.  The
            // per-receiver `builtin_protocol_dunders` membership check at the
            // call site still raises `AttributeError` for a name a given type
            // does not expose.
            | "__eq__"
            | "__ne__"
            | "__lt__"
            | "__le__"
            | "__gt__"
            | "__ge__"
            | "__hash__"
            | "__str__"
            | "__repr__"
            // `dict.__reversed__()` — reversible by insertion order (#2093);
            // the per-receiver membership check restricts it to `dict`.
            | "__reversed__"
    )
}

/// Issue #2276: `Some((type_name, dunder))` when `qualified` is one of the
/// type-qualified object-level dunder sentinels synthesised by
/// `get_attr_class::primitive_owned_object_dunder` (`"str.__hash__"`,
/// `"int.__format__"`, …); `None` otherwise (including `"object.__hash__"`).
/// Keep the ownership in sync with `primitive_owned_object_dunder`.
fn primitive_object_dunder_owner(qualified: &str) -> Option<(&str, &str)> {
    let (type_name, dunder) = qualified.split_once('.')?;
    let owned = match type_name {
        "bool" => matches!(dunder, "__repr__"),
        "int" | "float" => matches!(dunder, "__hash__" | "__repr__" | "__format__"),
        "complex" => matches!(dunder, "__hash__" | "__repr__"),
        "str" => matches!(dunder, "__hash__" | "__repr__" | "__str__" | "__format__"),
        "bytes" => matches!(dunder, "__hash__" | "__repr__" | "__str__"),
        "tuple" | "frozenset" => matches!(dunder, "__hash__" | "__repr__"),
        "list" | "dict" | "set" => dunder == "__repr__",
        _ => false,
    };
    owned.then_some((type_name, dunder))
}

/// Numeric-tower rank used by the scalar forward dunders (#2070):
/// `int`/`bool` = 0, `float` = 1, `complex` = 2.  Returns `None` for
/// non-numeric values.  A forward slot on a receiver of rank R accepts an
/// operand of rank ≤ R (`float.__add__(int)` works, `int.__add__(float)` →
/// `NotImplemented`), mirroring CPython's `nb_*` slot coercion direction.
fn numeric_tower_rank(v: &Value) -> Option<u8> {
    match v.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => Some(0),
        ValueKind::Float(_) => Some(1),
        ValueKind::Complex(..) => Some(2),
        _ => None,
    }
}

/// Whether a scalar numeric/bitwise forward dunder on `recv` accepts `operand`
/// (else the slot returns `NotImplemented`).  See [`numeric_tower_rank`].
fn numeric_operand_accepted(recv: &Value, operand: &Value) -> bool {
    match (numeric_tower_rank(recv), numeric_tower_rank(operand)) {
        (Some(r), Some(o)) => o <= r,
        _ => false,
    }
}

/// Whether a rich-comparison forward dunder on `recv` accepts `operand` (else
/// it returns `NotImplemented`).  `is_equality` distinguishes `__eq__`/`__ne__`
/// (defined for `complex` and `dict`) from the ordering slots (which `complex`
/// and `dict` do not define).  Numeric receivers follow the same tower-rank
/// acceptance as arithmetic; non-numeric receivers accept only operands of a
/// compatible type group (str/str, bytes/bytes, list/list, tuple/tuple,
/// dict/dict, and set↔frozenset interchangeably).
fn richcmp_operand_accepted(recv: &Value, operand: &Value, is_equality: bool) -> bool {
    // Numeric receivers: ordering is undefined for `complex`, so a complex
    // receiver does not accept an ordering comparison from any operand.
    if let Some(r) = numeric_tower_rank(recv) {
        if !is_equality && r == 2 {
            return false;
        }
        return numeric_operand_accepted(recv, operand);
    }
    let rt = pyrust_core::builtin_type_name(recv);
    let ot = pyrust_core::builtin_type_name(operand);
    match &*rt {
        // dict has equality but no ordering.
        "dict" => is_equality && ot == "dict",
        // set and frozenset are interchangeable for both equality and the
        // subset/superset ordering comparisons.
        "set" | "frozenset" => matches!(&*ot, "set" | "frozenset"),
        // Every other primitive sequence/string compares only with its own
        // exact type (`'a'.__eq__(b'a')` → NotImplemented).
        _ => rt == ot,
    }
}

/// Issue #2291: whether a container protocol dunder is a *named* method-wrapper
/// in CPython 3.12 (its error messages read `{type}.{method}()`) versus an
/// anonymous slot wrapper (whose messages read `wrapper {method}()`).  The
/// named set is `mp_subscript` (`list`/`dict` `__getitem__`) and `sq_contains`
/// (`dict`/`set`/`frozenset` `__contains__`); every other protocol dunder is an
/// anonymous slot wrapper.  Verified against `python3.12`.
pub(crate) fn is_named_protocol_wrapper(method: &str, type_name: &str) -> bool {
    matches!(
        (method, type_name),
        ("__getitem__", "list" | "dict") | ("__contains__", "dict" | "set" | "frozenset")
    )
}

pub(crate) fn builtin_protocol_dunders(type_name: &str) -> &'static [&'static str] {
    // Rich-comparison / `__hash__` / `__str__` / `__repr__` are exposed as
    // bound method-wrappers on every primitive type (issue #2070); the
    // per-type slices below interleave them with the container/numeric slots.
    match type_name {
        // list / dict / set / bytearray are unhashable, so `__hash__` is *not*
        // exposed (CPython sets `list.__hash__ = None`; `[1].__hash__()` raises
        // `'NoneType' object is not callable`, handled by the None-attr path).
        "list" => &[
            "__len__", "__getitem__", "__setitem__", "__delitem__", "__contains__",
            "__add__", "__mul__", "__iadd__", "__imul__",
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__str__", "__repr__",
        ],
        "tuple" | "str" | "bytes" => &[
            "__len__", "__getitem__", "__contains__", "__add__", "__mul__",
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__hash__", "__str__", "__repr__",
        ],
        "bytearray" => &[
            "__len__", "__getitem__", "__setitem__", "__delitem__", "__contains__",
            "__add__", "__mul__", "__iadd__", "__imul__",
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__str__", "__repr__",
        ],
        "dict" => &[
            "__len__", "__getitem__", "__setitem__", "__delitem__", "__contains__",
            "__or__", "__ror__", "__ior__", "__reversed__",
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__str__", "__repr__",
        ],
        "set" => &[
            "__len__", "__contains__", "__or__", "__ror__", "__and__", "__rand__",
            "__sub__", "__rsub__", "__xor__", "__rxor__", "__ior__", "__iand__",
            "__isub__", "__ixor__",
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__str__", "__repr__",
        ],
        "frozenset" => &[
            "__len__", "__contains__", "__or__", "__ror__", "__and__", "__rand__",
            "__sub__", "__rsub__", "__xor__", "__rxor__",
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__hash__", "__str__", "__repr__",
        ],
        // Issue #2070: scalar numeric types expose rich-comparison, numeric,
        // and (for int/bool) bitwise dunders as bound method-wrappers, plus
        // `__hash__`/`__str__`/`__repr__`/`__bool__`.  The arithmetic dunders
        // return `NotImplemented` for operand types the forward slot does not
        // accept (e.g. `(5).__add__(5.0)`), matching CPython exactly.
        // Issue #2215: the *reflected* numeric/bitwise dunders mirror each
        // type's forward set exactly (swapped-operand semantics, same
        // tower-rank `NotImplemented` gating), exposed as bound
        // method-wrappers alongside the forward slots.
        "int" | "bool" => &[
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__hash__", "__str__", "__repr__", "__bool__",
            "__add__", "__sub__", "__mul__", "__truediv__", "__floordiv__",
            "__mod__", "__pow__", "__divmod__", "__neg__", "__pos__", "__abs__",
            "__and__", "__or__", "__xor__", "__lshift__", "__rshift__", "__invert__",
            "__radd__", "__rsub__", "__rmul__", "__rtruediv__", "__rfloordiv__",
            "__rmod__", "__rpow__", "__rdivmod__",
            "__rand__", "__ror__", "__rxor__", "__rlshift__", "__rrshift__",
        ],
        "float" => &[
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__hash__", "__str__", "__repr__", "__bool__",
            "__add__", "__sub__", "__mul__", "__truediv__", "__floordiv__",
            "__mod__", "__pow__", "__divmod__", "__neg__", "__pos__", "__abs__",
            "__radd__", "__rsub__", "__rmul__", "__rtruediv__", "__rfloordiv__",
            "__rmod__", "__rpow__", "__rdivmod__",
        ],
        "complex" => &[
            "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
            "__hash__", "__str__", "__repr__", "__bool__",
            "__add__", "__sub__", "__mul__", "__truediv__", "__pow__",
            "__neg__", "__pos__", "__abs__",
            "__radd__", "__rsub__", "__rmul__", "__rtruediv__", "__rpow__",
        ],
        _ => &[],
    }
}

/// Public method names per built-in type for `dir()`.
///
/// Derives the list from each type's canonical `METHODS` slice in
/// `pyrust_builtins`, so adding a new method there automatically surfaces
/// it via `dir()` without a parallel table to maintain.  The container
/// protocol dunders CPython exposes (`__len__`, `__getitem__`,
/// `__contains__`, `__add__`, …) come from `builtin_protocol_dunders`
/// (issue #1909); `__iter__` is already part of each `METHODS` slice.
fn builtin_method_names(type_name: &str) -> Vec<String> {
    let names: &[&str] = match type_name {
        "int" => pyrust_builtins::int::METHODS,
        "bytes" => pyrust_builtins::bytes::METHODS,
        "str" => pyrust_builtins::string::METHODS,
        "list" => pyrust_builtins::list::METHODS,
        "tuple" => pyrust_builtins::tuple::METHODS,
        "dict" => pyrust_builtins::dict::METHODS,
        "set" => pyrust_builtins::set::METHODS,
        "frozenset" => pyrust_builtins::frozenset::METHODS,
        "bytearray" => pyrust_builtins::bytearray::METHODS,
        "slice" => pyrust_builtins::slice::METHODS,
        _ => &[],
    };
    let mut out: Vec<String> = names.iter().map(|s| (*s).to_string()).collect();
    for &d in builtin_protocol_dunders(type_name) {
        out.push(d.to_string());
    }
    if type_name == "str" {
        out.push("format".to_string());
        out.push("format_map".to_string());
        out.push("maketrans".to_string());
    }
    if type_name == "bytes" {
        out.push("maketrans".to_string());
        out.push("fromhex".to_string());
    }
    if type_name == "dict" {
        out.push("fromkeys".to_string());
    }
    out
}

impl Interpreter {
    /// Renders a value to a string using the same priority as `str(x)`:
    /// `__str__` first, then `__repr__`, then the default object repr.
    /// For non-PyInstance values falls back to `Value::to_py_str()`.
    /// Exception instances without a user-defined `__str__` use the built-in
    /// `to_py_str()` fallback (matching CPython's `BaseException.__str__`
    /// special-casing).  When a user-defined `__str__` exists, it is called
    /// normally.  Used by `str.format` for the `!s` conversion and the
    /// empty-spec path.
    fn render_value_as_str(&mut self, value: &Value) -> Result<String> {
        let ValueKind::PyInstance(inst) = value.kind() else {
            // gh-95778: enforce int_max_str_digits for base-10 int->str.  This
            // is the common `str(int)` / `f"{int}"` path; `check_int_str_conversion`
            // fast-rejects non-BigInt/non-container values from their NaN-box tag
            // alone, so plain-int rendering pays nothing.
            pyrust_core::check_int_str_conversion(value)?;
            return Ok(value.to_py_str());
        };
        let inst_rc = Rc::clone(inst);
        let class = Rc::clone(&inst_rc.borrow().class);
        // For exception instances, fall back to built-in formatting only when
        // the class has no user-defined __str__ (i.e. not registered as a
        // plain Rust BuiltinFunction).
        if is_exception_class(&class) {
            let has_user_str = lookup_class_attr(&class, "__str__")
                .map(|v| !matches!(v.kind(), ValueKind::BuiltinFunction(_)))
                .unwrap_or(false);
            if !has_user_str {
                return Ok(value.to_py_str());
            }
        }
        // Try user-defined __str__ first.  Skip `object.__str__` when the
        // instance has a primitive backing store (issue #1537): primitive
        // types now have `object` as an explicit MRO base, making
        // `object.__str__` reachable for subclasses.  The backing-data
        // path below produces the correct output for those cases.
        if let Some(method_val) = lookup_class_attr(&class, "__str__") {
            let is_object_str =
                matches!(method_val.kind(), ValueKind::BuiltinFunction("object.__str__"));
            if !is_object_str || instance_builtin_data(&inst_rc).is_none() {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Str(s) => Ok(s.to_string()),
                    _ => Err(pyrust_core::type_err!("__str__ returned non-string (type {})",
                            pyrust_core::builtin_type_name(&result))),
                };
            }
        }
        // Issue #1542: str/bytes subclasses return the raw backing value for
        // str() even when the subclass defines __repr__.  CPython's
        // str.__str__ and bytes.__str__ return `self` directly without
        // delegating to __repr__.  int/float subclasses do NOT share this
        // property — their str() dispatches __repr__ if defined.
        if let Some(backing) = instance_builtin_data(&inst_rc)
            && matches!(backing.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
                return Ok(backing.to_py_str());
            }
        // No user __str__ and no str/bytes backing: fall through to __repr__,
        // then to the numeric/container backing, matching CPython's str()
        // delegation chain for int/float subclasses and container subclasses.
        // Same object.__repr__ sentinel skip as in render_instance_repr.
        if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
            let is_object_repr =
                matches!(method_val.kind(), ValueKind::BuiltinFunction("object.__repr__"));
            if !is_object_repr || instance_builtin_data(&inst_rc).is_none() {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Str(s) => Ok(s.to_string()),
                    _ => Err(pyrust_core::type_err!("__repr__ returned non-string (type {})",
                            pyrust_core::builtin_type_name(&result))),
                };
            }
        }
        // Issue #1205 / #1542: no __str__ or __repr__ in MRO — delegate to
        // the backing value so that scalar and container subclasses render
        // their contents rather than the generic object repr.
        // Use render_value_repr (interp-aware) so that PyInstance elements
        // inside the backing container have their __repr__ called correctly.
        if let Some(backing) = instance_builtin_data(&inst_rc) {
            match backing.kind() {
                ValueKind::Str(_)
                | ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Bool(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
                | ValueKind::Bytes(_) => {
                    // gh-95778: an int subclass renders its backing in base 10.
                    pyrust_core::check_int_str_conversion(&backing)?;
                    return Ok(backing.to_py_str());
                }
                ValueKind::List(_) | ValueKind::Dict(_) | ValueKind::Tuple(_) => {
                    return crate::builtin_modules::builtins::render_value_repr(self, &backing);
                }
                ValueKind::Set(items) => {
                    let class_name = class.borrow().name.clone();
                    if items.is_empty() {
                        return Ok(format!("{class_name}()"));
                    }
                    let inner =
                        crate::builtin_modules::builtins::render_value_repr(self, &backing)?;
                    return Ok(format!("{class_name}({inner})"));
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
                {
                    let class_name = class.borrow().name.clone();
                    let items = pyrust_builtins::frozenset::as_items(&backing);
                    let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                    if is_empty {
                        return Ok(format!("{class_name}()"));
                    }
                    // Render elements as `{e1, e2}` without the outer `frozenset(...)`
                    // Use render_key_repr (interp-aware) so PyKey::Object elements
                    // have their user __repr__ called.
                    let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                    let mut inner_elems = Vec::with_capacity(snapshot.len());
                    for k in &snapshot {
                        inner_elems.push(
                            crate::builtin_modules::builtins::render_key_repr(self, k)?,
                        );
                    }
                    return Ok(format!("{class_name}({{{}}})", inner_elems.join(", ")));
                }
                _ => {}
            }
        }
        // No dunders found: fall back to Value::repr(), which produces
        // `<module.qualname object at 0xADDR>` matching CPython's object.__repr__.
        Ok(value.repr())
    }
}

/// Implements `str.format()`.  Parses `{...}` replacement fields in `template`
/// and substitutes positional or keyword arguments, optionally formatted by
/// a `:spec` and/or converted by `!r`/`!s`/`!a`.  Supports `{{` / `}}` for
/// literal braces and `{0.attr}` / `{0[key]}` field accessors.
impl Interpreter {
    /// Fast path for an f-string interpolation with no `!r/!s/!a` conversion and
    /// no format spec: equivalent to `format(value, "")` but without the
    /// `format` global lookup or the generic call frame (issue #1926, mirrors
    /// CPython's FORMAT_VALUE).
    ///
    /// For a non-`PyInstance` value, `format(value, "")` is exactly
    /// `apply_format_spec(value, "")`, i.e. `str(value)` — computed inline here.
    /// For the rarer `PyInstance` case (which may define a custom
    /// `__format__`/`__str__`), we delegate to the real `format` builtin so the
    /// dispatch is byte-for-byte identical to the call-based lowering.
    pub(crate) fn format_value_default(&mut self, value: &Value) -> Result<Value> {
        // Fast path (#alloc): a bare `{i}` field for an int (no format spec) is
        // `str(i)` — format the digits directly into the string Value, one
        // allocation instead of the int→`String`→`Value::string` pair the
        // generic `apply_format_spec("")` path takes.
        if let ValueKind::Int(n) = value.kind() {
            return Ok(Value::int_string(n));
        }
        if matches!(value.kind(), ValueKind::PyInstance(_)) {
            return self.call_function_expanded(
                Value::builtin_function("format"),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: value.clone(),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(""),
                    },
                ],
            );
        }
        // Non-instance: empty spec == str(value).
        apply_format_spec(value, "")
    }

    /// Dispatch `__format__(spec)` for a value, validating that the result is a
    /// `str`.  Mirrors the logic in the `format()` builtin (#1370).
    ///
    /// For `PyInstance`:
    ///   1. Look up `__format__` in the MRO.  If found and it is a user-defined
    ///      function (not the object builtin), call it and check the return is
    ///      `str`; if not, raise `TypeError`.
    ///   2. No user `__format__`: if there is backing primitive data, apply
    ///      `apply_format_spec` to the backing value.
    ///   3. Pure user class with neither: empty spec → `__str__` via
    ///      `render_value_as_str`; non-empty spec → `TypeError` (matching
    ///      CPython's `object.__format__` behaviour).
    ///
    /// For all other value kinds: delegate straight to `apply_format_spec`.
    pub(crate) fn dispatch_dunder_format(&mut self, value: &Value, spec: &str) -> Result<Value> {
        let ValueKind::PyInstance(inst) = value.kind() else {
            return apply_format_spec(value, spec);
        };
        let inst_rc = Rc::clone(inst);
        let class = Rc::clone(&inst_rc.borrow().class);
        if let Some(method_val) = lookup_class_attr(&class, "__format__") {
            // Only dispatch to user-defined __format__, not the object builtin.
            if !matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(spec),
                    }],
                )?;
                return if is_str_or_str_subclass(&result) {
                    Ok(result)
                } else {
                    Err(pyrust_core::type_err!("__format__ must return a str, not {}",
                            value_type_name_str(&result),))
                };
            }
        }
        // No user __format__ in MRO (or only the object builtin).
        if let Some(backing) = instance_builtin_data(&inst_rc) {
            // CPython names the *actual* subclass in an unsupported-spec
            // TypeError, not the backing primitive (`B.__format__`, not
            // `bytes.__format__`), so thread the receiver's own type name
            // through to `apply_format_spec` (issue #2212).
            let owner = value_type_name_str(value);
            return apply_format_spec_named(&backing, spec, Some(&owner));
        }
        // Pure user class with no custom __format__ and no backing data.
        if spec.is_empty() {
            Ok(Value::string(self.render_value_as_str(value)?))
        } else {
            let type_name = value_type_name_str(value);
            Err(pyrust_core::type_err!("unsupported format string passed to {}.__format__", type_name))
        }
    }

fn format_str_template(
    &mut self,
    template: &str,
    positional: &[Value],
    keyword: &[(String, Value)],
) -> Result<Value> {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    // Tracks auto-numbering of `{}` placeholders.  `Some(n)` = next index;
    // `None` = mixed manual/auto detected, raise on next auto.
    let mut auto_idx: Option<usize> = Some(0);
    let mut saw_manual = false;

    while i < bytes.len() {
        let c = bytes[i];
        if c == b'{' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                out.push('{');
                i += 2;
                continue;
            }
            // Find matching '}'. Track nested braces inside the format spec
            // (e.g. "{:{width}}") at depth-1 only.
            let mut depth = 1;
            let mut j = i + 1;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                j += 1;
            }
            if depth != 0 {
                return Err(pyrust_core::value_err!("Single '{' encountered in format string".to_string()));
            }
            let field = &template[i + 1..j];
            i = j + 1;

            // Split off the format spec at the first ':' that isn't inside `[]`.
            let (field_name_full, spec) = split_field_and_spec(field);
            // Split off the conversion (`!r`, `!s`, `!a`) from the field name.
            let (field_name, conversion) = match field_name_full.rsplit_once('!') {
                Some((name, conv)) if conv.len() == 1 => (name, Some(conv.chars().next().unwrap())),
                _ => (field_name_full, None),
            };

            // Resolve the base value from the field name's head segment.
            let (head, rest) = split_head_and_accessors(field_name);
            let base = if head.is_empty() {
                // Auto-numbered field
                if saw_manual {
                    return Err(pyrust_core::value_err!("cannot switch from manual field specification to automatic field numbering"));
                }
                let Some(idx) = auto_idx else { unreachable!() };
                auto_idx = Some(idx + 1);
                positional.get(idx).cloned().ok_or_else(|| pyrust_core::index_err!("Replacement index {idx} out of range for positional args tuple"))?
            } else if let Ok(n) = head.parse::<usize>() {
                if auto_idx.is_some() && auto_idx != Some(0) {
                    return Err(pyrust_core::value_err!("cannot switch from automatic field numbering to manual field specification"));
                }
                saw_manual = true;
                auto_idx = None;
                positional.get(n).cloned().ok_or_else(|| pyrust_core::index_err!("Replacement index {n} out of range for positional args tuple"))?
            } else {
                keyword
                    .iter()
                    .find(|(k, _)| k == head)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| PyError::key_error(Value::string(head)))?
            };

            // Apply field accessors (`.attr` / `[key]`) for any subscriptable type.
            let value = apply_field_accessors(self, base, rest)?;

            // Apply conversion (`!r`, `!s`, `!a`).
            // `!s` dispatches `__str__` on user instances (mirrors `str(x)`).
            let value = match conversion {
                Some('r') => Value::string(render_instance_repr(self, &value)?),
                Some('s') => Value::string(self.render_value_as_str(&value)?),
                Some('a') => Value::string(ascii_repr_interp(self, &value)?),
                Some(c) => {
                    return Err(pyrust_core::value_err!("Unknown conversion specifier {c}"));
                }
                None => value,
            };

            // Expand any `{field}` references inside the format spec before
            // applying it (PEP 3101 one-level nesting, e.g. `"{:{width}}"`).
            let expanded_spec;
            let spec = if spec.contains('{') {
                expanded_spec = expand_format_spec_positional(
                    spec,
                    positional,
                    keyword,
                    &mut auto_idx,
                    &mut saw_manual,
                )?;
                expanded_spec.as_str()
            } else {
                spec
            };

            // Dispatch __format__(spec) via the user class if applicable, then
            // validate the return is a str.  For non-instance values fall through
            // to apply_format_spec directly.
            let formatted = self.dispatch_dunder_format(&value, spec)?;
            out.push_str(&extract_str_value(&formatted));
        } else if c == b'}' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                out.push('}');
                i += 2;
            } else {
                return Err(pyrust_core::value_err!("Single '}' encountered in format string".to_string()));
            }
        } else {
            // Walk one UTF-8 char: advance past the start byte, then skip any
            // continuation bytes (0b10xxxxxx). The outer loop guarantees the
            // current byte is a start byte (the `{`/`}` branches handle the
            // ASCII delimiters; other ASCII bytes are 1-byte chars).
            let ch_start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                i += 1;
            }
            out.push_str(&template[ch_start..i]);
        }
    }
    Ok(Value::string(out))
}
} // impl Interpreter (format_str_template + render_value_as_str)

/// Splits a replacement field on the first `:` that is not inside `[]`,
/// returning `(field_name_with_conversion, format_spec)`.
fn split_field_and_spec(field: &str) -> (&str, &str) {
    let bytes = field.as_bytes();
    let mut bracket_depth = 0;
    for (idx, b) in bytes.iter().enumerate() {
        match b {
            b'[' => bracket_depth += 1,
            b']' if bracket_depth > 0 => bracket_depth -= 1,
            b':' if bracket_depth == 0 => return (&field[..idx], &field[idx + 1..]),
            _ => {}
        }
    }
    (field, "")
}

/// Splits a field name like `0.x[1].y` into `("0", ".x[1].y")`.
fn split_head_and_accessors(name: &str) -> (&str, &str) {
    let bytes = name.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'.' || *b == b'[' {
            return (&name[..i], &name[i..]);
        }
    }
    (name, "")
}

/// Applies a chain of `.attr` / `[key]` accessors to a value.
fn apply_field_accessors(
    interp: &mut Interpreter,
    mut value: Value,
    mut rest: &str,
) -> Result<Value> {
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        if bytes[0] == b'.' {
            // Find next '.' or '['
            let end = bytes[1..]
                .iter()
                .position(|&b| b == b'.' || b == b'[')
                .map(|i| i + 1)
                .unwrap_or(rest.len());
            let attr = &rest[1..end];
            rest = &rest[end..];
            // Dispatch getattr through the full attribute resolution path so
            // that built-in types (int, float, complex, …) work the same way
            // `getattr(value, attr)` does — not just PyInstance values (#1031).
            value = interp.get_attr(&value, attr)?;
        } else if bytes[0] == b'[' {
            let end = bytes
                .iter()
                .position(|&b| b == b']')
                .ok_or_else(|| pyrust_core::value_err!("Missing ']' in format field accessor"))?;
            let key_str = &rest[1..end];
            rest = &rest[end + 1..];
            // Per CPython 3.12: a subscript that parses as a non-negative
            // integer is passed as `int` to __getitem__; anything else
            // (non-numeric or negative like "-1") is passed as `str`.
            // CPython rejects numbers > i64::MAX with "Too many decimal
            // digits in format string" (matching CPython's internal
            // Py_ssize_t overflow check in _PyObject_GetMethod).
            let key = if let Ok(idx) = key_str.parse::<u64>() {
                if idx > i64::MAX as u64 {
                    return Err(pyrust_core::value_err!("Too many decimal digits in format string"));
                }
                Value::int(idx as i64)
            } else {
                Value::string(key_str)
            };
            value = interp.eval_index(&value, key)?;
        } else {
            return Err(pyrust_core::value_err!("unexpected character in format field: '{}'", &rest[..1]));
        }
    }
    Ok(value)
}

/// Expands `{field_name}` references within a format spec string (the part
/// after `:` in a replacement field).  Per PEP 3101, only one level of
/// nesting is allowed — the inner fields cannot have a conversion or a
/// further nested spec.
///
/// `auto_idx` and `saw_manual` are the same counters used by the enclosing
/// `format_str_template` call; the spec fields advance the same auto-number
/// sequence as top-level fields.
fn expand_format_spec_positional(
    spec: &str,
    positional: &[Value],
    keyword: &[(String, Value)],
    auto_idx: &mut Option<usize>,
    saw_manual: &mut bool,
) -> Result<String> {
    let bytes = spec.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                out.push('{');
                i += 2;
            }
            b'}' if i + 1 < bytes.len() && bytes[i + 1] == b'}' => {
                out.push('}');
                i += 2;
            }
            b'{' => {
                // Find the matching '}'.  No further nesting is allowed inside
                // a format spec's inner field.
                let start = i + 1;
                let end = bytes[start..]
                    .iter()
                    .position(|&b| b == b'}')
                    .ok_or_else(|| {
                        pyrust_core::value_err!("Single '{' encountered in format string".to_string())
                    })?
                    + start;
                // PEP 3101: inner fields cannot have a nested spec; if the user
                // wrote `{name:spec}` inside a format spec, CPython treats
                // everything before `:` as the field name.
                let inner_raw = &spec[start..end];
                i = end + 1;
                let inner = inner_raw
                    .split_once(':')
                    .map(|(name, _)| name)
                    .unwrap_or(inner_raw);

                // Inner fields do not support '!' conversion or nested ':' spec.
                let value = if inner.is_empty() {
                    // Auto-numbered
                    if *saw_manual {
                        return Err(pyrust_core::value_err!("cannot switch from manual field specification to automatic field numbering"));
                    }
                    let Some(idx) = *auto_idx else { unreachable!() };
                    *auto_idx = Some(idx + 1);
                    positional.get(idx).cloned().ok_or_else(|| {
                        pyrust_core::index_err!("Replacement index {idx} out of range for positional args tuple")
                    })?
                } else if let Ok(n) = inner.parse::<usize>() {
                    if auto_idx.is_some() && *auto_idx != Some(0) {
                        return Err(pyrust_core::value_err!("cannot switch from automatic field numbering to manual field specification"));
                    }
                    *saw_manual = true;
                    *auto_idx = None;
                    positional.get(n).cloned().ok_or_else(|| {
                        pyrust_core::index_err!("Replacement index {n} out of range for positional args tuple")
                    })?
                } else {
                    keyword
                        .iter()
                        .find(|(k, _)| k == inner)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| PyError::key_error(Value::string(inner)))?
                };
                out.push_str(&value.to_py_str());
            }
            b'}' => {
                return Err(pyrust_core::value_err!("Single '}' encountered in format string".to_string()));
            }
            _ => {
                let ch_start = i;
                i += 1;
                while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                    i += 1;
                }
                out.push_str(&spec[ch_start..i]);
            }
        }
    }
    Ok(out)
}

/// Does `err` signal end-of-sequence for the legacy `__getitem__`
/// iter protocol?  CPython terminates iteration on `IndexError`,
/// `StopIteration`, **and any subclass of those** raised from
/// `__getitem__`; anything else propagates to the caller.  Issue #394.
///
/// Subclass-aware: we look up the canonical `IndexError` /
/// `StopIteration` classes via [`lookup_name_in_module`] and walk the
/// raised instance's `class.base` chain with [`class_is_subclass_of`].
/// A user-defined class *named* `IndexError` that doesn't actually
/// derive from the built-in `IndexError` no longer falsely terminates
/// iteration.
pub(crate) fn is_sequence_iter_terminator(interp: &Interpreter, err: &PyError) -> bool {
    // `class_is_subclass_of` and `lookup_name_in_module` are pulled into
    // the `interpreter` module directly via `include!` in interpreter.rs
    // — no use-import needed.

    // Look up the canonical built-in exception classes in the module env.
    // If they aren't present (shouldn't happen — installed by
    // install_exception_builtins at startup), fall back to a name match
    // so the iterator still terminates rather than spinning.
    let (built_in_index, built_in_stop) = {
        let env = &interp.env;
        let idx = lookup_name_in_module(env, "IndexError");
        let stp = lookup_name_in_module(env, "StopIteration");
        let to_class = |v: Option<Value>| match v.as_ref().map(|v| v.kind()) {
            Some(ValueKind::PyClass(c)) => Some(Rc::clone(c)),
            _ => None,
        };
        (to_class(idx), to_class(stp))
    };

    let cls_is_terminator = |cls: &Rc<RefCell<PyClass>>| -> bool {
        if let Some(ref base) = built_in_index
            && class_is_subclass_of(cls, base)
        {
            return true;
        }
        if let Some(ref base) = built_in_stop
            && class_is_subclass_of(cls, base)
        {
            return true;
        }
        false
    };

    match err {
        // PyError::Named is the VM-internal raise shape that pre-dates
        // the PyInstance-backed exception path.  Match the canonical
        // built-in names directly — VM-internal raises never come from
        // user subclasses of IndexError/StopIteration.
        PyError::Named(cls, _) => cls == "IndexError" || cls == "StopIteration",
        // PyError::Class carries class identity directly; check the name the
        // same way as Named — no env lookup needed.
        PyError::Class(cls, _) => {
            let borrow = cls.borrow();
            borrow.name == "IndexError" || borrow.name == "StopIteration"
        }
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                cls_is_terminator(&class)
            }
            _ => false,
        },
        _ => false,
    }
}

/// Renders a value using its `__repr__` dunder for the `!r` conversion flag in
/// `str.format`.  Mirrors the `repr()` builtin's dispatch: for `PyInstance`
/// values, looks up `__repr__` via MRO, calls it, and validates the return is a
/// `str`.  Non-instances (built-in types) fall back to `value.repr()` unchanged.
///
/// Note: exception instances do not bypass `__repr__` here — CPython dispatches
/// `__repr__` on exceptions normally (only `__str__` has the special-case).
fn render_instance_repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    // gh-95778: enforce int_max_str_digits for base-10 int->str (no-op unless
    // `value` is, or transitively contains, an over-limit BigInt).
    pyrust_core::check_int_str_conversion(value)?;
    let ValueKind::PyInstance(inst) = value.kind() else {
        return Ok(value.repr());
    };
    let inst_rc = Rc::clone(inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
        // Issue #1537: primitive types now expose `object` as an explicit
        // MRO base, so `object.__repr__` is reachable for user subclasses
        // (e.g. `class MyList(list): pass`).  Skip the `object.__repr__`
        // sentinel when the instance has a primitive backing store — the
        // backing-data path below renders the contents correctly, matching
        // CPython's `list.__repr__`, `dict.__repr__`, etc. behaviour.
        let is_object_repr =
            matches!(method_val.kind(), ValueKind::BuiltinFunction("object.__repr__"));
        if !is_object_repr || instance_builtin_data(&inst_rc).is_none() {
            let result = invoke_class_method(
                interp,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            return match result.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(pyrust_core::type_err!("__repr__ returned non-string (type {})",
                        pyrust_core::builtin_type_name(&result))),
            };
        }
    }
    // Issue #1205: no __repr__ in MRO (or object.__repr__ skipped above) —
    // delegate to backing container so that list/dict/tuple/set subclasses
    // render their contents rather than the generic `<ClassName object at
    // 0x...>` object repr.
    // Use render_value_repr (interp-aware) so that PyInstance elements
    // inside the backing container have their __repr__ called correctly.
    // Issue #1542: scalar backings (int/float/str/bytes subclasses) also
    // need to delegate to the backing value's repr() so that
    // `"%r" % MyInt(42)` returns "42" rather than the address repr.
    if let Some(backing) = instance_builtin_data(&inst_rc) {
        match backing.kind() {
            ValueKind::Str(_)
            | ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Bool(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _)
            | ValueKind::Bytes(_) => return Ok(backing.repr()),
            ValueKind::List(_) | ValueKind::Dict(_) | ValueKind::Tuple(_) => {
                return crate::builtin_modules::builtins::render_value_repr(interp, &backing);
            }
            ValueKind::Set(items) => {
                let class_name = class.borrow().name.clone();
                if items.is_empty() {
                    return Ok(format!("{class_name}()"));
                }
                let inner =
                    crate::builtin_modules::builtins::render_value_repr(interp, &backing)?;
                return Ok(format!("{class_name}({inner})"));
            }
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
            {
                let class_name = class.borrow().name.clone();
                let items = pyrust_builtins::frozenset::as_items(&backing);
                let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                if is_empty {
                    return Ok(format!("{class_name}()"));
                }
                // Render elements as `{e1, e2}` without the outer `frozenset(...)`
                // Use render_key_repr (interp-aware) so PyKey::Object elements
                // have their user __repr__ called.
                let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                let mut inner_elems = Vec::with_capacity(snapshot.len());
                for k in &snapshot {
                    inner_elems
                        .push(crate::builtin_modules::builtins::render_key_repr(interp, k)?);
                }
                return Ok(format!("{class_name}({{{}}})", inner_elems.join(", ")));
            }
            _ => {}
        }
    }
    Ok(value.repr())
}

/// Escapes all non-ASCII characters in `s` using Python's `\xNN`, `\uNNNN`,
/// or `\UNNNNNNNN` notation.  This is the pure string-transformation step
/// used by `ascii_repr_interp`.
fn ascii_escape_str(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii() {
                vec![c]
            } else {
                let cp = c as u32;
                if cp <= 0xFF {
                    format!("\\x{cp:02x}").chars().collect()
                } else if cp <= 0xFFFF {
                    format!("\\u{cp:04x}").chars().collect()
                } else {
                    format!("\\U{cp:08x}").chars().collect()
                }
            }
        })
        .collect()
}

/// Interpreter-aware `ascii()` implementation.  Dispatches user `__repr__` for
/// `PyInstance` values (matching the behaviour of the `repr()` builtin), then
/// applies ASCII escaping to the resulting string.  Raises `TypeError` if
/// `__repr__` returns a non-string.
pub(crate) fn ascii_repr_interp(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let repr_str = render_instance_repr(interp, value)?;
    Ok(ascii_escape_str(&repr_str))
}

/// Merge keyword arguments into the positional `pos` buffer for str methods,
/// or raise `TypeError` for unknown kwargs or methods that accept none.
///
/// Methods that accept kwargs map them into the appropriate positional slot.
/// All other str methods reject any keyword arguments with a CPython-matching
/// `TypeError` message.  This is called only when `!kw.is_empty()`.
fn str_merge_kwargs(
    method: &str,
    pos: &mut Vec<Value>,
    kw: PyDict,
) -> Result<()> {
    match method {
        // split(sep=None, maxsplit=-1)
        "split" | "rsplit" => {
            let mut sep: Option<Value> = None;
            let mut maxsplit: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "sep" => {
                        if !pos.is_empty() {
                            return Err(pyrust_core::type_err!("argument for {method}() given by name ('sep') and position (1)"));
                        }
                        sep = Some(v);
                    }
                    "maxsplit" => {
                        if pos.get(1).is_some() {
                            return Err(pyrust_core::type_err!("argument for {method}() given by name ('maxsplit') and position (2)"));
                        }
                        maxsplit = Some(v);
                    }
                    other => {
                        return Err(pyrust_core::type_err!("'{other}' is an invalid keyword argument for {method}()"));
                    }
                }
            }
            // Merge into positional slots, extending as needed.
            // pos[0] = sep, pos[1] = maxsplit
            if let Some(ms) = maxsplit {
                // Ensure pos[0] exists (sep defaults to None)
                if pos.is_empty() {
                    pos.push(sep.unwrap_or_else(Value::none));
                } else if let Some(sep_val) = sep {
                    pos[0] = sep_val;
                }
                if pos.len() < 2 {
                    pos.push(ms);
                }
            } else if let Some(sep_val) = sep {
                if pos.is_empty() {
                    pos.push(sep_val);
                } else {
                    pos[0] = sep_val;
                }
            }
            Ok(())
        }
        // splitlines(keepends=False) — single positional-or-keyword arg.  Reuse
        // the shared merge from the bytes path (#1990) so the overflow / clash /
        // unknown-kwarg wording matches CPython byte-for-byte.
        "splitlines" => {
            *pos = pyrust_builtins::bytes::merge_single_kwarg("splitlines", "keepends", pos, &kw)?;
            Ok(())
        }
        // encode(encoding='utf-8', errors='strict')
        "encode" => {
            // CPython checks the total argument count before individual
            // duplicate checks.  When positional args are present the message
            // says "arguments"; when it is all-kwargs it says "keyword arguments".
            let total = pos.len() + kw.len();
            if total > 2 {
                if pos.is_empty() {
                    return Err(pyrust_core::type_err!("encode() takes at most 2 keyword arguments ({total} given)"));
                }
                return Err(pyrust_core::type_err!("encode() takes at most 2 arguments ({total} given)"));
            }
            let mut encoding: Option<Value> = None;
            let mut errors: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "encoding" => {
                        if !pos.is_empty() {
                            return Err(pyrust_core::type_err!("argument for encode() given by name ('encoding') and position (1)"));
                        }
                        encoding = Some(v);
                    }
                    "errors" => {
                        if pos.get(1).is_some() {
                            return Err(pyrust_core::type_err!("argument for encode() given by name ('errors') and position (2)"));
                        }
                        errors = Some(v);
                    }
                    other => {
                        return Err(pyrust_core::type_err!("'{other}' is an invalid keyword argument for encode()"));
                    }
                }
            }
            // Merge into positional slots
            if let Some(err_val) = errors {
                if pos.is_empty() {
                    pos.push(encoding.unwrap_or_else(|| Value::string("utf-8")));
                } else if let Some(enc_val) = encoding {
                    pos[0] = enc_val;
                }
                if pos.len() < 2 {
                    pos.push(err_val);
                }
            } else if let Some(enc_val) = encoding {
                if pos.is_empty() {
                    pos.push(enc_val);
                } else {
                    pos[0] = enc_val;
                }
            }
            Ok(())
        }
        // expandtabs(tabsize=8) — single positional-or-keyword arg.  Reuse the
        // shared merge from the bytes path (#1990) for CPython-matching error
        // wording (overflow / name+position clash / unknown kwarg).
        "expandtabs" => {
            *pos = pyrust_builtins::bytes::merge_single_kwarg("expandtabs", "tabsize", pos, &kw)?;
            Ok(())
        }
        // All str methods that take no keyword arguments use the `str.` prefix
        _ => Err(pyrust_core::type_err!("str.{method}() takes no keyword arguments")),
    }
}

/// CPython wording for `zip(strict=True)` when argument `short_idx+1` (1-based)
/// ran out before the others, after `count` rows have been yielded.
fn zip_shorter_message(short_idx: usize, _count: usize) -> String {
    if short_idx == 1 {
        format!("zip() argument {} is shorter than argument 1", short_idx + 1)
    } else {
        format!(
            "zip() argument {} is shorter than arguments 1-{}",
            short_idx + 1,
            short_idx
        )
    }
}

/// CPython wording for `zip(strict=True)` when argument `long_idx+1` (1-based)
/// still has a value after argument 1 (index 0) ran out, after `count` rows
/// have been yielded.
fn zip_longer_message(long_idx: usize, _count: usize) -> String {
    if long_idx == 1 {
        format!("zip() argument {} is longer than argument 1", long_idx + 1)
    } else {
        format!(
            "zip() argument {} is longer than arguments 1-{}",
            long_idx + 1,
            long_idx
        )
    }
}

/// Format a list of missing argument names the same way CPython 3.12 does:
/// - 1 name  → `'x'`
/// - 2 names → `'a' and 'b'`
/// - 3+ names → `'a', 'b', and 'c'`  (Oxford comma)
fn format_missing_args(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("'{only}'"),
        [first, second] => format!("'{first}' and '{second}'"),
        [init @ .., last] => {
            let quoted: Vec<String> = init.iter().map(|n| format!("'{n}'")).collect();
            format!("{}, and '{last}'", quoted.join(", "))
        }
    }
}

/// Raise the CPython-3.12 `TypeError` for unbound required parameters, if any.
///
/// Positional misses are reported before keyword-only misses (CPython only
/// surfaces the kwonly error once all positionals are satisfied).  `display`
/// is the function's qualified name (e.g. `"Foo.__new__"`).  Shared by both
/// the no-variadic and the `*args`/`**kwargs` binding paths in
/// `call_user_function_expanded`, which previously inlined identical copies.
fn check_missing_args(display: &str, missing_positional: &[&str], missing_kwonly: &[&str]) -> Result<()> {
    if !missing_positional.is_empty() {
        let count = missing_positional.len();
        let arg_word = if count == 1 { "argument" } else { "arguments" };
        let names_str = format_missing_args(missing_positional);
        return Err(pyrust_core::type_err!(
            "{display}() missing {count} required positional {arg_word}: {names_str}"
        ));
    }
    if !missing_kwonly.is_empty() {
        let count = missing_kwonly.len();
        let arg_word = if count == 1 { "argument" } else { "arguments" };
        let names_str = format_missing_args(missing_kwonly);
        return Err(pyrust_core::type_err!(
            "{display}() missing {count} required keyword-only {arg_word}: {names_str}"
        ));
    }
    Ok(())
}

impl Interpreter {
    /// Build a user function value from a compiled `FnProto`.
    ///
    /// Extracted from the `MakeFunction` VM dispatch arm so that changes to
    /// function-construction semantics (annotations, defaults, etc.) only
    /// require touching this method rather than vm.rs.
    pub(crate) fn exec_make_function(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        proto_idx: u8,
        defs_base: crate::bytecode::Reg,
        annots_base: crate::bytecode::Reg,
    ) -> Result<Value> {
        let proto = code.fn_protos.get(proto_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: fn_proto index {} out of range (pool size {})",
                proto_idx,
                code.fn_protos.len()
            ))
        })?;
        let proto_code = Rc::clone(&proto.code);
        let proto_name = proto.name.clone();
        let proto_qualname = proto.qualname.clone();
        let proto_local_index = Rc::clone(&proto.local_index);
        let proto_param_binds = Rc::clone(&proto.param_binds);
        let proto_self_bind = proto.self_bind;
        let proto_local_names = Rc::clone(&proto.local_names);
        let proto_global_names = Rc::clone(&proto.global_names);
        let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
        let param_spec = Rc::clone(&proto.param_spec);
        let annotation_keys = proto.annotation_keys.clone();
        let is_pure = proto.is_pure;
        let proto_doc = proto.docstring.as_ref().map(|s| Value::string(s.clone()));

        let mut params = Vec::with_capacity(param_spec.names.len());
        let mut def_slot = 0u32;
        for i in 0..param_spec.names.len() {
            let default = if param_spec.has_default[i] {
                let v = vm_read(regs, defs_base + def_slot, num_locals)?;
                def_slot += 1;
                Some(v)
            } else {
                None
            };
            params.push(UserFunctionParam {
                name: param_spec.names[i].clone(),
                default,
                is_args: param_spec.is_args[i],
                is_kwargs: param_spec.is_kwargs[i],
                is_keyword_only: param_spec.is_keyword_only[i],
                is_positional_only: param_spec.is_positional_only[i],
            });
        }
        let mut annotations_map: PyDict = PyDict::default();
        for (i, key) in annotation_keys.iter().enumerate() {
            let val = vm_read(regs, annots_base + i as u32, num_locals)?;
            annotations_map.insert(PyKey::str_from(key.as_str()), val);
        }
        // #2256: don't eagerly allocate an empty dict for the (common)
        // unannotated function — store the `unset()` sentinel and let
        // `__annotations__` materialise lazily on first access.
        let annotations = if annotations_map.is_empty() {
            Value::unset()
        } else {
            Value::dict(annotations_map)
        };
        for name in proto_nonlocal_names.iter() {
            if !has_local_binding_in_current_or_ancestor(&self.env, name) {
                return Err(pyrust_core::py_err!("SyntaxError", "no binding for nonlocal '{}' found", name));
            }
        }
        let func = Rc::new(UserFunction {
            id: crate::value::next_fn_id(),
            kind: crate::value::UserFunctionKind::Regular,
            name: proto_name,
            qualname: proto_qualname,
            name_overrides: std::cell::RefCell::new(None),
            // #2256: lazy `__main__` default — store the `unset()` sentinel so
            // the (universal) never-reassigned case carries no per-closure heap
            // `String`; `module_value()` materialises `"__main__"` on read.
            module: std::cell::RefCell::new(Value::unset()),
            doc: std::cell::RefCell::new(proto_doc.unwrap_or_else(Value::none)),
            attrs: std::cell::RefCell::new(None),
            annotations: std::cell::RefCell::new(annotations),
            params,
            param_binds: proto_param_binds,
            self_bind: proto_self_bind,
            local_names: proto_local_names,
            local_index: proto_local_index,
            global_names: proto_global_names,
            nonlocal_names: proto_nonlocal_names,
            env: Rc::clone(&self.env),
            is_pure,
            precompiled_code: Some(proto_code),
            wrapped_func: None,
        });
        Ok(Value::user_function(func))
    }

    /// Construct a class value from a compiled `FnProto` body.
    ///
    /// Extracted from the `MakeClass` VM dispatch arm so that changes to
    /// class-construction semantics (__slots__, PEP 487/695, etc.) only
    /// require touching this method rather than vm.rs.  The per-phase work is
    /// split into helpers below to keep this top-level flow readable:
    /// seed regs -> run body -> collect attrs -> resolve bases -> build class
    /// -> run PEP 487 hooks.
    // VM instruction entry: the arg list is the decoded `MakeClass` operands
    // (proto/bases/name/kwargs registers); packing them into a struct only moves
    // the operand list without simplifying the call site.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_make_class(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        proto_idx: u8,
        bases_base: crate::bytecode::Reg,
        bases_n: u8,
        name_idx: u16,
        kwarg_base: crate::bytecode::Reg,
    ) -> Result<Value> {
        let class_name = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {} out of range (pool size {})",
                    name_idx,
                    code.names.len()
                ))
            })?
            .clone();
        let proto = code.fn_protos.get(proto_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: fn_proto index {} out of range (pool size {})",
                proto_idx,
                code.fn_protos.len()
            ))
        })?;
        let class_code = Rc::clone(&proto.code);
        let local_index = Rc::clone(&proto.local_index);
        // Classes (one per `def`) take an owned `String`; the `Rc<str>` sharing
        // of #2256 targets the many-closures-per-def case, not class objects.
        let proto_qualname = proto.qualname.to_string();
        let proto_global_names = Rc::clone(&proto.global_names);
        let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
        let class_docstring = proto.docstring.clone();
        let class_kwarg_names = proto.class_kwarg_names.clone();

        // Run the class body, collecting the attrs dict it stores.
        let (mut attrs, class_env_rc) = self.run_class_body(
            &class_code,
            &local_index,
            &proto_qualname,
            proto_global_names,
            proto_nonlocal_names,
        )?;

        // Adjust the attrs dict per CPython's type_new rules, returning the
        // resolved __qualname__.
        let qualname =
            make_class_finalize_attrs(&mut attrs, proto_qualname, class_docstring.as_deref())?;

        // Resolve and validate the base classes.
        let (base, extra_bases_vec) =
            self.make_class_resolve_bases(regs, num_locals, bases_base, bases_n)?;

        let slots = make_class_extract_slots(&mut attrs, &class_name)?;
        let class = Rc::new(RefCell::new(PyClass {
            extra_bases: extra_bases_vec.clone(),
            slots,
            ..PyClass::new(class_name, qualname, base.clone(), attrs)
        }));
        class_mro_items(&class).map(|_| ())?;

        // Register as a subclass of every base, and seed the __class__ cell.
        if let Some(ref b) = base {
            b.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        }
        for eb in &extra_bases_vec {
            eb.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        }
        class_env_rc
            .borrow_mut()
            .values
            .insert("__class__", Value::py_class(Rc::clone(&class)));

        // PEP 487 hooks: __set_name__ on every descriptor, then
        // __init_subclass__ on the base.
        self.make_class_call_set_name(&class)?;
        self.make_class_call_init_subclass(&class, regs, kwarg_base, &class_kwarg_names)?;
        Ok(Value::py_class(class))
    }

    /// Construct a class through an explicit `metaclass=` (issue #2128/#2130).
    ///
    /// Mirrors CPython's `class` statement protocol for the metaclass path:
    ///   1. `ns = metaclass.__prepare__(name, bases, **kwds)`,
    ///   2. run the class body, collecting its assignments,
    ///   3. populate `ns` (`__module__`, `__qualname__`, then the body's
    ///      assignments in order) — going through `__setitem__` so a custom
    ///      recording mapping observes them,
    ///   4. `metaclass(name, bases_tuple, ns, **kwds)`.
    ///
    /// All class-creation hooks (`__set_name__`, `__init_subclass__`) then run
    /// once inside the metaclass call (`type.__new__` → `build_class_via_type`),
    /// not in this method.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_make_class_meta(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        proto_idx: u8,
        bases_base: crate::bytecode::Reg,
        bases_n: u8,
        name_idx: u16,
        kwarg_base: crate::bytecode::Reg,
        kwarg_n: u8,
        meta_reg: crate::bytecode::Reg,
    ) -> Result<Value> {
        let class_name = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {} out of range (pool size {})",
                    name_idx,
                    code.names.len()
                ))
            })?
            .clone();
        let proto = code.fn_protos.get(proto_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: fn_proto index {} out of range (pool size {})",
                proto_idx,
                code.fn_protos.len()
            ))
        })?;
        let class_code = Rc::clone(&proto.code);
        let local_index = Rc::clone(&proto.local_index);
        // Classes (one per `def`) take an owned `String`; the `Rc<str>` sharing
        // of #2256 targets the many-closures-per-def case, not class objects.
        let proto_qualname = proto.qualname.to_string();
        let proto_global_names = Rc::clone(&proto.global_names);
        let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
        let class_kwarg_names = proto.class_kwarg_names.clone();

        let metaclass = vm_read(regs, meta_reg, num_locals)?;

        // Build the bases tuple (used for __prepare__ and the metaclass call).
        let mut bases_vec: Vec<Value> = Vec::with_capacity(bases_n as usize);
        for i in 0..bases_n as usize {
            let reg = (bases_base as usize + i) as crate::bytecode::Reg;
            bases_vec.push(vm_read(regs, reg, num_locals)?);
        }
        let bases_tuple = Value::tuple(bases_vec);

        // Assemble the class keyword arguments (everything except `metaclass`,
        // which is not part of `keywords`).  Forwarded to both __prepare__ and
        // the metaclass call.
        let kwargs: ExpandedArgBuf = class_kwarg_names
            .iter()
            .take(kwarg_n as usize)
            .enumerate()
            .map(|(i, key)| {
                let reg = (kwarg_base as usize + i) as crate::bytecode::Reg;
                ExpandedCallArg {
                    name: Some(key.clone()),
                    value: regs[reg as usize].clone(),
                }
            })
            .collect();

        // 1. ns = metaclass.__prepare__(name, bases, **kwds).  __prepare__ is a
        //    classmethod resolved via the metaclass; `type.__prepare__` returns
        //    a fresh dict, so a metaclass without an override still gets a dict.
        //    A non-class callable metaclass (e.g. a plain function) has no
        //    `__prepare__` attribute at all — CPython falls back to a plain dict
        //    in that case (PEP 3115), so swallow the AttributeError here.
        let namespace = match self.get_attr(&metaclass, "__prepare__") {
            Ok(prepare_fn) => {
                let mut prepare_args: ExpandedArgBuf = smallvec![
                    ExpandedCallArg { name: None, value: Value::string(class_name.clone()) },
                    ExpandedCallArg { name: None, value: bases_tuple.clone() },
                ];
                prepare_args.extend(kwargs.iter().cloned());
                self.call_function_expanded(prepare_fn, &prepare_args)?
            }
            Err(ref e) if e.class_name_is("AttributeError") => {
                Value::dict(PyDict::default())
            }
            Err(e) => return Err(e),
        };

        // 2. Run the class body and collect its ordered assignments.
        let (attrs, class_env_rc) = self.run_class_body(
            &class_code,
            &local_index,
            &proto_qualname,
            proto_global_names,
            proto_nonlocal_names,
        )?;

        // 3. Populate the namespace in CPython's order: `__module__` first,
        //    `__qualname__` second (the latter is intercepted out of the body
        //    attrs by run_class_body), then the body's assignments in the order
        //    they ran (skipping the `__module__` already emitted).  Set via a
        //    generic setitem so a custom mapping's __setitem__ records them.
        if let Some(module_val) = attrs.get("__module__") {
            self.namespace_set_item(&namespace, "__module__", module_val.clone())?;
        }
        self.namespace_set_item(&namespace, "__qualname__", Value::string(proto_qualname))?;
        for (k, v) in &attrs {
            if k == "__module__" {
                continue;
            }
            self.namespace_set_item(&namespace, k, v.clone())?;
        }

        // 4. metaclass(name, bases_tuple, namespace, **kwds).
        let mut call_args: ExpandedArgBuf = smallvec![
            ExpandedCallArg { name: None, value: Value::string(class_name) },
            ExpandedCallArg { name: None, value: bases_tuple },
            ExpandedCallArg { name: None, value: namespace },
        ];
        call_args.extend(kwargs);
        let result = self.call_function_expanded(metaclass, &call_args)?;

        // Seed the `__class__` cell the body's methods closed over so that
        // zero-arg `super()` inside them resolves to the *final* class the
        // metaclass produced (mirrors the `MakeClass` path).  `class_env_rc` is
        // the env captured by every method defined in the body.
        class_env_rc
            .borrow_mut()
            .values
            .insert("__class__", result.clone());
        Ok(result)
    }

    /// Set `ns[key] = value` on a class-body namespace mapping returned by
    /// `__prepare__`.  Fast-paths a plain `dict`; otherwise dispatches to the
    /// mapping's `__setitem__` so a custom recording namespace observes the
    /// assignment (issue #2128).
    fn namespace_set_item(&mut self, ns: &Value, key: &str, value: Value) -> Result<()> {
        if ns.as_dict().is_some() {
            ns.dict_insert(PyKey::str_from(key), value)?;
            return Ok(());
        }
        let ValueKind::PyInstance(inst) = ns.kind() else {
            // BuiltinObject mappings without a user __setitem__ fall back to a
            // direct dict_insert when they expose one; otherwise this is a
            // namespace type we do not support — report it like CPython.
            if ns.dict_insert(PyKey::str_from(key), value.clone()).is_ok() {
                return Ok(());
            }
            let tname = pyrust_core::builtin_type_name(ns);
            return Err(pyrust_core::type_err!(
                "'{tname}' object does not support item assignment"
            ));
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_val) = lookup_class_attr(&class, "__setitem__") else {
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "'{class_name}' object does not support item assignment"
            ));
        };
        invoke_class_method(
            self,
            method_val,
            ns.clone(),
            &[
                ExpandedCallArg { name: None, value: Value::string(key) },
                ExpandedCallArg { name: None, value },
            ],
        )?;
        Ok(())
    }

    /// Run a class body and return its assembled attrs dict plus the class env
    /// (so the caller can seed `__class__` after the class object exists).
    fn run_class_body(
        &mut self,
        class_code: &Rc<crate::bytecode::FnCode>,
        local_index: &Rc<HashMap<String, crate::bytecode::Reg>>,
        proto_qualname: &str,
        proto_global_names: Rc<HashSet<String>>,
        proto_nonlocal_names: Rc<HashSet<String>>,
    ) -> Result<(IndexMap<String, Value>, EnvRef)> {
        let num_class_regs = class_code.num_regs as usize;
        let mut class_regs: RegsBuf = smallvec![Value::unset(); num_class_regs];
        // CPython pre-injects __qualname__ / __module__ / __annotations__ into
        // the class namespace before the body runs; seed those register slots.
        let qualname_slot = local_index.get("__qualname__").copied();
        let module_slot = local_index.get("__module__").copied();
        let annotations_slot = local_index.get("__annotations__").copied();
        seed_class_reg(&mut class_regs, qualname_slot, || {
            Value::string(proto_qualname)
        });
        seed_class_reg(&mut class_regs, module_slot, || {
            Value::string("__main__")
        });
        seed_class_reg(&mut class_regs, annotations_slot, || {
            Value::dict(PyDict::default())
        });
        // __module__ and __annotations__ always flow into the attrs dict;
        // __qualname__ is intercepted in get_attr (issue #553) so it is not
        // pre-ordered here.
        let mut pre_order: Vec<crate::bytecode::Reg> = Vec::new();
        pre_order.extend(module_slot);
        pre_order.extend(annotations_slot);
        self.class_store_order.push(pre_order);

        // Push a class env so methods capture __class__ (zero-arg super), and a
        // Class frame view so locals() inside the body sees the namespace.
        let class_env = self.alloc_env(Some(Rc::clone(&self.env)));
        {
            let mut e = class_env.borrow_mut();
            e.global_names = proto_global_names;
            e.nonlocal_names = proto_nonlocal_names;
        }
        let class_env_rc = Rc::clone(&class_env);
        let previous_env = std::mem::replace(&mut self.env, class_env);
        let class_regs_ptr =
            unsafe { std::ptr::NonNull::new_unchecked(class_regs.as_mut_ptr()) };
        let class_regs_len = class_regs.len();
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Class,
            // SAFETY: SmallVec/Vec allocation is always non-null.  `class_regs`
            // lives on this stack frame for the full duration of `run_bytecode`;
            // the view is popped before `class_regs` is dropped.
            regs_ptr: class_regs_ptr,
            regs_len: class_regs_len,
            local_index: Rc::clone(local_index),
            nonlocal_names: None,
            env: None,
            is_class_method: false,
            function: None,
        });
        // SAFETY: class_regs_ptr is valid for class_regs_len Values for the
        // lifetime of class_regs.  No &mut [Value] referencing class_regs is
        // held while the dispatch loop runs (issue #547, PR #646).
        let class_regs_slice =
            unsafe { RegSlice::from_raw(class_regs_ptr.as_ptr(), class_regs_len) };
        let body_result = self.run_bytecode(class_code, class_regs_slice);
        // Always pop both stacks, even on error, to keep them balanced.
        self.vm_frame_views.pop();
        self.env = previous_env;
        self.free_env(class_env_rc.clone());
        let store_order = self
            .class_store_order
            .pop()
            .expect("class_store_order stack popped to empty");
        body_result?;

        let attrs = collect_class_attrs(local_index, &class_regs, store_order, num_class_regs);
        Ok((attrs, class_env_rc))
    }

    /// Resolve `R[bases_base .. bases_base+bases_n]` into (primary, extras),
    /// rejecting non-class and non-subclassable bases, and incompatible layouts.
    fn make_class_resolve_bases(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        bases_base: crate::bytecode::Reg,
        bases_n: u8,
    ) -> Result<ResolvedBases> {
        let mut classes: Vec<Rc<RefCell<PyClass>>> = Vec::with_capacity(bases_n as usize);
        for i in 0..bases_n as usize {
            let reg = (bases_base as usize + i) as crate::bytecode::Reg;
            let base_val = vm_read(regs, reg, num_locals)?;
            // A generic alias used as a base (`class Sub[T](Base[T])` or
            // `class L(list[int])`) contributes its `__origin__` to the MRO,
            // matching CPython's `__mro_entries__` protocol.
            let base_val = pyrust_builtins::generic_alias::as_generic_alias_origin(&base_val)
                .unwrap_or(base_val);
            let ValueKind::PyClass(c) = base_val.kind() else {
                return Err(PyError::Runtime("class base must be a class".to_string()));
            };
            let cls = Rc::clone(c);
            if let Some(tname) = crate::interpreter::non_subclassable_builtin_name(&cls) {
                return Err(pyrust_core::type_err!("type '{tname}' is not an acceptable base type"));
            }
            classes.push(cls);
        }
        // CPython's `best_base` layout check (issue #1677 for C types,
        // issue #2109 for user `__slots__`): two bases whose instance layouts
        // (solid bases) are unrelated cannot be combined.
        if crate::interpreter::bases_have_layout_conflict(&classes) {
            return Err(pyrust_core::type_err!("multiple bases have instance lay-out conflict"));
        }
        let mut iter = classes.into_iter();
        let base = iter.next();
        Ok((base, iter.collect()))
    }

    /// PEP 487 / CPython type_new_set_names: call `__set_name__(cls, name)` on
    /// every namespace value whose *type* defines it.
    fn make_class_call_set_name(&mut self, class: &Rc<RefCell<PyClass>>) -> Result<()> {
        let cls_val = Value::py_class(Rc::clone(class));
        let attrs_snapshot: Vec<(String, Value)> = class
            .borrow()
            .attrs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (attr_name, attr_val) in &attrs_snapshot {
            let ValueKind::PyInstance(inst) = attr_val.kind() else {
                // Issue #1846: `property` is a built-in descriptor (BuiltinObject,
                // not PyInstance) with a native __set_name__ that records the
                // attribute name so its __set__/__delete__ errors can name it.
                pyrust_builtins::property::set_property_name(attr_val, attr_name);
                continue;
            };
            let inst_class = Rc::clone(&inst.borrow().class);
            let Some(set_name_fn) = lookup_class_attr(&inst_class, "__set_name__") else {
                continue;
            };
            invoke_class_method(
                self,
                set_name_fn,
                attr_val.clone(),
                &[
                    ExpandedCallArg { name: None, value: cls_val.clone() },
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(attr_name.clone()),
                    },
                ],
            )?;
        }
        Ok(())
    }

    /// PEP 487 / CPython type_new_init_subclass: call `base.__init_subclass__`
    /// with the class keyword arguments after the class is fully constructed.
    fn make_class_call_init_subclass(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        regs: &RegSlice,
        kwarg_base: crate::bytecode::Reg,
        class_kwarg_names: &[String],
    ) -> Result<()> {
        let kwarg_args: ExpandedArgBuf = class_kwarg_names
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let reg = (kwarg_base as usize + i) as crate::bytecode::Reg;
                ExpandedCallArg {
                    name: Some(key.clone()),
                    value: regs[reg as usize].clone(),
                }
            })
            .collect();
        self.make_class_call_init_subclass_with_kwargs(class, &kwarg_args)
    }

    /// Core of PEP 487 `__init_subclass__` dispatch shared by the `class`
    /// statement (`make_class_call_init_subclass`) and the `type()` /
    /// `type.__new__` constructor path: looks up `base.__init_subclass__` and
    /// invokes it with the already-assembled keyword arguments.
    fn make_class_call_init_subclass_with_kwargs(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        kwarg_args: &[ExpandedCallArg],
    ) -> Result<()> {
        let lookup_base = class
            .borrow()
            .base
            .clone()
            .unwrap_or_else(object_class_singleton);
        let Some(method_val) = lookup_class_attr(&lookup_base, "__init_subclass__") else {
            return Ok(());
        };
        let new_cls = Value::py_class(Rc::clone(class));
        invoke_class_method(self, method_val, new_cls, kwarg_args)?;
        Ok(())
    }

    /// Shared class-construction finalization used by both the `type()` and
    /// `type.__new__` constructor builtins.  Mirrors the `class` statement's
    /// post-body path (`exec_make_class`): adjust the namespace per CPython's
    /// `type_new` rules (__module__/__doc__/__hash__/__qualname__), process
    /// `__slots__`, build the `PyClass`, register it as a subclass of every
    /// base, run `__set_name__` on namespace descriptors, then call the base's
    /// `__init_subclass__`.  This gives `type(...)`-created classes the same
    /// hooks as a `class` statement (issues #2129 / #2130).
    pub(crate) fn build_class_via_type(
        &mut self,
        name: String,
        base: Option<Rc<RefCell<PyClass>>>,
        extra_bases: Vec<Rc<RefCell<PyClass>>>,
        mut attrs: IndexMap<String, Value>,
        metatype: Option<Rc<RefCell<PyClass>>>,
        init_subclass_kwargs: &[ExpandedCallArg],
    ) -> Result<Value> {
        let class_docstring = attrs.get("__doc__").and_then(|v| match v.kind() {
            ValueKind::Str(s) => Some(s.to_string()),
            _ => None,
        });
        let qualname =
            make_class_finalize_attrs(&mut attrs, name.clone(), class_docstring.as_deref())?;
        let slots = make_class_extract_slots(&mut attrs, &name)?;
        let class = Rc::new(RefCell::new(PyClass {
            extra_bases: extra_bases.clone(),
            slots,
            metatype,
            ..PyClass::new(name, qualname, base.clone(), attrs)
        }));
        class_mro_items(&class).map(|_| ())?;
        if let Some(ref b) = base {
            b.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        }
        for eb in &extra_bases {
            eb.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        }
        self.make_class_call_set_name(&class)?;
        self.make_class_call_init_subclass_with_kwargs(&class, init_subclass_kwargs)?;
        Ok(Value::py_class(class))
    }
}

/// Seed a class-body register slot with a value if the slot is allocated and
/// in range.  Used by `run_class_body` to pre-inject __qualname__/__module__/
/// __annotations__ before the body runs.
fn seed_class_reg(regs: &mut RegsBuf, slot: Option<crate::bytecode::Reg>, value: impl FnOnce() -> Value) {
    if let Some(slot) = slot {
        let slot = slot as usize;
        if slot < regs.len() {
            regs[slot] = value();
        }
    }
}

/// Build the class attrs dict from the body's fastlocal registers, in the
/// runtime store order recorded by RecordClassStore.
fn collect_class_attrs(
    local_index: &HashMap<String, crate::bytecode::Reg>,
    class_regs: &RegsBuf,
    store_order: Vec<crate::bytecode::Reg>,
    num_class_regs: usize,
) -> IndexMap<String, Value> {
    let mut slot_to_name: Vec<Option<&String>> = vec![None; num_class_regs];
    for (name, &slot) in local_index.iter() {
        if (slot as usize) < slot_to_name.len() {
            slot_to_name[slot as usize] = Some(name);
        }
    }
    let mut attrs = IndexMap::new();
    for slot in store_order {
        let Some(name) = slot_to_name.get(slot as usize).and_then(|n| *n) else {
            continue;
        };
        if let Some(v) = class_regs.get(slot as usize)
            && !v.is_unset()
        {
            attrs.insert(name.clone(), v.clone());
        }
    }
    attrs
}

/// Apply CPython's type_new attrs adjustments and return the resolved
/// __qualname__: wrap a bare __init_subclass__ as a classmethod, pop and
/// validate __qualname__, and seed __module__/__doc__/__hash__/__dict__/
/// __weakref__.
fn make_class_finalize_attrs(
    attrs: &mut IndexMap<String, Value>,
    proto_qualname: String,
    class_docstring: Option<&str>,
) -> Result<String> {
    // A bare __init_subclass__ defined in the body is implicitly a classmethod
    // (issue #1047) so super().__init_subclass__() binds cls correctly.
    let isc_wrapped = attrs.get("__init_subclass__").and_then(|v| {
        if let ValueKind::UserFunction(f) = v.kind()
            && f.kind == pyrust_core::UserFunctionKind::Regular
        {
            Some(Value::class_method(Rc::clone(f)))
        } else {
            None
        }
    });
    if let Some(wrapped) = isc_wrapped {
        attrs.insert("__init_subclass__".to_string(), wrapped);
    }
    // __qualname__ lives on `type` as a descriptor, not in the attrs dict, so
    // pop it; an explicit non-str assignment is a TypeError (issue #553).
    let qualname = match attrs.shift_remove("__qualname__") {
        None => proto_qualname,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                let tname = pyrust_core::builtin_type_name(&v).into_owned();
                return Err(pyrust_core::type_err!("type __qualname__ must be a str, not {tname}"));
            }
        },
    };
    attrs
        .entry("__module__".to_string())
        .or_insert_with(|| Value::string("__main__"));
    attrs.entry("__doc__".to_string()).or_insert_with(|| {
        class_docstring
            .map(Value::string)
            .unwrap_or_else(Value::none)
    });
    // A class defining __eq__ but not __hash__ is unhashable (CPython rule).
    if attrs.contains_key("__eq__") && !attrs.contains_key("__hash__") {
        attrs.insert("__hash__".to_string(), Value::none());
    }
    attrs.entry("__dict__".to_string()).or_insert_with(Value::none);
    attrs
        .entry("__weakref__".to_string())
        .or_insert_with(Value::none);
    Ok(qualname)
}

/// Extract the declared `__slots__` names (string / tuple / list of strings)
/// from the attrs dict.  Returns `None` when no `__slots__` is declared (the
/// instance gets a full __dict__); `Some(set)` restricts instance attributes.
/// When __slots__ is present without a `'__dict__'` slot, the `__dict__` /
/// `__weakref__` class entries are removed so slotted instances have no
/// per-instance dict (CPython parity).
///
/// For each declared slot name (other than `__dict__` / `__weakref__`) this
/// also installs a `member_descriptor` data descriptor in the class namespace
/// (issue #2084), so `S.x` is a `member_descriptor`, `'x' in S.__dict__` and
/// `'x' in dir(S)`.  The descriptor stores into / reads from the instance's
/// slot storage and is the mechanism that lets a slotted instance hold
/// attributes without a per-instance `__dict__` (issue #2076).
fn make_class_extract_slots(
    attrs: &mut IndexMap<String, Value>,
    class_name: &str,
) -> Result<Option<indexmap::IndexSet<String>>> {
    let Some(slots_val) = attrs.get("__slots__") else {
        return Ok(None);
    };
    let collect = |items: &[Value]| -> Vec<String> {
        items
            .iter()
            .filter_map(|v| match v.kind() {
                ValueKind::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect()
    };
    let slot_names: Vec<String> = match slots_val.kind() {
        ValueKind::Str(s) => vec![s.to_string()],
        ValueKind::Tuple(items) => collect(items),
        ValueKind::List(items) => collect(&items),
        _ => vec![],
    };
    let set: indexmap::IndexSet<String> = slot_names.into_iter().collect();
    // Issue #1971: a slot name that also has a class-variable assignment in
    // the class body is an error (CPython raises ValueError at type creation).
    // `__dict__` / `__weakref__` are handled specially by CPython before the
    // conflict loop, so they are exempt.  The `__dict__` sentinel below is
    // inserted only after this check so it never counts as a class variable.
    for slot in &set {
        if slot == "__dict__" || slot == "__weakref__" {
            continue;
        }
        if attrs.contains_key(slot) {
            return Err(pyrust_core::value_err!(
                "'{slot}' in __slots__ conflicts with class variable"
            ));
        }
    }
    // Install a member_descriptor per slot (issue #2084).  CPython turns each
    // slot name into a data descriptor on the class; `__dict__` / `__weakref__`
    // are special-cased by CPython (they enable per-instance dict / weakref
    // rather than allocating a member slot), so they don't get descriptors.
    for slot in &set {
        if slot == "__dict__" || slot == "__weakref__" {
            continue;
        }
        attrs.insert(
            slot.clone(),
            pyrust_builtins::member_descriptor::member_descriptor(slot, class_name),
        );
    }
    // An all-slots class (no `'__dict__'` slot) has neither a `__dict__` nor a
    // `__weakref__` entry in its class namespace (issue #2076): CPython only
    // adds those getset_descriptors when the layout actually carries a per-
    // instance dict / weakref.  `make_class_finalize_attrs` inserts them
    // unconditionally for the common case, so strip them back out here.  When
    // `'__dict__'` IS a declared slot, keep the `__dict__` entry so
    // `'__dict__' in S.__dict__` stays True.
    if !set.contains("__dict__") {
        attrs.shift_remove("__dict__");
        attrs.shift_remove("__weakref__");
    }
    Ok(Some(set))
}

impl Interpreter {
    /// Tag for a receiver that the `CallMethod*` opcodes dispatch through the
    /// unified builtin-container path (`dispatch_builtin_container_method`).
    ///
    /// `1=List 2=Dict 3=Tuple 4=Str 5=Set`, `0` = anything else (PyInstance,
    /// generator, BuiltinObject, …) which is handled by each opcode's own
    /// fall-through arm (those arms differ: only the no-kwargs opcode carries
    /// the attr inline cache + #976 backing-primitive fast path).
    #[inline(always)]
    fn builtin_container_tag(v: Option<&Value>) -> u8 {
        v.map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            ValueKind::Set(_) => 5u8,
            _ => 0u8,
        })
        .unwrap_or(0)
    }

    /// Shared dispatch body for method calls on the five tagged builtin
    /// container types (`list` / `dict` / `tuple` / `str` / `set`).  Both
    /// `Insn::CallMethod` (no kwargs) and `Insn::CallMethodExpanded`
    /// (kwargs / unpacking) route here once they have materialised the
    /// receiver, positional args, and keyword map — so the dispatch decision
    /// lives in exactly one place (#431).
    ///
    /// Caller guarantees `obj_kind_tag` is `1..=5` and `receiver`'s kind
    /// matches the tag.  `__iter__` is handled by the callers before this
    /// point (it needs no kwargs and is identical on both opcodes).
    ///
    /// The "needs interpreter" vs "needs Rc backing" vs "pure builtin"
    /// decision is predicate-driven via the `pyrust-builtins::<type>` modules
    /// (`list::requires_interpreter`, `dict::needs_rc`,
    /// `string::requires_vm_template`) — the single source of truth documented
    /// in `crates/pyrust-builtins/README.md`.
    fn dispatch_builtin_container_method(
        &mut self,
        obj_kind_tag: u8,
        receiver: Value,
        method: &str,
        mut pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        // Issue #2151: object-protocol methods (`__sizeof__`/`__dir__`/
        // `__reduce__`/`__reduce_ex__`) called directly via the method-call
        // opcode on a container.  Handled here (the per-type `call` below would
        // leak a RuntimeError) so `[1].__dir__()` returns a list.
        if method.starts_with("__") {
            if crate::interpreter::is_object_protocol_method(&receiver, method) {
                if !kw.is_empty() {
                    return Err(pyrust_core::type_err!("{}() takes no keyword arguments", method));
                }
                return Ok(self.object_protocol_method_result(method, &receiver));
            }
            // Issue #2191: `__format__` called directly via the method-call
            // opcode on a tagged container (`"hi".__format__('>5')`,
            // `[1,2].__format__('')`).  Route through `apply_format_spec` so the
            // result matches `format(receiver, spec)` — otherwise the per-type
            // `call` below leaks a RuntimeError (`'str' object has no attribute
            // '__format__'`).  Gated under the `__` prefix so the common
            // method-name path (`.append`, `.upper`, …) is untouched.
            if method == "__format__" {
                if !kw.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}.__format__() takes no keyword arguments",
                        format_dunder_owner(&receiver)
                    ));
                }
                let spec = format_dunder_spec_arg(&receiver, &pos)?;
                return apply_format_spec(&receiver, spec);
            }
        }
        // Issue #1909: container/sequence protocol dunders called directly via
        // the `obj.__getitem__(i)` method-call opcode (not through the
        // bound-method value).  Route through the shared dispatcher so the
        // result matches the operator behaviour.  `__iter__` is handled by the
        // callers before this point.
        if is_container_protocol_dunder_name(method) {
            let type_name = pyrust_core::builtin_type_name(&receiver);
            if builtin_protocol_dunders(&type_name).contains(&method) {
                if !kw.is_empty() {
                    return Err(pyrust_core::type_err!("{}() takes no keyword arguments", method));
                }
                return self.dispatch_builtin_protocol_dunder(method, receiver, pos);
            }
            // Issue #2070: `__hash__` on an unhashable built-in (list/dict/set/
            // bytearray) resolves to `None` in CPython, so `[1].__hash__()`
            // raises `'NoneType' object is not callable` — not `AttributeError`.
            if method == "__hash__"
                && matches!(&*type_name, "list" | "dict" | "set" | "bytearray")
            {
                return Err(pyrust_core::type_err!("'NoneType' object is not callable"));
            }
            // A protocol-dunder name that is valid for *other* types but not
            // this one (e.g. `set().__getitem__`, `"".__setitem__`) is
            // genuinely absent: raise a proper `AttributeError` rather than
            // letting the per-type `call` leak a `RuntimeError` (issue #1909).
            return Err(PyError::attribute_error(
                format!("'{type_name}' object has no attribute '{method}'"),
                Some(method.to_string()),
                Some(receiver),
            ));
        }
        match obj_kind_tag {
            1 => {
                // `list.insert(i, x)` / `list.pop([i])` accept any `__index__`
                // object as the index (CPython 3.12).  The receiver-only
                // `pyrust_builtins::list::call` cannot dispatch user dunders, so
                // resolve the index through the shared protocol here (#2022)
                // before delegating.
                if (method == "insert" || method == "pop") && !pos.is_empty() {
                    let idx = self.value_to_index(&pos[0], |v| {
                        pyrust_core::type_err!("'{}' object cannot be interpreted as an integer",
                                pyrust_core::builtin_type_name(v))
                    })?;
                    pos[0] = idx;
                }
                if pyrust_builtins::list::requires_interpreter(method) {
                    // `index` / `count` may fire user `__eq__`; `sort(key=)`
                    // runs a user callable.  The interpreter-free `call` cannot
                    // reach user code, so take the interpreter-aware path.
                    if method == "sort" {
                        return self.list_sort_with_kwargs(&receiver, pos, kw);
                    }
                    if method == "remove" {
                        return self.call_seq_remove(&receiver, pos);
                    }
                    // index / count: peek to decide whether values_user_eq
                    // dispatch is needed (resolve_seq_index_pos only touches
                    // pos[1..], so pos[0] (the target) is stable).
                    let needs_dispatch = pos
                        .first()
                        .map(|t| {
                            receiver
                                .list_with(|items| Self::seq_search_needs_dispatch(t, items))
                                .unwrap_or(true)
                        })
                        .unwrap_or(false);
                    let pos = if method == "index" {
                        self.resolve_seq_index_pos(pos)?
                    } else {
                        pos
                    };
                    if needs_dispatch {
                        let snapshot: Vec<Value> =
                            receiver.list_with(|items| items.to_vec()).ok_or_else(|| {
                                pyrust_core::type_err!("list.index receiver is not a list")
                            })?;
                        if method == "index" {
                            self.call_seq_index(snapshot, &pos, "list")
                        } else {
                            self.call_seq_count(snapshot, &pos, "list")
                        }
                    } else {
                        pyrust_builtins::list::call(method, &receiver, pos, kw)
                    }
                } else {
                    pyrust_builtins::list::call(method, &receiver, pos, kw)
                }
            }
            2 => {
                if pyrust_builtins::dict::needs_rc(method) {
                    // Lazy views need the Rc to share storage with the source
                    // dict — the regular dispatch path only sees a Vec snapshot.
                    let rc = receiver
                        .get_dict_rc()
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?
                        .clone();
                    return match method {
                        "keys" => Ok(pyrust_builtins::dict_views::dict_keys(rc)),
                        "values" => Ok(pyrust_builtins::dict_views::dict_values(rc)),
                        "items" => Ok(pyrust_builtins::dict_views::dict_items(rc)),
                        _ => unreachable!(),
                    };
                }
                self.call_dict_method(method, receiver, pos, kw)
            }
            3 => {
                // Snapshot the tuple's items once so the `&[Value]` borrow does
                // not straddle the `&mut self` calls below.  Tuples are
                // immutable, so the snapshot is exact.
                let items: Vec<Value> = match receiver.kind() {
                    ValueKind::Tuple(items) => items.to_vec(),
                    _ => return Err(PyError::Runtime("internal: expected tuple".to_string())),
                };
                if pyrust_builtins::tuple::requires_interpreter(method) {
                    let needs_dispatch = pos
                        .first()
                        .map(|t| Self::seq_search_needs_dispatch(t, &items))
                        .unwrap_or(false);
                    let pos = if method == "index" {
                        self.resolve_seq_index_pos(pos)?
                    } else {
                        pos
                    };
                    if needs_dispatch {
                        if method == "index" {
                            self.call_seq_index(items, &pos, "tuple")
                        } else {
                            self.call_seq_count(items, &pos, "tuple")
                        }
                    } else {
                        pyrust_builtins::tuple::call(method, &items, pos)
                    }
                } else {
                    pyrust_builtins::tuple::call(method, &items, pos)
                }
            }
            4 => {
                if pyrust_builtins::string::requires_vm_template(method) {
                    return self.str_template_method(&receiver, method, pos, kw);
                }
                if kw.is_empty() {
                    self.call_str_method(method, receiver, pos)
                } else {
                    let mut pos = pos;
                    str_merge_kwargs(method, &mut pos, kw.clone())?;
                    self.call_str_method(method, receiver, pos)
                }
            }
            5 => self.call_set_method(method, receiver, pos),
            _ => Err(PyError::Runtime(
                "dispatch_builtin_container_method called with non-container tag".to_string(),
            )),
        }
    }

    /// Compute the result of an object-protocol method call (#2151) on a
    /// built-in data value whose receiver is already bound.  Single source of
    /// truth shared by the bound-method dispatch and the container method-call
    /// fast path; mirrors the `object.__sizeof__`/`__dir__`/`__reduce*` and
    /// `NoneType.__bool__` registry handlers.  `method` must satisfy
    /// `is_object_protocol_method(receiver, method)`.
    fn object_protocol_method_result(&self, method: &str, receiver: &Value) -> Value {
        match method {
            // Implementation-specific size; tests assert the int type only.
            "__sizeof__" => Value::int(std::mem::size_of::<Value>() as i64),
            "__dir__" => {
                let mut names = dir_names(receiver);
                names.sort();
                names.dedup();
                Value::list(names.into_iter().map(Value::string).collect())
            }
            // A tuple of the correct shape (`(class, ())`); pyrust does not
            // model copyreg, so the exact pickle reduction is not reproduced.
            "__reduce__" | "__reduce_ex__" => Value::tuple(vec![
                crate::builtin_modules::builtins::value_class(receiver),
                Value::tuple(Vec::new()),
            ]),
            "__bool__" => Value::bool_(false),
            // `__getstate__()` returns None for objects with no instance state.
            "__getstate__" => Value::none(),
            _ => unreachable!("is_object_protocol_method guard"),
        }
    }

    /// `list.sort(...)` with possible `key=` / `reverse=` kwargs.  Both opcodes
    /// share this (#431).  The no-kwargs opcode passes an empty `kw`, reducing
    /// this to a plain `pyrust_builtins::list::call("sort", …)`.
    fn list_sort_with_kwargs(
        &mut self,
        receiver: &Value,
        pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        // `list.sort` is keyword-only in CPython 3.12 — `sort(*, key=None,
        // reverse=False)`.  Any positional arg is a TypeError, and the list is
        // left unchanged (#1949).  Enforced centrally so every dispatch site
        // (bound-method, subclass, …) inherits the rejection.
        if !pos.is_empty() {
            return Err(pyrust_core::type_err!("sort() takes no positional arguments"));
        }
        // StrKey probes (issue #506): zero-alloc borrowed-str lookup — no
        // PyKey::Str(Value) RC bump on every sort call.
        for k in kw.keys() {
            if let PyKey::Str(s) = k {
                let s = s.as_str().unwrap_or("");
                if s != "key" && s != "reverse" {
                    return Err(pyrust_core::type_err!("sort() got an unexpected keyword argument '{s}'"));
                }
            }
        }
        // An explicit `key=None` means "no key function" (default comparison),
        // mirroring `sorted`/`min`/`max` (#1937).
        let key_fn = kw.get(&StrKey("key")).cloned().filter(|v| !v.is_none());
        // CPython applies `bool(reverse)` to *any* object — a non-empty
        // list/str or an arbitrary object is truthy and reverses (issue #2126).
        // Compute it interpreter-side so a user `__bool__`/`__len__` is honoured,
        // matching `sorted()`.  This single computed flag is threaded into every
        // sort path below; the primitive fast path no longer re-parses `reverse`
        // (its receiver-only `extract_reverse` recognised only Bool/Int/Float).
        let reverse = match kw.get(&StrKey("reverse")) {
            Some(v) => self.truthy_value(&v.clone())?,
            None => false,
        };
        if let Some(key_fn_val) = key_fn {
            // Compute keys via the interpreter, then delegate sorting to builtins.
            let items_snapshot: Vec<Value> = receiver
                .list_with(|items| items.to_vec())
                .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
            let mut keys: Vec<Value> = Vec::with_capacity(items_snapshot.len());
            for item in &items_snapshot {
                let key_val = {
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    buf.push(ExpandedCallArg { name: None, value: item.clone() });
                    let r = self.call_function_expanded(key_fn_val.clone(), &buf);
                    self.call_arg_buf = buf;
                    r?
                };
                keys.push(key_val);
            }
            return pyrust_builtins::list::sort_with_precomputed_keys(receiver, keys, reverse);
        }
        // No key.  Pre-scan: if any element is a user instance, comparisons
        // may dispatch `__lt__` (and the reflected `__gt__`), which the
        // interpreter-free `pyrust_builtins::list::call("sort", …)` cannot
        // reach — it would raise a spurious TypeError (#1925).  Route those
        // through the interpreter-aware `richcmp_order`, exactly as `sorted()`
        // does.  All-primitive lists keep the in-crate fast sort: no perf
        // regression on the common int/str/float case.
        let has_instance = receiver
            .list_with(|items| items.iter().any(|v| matches!(v.kind(), ValueKind::PyInstance(_))))
            .unwrap_or(false);
        if !has_instance {
            // Primitive fast path: delegate to builtins with the already-resolved
            // `reverse` bool (issue #2126).  Going through `call("sort", …)` would
            // re-parse `reverse` via the receiver-only `extract_reverse`, which
            // recognises only Bool/Int/Float and silently drops a truthy
            // list/str/object — diverging from `sorted()` and CPython.
            return pyrust_builtins::list::sort_no_key(receiver, reverse);
        }
        // Snapshot the items so the comparator (which may re-enter the same
        // list via user `__lt__`) does not straddle the receiver's borrow.
        // Write the sorted result back inside a `list_with_mut` window, mirroring
        // `pyrust_builtins::list::sort_by_cmp`.
        let mut snapshot: Vec<Value> = receiver
            .list_with(|items| items.clone())
            .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
        let mut sort_err: Option<PyError> = None;
        snapshot.sort_by(|a, b| {
            if sort_err.is_some() {
                return std::cmp::Ordering::Equal;
            }
            let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
            match self.richcmp_order(lhs, rhs) {
                Ok(ord) => ord,
                Err(e) => {
                    sort_err = Some(e);
                    std::cmp::Ordering::Equal
                }
            }
        });
        if let Some(e) = sort_err {
            return Err(e);
        }
        receiver.list_with_mut(|items| *items = snapshot);
        Ok(Value::none())
    }

    /// `str.format` / `str.format_map` / `str.maketrans` — the templating
    /// methods that must run in the interpreter rather than
    /// `pyrust_builtins::string::call`.  Both opcodes share this (#431).
    /// Caller guarantees `string::requires_vm_template(method)`.
    fn str_template_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        match method {
            "format" => {
                let mut keyword: Vec<(String, Value)> = Vec::with_capacity(kw.len());
                for (k, v) in kw {
                    if let PyKey::Str(name) = k {
                        keyword.push((name.as_str().unwrap_or("").to_owned(), v.clone()));
                    }
                }
                let template = receiver
                    .as_str()
                    .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?;
                self.format_str_template(template, &pos, &keyword)
            }
            "format_map" => {
                if pos.len() != 1 || !kw.is_empty() {
                    return Err(pyrust_core::type_err!("str.format_map() takes exactly one argument ({} given)",
                            pos.len() + kw.len()));
                }
                let mapping = pos.into_iter().next().unwrap();
                let template = receiver
                    .as_str()
                    .ok_or_else(|| PyError::Runtime("internal: expected str".to_string()))?
                    .to_string();
                self.format_str_template_map(&template, mapping)
            }
            // `maketrans` is a staticmethod on str: the receiver is discarded
            // and the call is forwarded to str_maketrans exactly like
            // `str.maketrans(...)` would be.
            "maketrans" => pyrust_builtins::string::str_maketrans(&pos),
            _ => unreachable!("str_template_method called with non-template method"),
        }
    }
    /// Dispatch a method call on the backing primitive of a `list`/`dict`/`set`
    /// subclass instance (#976).  The cached method is a
    /// `BuiltinFunction("<type>.<method>")` and `backing` is its
    /// `__builtin_data__` value; route to the same per-type path the bare
    /// container would take.  `index`/`count` on a list backing may fire user
    /// `__eq__`, so they go through the interpreter-aware sequence helpers.
    fn dispatch_backing_primitive_method(
        &mut self,
        prim_type: &str,
        prim_method: &str,
        backing: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        // Issue #1909: container protocol dunders on a `list`/`dict`/`set`
        // *subclass* instance (`MyList().__len__()`) operate on the backing
        // primitive — route through the shared dispatcher so they match the
        // plain-primitive form rather than leaking a `RuntimeError` from the
        // per-type `call`.
        if prim_method.starts_with("__")
            && builtin_protocol_dunders(prim_type).contains(&prim_method)
        {
            return self.dispatch_builtin_protocol_dunder(prim_method, backing, args);
        }
        match prim_type {
            "list" => {
                if prim_method == "remove" {
                    self.call_seq_remove(&backing, args)
                } else if prim_method == "index" || prim_method == "count" {
                    let needs_dispatch = args
                        .first()
                        .map(|t| {
                            backing
                                .list_with(|items| Self::seq_search_needs_dispatch(t, items))
                                .unwrap_or(true)
                        })
                        .unwrap_or(false);
                    let args = if prim_method == "index" {
                        self.resolve_seq_index_pos(args)?
                    } else {
                        args
                    };
                    if needs_dispatch {
                        let snapshot = backing.list_with(|items| items.clone()).ok_or_else(|| {
                            pyrust_core::type_err!("list.index receiver is not a list")
                        })?;
                        if prim_method == "index" {
                            self.call_seq_index(snapshot, &args, "list")
                        } else {
                            self.call_seq_count(snapshot, &args, "list")
                        }
                    } else {
                        let empty_kw = PyDict::default();
                        pyrust_builtins::list::call(prim_method, &backing, args, &empty_kw)
                    }
                } else {
                    let empty_kw = PyDict::default();
                    pyrust_builtins::list::call(prim_method, &backing, args, &empty_kw)
                }
            }
            "dict" => {
                let empty_kw = PyDict::default();
                self.call_dict_method(prim_method, backing, args, &empty_kw)
            }
            "set" => self.call_set_method(prim_method, backing, args),
            _ => unreachable!("dispatch_backing_primitive_method: bad prim_type {prim_type}"),
        }
    }
    /// Fill or update the `CallMethod` inline cache after a slow-path dispatch.
    /// Mirrors the `GetAttr` cache policy: a different class at this site goes
    /// `Megamorphic`; a stale `ClassAttr` resets to `Empty` for a refill; an
    /// `Empty` slot caches the resolved unbound method when it is a cacheable
    /// Regular `UserFunction` / `BuiltinFunction` with no instance shadow and no
    /// user `__getattribute__` (#1254).
    fn update_call_method_cache(
        &self,
        obj_val: &Value,
        method: &str,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) {
        use crate::bytecode::AttrCacheEntry;
        use pyrust_core::UserFunctionKind;

        let Some(inst_rc) = obj_val.as_py_instance_rc() else {
            return;
        };
        let mut cache = code.attr_cache.borrow_mut();
        match &cache[call_site_pc] {
            AttrCacheEntry::Megamorphic => {}
            AttrCacheEntry::ClassAttr { class_ptr: existing_ptr, .. } => {
                let new_ptr = Rc::as_ptr(&inst_rc.borrow().class) as *const ();
                if new_ptr != *existing_ptr {
                    // Different class at this call site — go megamorphic.
                    cache[call_site_pc] = AttrCacheEntry::Megamorphic;
                } else {
                    // Same class but version changed (or instance shadow exists).
                    // Reset to Empty so the next slow-path execution refills
                    // with the current class version and updated method value.
                    cache[call_site_pc] = AttrCacheEntry::Empty;
                }
            }
            AttrCacheEntry::Empty => {
                let inst = inst_rc.borrow();
                if inst.attrs.contains_key(method) {
                    return;
                }
                // Don't cache when the class has a user-defined __getattribute__ —
                // the CallMethod cache fast path bypasses get_attr entirely,
                // skipping the dispatch (issue #1254, same as the GetAttr guard).
                let has_custom_getattribute = lookup_class_attr(&inst.class, "__getattribute__")
                    .is_some_and(|v| matches!(v.kind(), ValueKind::UserFunction(_)));
                if has_custom_getattribute {
                    return;
                }
                let Some(unbound_val) = lookup_class_attr(&inst.class, method) else {
                    return;
                };
                // Only cache Regular UserFunctions and BuiltinFunctions.
                // StaticMethod / ClassMethod need special receiver treatment —
                // let them always go through get_attr + call_function_expanded.
                let cacheable = match unbound_val.kind() {
                    ValueKind::UserFunction(f) => matches!(f.kind, UserFunctionKind::Regular),
                    ValueKind::BuiltinFunction(_) => true,
                    _ => false,
                };
                if cacheable {
                    cache[call_site_pc] = AttrCacheEntry::ClassAttr {
                        class_ptr: Rc::as_ptr(&inst.class) as *const (),
                        class_version: inst.class.borrow().mutation_version.get(),
                        epoch: pyrust_core::class_epoch(),
                        value: unbound_val,
                    };
                }
            }
            // InstanceAttr / SlotAttr / SetInstanceAttr are GetAttr / SetAttr-only
            // entries (#1912 / #2207 / #1998).  A CallMethod site never produces
            // them, but be defensive: drop to Empty so the next pass refills.
            AttrCacheEntry::InstanceAttr { .. }
            | AttrCacheEntry::SlotAttr { .. }
            | AttrCacheEntry::SetInstanceAttr { .. } => {
                cache[call_site_pc] = AttrCacheEntry::Empty;
            }
        }
    }
    /// Inline-cache fast path for `obj.method(...)` on a user-defined
    /// `PyInstance`.  Returns `Ok(Some(result))` on a cache hit (including the
    /// #976 backing-primitive route), `Ok(None)` on a miss so the caller takes
    /// the slow `get_attr` + `call_function_expanded` path.  Only Regular
    /// `UserFunction`s and `BuiltinFunction`s are cached; the guard checks
    /// class identity, instance-shadow, class version, and epoch.
    fn try_call_method_cached(
        &mut self,
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        method: &str,
        args: &[Value],
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Result<Option<Value>> {
        use crate::bytecode::AttrCacheEntry;

        enum CallMethodFast {
            Hit(Value),
            Miss,
        }
                let fast = {
                    let cache = code.attr_cache.borrow();
                    if let AttrCacheEntry::ClassAttr { class_ptr, class_version, epoch, value: unbound } =
                        &cache[call_site_pc]
                    {
                        if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                            let inst = inst_rc.borrow();
                            let same_class =
                                Rc::as_ptr(&inst.class) as *const () == *class_ptr;
                            let no_shadow = !inst.attrs.contains_key(method);
                            let version_ok = inst.class.borrow().mutation_version.get()
                                == *class_version;
                            let epoch_ok = pyrust_core::class_epoch() == *epoch;
                            if same_class && no_shadow && version_ok && epoch_ok {
                                let unbound = unbound.clone();
                                let inst_rc_clone = Rc::clone(inst_rc);
                                drop(inst);
                                // Issue #976: if the cached method is a primitive
                                // builtin (list.X / dict.X / set.X) and the instance
                                // has a __builtin_data__ backing value, dispatch
                                // directly to the backing primitive.
                                // invoke_class_method cannot handle BuiltinFunction
                                // method names — it looks them up in the top-level
                                // registry which has no "list.append" entry.
                                if let ValueKind::BuiltinFunction(fn_name) = unbound.kind()
                                    && let Some((prim_type, prim_method)) =
                                        fn_name.split_once('.')
                                        .filter(|(t, _)| matches!(*t, "dict" | "list" | "set"))
                                        && let Some(backing) = instance_builtin_data(&inst_rc_clone) {
                                            return self
                                                .dispatch_backing_primitive_method(
                                                    prim_type,
                                                    prim_method,
                                                    backing,
                                                    args.to_vec(),
                                                )
                                                .map(Some);
                                        }
                                let inst_val = Value::py_instance(inst_rc_clone);
                                let mut buf = std::mem::take(&mut self.call_arg_buf);
                                buf.clear();
                                for arg in args.iter() {
                                    buf.push(ExpandedCallArg { name: None, value: arg.clone() });
                                }
                                let r = invoke_class_method(self, unbound, inst_val, &buf);
                                self.call_arg_buf = buf;
                                CallMethodFast::Hit(r?)
                            } else {
                                CallMethodFast::Miss
                            }
                        } else {
                            CallMethodFast::Miss
                        }
                    } else {
                        CallMethodFast::Miss
                    }
                };
        match fast {
            CallMethodFast::Hit(result) => Ok(Some(result)),
            CallMethodFast::Miss => Ok(None),
        }
    }

    /// Trampoline-resolution fast path for `o.m(...)` (#2345).  On an inline-cache
    /// hit whose cached value is a *Regular* `UserFunction` (a plain Python
    /// method), returns the unbound method and the receiver so the VM dispatch
    /// loop can trampoline the call — binding the receiver to `self` and looping
    /// instead of re-entering `call_user_function_expanded` natively, matching
    /// the speedup plain function calls already enjoy.
    ///
    /// Returns `None` (caller falls back to `exec_call_method`) when: the cache
    /// is not a `ClassAttr` hit, the receiver shadows the method, the class
    /// version/epoch changed, the cached value is a `BuiltinFunction` or a
    /// backing-primitive method, or the object is not a `PyInstance`.  Pure
    /// resolution: it does not touch `attr_cache` (a populated cache means a
    /// prior slow pass already filled it) or invoke anything.
    #[inline]
    fn resolve_method_cached(
        &self,
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        method: &str,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Option<(Rc<UserFunction>, Rc<std::cell::RefCell<pyrust_core::PyInstance>>)> {
        use crate::bytecode::AttrCacheEntry;
        let cache = code.attr_cache.borrow();
        let AttrCacheEntry::ClassAttr {
            class_ptr,
            class_version,
            epoch,
            value: unbound,
        } = &cache[call_site_pc]
        else {
            return None;
        };
        // Only Regular user methods are trampolinable; BuiltinFunction methods
        // (list.append / backing-primitive routes) need the native path.
        let ValueKind::UserFunction(f) = unbound.kind() else {
            return None;
        };
        if !matches!(f.kind, pyrust_core::UserFunctionKind::Regular) {
            return None;
        }
        let inst_rc = regs[obj as usize].as_py_instance_rc()?;
        let inst = inst_rc.borrow();
        let same_class = Rc::as_ptr(&inst.class) as *const () == *class_ptr;
        let no_shadow = !inst.attrs.contains_key(method);
        let version_ok = inst.class.borrow().mutation_version.get() == *class_version;
        let epoch_ok = pyrust_core::class_epoch() == *epoch;
        if !(same_class && no_shadow && version_ok && epoch_ok) {
            return None;
        }
        let f = Rc::clone(f);
        let recv = Rc::clone(inst_rc);
        drop(inst);
        Some((f, recv))
    }



    #[allow(clippy::too_many_arguments)]
    fn exec_call_method(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        args_base: crate::bytecode::Reg,
        nargs: u8,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
        cur_line: u32,
    ) -> Result<Value> {
        let method: &str = code.names.get(name_idx as usize)
            .ok_or_else(|| PyError::Runtime(format!("bytecode error: name index {name_idx} out of range")))?
            .as_str();
        let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
        for i in 0..crate::bytecode::Reg::from(nargs) {
            args.push(vm_read(regs, args_base + i, num_locals)?);
        }
        // Check if obj is a List, Dict, Tuple, Str, or Set via kind()
        let obj_kind_tag = Self::builtin_container_tag(regs[obj as usize].as_some());

        // No upfront unalias needed (#448): each builtin scopes its
        // own `RefCell::borrow_mut()` and snapshots iterable args
        // before opening the borrow.  Aliased self-references like
        // `lst.extend(lst)` are now safe by construction.

        // __iter__ on any tagged builtin type uses the same logic as
        // iter(receiver): produce a NativeIterFrame generator.
        if method == "__iter__" && obj_kind_tag != 0 {
            if !args.is_empty() {
                return Err(pyrust_core::type_err!("expected 0 arguments, got {}", args.len()));
            }
            let receiver = vm_read(regs, obj, num_locals)?;
            let iter_arg = ExpandedCallArg { name: None, value: receiver };
            let dispatch = crate::builtin_registry::lookup("iter")
                .expect("iter must be in the registry");
            return dispatch(self, &[iter_arg]);
        }

        // Tagged builtin containers share their dispatch body with the
        // expanded opcode (#431).  The no-kwargs path passes an empty kwargs
        // map; `IndexMap::new()` does not allocate until the first insert, so
        // the hot path stays cheap.
        if obj_kind_tag != 0 {
            let receiver = regs[obj as usize].clone();
            let empty_kw = PyDict::default();
            return self.dispatch_builtin_container_method(
                obj_kind_tag,
                receiver,
                method,
                args,
                &empty_kw,
            );
        }

        {
                // Generator methods (close, throw, __next__, __iter__) are
                // dispatched directly here — they need access to the VM/frame
                // and are not regular attributes on the Generator value.
                let is_generator = matches!(
                    regs[obj as usize].as_some().map(|v| v.kind()),
                    Some(ValueKind::Generator(_))
                );
                if is_generator {
                    let obj_val = vm_read(regs, obj, num_locals)?;
                    return self.call_generator_method(obj_val, method, args);
                }

        // Inline cache fast path for user-defined class methods on PyInstance
        // objects (Regular UserFunctions / BuiltinFunctions only).
        if let Some(result) =
            self.try_call_method_cached(regs, obj, method, &args, code, call_site_pc)?
        {
            return Ok(result);
        }

                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(&obj_val, method)?;
                // Publish the register-resident current line before invoking
                // `sys._getframe()` — which compiles to a CallMethod on the
                // `sys` module (registered name `"sys._getframe"`) — so
                // `f_lineno` reads the call's line (issue #2185).  Gated on the
                // `_getframe` name (a cheap compare that short-circuits
                // otherwise); this is already the cold generic path (tagged
                // containers and cached methods returned above), so the common
                // `x.append()` / `s.upper()` calls pay nothing.
                if matches!(method_val.kind(), ValueKind::BuiltinFunction(n) if n.ends_with("_getframe"))
                {
                    pyrust_core::set_current_vm_line(cur_line);
                }
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                for arg in args {
                    buf.push(ExpandedCallArg { name: None, value: arg });
                }
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;

                // Fill or update the inline cache for this user-object call site.
                self.update_call_method_cache(&obj_val, method, code, call_site_pc);

                r
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_call_method_expanded(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        pos_list: crate::bytecode::Reg,
        kw_dict: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
    ) -> Result<Value> {
        let method: &str = code.names.get(name_idx as usize)
            .ok_or_else(|| PyError::Runtime(format!("bytecode error: name index {name_idx} out of range")))?
            .as_str();
        let v = vm_read(regs, pos_list, num_locals)?;
        let pos_items: Vec<Value> = match v.kind() {
            ValueKind::List(items) => items.to_vec(),
            _ => return Err(PyError::Runtime("CallMethodExpanded: pos_list must be a list".to_string())),
        };
        let v = vm_read(regs, kw_dict, num_locals)?;
        let kw_map = match v.kind() {
            ValueKind::Dict(d) => d.clone(),
            _ => return Err(PyError::Runtime("CallMethodExpanded: kw_dict must be a dict".to_string())),
        };
        // CPython: a non-string `**` key is a TypeError (`keywords must be
        // strings`), not a silently dropped keyword argument.
        if kw_map.keys().any(|k| !matches!(k, PyKey::Str(_))) {
            return Err(pyrust_core::type_err!("keywords must be strings"));
        }

        let obj_kind_tag = Self::builtin_container_tag(regs[obj as usize].as_some());

        // No upfront unalias needed (#448): each builtin scopes its
        // own `borrow_mut()` and snapshots iterables before opening
        // the borrow.

        // __iter__ on any tagged builtin type: same logic as iter(receiver).
        if method == "__iter__" && obj_kind_tag != 0 {
            if !kw_map.is_empty() {
                return Err(pyrust_core::type_err!("wrapper __iter__() takes no keyword arguments"));
            }
            if !pos_items.is_empty() {
                return Err(pyrust_core::type_err!("expected 0 arguments, got {}", pos_items.len()));
            }
            let receiver = vm_read(regs, obj, num_locals)?;
            let iter_arg = ExpandedCallArg { name: None, value: receiver };
            let dispatch = crate::builtin_registry::lookup("iter")
                .expect("iter must be in the registry");
            return dispatch(self, &[iter_arg]);
        }

        // Tagged builtin containers share their dispatch body with the
        // no-kwargs opcode (#431).
        if obj_kind_tag != 0 {
            let receiver = regs[obj as usize].clone();
            return self.dispatch_builtin_container_method(
                obj_kind_tag,
                receiver,
                method,
                pos_items,
                &kw_map,
            );
        }

        {
                // Generator methods — see `exec_call_method` for context.
                let is_generator = matches!(
                    regs[obj as usize].as_some().map(|v| v.kind()),
                    Some(ValueKind::Generator(_))
                );
                if is_generator {
                    if !kw_map.is_empty() {
                        return Err(pyrust_core::type_err!("generator.{method}() takes no keyword arguments"));
                    }
                    let obj_val = vm_read(regs, obj, num_locals)?;
                    return self.call_generator_method(obj_val, method, pos_items);
                }
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(&obj_val, method)?;
                // Build directly into the reusable call buffer, bypassing the
                // intermediate ExpandedArgBuf allocation.
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                buf.extend(pos_items.into_iter().map(|v| ExpandedCallArg { name: None, value: v }));
                for (k, v) in &kw_map {
                    if let PyKey::Str(name) = k {
                        buf.push(ExpandedCallArg {
                            name: Some(name.as_str().unwrap_or("").to_owned()),
                            value: v.clone(),
                        });
                    }
                }
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;
                r
        }
    }
}
