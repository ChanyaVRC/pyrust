//! Typed argument wrappers + parsing for `pyrust_module!` builtins.
//!
//! # Wrappers
//!
//! Each wrapper (e.g. [`PyInt`], [`PyFloat`], [`PyStr`]) implements
//! [`FromValue`], which validates a `Value` against **exactly one** Python
//! type and produces a typed Rust local.  Wrappers are **strict 1:1** with
//! Python types: `PyFloat` accepts only `float`, `PyInt` accepts only `int`,
//! etc. — no implicit promotion.  See [`PyValue`] for the catch-all.
//!
//! # Overload dispatch
//!
//! Because wrappers are strict, builtins that handle multiple type
//! combinations (e.g. `pow(int, int)` vs `pow(float, float)`) declare one
//! `fn` per combination; `pyrust_module!` groups them by Python-level name
//! and generates a dispatcher that picks the first overload whose
//! parameters all match.  The [`FromValue::matches`] predicate is the
//! allocation-free type check used by the dispatcher.
//!
//! # Generated prelude (single-body case)
//!
//! For a typed-signature builtin the macro generates a prelude that:
//!
//! 1. Rejects unknown keyword arguments.
//! 2. Checks min / max positional arg counts.
//! 3. Resolves each parameter from positional + keyword args, applying
//!    `#[default(...)]` if supplied.
//! 4. Calls `FromValue::try_from_value` for the strict type check.
//! 5. Binds the result as a local with the parameter's name.
//!
//! After the prelude, the user-written body sees typed locals (`path: PyStr`,
//! `mode: PyStr`, etc.) and can call straightforwardly into native Rust APIs.
//!
//! # `PyIterable` — "anything iterable" argument
//!
//! [`PyIterable`] is the wrapper for builtins whose canonical signature
//! is "anything iterable" — `list()`, `tuple()`, `set()`, `dict()`,
//! `sum()`, `min()`, `max()`, `any()`, `all()`, `sorted()`,
//! `reversed()`, `map()`, `filter()`, `zip()`, `enumerate()`, `iter()`,
//! `next()`, etc.  It materialises the source into a `Vec<Value>` at
//! `try_from_value` time (eager, matching the existing
//! `pyrust_builtins::iter_helpers` shape).  A future lazy `PyIter<'a>`
//! variant can be added if profiles show the materialisation cost
//! matters.
//!
//! Materialisation routes through
//! [`pyrust_core::iter_values_via_registry`], which the interpreter
//! installs at startup ([`Interpreter::default`] in
//! `crates/pyrust/src/interpreter.rs`).  That removes the need for an
//! interpreter handle on the [`FromValue`] trait — the wrapper drains
//! lists, tuples, dicts, sets, strings, bytes, ranges, generators,
//! iterable `BuiltinObject`s, and user-class `PyInstance`s with
//! `__iter__` via the same path the rest of the interpreter uses.
//!
//! Per-builtin migrations off the legacy `(args)` dialect are tracked
//! under #400; landing the wrapper alone (this module) is #398.

use std::borrow::Cow;
use std::ops::Deref;
use std::rc::Rc;

use smallvec::SmallVec;

use crate::error::{PyError, Result};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};

use super::ExpandedCallArg;

/// Inline storage for the positional-args list a typed builtin's
/// dispatcher prelude collects from `validate_kwargs_and_collect_positional`.
/// All migrated builtins so far have ≤ 4 parameters, so the `Vec` path
/// is heap-free for them; longer signatures still work via the
/// `SmallVec` overflow spill.  Sized at 4 to match the
/// `Interpreter::call_arg_buf` budget used elsewhere in this crate.
///
/// Per-call hot-path benchmarks (see PR following #403) showed the
/// previous `Vec::with_capacity(args.len())` was the dominant per-call
/// cost (~8 ns/call), shading every Tier 1 migration.  Replacing with
/// `SmallVec` eliminates the alloc for the common case.
pub(crate) type PositionalArgs<'a> = SmallVec<[&'a ExpandedCallArg; 4]>;

// Typed builtin argument handling grouped by conversion responsibility.

include!("builtin_args/common.rs");
include!("builtin_args/scalars.rs");
include!("builtin_args/containers.rs");
include!("builtin_args/iterables.rs");
include!("builtin_args/optional.rs");
include!("builtin_args/binding.rs");
include!("builtin_args/arity.rs");
include!("builtin_args/tests.rs");
