// Python protocol-slot dispatch for built-in values.
impl Interpreter {
    /// Dispatch an unbound type-qualified object-level dunder
    /// (`str.__hash__(x)`, `int.__format__(5, 'x')`, …) whose owner is the
    /// primitive `type_name`. Validates the receiver against the called type
    /// and delegates slot wrappers to the object registry or `__format__` to
    /// the shared format implementation.
    pub(super) fn call_primitive_object_dunder(
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
            ("bytearray", ValueKind::BuiltinObject { ops, .. })
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                true
            }
            ("list", ValueKind::List(_)) => true,
            ("tuple", ValueKind::Tuple(_)) => true,
            ("dict", ValueKind::Dict(_)) => true,
            ("set", ValueKind::Set(_)) => true,
            ("frozenset", ValueKind::BuiltinObject { ops, .. })
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                true
            }
            (_, ValueKind::PyInstance(_)) => builtin_data_backing(&self_arg.value)
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
            let h = hash_value_with_interp(self, &self_arg.value)?;
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
        // Issue #2297: `int.__round__(ndigits=None)` accepts 0 or 1 positional
        // argument — handle its arity separately from the fixed-arity slots below
        // (CPython's C-clinic wording: `__round__ expected at most 1 argument,
        // got N`).  Routed before the fixed `want` check so the optional second
        // operand does not trip the generic "expected 1 argument" path.
        if method == "__round__" {
            if args.len() > 1 {
                return Err(pyrust_core::type_err!(
                    "__round__ expected at most 1 argument, got {}",
                    args.len()
                ));
            }
            // Issue #2481: `float.__round__([ndigits])` returns an `int` when
            // `ndigits` is omitted (`(1.5).__round__()` → `2`, banker's
            // rounding) and a `float` when given (`(1.5).__round__(0)` →
            // `2.0`).  Unlike `int.__round__`, the float slot treats an explicit
            // `ndigits=None` as omitted (`(1.7).__round__(None)` → `2`), exactly
            // as the `round()` builtin does — so route straight through it
            // rather than the int-only `int_round_dunder` (which rejects None).
            if type_name == "float" {
                let dispatch = crate::builtin_registry::lookup("round")
                    .expect("round must be in the registry");
                let mut call_args = vec![ExpandedCallArg {
                    name: None,
                    value: receiver.clone(),
                }];
                if let Some(n) = args.pop() {
                    call_args.push(ExpandedCallArg {
                        name: None,
                        value: n,
                    });
                }
                return dispatch(self, &call_args);
            }
            return self.int_round_dunder(&receiver, args.pop());
        }
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
            | "__str__" | "__repr__" | "__bool__" | "__reversed__" | "__iter__"
            | "__float__" | "__int__"
            // Issue #2297: `int.__index__`/`__trunc__`/`__floor__`/`__ceil__` are
            // zero-arg slot wrappers.
            | "__index__" | "__trunc__" | "__floor__" | "__ceil__" => 0,
            "__setitem__" => 2,
            _ => 1,
        };
        if args.len() != want {
            // `dict.__reversed__()` is a named no-arg method-wrapper in CPython
            // 3.12: `dict.__reversed__() takes no arguments (N given)` (#2093).
            // Checked before the generic named-wrapper arm below because
            // `__reversed__` is in `is_named_protocol_wrapper` (its kwarg
            // rejection is named, #2398) but its *arity* wording is the no-arg
            // form, not "takes exactly one argument".
            if method == "__reversed__" {
                return Err(pyrust_core::type_err!(
                    "{type_name}.__reversed__() takes no arguments ({} given)",
                    args.len()
                ));
            }
            // Issue #2297/#2481: `int`/`float`.`__trunc__`/`__floor__`/`__ceil__`
            // are named no-arg method-wrappers in CPython 3.12.  The int slots
            // report the owning type `int` even for a `bool` receiver
            // (`int.__trunc__() takes no arguments (N given)`); the float slots
            // report `float`.  `__index__` (just below, anonymous int slot)
            // keeps the "expected 0 arguments, got N" form instead.
            if matches!(method, "__trunc__" | "__floor__" | "__ceil__") {
                let owner = if type_name == "float" { "float" } else { "int" };
                return Err(pyrust_core::type_err!(
                    "{owner}.{method}() takes no arguments ({} given)",
                    args.len()
                ));
            }
            if is_named_protocol_wrapper(method, &type_name) {
                return Err(pyrust_core::type_err!(
                    "{type_name}.{method}() takes exactly one argument ({} given)",
                    args.len()
                ));
            }
            // `__mul__` (sq_repeat), `__imul__` (sq_inplace_repeat) and
            // `__setitem__` (sq_ass_item) print a leading space before
            // "expected" in CPython 3.12.
            let lead = if matches!(method, "__mul__" | "__imul__" | "__setitem__") {
                " "
            } else {
                ""
            };
            let plural = if want == 1 { "argument" } else { "arguments" };
            return Err(pyrust_core::type_err!(
                "{lead}expected {want} {plural}, got {}",
                args.len()
            ));
        }
        // Issue #2070: scalar/object dunders that route to a registry builtin or
        // an interpreter helper, exposed on every primitive type.  These never
        // collide with the container `__add__`/`__mul__`/… arms below.
        match method {
            "__hash__" => {
                let h = hash_value_with_interp(self, &receiver)?;
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
            // Issue #2433: `int.__float__`/`int.__int__` (exposed unbound and
            // bound on `int`).  CPython exposes these as int-owned slot wrappers;
            // `(5).__float__()` → `5.0`, `(5).__int__()` → `5`.  Route through
            // the `float`/`int` constructors so int-subclass receivers coerce
            // exactly as the builtins do.
            "__float__" | "__int__" => {
                let ctor = if method == "__float__" {
                    "float"
                } else {
                    "int"
                };
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup(ctor)
                    .unwrap_or_else(|| panic!("{ctor} must be in the registry"));
                return dispatch(self, &[arg]);
            }
            // Issue #2481: `float.__trunc__`/`__floor__`/`__ceil__` round the
            // float to an `int` toward zero / -inf / +inf respectively
            // (`(-1.7).__floor__()` → `-2`, `(-1.7).__ceil__()` → `-1`) — they
            // are *not* identity like the int slots, so route through the
            // dedicated `float.__X__` registry bodies (which already handle
            // NaN/inf → ValueError/OverflowError, BigInt promotion and the
            // float-subclass receiver).
            "__trunc__" | "__floor__" | "__ceil__" if type_name == "float" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup(&format!("float.{method}"))
                    .expect("float.__trunc__/__floor__/__ceil__ must be in the registry");
                return dispatch(self, &[arg]);
            }
            // Issue #2297: `int.__index__`/`__trunc__`/`__floor__`/`__ceil__`
            // all return the integer value of the receiver unchanged
            // (`(5).__trunc__()` → `5`, `(7).__floor__()` → `7`).  Like CPython,
            // a `bool` receiver normalises to plain `int`
            // (`True.__index__()` → `1`, not `True`); route through the `int`
            // constructor, which preserves `BigInt` and performs that coercion.
            "__index__" | "__trunc__" | "__floor__" | "__ceil__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch =
                    crate::builtin_registry::lookup("int").expect("int must be in the registry");
                return dispatch(self, &[arg]);
            }
            // #2093: `dict.__reversed__()` yields keys in reverse insertion
            // order.  Routes through the `reversed` builtin, which handles the
            // dict case directly.
            "__reversed__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch = crate::builtin_registry::lookup("reversed")
                    .expect("reversed must be in the registry");
                return dispatch(self, &[arg]);
            }
            // Issue #2387: `__iter__` reached here via the type-level unbound
            // (`list.__iter__([1])`) or builtin-subclass (`LI([1]).__iter__()`)
            // paths — the bound primitive-instance form is intercepted earlier.
            // Wrap the (already-unwrapped) backing in the same `NativeIterFrame`
            // generator the `iter()` builtin produces.
            "__iter__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch =
                    crate::builtin_registry::lookup("iter").expect("iter must be in the registry");
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
            && let Some(result) = self.primitive_numeric_dunder(method, &receiver, &mut args)?
        {
            return Ok(result);
        }
        match method {
            "__len__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: receiver,
                };
                let dispatch =
                    crate::builtin_registry::lookup("len").expect("len must be in the registry");
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
                    pyrust_core::type_err!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(v)
                    )
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
                let mut regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
                self.exec_set_item(&mut regs, 0, 0, 1, 2)?;
                Ok(Value::none())
            }
            "__delitem__" => {
                let index = args.pop().unwrap();
                let mut scratch = vec![receiver, index];
                let mut regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
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
                    pyrust_core::type_err!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(v)
                    )
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
            // Issue #2387: str/bytes/bytearray `%` formatting slots, exposed as
            // method-wrappers (`'%s'.__mod__('x')`, `hasattr(bytes, '__mod__')`).
            // `a.__rmod__(b)` computes `b % a` (swapped operands).  Both delegate
            // to the same `eval_binary` Mod path the `%` operator uses, so the
            // format-machinery TypeErrors match CPython byte-for-byte.
            "__mod__" if matches!(&*type_name, "str" | "bytes" | "bytearray") => {
                let other = args.pop().unwrap();
                self.eval_binary(receiver, crate::ast::BinaryOp::Mod, other)
            }
            "__rmod__" if matches!(&*type_name, "str" | "bytes" | "bytearray") => {
                let other = args.pop().unwrap();
                self.eval_binary(other, crate::ast::BinaryOp::Mod, receiver)
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
        // Issue #2847: an explicitly selected primitive base slot compares
        // the builtin payload of subclass instances; it must neither reject
        // their visible subclass names nor redispatch their user overrides.
        // Keep genuine user objects intact so incompatible operands still
        // produce the base slot's NotImplemented result below.
        let recv = coerce_subclass_backing(recv, &[]).unwrap_or_else(|| recv.clone());
        let other = coerce_subclass_backing(other, &[]).unwrap_or_else(|| other.clone());
        let is_equality = matches!(method, "__eq__" | "__ne__");
        if !richcmp_operand_accepted(&recv, &other, is_equality) {
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
        self.eval_binary(recv, op, other)
    }

    /// Issue #2297: `int.__round__([ndigits])`.  CPython's `int.__round__`
    /// always returns an `int` (`(125).__round__(-1)` → `120`, banker's
    /// rounding); omitted `ndigits` returns the value unchanged.  Routes
    /// through the `round` registry builtin, which already implements the int +
    /// `ndigits` semantics (negative `ndigits` rounding, `BigInt` results).
    ///
    /// Unlike the `round()` builtin, the `int.__round__` *slot* index-coerces
    /// `ndigits` with no `None` special-case: `(5).__round__(None)` raises
    /// `TypeError: 'NoneType' object cannot be interpreted as an integer`,
    /// whereas `round(5, None)` returns `5`.  `round` swallows an explicit
    /// `None` (treats it as "omitted"), so reject it here before delegating.
    fn int_round_dunder(&mut self, recv: &Value, ndigits: Option<Value>) -> Result<Value> {
        if let Some(n) = &ndigits
            && matches!(n.kind(), ValueKind::None)
        {
            return Err(pyrust_core::type_err!(
                "'NoneType' object cannot be interpreted as an integer"
            ));
        }
        let dispatch =
            crate::builtin_registry::lookup("round").expect("round must be in the registry");
        let mut call_args = vec![ExpandedCallArg {
            name: None,
            value: recv.clone(),
        }];
        if let Some(n) = ndigits {
            call_args.push(ExpandedCallArg {
                name: None,
                value: n,
            });
        }
        dispatch(self, &call_args)
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
        // `__pos__` / `__invert__` route through canonical built-in unary
        // evaluation; `__abs__` goes through the registry builtin (complex
        // `abs` returns a float, which unary evaluation does not cover).
        match method {
            "__neg__" => {
                return Ok(Some(eval_builtin_unary(
                    crate::ast::UnaryOp::Neg,
                    recv.clone(),
                )?));
            }
            "__pos__" => {
                return Ok(Some(eval_builtin_unary(
                    crate::ast::UnaryOp::Pos,
                    recv.clone(),
                )?));
            }
            "__invert__" => {
                return Ok(Some(eval_builtin_unary(
                    crate::ast::UnaryOp::BitNot,
                    recv.clone(),
                )?));
            }
            "__abs__" => {
                let arg = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
                let dispatch =
                    crate::builtin_registry::lookup("abs").expect("abs must be in the registry");
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
                let a = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
                let b = ExpandedCallArg {
                    name: None,
                    value: other,
                };
                return Ok(Some(dispatch(self, &[a, b])?));
            }
            "__rdivmod__" => {
                let other = args.pop().unwrap();
                if !numeric_operand_accepted(recv, &other) {
                    return Ok(Some(Value::not_implemented()));
                }
                let dispatch = crate::builtin_registry::lookup("divmod")
                    .expect("divmod must be in the registry");
                let a = ExpandedCallArg {
                    name: None,
                    value: other,
                };
                let b = ExpandedCallArg {
                    name: None,
                    value: recv.clone(),
                };
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
}
