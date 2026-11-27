// ─────────────────────────────────────────────────────────────────────────────
// Opaque — heap-allocated types that don't fit in 48 bits
// ─────────────────────────────────────────────────────────────────────────────

pub enum Opaque {
    PyBigInt(Rc<BigInt>),
    Dict(Rc<RefCell<PyDict>>),
    /// Mutable `set` storage.  Shared via `Rc` so `Value::clone` produces an
    /// alias rather than a deep copy, matching Python's reference semantics.
    /// See [`SetInner`] and issue #305.
    Set(Rc<SetInner>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    /// Arbitrary-precision `range` whose start/stop/step do not all fit in `i64`
    /// (#2118).  The common i64 case stays on [`Opaque::Range`]; this variant is
    /// only produced when `Value::range_big` sees a bound outside i64.
    BigRange(Rc<BigRangeData>),
    UserFunction(Rc<UserFunction>),
    PyClass(Rc<RefCell<PyClass>>),
    PyInstance(Rc<RefCell<PyInstance>>),
    PyModule(Rc<RefCell<PyModule>>),
    BoundMethod {
        function: Rc<UserFunction>,
        receiver: Rc<RefCell<PyInstance>>,
        /// Monotonic allocation id so that `a = obj.method; a is a` is True
        /// while `obj.method is obj.method` is False (#722).  Preserved across
        /// clones (Rc-sharing semantics), matches the `SmallTuple2` pattern.
        obj_id: u64,
    },
    /// A classmethod bound to a specific class (the first argument will be `cls`).
    ClassBoundMethod {
        function: Rc<UserFunction>,
        class: Rc<RefCell<PyClass>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// Proxy returned by `super(cls, instance)`. Attribute lookup on this proxy
    /// starts from `cls`'s parent class and binds to `instance`.
    ///
    /// Note: zero-argument `super()` (CPython's implicit `__class__` cell) is not
    /// supported. Use the two-argument form `super(CurrentClass, self)` explicitly.
    SuperProxy {
        class: Rc<RefCell<PyClass>>,
        instance: Rc<RefCell<PyInstance>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// Proxy returned by `super(cls, cls_instance)` where the second argument is
    /// a class (used in classmethods). Attribute lookup starts from `cls`'s parent
    /// and binds as a `ClassBoundMethod` to `obj_class`.
    SuperProxyClass {
        class: Rc<RefCell<PyClass>>,
        obj_class: Rc<RefCell<PyClass>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// Proxy returned by the one-argument form `super(cls)` (#2704). It is an
    /// *unbound* super object: `__self__` / `__self_class__` are `None`. It acts
    /// as a descriptor — `super(cls).__get__(obj, owner)` returns a bound
    /// `super(cls, obj)` (or `super(cls, owner)` when `obj` is a class).
    SuperProxyUnbound {
        class: Rc<RefCell<PyClass>>,
        /// Monotonic allocation id for identity semantics (#722).
        obj_id: u64,
    },
    /// A live generator object.  The concrete execution state (registers, pc,
    /// iterator slots, etc.) is stored as a type-erased `Box<dyn Any>` so that
    /// `pyrust-core` does not need to depend on `pyrust`'s bytecode types;
    /// [`GeneratorCell`] pairs it with the object-model facts that must stay
    /// readable while the state is checked out (its type tag and names).
    Generator(Rc<GeneratorCell>),
    /// An immutable byte string.  Constructed via the `b"..."` literal or
    /// the `bytes(...)` builtin.  Stored behind `Rc` for cheap clones.
    Bytes(Rc<Vec<u8>>),
    /// A Python `complex` number stored as (real, imag).
    Complex(f64, f64),
    /// Inline storage for a 1-element tuple.  Same rationale as
    /// `SmallTuple2`; this is the shape of the `args` of every exception
    /// constructed with one argument (`ValueError(msg)`, `KeyError(key)`),
    /// so raising one no longer allocates a tuple header and a backing
    /// `Vec` (issue #3426).
    SmallTuple1 {
        items: [Value; 1],
        obj_id: u64,
    },
    /// Inline storage for a 2-element tuple.  Eliminates the secondary
    /// `Vec<Value>` heap allocation for the most common Python tuple shape
    /// (e.g. `dict.items()` entries, `enumerate()` yields, `divmod()`).
    /// `obj_id` preserves stable `id()` identity across clones, matching the
    /// behaviour of the pool-based tuple path.
    SmallTuple2 {
        items: [Value; 2],
        obj_id: u64,
    },
    /// Inline storage for a 3-element tuple.  Same rationale as
    /// `SmallTuple2`; covers `str.partition()`/`rpartition()` and similar
    /// fixed-arity returns.
    SmallTuple3 {
        items: [Value; 3],
        obj_id: u64,
    },
    /// A type-erased built-in object whose operations are dispatched through
    /// the installed [`BuiltinTypeOps`] table.  Used for built-in types whose
    /// payload doesn't justify a dedicated Tier 1 variant (file, property,
    /// frozenset value, dict views, iterator helpers, …).  `pyrust-core`
    /// never names the concrete type — it only calls through `ops`.
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: BuiltinState,
    },
}

/// Backing data for an arbitrary-precision `range` (#2118).  `start`/`stop`/`step`
/// are full Python ints; at least one of them is outside the `i64` range (otherwise
/// the value would be stored as the cheaper i64-backed [`Opaque::Range`]).
#[derive(Debug, PartialEq)]
pub struct BigRangeData {
    pub start: BigInt,
    pub stop: BigInt,
    pub step: BigInt,
}

/// Exact element count of an i64-backed Python range.
///
/// The result intentionally uses `i128`: although each bound fits in `i64`,
/// the distance from `i64::MIN` to `i64::MAX` does not. Keeping the
/// subtraction and `-step` out of signed i64 arithmetic also covers
/// `step == i64::MIN` without debug-build panics or release-build wrapping.
///
/// The calculation itself stays in `u64`, so ordinary range length queries do
/// not pay for a software i128 division on platforms without native support.
#[inline]
pub fn range_len(start: i64, stop: i64, step: i64) -> i128 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            0
        } else {
            let distance = stop.abs_diff(start);
            i128::from(((distance - 1) / (step as u64)) + 1)
        }
    } else if start <= stop {
        0
    } else {
        let distance = start.abs_diff(stop);
        i128::from(((distance - 1) / step.unsigned_abs()) + 1)
    }
}

