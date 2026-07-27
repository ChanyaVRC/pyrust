# Migrating built-in callables to `pyrust_module!`

This document is for contributors moving built-in Python callables from
the legacy `match ValueKind::BuiltinFunction("name") => …` cascade in
[crates/pyrust/src/interpreter/runtime/calls.rs](../crates/pyrust/src/interpreter/runtime/calls.rs)
into the file-scoped `pyrust_module!` macro.  The infrastructure landed
with `math.*` and `sys.exit`, and the bulk of the top-level Python
builtins (`abs`, `len`, `print`, `range`, `int`, `isinstance`, `super`,
`getattr`, …) has since followed in
[`builtin_modules/bodies/builtins.rs`](../crates/pyrust/src/builtin_modules/bodies/builtins.rs)
under the `@flat` namespace.  Only **pattern-guarded dispatch** stays
in `calls.rs`:

- The `property` accessor branch matches on the `Value`'s internal
  partial-slot state, not on a registered name, so registry lookup
  can't find it.
- The bound-method dispatch and the `str.*` method dispatch (which keys
  on a name *prefix*, not a fixed string).

Python names that collide with Rust keywords are handled in one of two
ways:

- For keywords that accept the raw-identifier form (`type`, `match`,
  …): write `fn r#type(args)` — the macro strips the `r#` prefix and
  registers as `"type"`.
- For strict keywords with **no** raw-ident form (`super`): add
  `#[py_name = "super"]` above an ordinary Rust ident — e.g.
  `fn super_fn(args)` — and the macro uses the override for the
  Python-level name while keeping the Rust ident unchanged.

## Why this exists

The legacy dispatch had three growing pains:

1. **One file balloons.**  `calls.rs::call_function_expanded` had grown
   to 3000+ lines because every new built-in added an arm.
2. **String match cascade.**  73 arms keyed on `BuiltinFunction("name")`
   — the compiler optimises this to a jump table, but the source is
   hard to scan and any new arm touches the same hot file.
3. **Drift between three sources of truth.**  Adding `math.foo`
   required (a) a `BuiltinFunction("math.foo")` arm in `calls.rs`,
   (b) a `Value::builtin_function("math.foo")` entry in
   `helpers.rs::make_math_module`, and (c) the Python name appearing
   in both as a string literal that had to match exactly.

The new pattern moves each module's declarations into a single file
under `crates/pyrust/src/builtin_modules/`, where one
`pyrust_module! { … }` invocation generates:

- the unified-signature `fn` for each callable,
- one `BuiltinReg` constant per callable,
- the `REGS: &[BuiltinReg]` slice consumed by the central registry,
- a `module()` constructor consumed by `env.rs::load_module`.

## Anatomy of a module body

Real example — `crates/pyrust/src/builtin_modules/bodies/math.rs`:

```rust
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{float_to_bigint, reject_keyword_args_expanded, value_to_float};
use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "pi"  => Value::float(std::f64::consts::PI),
        "e"   => Value::float(std::f64::consts::E),
        // …
    }

    /// CPython: math.sqrt(x) → float.
    /// <https://docs.python.org/3/library/math.html#math.sqrt>
    fn sqrt(args) -> Result<Value> {
        Ok(Value::float(single_float(FN_NAME, args)?.sqrt()))
    }

    /// CPython: math.pow(x, y) → float.
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly two arguments")));
        }
        let x = value_to_float(&args[0].value, FN_NAME)?;
        let y = value_to_float(&args[1].value, FN_NAME)?;
        Ok(Value::float(x.powf(y)))
    }
    // … more fns …
}

// Module-local helpers are plain Rust `fn`s outside the macro.
fn single_float(fn_name: &str, args: &[ExpandedCallArg]) -> Result<f64> { /* … */ }
```

**Note:** `pyrust_module!` does *not* take a `name` field.  The module's
Python-level name is injected from `mod.rs::pyrust_builtin_modules!`
as a sibling `MODULE_NAME: &str` constant.  Inside the macro body,
`FN_NAME` is also auto-injected per fn as `&str` equal to
`"<MODULE_NAME>.<short>"`, so error messages and helper calls
reference a single source of truth.

