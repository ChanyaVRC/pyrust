#[derive(Debug, Clone)]
pub struct UserFunctionParam {
    pub name: String,
    pub default: Option<Value>,
    pub is_args: bool,
    pub is_kwargs: bool,
    pub is_keyword_only: bool,
    pub is_positional_only: bool,
}

/// Precomputed bind target for a parameter, resolved once at compile time
/// (issue #1918).  The parameter→register mapping is static per function, so
/// the call path can bind each positional argument by a direct slot lookup
/// instead of hashing the parameter name into `local_index` (and linearly
/// scanning `cell_vars`) on every call.
///
/// `param_binds[i]` is the target for `params[i]`:
/// - `Reg(r)` → write the bound value into register `r`.
/// - `Cell` → the parameter is a cell variable; insert it into the local env by
///   name (rare; only closures that capture a parameter).
/// - `None` → the parameter has no local slot (an unused `*args` / `**kwargs`
///   placeholder); nothing to bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamBind {
    Reg(u32),
    Cell,
    None,
}

/// Discriminator for `UserFunction` semantics.  `@classmethod` and
/// `@staticmethod` decorators produce a UserFunction whose body Rc-shares
/// with the original, distinguished only by this tag — no wrapper variant.
/// `Builtin` is the relocated form of the former `Opaque::BuiltinFunction`
/// variant: a Rust built-in dispatched by name (`len`, `print`, …).  Same
/// representable state as the old variant, but unified into the function
/// value's kind tag so `Opaque` shrinks by one variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserFunctionKind {
    #[default]
    Regular,
    ClassMethod,
    StaticMethod,
    Builtin(&'static str),
}

/// Boxed `f.__name__` / `f.__qualname__` overrides (#2256).  Held behind an
/// `Option<Box<…>>` on `UserFunction` so the common case (no override) costs a
/// single null pointer rather than two inline `RefCell<Option<String>>`.
#[derive(Debug, Clone, Default)]
pub struct FnNameOverrides {
    pub name: Option<String>,
    pub qualname: Option<String>,
}

/// Lazily-boxed per-object overrides for `f.__defaults__` / `f.__kwdefaults__`
/// (#2395).  Each slot uses three states encoded in a single `Value`:
/// - `unset()` — not overridden; the binder/getter falls back to the
///   compile-time `params[].default` values.
/// - `none()`  — explicitly cleared (`f.__defaults__ = None` or `del`).
/// - tuple/dict — the reassigned value, observed verbatim by `__defaults__` /
///   `__kwdefaults__` reads and applied by the call binder.
///
/// `unset()` is never a user-visible value, so it is unambiguous as the
/// "not overridden" marker.
#[derive(Debug, Clone)]
pub struct DefaultsOverride {
    pub defaults: Value,
    pub kwdefaults: Value,
}