/// Whether every cursor value, including the one-past-the-end increment, fits
/// in the compact i64 range-iterator state.
///
/// The stop bound fitting in i64 is not enough: a final yielded value may be
/// near one extreme while `step` crosses that extreme. The direct VM loop, the
/// concrete iterator object, and the optimizer's closed-form loop folding all
/// share this construction invariant — the last decides whether a range can
/// ever reach the compact cursor its guard tests for.
pub fn i64_range_native_cursor_safe(start: i64, stop: i64, step: i64) -> bool {
    let len = range_len(start, stop, step);
    if len == 0 {
        return true;
    }

    let start_wide = i128::from(start);
    let step_wide = i128::from(step);
    let last = start_wide + (len - 1) * step_wide;
    let after_last = last + step_wide;
    len <= i128::from(i64::MAX)
        && after_last >= i128::from(i64::MIN)
        && after_last <= i128::from(i64::MAX)
        && step != i64::MIN
}

/// Element count of a big range, as CPython computes `len(range(...))`: the count
/// of `start, start+step, …` strictly before `stop`.  Mirrors [`range_len`] but in
/// `BigInt` arithmetic; returns a non-negative `BigInt` (0 when empty).
pub fn bigrange_len(start: &BigInt, stop: &BigInt, step: &BigInt) -> BigInt {
    let zero = BigInt::from(0);
    let one = BigInt::from(1);
    if *step > zero {
        if stop > start {
            (stop - start - &one) / step + &one
        } else {
            zero
        }
    } else if *step < zero {
        if start > stop {
            (start - stop - &one) / (-step) + &one
        } else {
            zero
        }
    } else {
        zero
    }
}

