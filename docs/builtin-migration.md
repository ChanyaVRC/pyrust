# Migrating built-in callables to `pyrust_module!`

This document is for contributors moving built-in Python callables from
the legacy `match ValueKind::BuiltinFunction("name") => …` cascade in
[crates/pyrust/src/interpreter/runtime/calls.rs](../crates/pyrust/src/interpreter/runtime/calls.rs)
into the file-scoped `pyrust_module!` macro.  Phase 1 (the mechanism)
migrated `math.*` and `sys.exit`; the remaining ~60 arms are scheduled
for phase-2 PRs.

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
under `crates/pyrust/src/builtin_registry_modules/`, where one
`pyrust_module! { … }` invocation generates:

- the unified-signature `fn` for each callable,
- one `BuiltinReg` constant per callable,
- the `REGS: &[BuiltinReg]` slice consumed by the central registry,
- a `module()` constructor consumed by `env.rs::load_module`.

## Anatomy of a module file

Real example — `crates/pyrust/src/builtin_registry_modules/math.rs`:

```rust
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{float_to_bigint, reject_keyword_args_expanded, value_to_float};
use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    name = "math",

    constants {
        "pi"  => Value::float(std::f64::consts::PI),
        "e"   => Value::float(std::f64::consts::E),
        // …
    }

    /// CPython: math.sqrt(x) → float.
    /// <https://docs.python.org/3/library/math.html#math.sqrt>
    fn sqrt(args) -> Result<Value> {
        Ok(Value::float(single_float("math.sqrt", args)?.sqrt()))
    }

    /// CPython: math.pow(x, y) → float.
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded("math.pow", args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime("math.pow() takes exactly two arguments".to_string()));
        }
        let x = value_to_float(&args[0].value, "math.pow")?;
        let y = value_to_float(&args[1].value, "math.pow")?;
        Ok(Value::float(x.powf(y)))
    }
    // … more fns …
}

// Module-local helpers are plain Rust `fn`s outside the macro.
fn single_float(fn_name: &str, args: &[ExpandedCallArg]) -> Result<f64> { /* … */ }
```

The macro expands each `fn sqrt(args) -> Result<Value> { … }` to a
real Rust fn with the canonical signature
`fn math_sqrt(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value>`,
emits a `MATH_SQRT: BuiltinReg = { name: "math.sqrt", dispatch: math_sqrt }`,
collects every such constant into `REGS`, and generates a `module()`
that returns a `PyModule` whose `attrs` are the declared constants
plus each function bound to `Value::builtin_function("math.sqrt")`.

## Where to put a new module

1. **Pick a module name** matching the CPython library name
   (`os`, `os.path`, `functools`, `itertools`, …).
2. **Create
   `crates/pyrust/src/builtin_registry_modules/<name>.rs`**.  Use
   `math.rs` or `sys.rs` as a template.
3. **Register the file in
   [`builtin_registry_modules/mod.rs`](../crates/pyrust/src/builtin_registry_modules/mod.rs)**
   (`pub mod foo;`).
4. **Wire the module into the central registry** by appending
   `all.extend_from_slice(crate::builtin_registry_modules::foo::REGS);`
   inside the `LazyLock` in
   [`builtin_registry.rs`](../crates/pyrust/src/builtin_registry.rs).
5. **Wire `module()` into `load_module`** in
   [`runtime/env.rs`](../crates/pyrust/src/interpreter/runtime/env.rs):
   `"foo" => Some(crate::builtin_registry_modules::foo::module())`.

After these five edits, `import foo; foo.bar()` resolves via the
registry on the call side and via `module()` on the import side, with
the function-name string appearing exactly once (inside the macro).

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
as `pub(crate)` so they're callable from `builtin_registry_modules`.
If you need a helper that's still private, promote it to `pub(crate)`
or inline it into your migrated function.

The `_interp` parameter is injected by the macro at the canonical
position; access the interpreter inside any function as `_interp`.
Bindings like `_interp.env` work because `Interpreter::env` is
`pub(crate)`.

## One-off: `#[pyfunction]`

For migrating a single arm that doesn't justify a whole module file
(e.g. moving `__vcall__` incrementally), you can still use the
per-function attribute form:

```rust
#[pyfunction(name = "module.fn")]
fn module_fn(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    // …
}
```

The expanded output is one `BuiltinReg` constant — you still need to
list it in some `REGS` slice (in a sibling module or
`builtin_registry_modules/mod.rs`).  Prefer `pyrust_module!` when
moving a whole module group.

## Reference

- [`pyrust-derive::pyrust_module`](../crates/pyrust-derive/src/lib.rs)
  — the function-like macro generating the module + REGS + per-fn
  items.
- [`pyrust-derive::pyfunction`](../crates/pyrust-derive/src/lib.rs)
  — the per-function attribute fallback.
- [`builtin_registry`](../crates/pyrust/src/builtin_registry.rs) —
  `BuiltinReg`, `BuiltinDispatchFn`, `lookup`.
- [`builtin_registry_modules/math.rs`](../crates/pyrust/src/builtin_registry_modules/math.rs)
  and
  [`sys.rs`](../crates/pyrust/src/builtin_registry_modules/sys.rs) —
  phase-1 migrated modules; use as templates.
