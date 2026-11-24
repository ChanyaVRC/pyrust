// ─────────────────────────────────────────────────────────────────────────────
// Shared types
// ─────────────────────────────────────────────────────────────────────────────

pub type NameSet = Rc<HashSet<String>>;

/// Shared, reference-counted mutable cell backing one captured function local
/// (issues #452 / #2339 / #2348).
///
/// Replaces the old name-keyed `Environment::values` lookup for closures,
/// `nonlocal`, and `function.__closure__`: both the owning function frame and
/// every nested closure / `cell` introspection object hold clones of the same
/// `CellSlot`, so reads/writes/deletes execute in O(1) by compile-time slot
/// index without hashing, string comparison, or parent-chain walks.
///
/// `None` represents an unbound or deleted cell (raising `UnboundLocalError` /
/// `NameError` in bytecode or `ValueError: Cell is empty` on
/// `cell.cell_contents`).
#[derive(Debug, Clone)]
pub struct CellSlot(Rc<RefCell<Option<Value>>>);

impl CellSlot {
    /// Allocate an initially unbound cell slot.
    #[inline]
    pub fn new_empty() -> Self {
        Self(Rc::new(RefCell::new(None)))
    }

    /// Allocate a cell slot initialized with `val`.
    #[inline]
    pub fn new(val: Value) -> Self {
        Self(Rc::new(RefCell::new(Some(val))))
    }

    /// Clone the current value if bound, or `None` if empty.
    #[inline]
    pub fn get(&self) -> Option<Value> {
        self.0.borrow().clone()
    }

    /// Bind or rebind this cell slot to `val`.
    #[inline]
    pub fn set(&self, val: Value) {
        *self.0.borrow_mut() = Some(val);
    }

    /// Clear this cell slot and return the previous value, if any.
    #[inline]
    pub fn take(&self) -> Option<Value> {
        self.0.borrow_mut().take()
    }

    /// Return `true` when this cell slot currently holds a value.
    #[inline]
    pub fn is_bound(&self) -> bool {
        self.0.borrow().is_some()
    }

    /// Inspect the cell's current value without cloning it.
    #[inline]
    pub fn with_value<R>(&self, f: impl FnOnce(Option<&Value>) -> R) -> R {
        let borrow = self.0.borrow();
        f(borrow.as_ref())
    }

    /// Return `true` if `self` and `other` point to the exact same cell backing.
    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Stable address of the underlying cell allocation (for Python `cell` identity).
    #[inline]
    pub fn as_ptr(&self) -> *const RefCell<Option<Value>> {
        Rc::as_ptr(&self.0)
    }
}

/// Per-scope namespace storage for [`Environment`] (issues #452 / #3162).
///
/// Every `Environment` owns a name-keyed namespace dictionary (module, class
/// body, PEP 695 type-parameter scope, or explicit `exec`/`eval` namespace).
/// Function frames do not allocate an `Environment` at all; their cells live
/// directly on the active VM frame / `GeneratorFrame` and
/// `UserFunction::closure_cells`.
#[derive(Debug, Clone)]
pub enum EnvironmentStorage {
    /// Name-keyed bindings for module, class, type-param, and `exec`/`eval` scopes.
    Namespace(EnvValues),
}

impl Default for EnvironmentStorage {
    fn default() -> Self {
        Self::Namespace(EnvValues::new())
    }
}

/// Number of `(name, value)` bindings an `EnvValues` keeps inline before it
/// promotes to a hashed map.  A closure scope captures very few cells (the
/// variables that escape into a nested function — almost always 1 or 2), so the
/// inline form holds them with zero heap allocation; a module scope, which
/// accumulates many globals, crosses this threshold once and promotes.
///
/// `2` is the measured sweet spot for the 200k-closure RSS benchmark (issue
/// #452): it keeps both the single-capture and two-capture cases fully inline
/// without spilling to the heap, while a larger inline buffer wastes per-env
/// bytes on the common single-capture closure.  Each inline entry is
/// `(Rc<str>, Value)` = 24 bytes, so the inline buffer is 48 bytes.
const ENV_INLINE_CAP: usize = 2;

/// The name-keyed half of a scope's bindings (issue #452).
///
/// Closures and generators dominate `Environment` allocation, and each one
/// previously paid for a full `HashMap<String, Value>` — a heap bucket array
/// plus an owned `String` key — even to capture a single cell variable. That
/// HashMap is the bulk of the ~2.3× closure / ~2.2× generator RSS gap vs
/// CPython (issue #452, #2256).
///
/// `EnvValues` keeps the bindings of a small scope inline in a
/// `SmallVec<[(Rc<str>, Value); ENV_INLINE_CAP]>` (no heap allocation for the
/// common 1–2 cell capture) and shares the name string via `Rc<str>` rather
/// than owning a per-scope `String`. A scope that grows past `ENV_INLINE_CAP`
/// (the module namespace, a class body with many attributes) promotes to a
/// hashed `IndexMap<Rc<str>, Value>`, so module-global lookups keep their
/// near-O(1) cost.
///
/// Both representations are **insertion ordered** (issue #2903): a module
/// namespace materialised from these bindings is a Python dict, and CPython
/// guarantees dict order. Rebinding an existing name keeps its position;
/// removing and re-inserting moves it to the end, exactly like `dict`.
///
/// The public surface is deliberately small — `get` / `insert` / `remove` /
/// `clear` / `iter` / `keys` / `values` / `is_empty` / `len` /
/// `get_or_insert_with` — mirroring the slice of the `HashMap` API the runtime
/// actually used, so the existing four lookup paths (THE RULE; see
/// `interpreter/helpers.rs`) carry over unchanged.
#[derive(Debug, Clone)]
pub enum EnvValues {
    /// Few bindings, stored inline (function / closure / generator scopes).
    Inline(smallvec::SmallVec<[(Rc<str>, Value); ENV_INLINE_CAP]>),
    /// Many bindings (module / class scope), hashed by name and kept in
    /// insertion order.
    Map(IndexMap<Rc<str>, Value, FxBuildHasher>),
}