/// CPython `range_equals` (Objects/rangeobject.c) over arbitrary-precision bounds:
/// two ranges are equal iff they yield the same sequence — same length, and (when
/// non-empty) same first element, and (when length ≥ 2) same step.  Mirrors the
/// content-based range hash so equal ranges hash equal.
pub fn bigrange_eq(
    as_: &BigInt,
    ao: &BigInt,
    at: &BigInt,
    bs: &BigInt,
    bo: &BigInt,
    bt: &BigInt,
) -> bool {
    let la = bigrange_len(as_, ao, at);
    let lb = bigrange_len(bs, bo, bt);
    let one = BigInt::from(1);
    let two = BigInt::from(2);
    la == lb && (la < one || as_ == bs) && (la < two || at == bt)
}

impl Clone for Opaque {
    fn clone(&self) -> Self {
        match self {
            Opaque::PyBigInt(rc) => Opaque::PyBigInt(Rc::clone(rc)),
            Opaque::Dict(rc) => Opaque::Dict(Rc::clone(rc)),
            // Sets share backing storage on clone; see `SetInner` and #305.
            Opaque::Set(rc) => Opaque::Set(Rc::clone(rc)),
            Opaque::Range { start, stop, step } => Opaque::Range {
                start: *start,
                stop: *stop,
                step: *step,
            },
            Opaque::BigRange(rc) => Opaque::BigRange(Rc::clone(rc)),
            Opaque::UserFunction(f) => Opaque::UserFunction(Rc::clone(f)),
            Opaque::PyClass(c) => Opaque::PyClass(Rc::clone(c)),
            Opaque::PyInstance(i) => Opaque::PyInstance(Rc::clone(i)),
            Opaque::PyModule(m) => Opaque::PyModule(Rc::clone(m)),
            Opaque::BoundMethod {
                function,
                receiver,
                obj_id,
            } => Opaque::BoundMethod {
                function: Rc::clone(function),
                receiver: Rc::clone(receiver),
                obj_id: *obj_id,
            },
            Opaque::ClassBoundMethod {
                function,
                class,
                obj_id,
            } => Opaque::ClassBoundMethod {
                function: Rc::clone(function),
                class: Rc::clone(class),
                obj_id: *obj_id,
            },
            Opaque::SuperProxy {
                class,
                instance,
                obj_id,
            } => Opaque::SuperProxy {
                class: Rc::clone(class),
                instance: Rc::clone(instance),
                obj_id: *obj_id,
            },
            Opaque::SuperProxyClass {
                class,
                obj_class,
                obj_id,
            } => Opaque::SuperProxyClass {
                class: Rc::clone(class),
                obj_class: Rc::clone(obj_class),
                obj_id: *obj_id,
            },
            Opaque::SuperProxyUnbound { class, obj_id } => Opaque::SuperProxyUnbound {
                class: Rc::clone(class),
                obj_id: *obj_id,
            },
            Opaque::Generator(state) => Opaque::Generator(Rc::clone(state)),
            Opaque::Bytes(rc) => Opaque::Bytes(Rc::clone(rc)),
            Opaque::Complex(re, im) => Opaque::Complex(*re, *im),
            Opaque::SmallTuple1 { items, obj_id } => Opaque::SmallTuple1 {
                items: [items[0].clone()],
                obj_id: *obj_id,
            },
            Opaque::SmallTuple2 { items, obj_id } => Opaque::SmallTuple2 {
                items: [items[0].clone(), items[1].clone()],
                obj_id: *obj_id,
            },
            Opaque::SmallTuple3 { items, obj_id } => Opaque::SmallTuple3 {
                items: [items[0].clone(), items[1].clone(), items[2].clone()],
                obj_id: *obj_id,
            },
            Opaque::BuiltinObject { ops, state } => Opaque::BuiltinObject {
                ops: *ops,
                state: Rc::clone(state),
            },
        }
    }
}
