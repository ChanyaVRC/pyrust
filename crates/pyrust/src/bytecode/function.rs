//! Function prototype metadata and compile-time parameter binding plans.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use smallvec::SmallVec;

use super::{FnCode, Reg};

/// Identifies a named variable that is captured by a nested function via `nonlocal`.
/// These live in the env (not registers) so nested closures can share them.
pub type CellVar = String;

/// Static parameter metadata for a function prototype.  Shared via `Rc` so that
/// `MakeFunction` (which may run on every loop iteration) pays only a refcount
/// bump instead of cloning four separate `Vec`s.
///
/// `SmallVec<[_; 6]>` avoids heap allocation for the common case of functions
/// with six or fewer parameters.
#[derive(Debug, Clone)]
pub struct FnParamSpec {
    pub names: SmallVec<[String; 6]>,
    pub has_default: SmallVec<[bool; 6]>,
    pub is_args: SmallVec<[bool; 6]>,
    pub is_kwargs: SmallVec<[bool; 6]>,
    pub is_keyword_only: SmallVec<[bool; 6]>,
    pub is_positional_only: SmallVec<[bool; 6]>,
}

/// Prototype for a nested function or class body.  Created at compile time,
/// instantiated into a `UserFunction` / class value at runtime via `MakeFunction`
/// / `MakeClass`.
#[derive(Debug, Clone)]
pub struct FnProto {
    /// `Rc<str>` (#2256): every `UserFunction` built from this prototype
    /// `Rc::clone`s these names instead of allocating its own `String`, so all
    /// closures of one `def` share a single name/qualname allocation.
    pub name: Rc<str>,
    /// Fully-qualified name (dotted path) computed at compile time.
    /// For a top-level `class Foo`, this equals `name`.
    /// For `class Outer: class Inner`, this equals `"Outer.Inner"`.
    /// For a class defined inside a function, this equals `"fn.<locals>.ClassName"`.
    /// Used by `MakeClass` to pre-populate the `__qualname__` register slot.
    pub qualname: Rc<str>,
    /// Shared param metadata — `Rc::clone` in `MakeFunction` instead of four `Vec::clone`s.
    pub param_spec: Rc<FnParamSpec>,
    pub code: Rc<FnCode>,
    pub local_index: Rc<HashMap<String, Reg>>,
    /// Precomputed bind target for each parameter (parallel to
    /// `param_spec.names`), resolved once at compile time so the call path binds
    /// positional arguments by register index instead of hashing the parameter
    /// name on every call (issue #1918).  Shared via `Rc` onto every
    /// `UserFunction` built from this prototype.
    pub param_binds: Rc<Vec<pyrust_core::ParamBind>>,
    /// Precomputed self-reference register (the slot the recursive-call name
    /// binds to), or `None` when the function name has no local register slot
    /// or is a cell var.
    pub self_bind: Option<Reg>,
    /// Pre-computed set of local variable names (keys of `local_index`).
    /// Avoids an O(n) `HashSet` rebuild on every `MakeFunction` call.
    pub local_names: Rc<HashSet<String>>,
    pub global_names: Rc<HashSet<String>>,
    pub nonlocal_names: Rc<HashSet<String>>,
    /// True when a call to this function may have its result *cached and reused*
    /// for equal arguments (issue #2523).  Read by the VM's `CallMemo` result
    /// cache (`vm.rs::Insn::CallMemo`). The cache only fires for all-integer
    /// arguments and a scalar result. Derived from
    /// `interpreter::is_memo_pure_body`.
    pub is_memo_pure: bool,
    /// Names for annotation registers passed to `MakeFunction`.  Parallel to
    /// the `annots_base..+annots_n` register window: `annotation_keys[i]` is
    /// the dict key (parameter name or `"return"`) for `R[annots_base + i]`.
    /// Empty when the function has no annotations.
    /// `SmallVec<[_; 4]>` avoids heap allocation for the common case of
    /// functions with four or fewer annotated parameters.
    pub annotation_keys: SmallVec<[String; 4]>,
    /// Docstring extracted from the first statement of the body if it is a
    /// bare string literal (`Stmt::Expr(Expr::Str(...))`), matching CPython's
    /// `co_consts[0]` / `__doc__` extraction.  `None` when no docstring
    /// is present.
    pub docstring: Option<String>,
    /// PEP 487 keyword argument names from the class header (e.g. `key` in
    /// `class Foo(Base, key=val)`).  Parallel to the kwarg value registers in
    /// `MakeClass` (`kwarg_base..kwarg_base+kwarg_n`).  Empty for functions.
    /// `SmallVec<[_; 2]>` avoids heap allocation for the typical case of
    /// zero to two keyword arguments in a class header.
    pub class_kwarg_names: SmallVec<[String; 2]>,
}

/// Resolve each parameter's static bind target once at compile time
/// (issue #1918).  A parameter that is captured as a cell var binds into the
/// local env by name; otherwise it binds into its register slot; a parameter
/// with no local slot (an unused variadic placeholder) binds to nothing.
///
/// Mirrors the per-call decision the binding loop used to make, hoisted out of
/// the hot path so the call only does a direct `match` on the precomputed slot.
pub fn compute_param_binds(
    param_spec: &FnParamSpec,
    local_index: &HashMap<String, Reg>,
    cell_vars: &[CellVar],
) -> Vec<pyrust_core::ParamBind> {
    use pyrust_core::ParamBind;
    param_spec
        .names
        .iter()
        .map(|name| {
            if cell_vars.iter().any(|c| c == name) {
                ParamBind::Cell
            } else if let Some(&reg) = local_index.get(name) {
                ParamBind::Reg(reg)
            } else {
                ParamBind::None
            }
        })
        .collect()
}

/// Resolve the self-reference register for recursive calls once at compile
/// time: the slot the function's own name binds to, unless that name is a cell
/// var or has no local register.
pub fn compute_self_bind(
    name: &str,
    local_index: &HashMap<String, Reg>,
    cell_vars: &[CellVar],
) -> Option<Reg> {
    if cell_vars.iter().any(|c| c == name) {
        None
    } else {
        local_index.get(name).copied()
    }
}
