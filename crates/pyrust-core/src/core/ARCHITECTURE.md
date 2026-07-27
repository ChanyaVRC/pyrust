# pyrust-core ownership

`lib.rs` is a public facade only. It declares real Rust modules and explicitly
re-exports the stable API that downstream crates already consume.

| Module | Responsibility |
| --- | --- |
| `arguments` | Typed extraction and argument-count diagnostics |
| `environment` | Lexical scope storage and environment links |
| `errors` | Structured Python error values |
| `traceback` | Error-frame capture and traceback rendering |
| `class_epoch` | Class-mutation cache invalidation |
| `object_identity` | Monotonic internal object IDs |
| `string_interning` | Bounded constant-string interning |
| `cycle_guards` | Thread-local recursion guards for repr/equality |
| `object_model` | The mutually recursive `Value` representation, keys, containers, functions, and classes |

The value representation remains cohesive on purpose. Its fragments share
private NaN-box and allocation invariants; turning those internals into
cross-module APIs would weaken safety without establishing a useful ownership
boundary. Non-representation responsibilities must not be included back into
`object_model`.

The ownership tests enforce three structural rules:

1. the crate root must not use `include!`;
2. the facade must enumerate re-exports rather than glob-exporting domains;
3. production domain modules must not use `use super::*`.