impl Default for DefaultsOverride {
    fn default() -> Self {
        DefaultsOverride {
            defaults: Value::unset(),
            kwdefaults: Value::unset(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserFunction {
    /// Globally unique identity for fn_cache keying — stable across Rc drops/reallocations.
    pub id: u64,
    pub kind: UserFunctionKind,
    /// The bare function name as declared.  Used for self-recursive slot lookup
    /// and error messages.  Do not mutate through this field; user code that
    /// assigns `f.__name__ = "x"` writes to `name_overrides` instead.
    ///
    /// `Rc<str>` (#2256): the declared name is immutable per-`def`, so every
    /// closure produced by the same `def` shares one allocation instead of each
    /// carrying its own `String` copy.  Derefs to `str`, so read sites are
    /// unchanged; `.clone()` is a cheap refcount bump.
    pub name: Rc<str>,
    /// Fully-qualified compile-time name (e.g. `"outer.<locals>.inner"`).
    /// Exposed as `f.__qualname__`.  User code that assigns `f.__qualname__ = "x"`
    /// writes to `name_overrides` instead.  `Rc<str>` shared per-`def` (#2256).
    pub qualname: Rc<str>,
    /// Lazily-boxed user overrides for `f.__name__` / `f.__qualname__` (#2256).
    /// `None` (the common case — virtually every function and closure) falls back
    /// to `name` / `qualname` and costs one pointer instead of two inline
    /// `RefCell<Option<String>>` (64 bytes), trimming `UserFunction` per object;
    /// the `Box` is allocated only when user code actually reassigns one of these
    /// dunders.  Access via `effective_name` / `effective_qualname` /
    /// `set_user_name` / `set_user_qualname`.
    pub name_overrides: RefCell<Option<Box<FnNameOverrides>>>,
    /// `f.__module__` — the name of the module in which the function was defined.
    /// Defaults to `"__main__"` (module-name tracking is not yet implemented).
    /// User code may assign any value; `del f.__module__` resets it to `None`.
    ///
    /// Stored lazily (#2256): the `Value::unset()` sentinel means "not yet
    /// materialised — falls back to the `"__main__"` default", so the common
    /// case (every function/closure, none of which reassign `__module__`) costs
    /// no per-object heap `String`.  An explicit `del f.__module__` writes
    /// `Value::none()` (distinct from `unset`), and `f.__module__ = v` writes
    /// `v`.  Read through `module_value()`, never `module.borrow()` directly.
    pub module: RefCell<Value>,
    /// `f.__doc__` — the function's docstring, or `None` if absent.
    /// pyrust does not yet extract docstrings at compile time, so this is always
    /// `None` at construction time.  User code may assign any value;
    /// `del f.__doc__` resets it to `None`.
    pub doc: RefCell<Value>,
    /// Arbitrary dynamic attributes set by user code (`f.x = v`).
    /// Exposed as `f.__dict__`.  Stored as a `Value::dict` wrapped in
    /// `Rc<RefCell<...>>` so that:
    ///   1. `get_attr("__dict__")` returns the **live** dict object (CPython
    ///      semantics: mutations through the returned dict propagate back to
    ///      the function).
    ///   2. `f.__dict__ = new_dict` replaces the inner Value via
    ///      `*attrs.borrow_mut() = new_dict_value`.
    ///   3. Bound-method copies and `@classmethod`/`@staticmethod` wrappers
    ///      share the same `Rc` (same as before) so they all see the same dict.
    ///
    /// Initialized lazily on first use (`None` means no attrs have been set
    /// yet).  The `RefCell` provides interior mutability so that
    /// `get_attr("__dict__")` can initialize the dict through a shared
    /// `Rc<UserFunction>` without requiring `&mut self`.
    pub attrs: RefCell<Option<Rc<RefCell<Value>>>>,
    /// `f.__annotations__` — dict mapping annotated parameter names (and
    /// `'return'` for the return annotation) to their evaluated annotation
    /// values.  Populated at function-definition time (matching CPython's
    /// runtime evaluation semantics).  User code may replace the entire dict
    /// via `f.__annotations__ = {...}`.
    ///
    /// Stored as a `Value` (always `Value::dict(...)`) so that repeated
    /// attribute reads return the *same* dict object (Rc identity), matching
    /// CPython: `f.__annotations__ is f.__annotations__` is `True`.
    pub annotations: RefCell<Value>,
    /// Lazily-boxed per-object overrides for `f.__defaults__` /
    /// `f.__kwdefaults__` (#2395).  `None` (the common case — virtually every
    /// function/closure) costs a single null pointer and leaves the binder
    /// reading the compile-time `params[].default` values, so the hot call path
    /// is unaffected (an inline form regressed plain calls via the larger
    /// `UserFunction`, the #2256 size landmine).  The box is allocated only when
    /// user code reassigns/deletes one of these slots.  Access through
    /// `defaults_value` / `kwdefaults_value` / `positional_default` /
    /// `kwonly_default` / `set_defaults_override` / `set_kwdefaults_override`.
    pub defaults_override: RefCell<Option<Box<DefaultsOverride>>>,
    pub params: Vec<UserFunctionParam>,
    /// Precomputed bind target for each parameter (parallel to `params`),
    /// resolved once at compile time so the call path binds positional args by
    /// direct register index rather than hashing the parameter name on every
    /// call (issue #1918).  Shared via `Rc` across all instances of the same
    /// function prototype.
    pub param_binds: Rc<Vec<ParamBind>>,
    /// Number of ordinary parameters that accept a positional argument.
    ///
    /// `CallMemo` probes this on every cache hit. Keeping the immutable count
    /// beside the binding plan avoids rescanning all parameter flags in that
    /// hot path, while still requiring every such argument to be supplied so
    /// mutable `__defaults__` state cannot be omitted from the memo key.
    pub memo_positional_parameter_count: u16,
    /// Precomputed bind target for the function's own name (the self-reference
    /// register used by recursive calls), or `None` when the name has no local
    /// slot / is a cell var.  Avoids a per-call `local_index` hash + `cell_vars`
    /// scan for the recursion self-bind.
    pub self_bind: Option<u32>,
    pub local_names: NameSet,
    pub local_index: Rc<HashMap<String, u32>>,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub env: EnvRef,
    /// True when a call to this function may have its result *cached and reused*
    /// for equal arguments — read by the VM's `CallMemo` result cache
    /// (`vm.rs::Insn::CallMemo`).  More permissive than DCE-purity: a benign
    /// raise or user dunder dispatch is acceptable because the cache only fires
    /// for all-integer arguments and a scalar result (issue #2523).  Copied from
    /// the function's `FnProto::is_memo_pure`.
    pub is_memo_pure: bool,
    /// `types.coroutine` marks a generator *function* in place while returning
    /// that exact function object.  The marker belongs to this function
    /// instance rather than its shared compiled code: CPython replaces only
    /// the decorated function's `__code__`, so sibling closures compiled from
    /// the same prototype must remain ordinary generators.
    pub iterable_coroutine: std::cell::Cell<bool>,
    pub precompiled_code: Option<Rc<dyn Any>>,
    /// When `kind` is `StaticMethod` or `ClassMethod`, holds the original
    /// wrapped function `Rc` so that `sm.__func__` can return the exact same
    /// object that was passed to `staticmethod(f)` / `classmethod(f)`, preserving
    /// `sm.__func__ is f` identity.  `None` for `Regular` and `Builtin` functions.
    pub wrapped_func: Option<Rc<UserFunction>>,
}

impl UserFunction {
    /// `f.__name__`: the user override (`f.__name__ = …`) when set, else the
    /// declared `name` (#2256).
    pub fn effective_name(&self) -> String {
        match self.name_overrides.borrow().as_deref() {
            Some(FnNameOverrides { name: Some(n), .. }) => n.clone(),
            _ => self.name.to_string(),
        }
    }

    /// `f.__qualname__`: the user override when set, else the declared `qualname`.
    pub fn effective_qualname(&self) -> String {
        match self.name_overrides.borrow().as_deref() {
            Some(FnNameOverrides {
                qualname: Some(q), ..
            }) => q.clone(),
            _ => self.qualname.to_string(),
        }
    }

    /// Assign `f.__name__` (allocates the overrides box on first use).
    pub fn set_user_name(&self, name: String) {
        self.name_overrides
            .borrow_mut()
            .get_or_insert_with(Box::<FnNameOverrides>::default)
            .name = Some(name);
    }

    /// `f.__module__` — the module name in which the function was defined.
    ///
    /// Lazily materialised (#2256): when the stored cell is the `unset()`
    /// sentinel (the common, never-reassigned case), this returns the
    /// `"__main__"` default without the function having to carry a per-object
    /// heap `String`.  An explicit `del f.__module__` stores `Value::none()`
    /// (which is *not* `unset`), so this correctly returns `None` afterwards.
    pub fn module_value_with_default(&self, default: Option<&str>) -> Value {
        let cur = self.module.borrow();
        if cur.is_unset() {
            default.map_or_else(Value::none, Value::string)
        } else {
            cur.clone()
        }
    }

    /// User-function convenience wrapper. Native builtins instead pass their
    /// registry-owned declaring module to [`Self::module_value_with_default`].
    pub fn module_value(&self) -> Value {
        self.module_value_with_default(Some("__main__"))
    }

    /// Assign `f.__qualname__` (allocates the overrides box on first use).
    pub fn set_user_qualname(&self, qualname: String) {
        self.name_overrides
            .borrow_mut()
            .get_or_insert_with(Box::<FnNameOverrides>::default)
            .qualname = Some(qualname);
    }

    /// `f.__annotations__` — the function's annotation dict.
    ///
    /// Lazily materialised (#2256): a function with no annotations stores the
    /// `Value::unset()` sentinel rather than an eagerly-allocated empty dict, so
    /// the (very common) unannotated function/closure does not each carry a heap
    /// dict.  On first access the empty dict is created and stored, so repeated
    /// reads return the same object — matching CPython's
    /// `f.__annotations__ is f.__annotations__` identity.  `unset()` is never a
    /// user-visible value, so it is unambiguous as the "not yet created" marker.
    pub fn annotations_value(&self) -> Value {
        {
            let cur = self.annotations.borrow();
            if !cur.is_unset() {
                return cur.clone();
            }
        }
        let dict = Value::dict(PyDict::default());
        *self.annotations.borrow_mut() = dict.clone();
        dict
    }

    /// Indices into `params` of the positional-or-keyword parameters (i.e. the
    /// ones `f.__defaults__` aligns to): not `*args` / `**kwargs` and not
    /// keyword-only.  Positional-only params are included (their defaults are
    /// part of `__defaults__`), matching CPython.
    fn positional_param_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_args && !p.is_kwargs && !p.is_keyword_only)
            .map(|(i, _)| i)
    }

    /// The `f.__defaults__` override slot, or `unset()` when not overridden
    /// (#2395).  Reads the lazily-boxed `DefaultsOverride` without allocating.
    #[inline]
    fn defaults_override_slot(&self) -> Value {
        match self.defaults_override.borrow().as_deref() {
            Some(d) => d.defaults.clone(),
            None => Value::unset(),
        }
    }

    /// The `f.__kwdefaults__` override slot, or `unset()` when not overridden.
    #[inline]
    fn kwdefaults_override_slot(&self) -> Value {
        match self.defaults_override.borrow().as_deref() {
            Some(d) => d.kwdefaults.clone(),
            None => Value::unset(),
        }
    }

    /// Assign the `f.__defaults__` override (#2395).  `value` is a tuple (the
    /// reassigned defaults) or `None` (cleared); both observed by later calls and
    /// reads.  Allocates the box on first use.
    pub fn set_defaults_override(&self, value: Value) {
        self.defaults_override
            .borrow_mut()
            .get_or_insert_with(Box::<DefaultsOverride>::default)
            .defaults = value;
    }

    /// Assign the `f.__kwdefaults__` override (#2395).  `value` is a dict or
    /// `None`.  Allocates the box on first use.
    pub fn set_kwdefaults_override(&self, value: Value) {
        self.defaults_override
            .borrow_mut()
            .get_or_insert_with(Box::<DefaultsOverride>::default)
            .kwdefaults = value;
    }

    /// `f.__defaults__` — the tuple of positional defaults, or `None`.  Honours a
    /// per-object override (`f.__defaults__ = …`) when present (#2395); otherwise
    /// collects the compile-time `params[].default` values in declaration order.
    pub fn defaults_value(&self) -> Value {
        let ov = self.defaults_override_slot();
        if !ov.is_unset() {
            // An override of `None` (or `del`) reports `None`; a tuple reports
            // itself verbatim (round-trips exactly what was assigned).
            return ov;
        }
        let defs: Vec<Value> = self
            .positional_param_indices()
            .filter_map(|i| self.params[i].default.clone())
            .collect();
        if defs.is_empty() {
            Value::none()
        } else {
            Value::tuple(defs)
        }
    }

    /// `f.__kwdefaults__` — the dict of keyword-only defaults, or `None`.
    /// Honours a per-object override when present (#2395).
    pub fn kwdefaults_value(&self) -> Value {
        let ov = self.kwdefaults_override_slot();
        if !ov.is_unset() {
            return ov;
        }
        let mut d: PyDict = PyDict::default();
        for p in &self.params {
            if p.is_keyword_only
                && let Some(def) = &p.default
            {
                d.insert(PyKey::str_from(&p.name), def.clone());
            }
        }
        if d.is_empty() {
            Value::none()
        } else {
            Value::dict(d)
        }
    }

    /// Resolve the default value bound to positional-or-keyword parameter
    /// `params[pi]` at call time, honouring an `f.__defaults__` override (#2395).
    ///
    /// With no override this is just `params[pi].default` (one borrow + null
    /// check — the common case stays off the allocation path).  With an override
    /// tuple of length `n`, CPython aligns it to the *last n* positional params,
    /// so `params[pi]` receives a default only when its position among the
    /// positional params falls inside that trailing window.  An override of
    /// `None` removes all positional defaults.
    pub fn positional_default(&self, pi: usize) -> Option<Value> {
        let ov = self.defaults_override_slot();
        if ov.is_unset() {
            return self.params.get(pi).and_then(|p| p.default.clone());
        }
        let slice = ov.as_tuple()?; // override is `None` → no positional defaults
        // Position of `pi` among the positional params.
        let positions: smallvec::SmallVec<[usize; 8]> = self.positional_param_indices().collect();
        let j = positions.iter().position(|&idx| idx == pi)?;
        let npos = positions.len();
        // CPython aligns the override tuple to the *trailing* positional params:
        // param at positional index `j` maps to tuple index `len - npos + j`.
        // When `len <= npos` this is the leading params getting no default
        // (negative index → `None`); when `len > npos` it skips the *front* of
        // the tuple, so every positional param gets the value from the tail.
        let idx = slice.len() as isize - npos as isize + j as isize;
        if idx >= 0 {
            slice.get(idx as usize).cloned()
        } else {
            None
        }
    }

    /// Resolve the default bound to keyword-only parameter `params[pi]` at call
    /// time, honouring an `f.__kwdefaults__` override (#2395).
    pub fn kwonly_default(&self, pi: usize) -> Option<Value> {
        let param = self.params.get(pi)?;
        let ov = self.kwdefaults_override_slot();
        if ov.is_unset() {
            return param.default.clone();
        }
        ov.as_dict()
            .and_then(|d| d.get(&StrKey(&param.name)).cloned())
    }
}
