// ─────────────────────────────────────────────────────────────────────────────
// Shared backing storage for mutable Tier 1 containers
//
// Lists and sets share their items behind an `Rc<…RefCell…>` so that
// `Value::clone` preserves Python's reference semantics for mutable
// containers: a copy of the Value points at the same backing storage as the
// original, mutations through either alias propagate, and `id(a) == id(b)`
// after `b = a`.  Dict already used this shape (`Rc<RefCell<IndexMap<…>>>`
// inside `Opaque::Dict`); see issue #305.
// ─────────────────────────────────────────────────────────────────────────────

/// Shared backing for a Python `list`.  `items` holds the elements; `obj_id`
/// is a monotonic identity captured at construction and inherited by every
/// `Rc::clone` so `id(x) == id(y)` whenever `y` is an aliased clone of `x`.
pub struct ListInner {
    pub items: RefCell<Vec<Value>>,
    pub obj_id: u64,
}

/// Shared backing for a Python `tuple` (heap variant: the empty tuple and
/// tuples of 4 or more elements).  Tuples are
/// immutable, so the backing is a plain `Vec<Value>` behind an `Rc` with no
/// `RefCell` — `Value::clone` / `Insn::Move` is an O(1) strong-count bump that
/// shares the backing instead of deep-copying it (#2268).  `obj_id` is captured
/// at construction and inherited by every aliased clone so `id(x) == id(y)` for
/// `y = x`.  The 1- to 3-element shapes stay inline in
/// `Opaque::SmallTuple1/2/3`.
pub struct TupleInner {
    pub items: Vec<Value>,
    pub obj_id: u64,
}

/// Shared backing for a Python `set`.  Same shape and rationale as
/// [`ListInner`]; `items` is an [`IndexSet`] (insertion-ordered) so iteration
/// order matches the rest of the interpreter's set surface.
pub struct SetInner {
    pub items: RefCell<PySet>,
    pub obj_id: u64,
}
