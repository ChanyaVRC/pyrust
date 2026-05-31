# pyrust-builtins

Type-method dispatch tables for the built-in container types — `string`,
`list`, `tuple`, `dict`, `set`, `frozenset`, `bytes`, … — plus the low-level
helpers they share (`sequence`, `mutable_sequence`, `dict_views`).

The crate is **interpreter-free**: it depends only on `pyrust-core`
(`Value`, `PyKey`, `PyError`). It has no access to the `Interpreter`, so it
cannot fire user-defined Python code (`__hash__`, `__eq__`, `__lt__`, a
`key=` callable, …) or build interpreter-owned objects (lazy views that share
the source container's `Rc<RefCell<…>>` backing storage).

Each type module exposes a receiver-only entry point:

```rust
pub fn call(method: &str, receiver: &Value, args: Vec<Value> /*, kwargs */) -> Result<Value>;
```

## The interception contract

Method-call dispatch enters the VM at `Insn::CallMethod` /
`Insn::CallMethodExpanded`
(`crates/pyrust/src/interpreter/runtime/vm.rs`). For the five tagged
container types the VM routes through a single shared dispatcher,
`Interpreter::dispatch_builtin_container_method` (#431). That dispatcher
decides, **per method**, one of three things:

1. **Pure builtin** — hand straight to `pyrust_builtins::<type>::call`. The
   common case; no interpreter needed.
2. **Needs the interpreter** — the method can reach user Python code, so the
   VM must drive it. Routed to `Interpreter::call_<type>_method` (or a
   dedicated helper such as `list_sort_with_kwargs` / `str_template_method`).
3. **Needs the `Rc` backing** — the method returns a *live view* that shares
   storage with the source container. Routed to `dict_views::*`.

The decision is **predicate-driven**, not ad-hoc inline string matching. Each
type module owns the predicate, so it is the single source of truth and the VM
never re-encodes the carve-out list:

| Predicate | Methods | Why upstream |
|---|---|---|
| `list::requires_interpreter` | `sort`, `index`, `count` | `sort(key=)` runs a user callable; `index`/`count` may fire user `__eq__` |
| `dict::needs_rc` | `keys`, `values`, `items` | live views need the `Rc<RefCell<IndexMap>>`, not a `Vec` snapshot |
| `string::requires_vm_template` | `format`, `format_map`, `maketrans` | `format` needs kwargs + the interpreter's templating; `maketrans` is a staticmethod forwarded to `str_maketrans` |

A second layer of interception lives *inside* `Interpreter::call_<type>_method`
— these are not VM-visible predicates because the methods always enter the
interpreter first and the interpreter decides whether to delegate to
`pyrust_builtins::<type>::call`:

| Method(s) | Interceptor | Why |
|---|---|---|
| `dict.get` / `dict.pop` / `dict.setdefault` / `dict.__contains__` | `call_dict_method` | need user `__hash__` / `__eq__` on the key (#368, #425) |
| `dict.update` (non-primitive iterable arg) | `call_dict_method` | drive a user iterable / fire user `__hash__` |
| `set.add` / `set.discard` / `set.remove` / `set.__contains__` | `call_set_method` | need user `__hash__` / `__eq__` on the element (#368) |
| `set.update` | `call_set_method` | hashable slices / `PyInstance` elements need `value_to_pykey` |
| `str.join` | `call_str_method` | accepts any user iterable via `collect_iterable` |

When such a method is intercepted, the corresponding arm in
`pyrust_builtins::<type>::call` is **unreachable at runtime** but is kept as a
drift-guard stub (so `getattr`/`hasattr` via the `METHODS` table stay
consistent) — see the comments in `dict.rs` / `list.rs` / `tuple.rs` /
`string.rs` and #425.

## When you add a new method

1. Add it to the type module's `METHODS` table (so `hasattr`/`getattr` see it)
   and give it a `call` arm.
2. If it can reach user Python code or needs `Rc` backing, **add it to the
   relevant predicate** (`requires_interpreter` / `needs_rc` /
   `requires_vm_template`) and implement the interpreter-side path. Do **not**
   add an inline `if method == "…"` check in `vm.rs` — that is exactly the
   fragmentation #431 removed.
3. If it is intercepted inside `call_<type>_method`, leave its `call` arm as a
   drift-guard stub and note it in the table above.