impl Default for EnvValues {
    fn default() -> Self {
        EnvValues::Inline(smallvec::SmallVec::new())
    }
}

impl EnvValues {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            EnvValues::Inline(v) => v.iter().find(|(k, _)| &**k == name).map(|(_, val)| val),
            EnvValues::Map(m) => m.get(name),
        }
    }

    /// Insert (or overwrite) a binding. Promotes Inline→Map when the inline
    /// store would exceed `ENV_INLINE_CAP` distinct keys.
    #[inline]
    pub fn insert(&mut self, name: &str, value: Value) {
        match self {
            EnvValues::Inline(v) => {
                if let Some(slot) = v.iter_mut().find(|(k, _)| &**k == name) {
                    slot.1 = value;
                    return;
                }
                if v.len() >= ENV_INLINE_CAP {
                    self.promote();
                    if let EnvValues::Map(m) = self {
                        m.insert(Rc::from(name), value);
                    }
                    return;
                }
                v.push((Rc::from(name), value));
            }
            EnvValues::Map(m) => {
                m.insert(Rc::from(name), value);
            }
        }
    }

    /// Update an existing binding in place without inserting a new key.
    ///
    /// Returns `true` if `name` was already present and updated, or `false` if
    /// absent. Used by `SyncModuleGlobal` so a conservative
    /// `env_binding_interest` Bloom false positive never allocates an
    /// `Rc<str>` or inserts an absent fast-local name into `EnvValues`, which
    /// would perturb Python-visible namespace ordering (issue #2925).
    #[inline]
    pub fn update_if_present(&mut self, name: &str, value: Value) -> bool {
        match self {
            EnvValues::Inline(v) => {
                if let Some(slot) = v.iter_mut().find(|(k, _)| &**k == name) {
                    slot.1 = value;
                    return true;
                }
                false
            }
            EnvValues::Map(m) => {
                if let Some(slot) = m.get_mut(name) {
                    *slot = value;
                    return true;
                }
                false
            }
        }
    }

    #[inline]
    pub fn remove(&mut self, name: &str) -> Option<Value> {
        match self {
            EnvValues::Inline(v) => v
                .iter()
                .position(|(k, _)| &**k == name)
                .map(|i| v.remove(i).1),
            // `shift_remove` keeps the surviving bindings in insertion order,
            // so a later re-insertion appends like `del d[k]; d[k] = v` does.
            EnvValues::Map(m) => m.shift_remove(name),
        }
    }

    /// Like `HashMap::entry(..).or_insert_with(..)` for the `&str` key case:
    /// returns a mutable reference to the existing value, or inserts the result
    /// of `f` and returns a reference to it.
    #[inline]
    pub fn get_or_insert_with(&mut self, name: &str, f: impl FnOnce() -> Value) -> &mut Value {
        // Promote first if the inline store is full and the key is absent, so
        // the returned reference stays valid for the whole match.
        if let EnvValues::Inline(v) = self
            && v.len() >= ENV_INLINE_CAP
            && !v.iter().any(|(k, _)| &**k == name)
        {
            self.promote();
        }
        match self {
            EnvValues::Inline(v) => {
                if let Some(pos) = v.iter().position(|(k, _)| &**k == name) {
                    &mut v[pos].1
                } else {
                    v.push((Rc::from(name), f()));
                    &mut v.last_mut().unwrap().1
                }
            }
            EnvValues::Map(m) => m.entry(Rc::from(name)).or_insert_with(f),
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        match self {
            EnvValues::Inline(v) => v.clear(),
            // Reset to the inline form so a pooled env that briefly grew large
            // does not keep an oversized map alive after reuse.
            EnvValues::Map(_) => *self = EnvValues::new(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        match self {
            EnvValues::Inline(v) => v.is_empty(),
            EnvValues::Map(m) => m.is_empty(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            EnvValues::Inline(v) => v.len(),
            EnvValues::Map(m) => m.len(),
        }
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = (&str, &Value)> + '_> {
        match self {
            EnvValues::Inline(v) => Box::new(v.iter().map(|(k, val)| (&**k, val))),
            EnvValues::Map(m) => Box::new(m.iter().map(|(k, val)| (&**k, val))),
        }
    }

    pub fn keys(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            EnvValues::Inline(v) => Box::new(v.iter().map(|(k, _)| &**k)),
            EnvValues::Map(m) => Box::new(m.keys().map(|k| &**k)),
        }
    }

    pub fn values(&self) -> Box<dyn Iterator<Item = &Value> + '_> {
        match self {
            EnvValues::Inline(v) => Box::new(v.iter().map(|(_, val)| val)),
            EnvValues::Map(m) => Box::new(m.values()),
        }
    }

    /// Convert an `Inline` store to a `Map` (one-way; callers only promote when
    /// the scope is known to be growing).
    fn promote(&mut self) {
        if let EnvValues::Inline(v) = self {
            let mut m: IndexMap<Rc<str>, Value, FxBuildHasher> =
                IndexMap::with_capacity_and_hasher(v.len() * 2, FxBuildHasher);
            for (k, val) in v.drain(..) {
                m.insert(k, val);
            }
            *self = EnvValues::Map(m);
        }
    }
}
