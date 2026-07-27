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
`Insn::CallMethodExpanded`, handled by
`Interpreter::exec_call_method` / `exec_call_method_expanded`
(`crates/pyrust/src/interpreter/runtime/builtin_methods/container_dispatch.rs`).
For the five classified container types, both opcodes route through a shared
dispatcher, `Interpreter::dispatch_builtin_container_method` (#431). That
dispatcher decides, **per method**, one of three things:

1. **Pure builtin** — hand straight to `pyrust_builtins::<type>::call`. The
   common case; no interpreter needed.
2. **Needs the interpreter** — the method can reach user Python code, so the
   VM must drive it. Routed to `Interpreter::call_<type>_method` (or a
   dedicated helper such as `list_sort_with_kwargs` / `str_template_method`).
3. **Needs the `Rc` backing** — the method returns a *live view* that shares
   storage with the source container. Routed to `dict_views::*`.

The decision is **route-driven**, not ad-hoc inline string matching. Each type
module classifies the Python method name into a semantic enum, so it is the
single source of truth and the VM does not compare the name again:

| Classifier | Methods | Why upstream |
|---|---|---|
| `list::interpreter_method` | `sort`, `index`, `count`, `remove`, `extend` | may run a callable, comparison dunder, or user iterator |
| `tuple::interpreter_method` | `index`, `count` | may fire user `__eq__` while scanning the tuple |
| `dict::view_method` | `keys`, `values`, `items` | live views need the `Rc<RefCell<IndexMap>>`, not a snapshot |
| `string::interpreter_method` | `format`, `format_map`, `maketrans` | needs interpreter templating or staticmethod forwarding |

One narrow exception is *not* a routing predicate: `list.insert` / `list.pop`
accept any `__index__` object as their index argument, so the dispatcher
coerces `pos[0]` through `value_to_index` before delegating (#2022). This is
argument coercion, not a "which path" decision, so it stays inline in the
`list` arm rather than in the `interpreter_method` route.

Subscript assignment has the same boundary without being a named-method
route. `expressions::exec_set_item` resolves callback-capable bytearray slice
bounds, drives a generator or user `__iter__`, and resolves element
`__index__` methods before calling `BuiltinTypeOps::set_item`. The bytearray
storage implementation receives an owned, materialised RHS and never reaches
back into the interpreter. This also keeps all callbacks outside the
bytearray's mutable `RefCell` borrow.

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
   relevant classifier** (`interpreter_method` / `view_method` /
   `interpreter_method`) and implement the interpreter-side route. Do **not**
   add a second inline `if method == "…"` check in the VM — that is exactly the
   fragmentation these classifiers avoid.
3. If it is intercepted inside `call_<type>_method`, leave its `call` arm as a
   drift-guard stub and note it in the table above.
