//! Keyword-call binding and expanded-call shape cache protocol.

use smallvec::SmallVec;

/// Per-call-site inline cache for `Insn::CallKw` (issue #2382).
///
/// A keyword call `f(a=1, b=2, c=3)` binds each keyword argument to a parameter
/// by name.  The slow path linearly scans `function.params` for every keyword
/// on every call — O(nkw × nparams) string comparisons.  This cache records,
/// once per call site, the parameter index each keyword name maps to, so the
/// binder can write each keyword value straight into its slot with no string
/// comparison and no defaults/missing scan when the cached shape matches.
///
/// Identity guard: `param_binds_ptr` is a weak reference to
/// `function.param_binds`. `param_binds` is shared (via `Rc`) across every
/// closure produced by the same `def`, and is immutable, so its allocation is a stable identity for
/// "this exact function prototype".  Two closures from one `def` share the
/// pointer → hit (correct: same params); a different function → different
/// pointer → miss.  No version/epoch is needed because `param_binds` never
/// mutates after construction.
///
/// `slots[i]` is the parameter index that the `i`-th keyword name (in the call
/// site's `kwnames` tuple order) binds to.  Filled only when the cached call is
/// *simple*: every keyword maps to a distinct, non-positional-only,
/// non-keyword-collecting parameter, the positionals exactly fill the leading
/// params, and no parameter is bound twice.  Any deviation (unexpected keyword,
/// duplicate, positional-only-as-keyword, missing required, **kwargs param,
/// arity mismatch) marks the site `Fallback` so it permanently takes the
/// general binder, which owns the CPython-parity diagnostics.
///
/// The cache retains a weak allocation identity for `param_binds`.  A call-site
/// `FnCode` may outlive a previously observed callee, so a bare pointer would
/// permit allocator-address reuse to apply the old slot plan to a different
/// signature.  `Weak` prevents that ABA collision without retaining the
/// function strongly.
#[derive(Clone)]
pub enum KwCallCacheEntry {
    /// No observation yet.
    Empty,
    /// Monomorphic: one function prototype seen, and its binding is simple.
    /// `slots[i]` = param index for keyword `i`; `npos` positional args fill
    /// params `0..npos`.  Validated by `param_binds_ptr` identity.
    Simple {
        param_binds_ptr: std::rc::Weak<Vec<pyrust_core::ParamBind>>,
        npos: u8,
        /// One param index per keyword name, in `kwnames` tuple order.
        slots: SmallVec<[u32; 4]>,
    },
    /// This site is not simple (or went polymorphic) — always use the general
    /// binder.  Set permanently; never re-filled.
    Fallback,
    /// `Insn::CallEx` (`f(**d)`) monomorphic shape cache (issue #2393).  The
    /// `**d` keys are dynamic, so in addition to the `param_binds_ptr` callee
    /// identity this records the exact `keyset` last observed for the splat dict
    /// (its `str` keys in iteration order).  On a hit — same callee prototype,
    /// same `npos`, and the dict's keys equal `keyset` in order — the keyword
    /// values bind straight into `slots` (the parameter index for each key, in
    /// `keyset` order), reusing the #2382 fast bind with no dict copy and no name
    /// scan.  Any key-set change re-resolves (re-fills) rather than pinning to
    /// `Fallback`, so a call site cycling over a small number of stable shapes
    /// still gets the fast bind on the shape it most recently saw.
    ExSimple {
        param_binds_ptr: std::rc::Weak<Vec<pyrust_core::ParamBind>>,
        npos: u8,
        /// The `**d` dict's `str` keys, in iteration order, for the shape guard.
        keyset: SmallVec<[Box<str>; 4]>,
        /// One param index per key in `keyset` order.
        slots: SmallVec<[u32; 4]>,
    },
    /// `Insn::CallExArgs` (`f(<pos…>, *args[, **kw])`) monomorphic shape cache.
    /// Both the `*args` length and the `**kw` keys are dynamic, so alongside the
    /// `param_binds_ptr` callee identity this records the exact `total_pos`
    /// (leading positionals + splat length) and `**kw` `keyset` last observed.  On
    /// a hit — same callee prototype, same `total_pos`, and the dict's keys equal
    /// `keyset` in order — the positional and keyword values bind straight into
    /// `slots`.  A `total_pos` or key-set change re-resolves (re-fills) rather than
    /// pinning `Fallback`, so a wrapper forwarding a varying number of positionals
    /// still fast-binds on the arity it most recently saw.  `keyset` is empty when
    /// the call has no `**kw`.
    ExArgs {
        param_binds_ptr: std::rc::Weak<Vec<pyrust_core::ParamBind>>,
        total_pos: u32,
        /// The `**kw` dict's `str` keys, in iteration order (empty if no `**kw`).
        keyset: SmallVec<[Box<str>; 4]>,
        /// One param index per keyword in `keyset` order.
        slots: SmallVec<[u32; 4]>,
    },
    /// `Insn::CallExArgs` where the callee is a VARIADIC (`*args`/`**kwargs`)
    /// plain user function — the decorator-chain forward shape
    /// `wrapper(*a,**k) -> inner(*args, **kw)`.  Such callees can't fast-bind into
    /// fixed slots (the general `kwcall_resolve_simple` rejects them), but the
    /// splat handler can still skip the `ExpandedCallArg` buffer + the second
    /// per-arg clone by feeding the leading positionals + splat elements and the
    /// `**kw` entries STRAIGHT into `call_user_function_variadic_split`.  Only the
    /// `param_binds_ptr` callee identity is cached (the arg counts / keys are
    /// re-read each call); a polymorphic site whose callee prototype changes
    /// re-resolves.
    ///
    /// `pure_forward` records the once-detected callee shape: `true` iff the
    /// callee's params are exactly a single `*args` plus an optional `**kwargs`
    /// and NOTHING else (no fixed positional / keyword-only / positional-only
    /// params) — the pure `def inner(*A)` / `def inner(*A, **K)` forward target.
    /// On a hit the splat handler builds the callee's `*A` tuple and `**K` dict
    /// DIRECTLY and binds them into the two param registers, skipping the
    /// `positional_vals` / `keyword_vals` / `param_vals` intermediate vectors
    /// (#2852).  `false` keeps the generic `call_user_function_variadic_split`
    /// forward.
    ExArgsVariadic {
        param_binds_ptr: std::rc::Weak<Vec<pyrust_core::ParamBind>>,
        pure_forward: bool,
    },
}

impl std::fmt::Debug for KwCallCacheEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KwCallCacheEntry::Empty => write!(f, "Empty"),
            KwCallCacheEntry::Simple { npos, slots, .. } => {
                write!(f, "Simple {{ npos: {npos}, slots: {slots:?} }}")
            }
            KwCallCacheEntry::Fallback => write!(f, "Fallback"),
            KwCallCacheEntry::ExSimple {
                npos,
                keyset,
                slots,
                ..
            } => {
                write!(
                    f,
                    "ExSimple {{ npos: {npos}, keyset: {keyset:?}, slots: {slots:?} }}"
                )
            }
            KwCallCacheEntry::ExArgs {
                total_pos,
                keyset,
                slots,
                ..
            } => {
                write!(
                    f,
                    "ExArgs {{ total_pos: {total_pos}, keyset: {keyset:?}, slots: {slots:?} }}"
                )
            }
            KwCallCacheEntry::ExArgsVariadic { .. } => write!(f, "ExArgsVariadic"),
        }
    }
}