The macro expands each `fn sqrt(args) -> Result<Value> { … }` to a
real Rust fn with the canonical signature
`fn __pyfn_sqrt(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value>`,
adds it to a per-module `regs() -> &'static [BuiltinReg]` (whose
names are leaked at first use from `MODULE_NAME + ".sqrt"`), and
generates a `module() -> Value` returning a `PyModule` whose `attrs`
are the declared constants plus each function bound to
`Value::builtin_function("math.sqrt")`.

## Where to put a new module

1. **Pick a module name** matching the CPython library name
   (`os`, `os.path`, `functools`, `itertools`, …).
2. **Create
   `crates/pyrust/src/builtin_modules/bodies/<ident>.rs`**.
   Use `bodies/math.rs` or `bodies/sys.rs` as a template — note the
   file has no `mod` declaration and no `name = "..."` literal.
3. **Add the module to the list in
   [`builtin_modules/mod.rs`](../crates/pyrust/src/builtin_modules/mod.rs)**:

   ```rust
   pyrust_builtin_modules! {
       math,
       sys,
       <ident>,                          // simple name, file: bodies/<ident>.rs
       "py.dotted.name" as <ident>,      // dotted Python name (e.g. "os.path")
   }
   ```

That's it.  **The module name appears only on this one line.**  The
`pyrust_builtin_modules!` macro:

- creates `pub mod <ident> { … }` with an injected
  `MODULE_NAME: &str = "<py.name>"` constant,
- `include!`s `bodies/<ident>.rs` into that module,
- contributes the module's `regs()` to `all_regs()` (consumed by
  `builtin_registry::REGISTRY`),
- adds a branch to `load_builtin_module` keyed on the Python-level
  name (consumed by `env.rs::load_module`).

After this single edit, `import <name>; <name>.foo()` resolves on
both the import path and the call dispatch.  The body file never
mentions its own module name — `FN_NAME` (per fn) and `MODULE_NAME`
(once per module, in `mod.rs`) are the only source-of-truth.

## When `pyrust_module!` doesn't fit

Some legacy arms key on the same function but with very different
shapes (e.g. `min` and `max` share one arm with an `is_max` boolean).
For those, either:

1. Split into two functions that delegate to a shared helper inside
   the macro, or
2. Keep the legacy arm in `calls.rs` for now; the registry probe at
   the top of `call_function_expanded` falls through to the cascade
   when the name isn't registered.

There is **no flag day** — incremental migration is safe.

## Helpers visibility

Most arm bodies in `calls.rs` use helpers like
`reject_keyword_args_expanded`, `value_to_float`, `float_to_bigint`,
`instantiate_exception`, `lookup_name_in_module`.  These are exposed
as `pub(crate)` so they're callable from `builtin_modules`.
If you need a helper that's still private, promote it to `pub(crate)`
or inline it into your migrated function.

The `_interp` parameter is injected by the macro at the canonical
position; access the interpreter inside any function as `_interp`.
Bindings like `_interp.env` work because `Interpreter::env` is
`pub(crate)`.

## One-off: `#[pyfunction]`

For migrating a single arm that doesn't justify a whole module file
(e.g. moving one legacy compatibility service incrementally), you can still use the
per-function attribute form:

```rust
#[pyfunction(name = "module.fn")]
fn module_fn(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    // …
}
```

The expanded output is one `BuiltinReg` constant — you still need to
list it in some `REGS` slice (in a sibling module or
`builtin_modules/mod.rs`).  Prefer `pyrust_module!` when
moving a whole module group.

## Reference

- [`pyrust-derive::pyrust_module`](../crates/pyrust-derive/src/lib.rs)
  — the function-like macro generating the module + REGS + per-fn
  items.
- [`pyrust-derive::pyfunction`](../crates/pyrust-derive/src/lib.rs)
  — the per-function attribute fallback.
- [`builtin_registry`](../crates/pyrust/src/builtin_registry.rs) —
  `BuiltinReg`, `BuiltinDispatchFn`, `lookup`.
- [`builtin_modules/bodies/math.rs`](../crates/pyrust/src/builtin_modules/bodies/math.rs),
  [`sys.rs`](../crates/pyrust/src/builtin_modules/bodies/sys.rs),
  [`builtins.rs`](../crates/pyrust/src/builtin_modules/bodies/builtins.rs) —
  migrated modules; use as templates.
