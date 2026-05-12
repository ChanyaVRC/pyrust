# Migrating built-in callables to `#[pyfunction]`

This document is for contributors moving built-in Python callables from the
legacy `match ValueKind::BuiltinFunction("name") => …` cascade in
[crates/pyrust/src/interpreter/runtime/calls.rs](../crates/pyrust/src/interpreter/runtime/calls.rs)
into the new `#[pyfunction]` registry.  Phase 1 (the introduction of the
mechanism) migrated `math.*` and `sys.exit` as proof; the remaining ~60
arms are scheduled for phase-2 PRs.

## Why this exists

The legacy dispatch had three growing pains:

1. **One file balloons.**  `calls.rs::call_function_expanded` had grown to
   3000+ lines because every new built-in added an arm.
2. **String match cascade.**  73 arms key on `BuiltinFunction("name")` —
   the compiler optimises this to a jump table, but the source is hard to
   scan and any new arm requires touching the same hot file.
3. **Drift between declaration and dispatch.**  Module shells in
   [helpers.rs](../crates/pyrust/src/interpreter/helpers.rs) list a name
   (`Value::builtin_function("math.sqrt")`) that must match a string in
   `calls.rs`.  A typo breaks dispatch silently.

The new pattern moves each built-in to its own free `fn`, annotated with
`#[pyfunction(name = "module.fn")]` and collected into a static registry
that `call_function_expanded` consults via O(log n) binary search.

## Anatomy of a migration

Example — `math.sqrt`.

### 1. Find the legacy arm

```rust
ValueKind::BuiltinFunction("math.sqrt") => {
    reject_keyword_args_expanded("math.sqrt", args)?;
    if args.len() != 1 {
        return Err(PyError::Runtime("math.sqrt() takes exactly one argument".into()));
    }
    let x = value_to_float(&args[0].value, "math.sqrt")?;
    Ok(Value::float(x.sqrt()))
}
```

### 2. Choose the target module

[crates/pyrust/src/builtin_registry_modules/](../crates/pyrust/src/builtin_registry_modules/)
holds one file per logical group:

- `math.rs` — `math.*`
- `sys.rs` — `sys.*`
- (add new files for `os.path`, `functools`, `itertools`, `collections`,
  `pure_builtins` for `abs`/`len`/etc., …)

### 3. Write the annotated function

```rust
/// CPython: `math.sqrt(x)` → float.
/// <https://docs.python.org/3/library/math.html#math.sqrt>
#[pyfunction(name = "math.sqrt")]
fn math_sqrt(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded("math.sqrt", args)?;
    if args.len() != 1 {
        return Err(PyError::Runtime("math.sqrt() takes exactly one argument".into()));
    }
    let x = value_to_float(&args[0].value, "math.sqrt")?;
    Ok(Value::float(x.sqrt()))
}
```

**Signature rules:**

- The function name (`math_sqrt`) is a snake_case Rust identifier; it
  doesn't have to mirror the Python name exactly.
- The Python name in `name = "..."` is what dispatch matches.
- The signature must be exactly
  `fn(&mut Interpreter, &[ExpandedCallArg]) -> Result<Value>`.
  Even pure built-ins that don't read the interpreter take `_interp:
  &mut Interpreter` and ignore it — uniform signatures let the registry
  store one `fn` pointer type.
- **Quote the CPython doc URL** in a `///` comment above the function.
  Reviewers and future maintainers verify against the same source.

### 4. Register in the module's `REGS` slice

```rust
pub(crate) const REGS: &[BuiltinReg] = &[
    // … other entries …
    MATH_SQRT,
    // …
];
```

The `#[pyfunction]` macro emits a `pub const MATH_SQRT: BuiltinReg = …`
automatically — you just list it.  The constant name is the
SCREAMING_SNAKE_CASE of the Rust function name.

### 5. Wire the module into the central registry

Edit [`builtin_registry.rs`](../crates/pyrust/src/builtin_registry.rs):

```rust
static REGISTRY: LazyLock<Vec<BuiltinReg>> = LazyLock::new(|| {
    let mut all: Vec<BuiltinReg> = Vec::new();
    all.extend_from_slice(crate::builtin_registry_modules::math::REGS);
    all.extend_from_slice(crate::builtin_registry_modules::sys::REGS);
    // add your new module here ↓
    all.extend_from_slice(crate::builtin_registry_modules::your_module::REGS);
    all.sort_by_key(|r| r.name);
    // …
});
```

### 6. Remove the legacy arm

Replace the arm in `calls.rs::call_function_expanded` with a one-line
breadcrumb comment so reviewers can see where it went:

```rust
// `math.sqrt` migrated to `crate::builtin_registry_modules::math`.
```

### 7. Verify

```bash
cargo test --release
```

The `builtin_registry::tests::lookup_finds_a_known_builtin` smoke test
confirms the registry is wired; the parity-compare test exercises real
Python behaviour.

## Helper visibility

Most arm bodies in `calls.rs` use private helpers like
`reject_keyword_args_expanded`, `value_to_float`, `float_to_bigint`,
`instantiate_exception`, `lookup_name_in_module`.  These have been
exposed as `pub(crate)` so they're callable from
`builtin_registry_modules`.  If you need a helper that's still private,
either:

1. Promote it to `pub(crate)` (preferred when the helper is generic
   utility code).
2. Inline it into your migrated function (preferred when the helper is
   one-off and tightly coupled to a single arm).

## Things to *not* migrate yet

The registry probe sits **before** the legacy `match` in
`call_function_expanded`.  Arms that aren't yet migrated still work via
the cascade — there is no flag day.  That makes incremental migration
safe.

However, **don't migrate arms that match across multiple names with
shared local state** (e.g. `min` and `max` share one arm with an
`is_max` boolean).  Either:

- Split them into two functions that delegate to a shared helper, or
- Leave the multi-name arm alone for now and migrate it last.

## Reference

- [`pyrust-derive::pyfunction`](../crates/pyrust-derive/src/lib.rs) —
  the proc-macro that emits `BuiltinReg` constants.
- [`builtin_registry`](../crates/pyrust/src/builtin_registry.rs) — the
  `BuiltinReg` type, `BuiltinDispatchFn` signature, and `lookup` entry
  point.
- [`builtin_registry_modules/math.rs`](../crates/pyrust/src/builtin_registry_modules/math.rs)
  and [`sys.rs`](../crates/pyrust/src/builtin_registry_modules/sys.rs)
  — phase-1 migrated modules; use as templates.
