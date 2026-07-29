# Runtime ownership boundaries

The runtime is divided by reason to change, not by a target line count. A long
state machine can be one responsibility; a short file that mixes callable
routing, concrete built-in methods, and generator policy is not.

## Domain ownership

| Domain | Owns | Must not own |
| --- | --- | --- |
| `execution` | Register/frame state, opcode decoding, handler-stack control transfer, generator-frame switching, and resume entry points | Concrete built-in implementations, exception-object fields, namespace policy, object-model classification, or reusable optimization policy |
| `fast_path` | Typed, semantics-preserving specializations used by opcode dispatch, including global/call caches, tagged-integer unary/binary, iterator-slot, branch, and allocation fast paths | Canonical protocol semantics, direct `pyrust_builtins` representation decoding, or Python-visible API policy |
| `call_dispatch` | Classification of callable `ValueKind`s and delegation to the selected call path | Descriptors, built-in constructors, container methods, or generator methods |
| `calls` | User-function argument binding, frame preparation, class invocation, and instance construction | Built-in method tables or primitive type-name classification |
| `classes` | Class-body execution, metaclass selection, protocol-based base/MRO handling, finalization, and primitive storage-layout metadata | Typing-marker identities, concrete descriptor representations, or generic callable dispatch |
| `builtin_methods` | Exact built-in callable and method names, built-in descriptors, interpreter-aware adapters, typing extension hooks, and container/text method routing | VM instruction decoding or generic iteration/materialisation |
| `generator_protocols` | Generator, coroutine, and async-generator attributes and operations such as `send`, `throw`, and `close` | General frame execution |
| `attributes` | Generic descriptor order, class/instance attribute lookup/assignment/deletion, and typed read/write cache plans and binding policy consumed by `fast_path` | Python method registration, bytecode cache state, or opcode decoding |
| `namespaces` | Module attributes/import diagnostics, globals/locals, type-parameter scopes, cache-safe name resolution/synchronisation, the interpreter-owned import registry, and generic module loading | Object descriptor policy, module-specific finalization, or namespace generations/providers stored on the active `Interpreter` |
| `value_protocols`, `expressions`, `iteration`, `truthiness`, `slicing` | Python data-model protocol dispatch; `expressions` owns canonical unary/binary operation semantics shared with optimizer folding; `iteration` owns iterator factories, canonical one-step advancement, `next`, materialisation, unpacking, loop-state construction, concrete iterator adapters, and mutation guards | Exact concrete container/text APIs |
| `collection_keys`, `collection_ops` | Interpreter-aware hash/equality keys, fresh/live dict and set mutation, mapping expansion/merge, and set algebra | Named method routing or call-diagnostic naming |
| `formatting` | Formatting grammars, interpreter-aware repr/str/`__format__` dispatch, and format-site cache policy | VM value-kind classification |
| `exceptions` | Exception construction/coercion, traceback/cause/context representation, group semantics, and presentation | VM handler-stack jumps |
| `type_objects` | Runtime class/type-name classification, slot-descriptor construction, and language-level type objects created by syntax | The `type()` builtin entry point or VM register policy |

The explicit imports and re-exports in `runtime.rs` are the facade between
these domains. Do not replace them with wildcard imports: doing so flattens the
namespace and hides which domain a caller depends on.

## Dependency rules

1. Generic routers may refer to Python data-model protocols required to select
   behavior, such as `__call__`, `__new__`, `__iter__`, or `__getattribute__`.
   Those names describe the language protocol itself.
2. Exact non-protocol APIs such as `list.append`, `dict.update`, `str.split`,
   `property.getter`, or `generator.send` live in `builtin_methods`,
   `generator_protocols`, or the built-in module/type that owns them.
3. A VM optimization for a concrete built-in is exposed as a typed helper from
   the owning domain. The opcode loop may call that helper, but it must not
   duplicate the method-name table or implementation.
4. Primitive subclass storage is class metadata. `calls` consumes the typed
   `PrimitiveLayout`; it does not classify built-in classes by name.
5. Cross-domain items are exported deliberately and by name. A new export is a
   design decision, not a convenience import.
6. Runtime domains never call implementation helpers through
   `builtin_modules::builtins`. The flat `builtins` namespace owns Python
   entry-point validation and registration; it consumes typed services from
   `collection_keys`, `iteration`, `formatting`, `type_objects`, and
   `generator_protocols`.
7. The opcode loop owns iterator slots and control flow, but not Python object
   classification, materialisation, unpacking, or container-specific adapters.
   `GetIter`, `next`, and user-iterator advancement delegate canonical policy to
   `iteration`; specializations that only change execution cost live in
   `fast_path`.
8. Runtime reaches `builtin_modules` only through the generic provider calls in
   the import boundary. Exact finalization for `builtins`, `collections`,
   `itertools`, `io`, and other modules stays with that provider.
9. Standard-library runtime objects implement ordinary data-model protocols
   where possible. For example, `typing._GenericAlias` supplies
   `__mro_entries__`; class construction does not identify the typing module or
   inspect its private representation.
10. Descriptor objects expose ordinary hooks where Python defines them.
    `property` implements `__set_name__`, and class finalization invokes that
    protocol without a property-specific branch. Concrete typing class-builder
    compatibility is isolated behind `builtin_methods` extension hooks.
11. The VM passes operands to `namespaces`, `formatting`, `exceptions`,
    `collection_keys`, `collection_ops`, and `type_objects`; those domains own
    Python-visible error wording and representation mutation.
12. Attribute cache files translate typed read/write plans into bytecode-local
    cache entries. Lazy TypeVar fields, deferred tracebacks, descriptor
    dynamism, and default-assignment eligibility are decided by `attributes`.
13. Fast call paths may consume a validated call shape, but general `**mapping`
    expansion and keyword diagnostics belong to `calls`.
14. Exception handler target selection and stack transfer stay in `execution`;
    `PyError` materialisation, implicit chaining, and caught-traceback slot
    updates belong to `exceptions`.
15. A standard-library iterator's algorithm and private cursor stay with its
    owning module. `itertools.permutations`, `product`, and `combinations` may
    use opaque Rust state to avoid Python-list round trips, but they are not VM
    fast paths: their behavior is the public `itertools` API itself.
16. Interpreter-free storage that is shared by several operations belongs in
    `pyrust-builtins` (for example deque's opaque `VecDeque` backing).
    Interpreter-aware policy—user iteration, equality callbacks, exception
    identity, and partial-update timing—stays in `builtin_methods` or the
    standard-library module that defines the operation.
17. `PyClass::name`, `qualname`, and `__module__` are mutable presentation
    metadata, never semantic type identifiers. Primitive/object semantics use
    immutable core tags; built-in exceptions use their immutable exception
    tag; standard-library classes use pointer identity held by the module that
    owns them. Re-importable module registries are weak and generation-aware so
    old instances retain their class without leaking dead module generations.
18. Descriptor category is metadata owned by the class or module that installs
    the attribute. Generic lookup binds an explicit `classmethod`,
    `staticmethod`, property, or method descriptor; it never infers descriptor
    behavior from a builtin registry string such as a qualified Python method
    name.
19. `BuiltinTypeOps::type_name()` is presentation metadata. Canonical primitive
    families are classified by `CanonicalClassTag`; opaque built-ins expose a
    typed predicate from their owning module. Consumers must not name another
    module's concrete `*Ops` implementation merely to inspect its identity.
20. `builtin_methods` may route a Python-visible `str`/`repr` method call, but
    the selection and validation of `__str__`, `__repr__`, exception rendering,
    and primitive-backed fallbacks belongs to `formatting`.
21. Mutable Python interpreter policy belongs to `Interpreter`, not to
    process/thread-global core state. If an interpreter-independent core
    operation needs that policy, install it through a scoped execution-entry
    adapter that restores the previous host value. Per-function call paths
    must not pay to republish configuration that is unchanged for the whole
    script/module/exec frame.
22. Equality-versioned caches must have an exhausted state that can never
    alias an older generation. Saturated class/module/global versions disable
    cache fill and hit; collection iterator generations may continue wrapping
    only when a separate sticky exhaustion flag permanently disables their
    use as long-lived cache stamps.
23. Global-cache generations, cache-disable state, and the globals provider
    belong to the root `Environment` namespace shared by its lexical children.
    They must not be stored on the active `Interpreter`: imported functions and
    explicit exec/eval functions can be invoked later by a different
    interpreter object while retaining the same root namespace.
24. Each `FnCode` name slot stores one final `LoadGlobal` resolution entry.
    Environment and canonical-builtins results are exclusive variants of that
    entry and are validated through one cache borrow against the root namespace
    snapshot. Do not restore parallel environment and builtins cache vectors.
25. A filesystem-backed `PyModule`, its functions, and its `__dict__` share the
    root namespace's one globals dictionary. The module keeps that dictionary
    strongly but its loader environment weakly, avoiding a direct strong cycle
    across circular imports; captured functions independently retain the root.
    The pre-existing Python-level globals/function/environment cycle remains a
    general object-graph collection concern; this link must not add a second
    strong module/environment edge.
    Built-in modules keep their direct `attrs` storage and generation fast path.
    Exposing the filesystem dictionary disables both root global caches and the
    linked module mutation generation before arbitrary alias writes can occur.
26. A dictionary that aliases a live globals or locals namespace reports
    mutations at the core dictionary-storage boundary. Single-string-key
    writes use keyed notifications; opaque or batched mutation uses a full
    refresh. Interpreter adapters must not duplicate these hooks, because
    aliases can be mutated through instance attributes, module dictionaries,
    or another interpreter.
27. A live script frame registers its fast-local layout with the root namespace
    through an RAII mirror guard. The root advances a non-wrapping mirror epoch
    whenever that stack changes. A global-cache entry may retain weak layout
    metadata and a register number, but never a raw register-file pointer or a
    mirror-vector index; every script-derived hit is validated against the
    epoch and resolves the newest live matching mirror first. An unset inner
    slot falls through to an outer mirror.
28. Namespace write invalidation may use conservative name-interest masks to
    avoid an unconditional hash lookup. False positives only cost a cache
    generation bump; false negatives are forbidden. Canonical-builtin fallback
    names and names that also exist in `EnvValues` use separate root-owned
    masks so a fast-local write cannot leave an older environment binding
    observable.
29. `SyncModuleGlobal` is an observability boundary, not freely movable cleanup.
    An optimizer must not sink it across an operation that can invoke Python
    code, including arithmetic and comparison protocols. A specialized loop
    rewrite may preserve one final sync only when it proves the relevant
    operations are non-reentrant primitive operations and preserves every
    externally observable value. A per-iteration fact — a sequence element's
    type, a subscript's bounds — is proven by a guard inside the specialized
    copy whose failure edge flushes every deferred sync before resuming the
    original stream at the corresponding instruction. Such a mid-loop side exit
    may only target an operation that reproduces the fast copy's effect when
    re-executed, so the raise, the source line, and the caret span stay on the
    original path.
30. Python slot-result validation belongs to `value_protocols`, not to the
    built-in entry point that first exposed a bug. In particular, `__len__`
    results pass through one index-protocol and Py_ssize_t boundary shared by
    `len()`, truth-value testing, `reversed()`, and iterator length hints.
    Consumer-specific policy, such as clearing `TypeError` from a length hint,
    stays with that consumer after canonical validation. Numeric consumers such
    as `round(..., ndigits)` likewise call the shared index protocol before
    applying their own range clamp. Optional consumers use
    `try_value_to_index`: `None` means only that the slot is absent, while slot
    exceptions and invalid results remain errors. The boundary resolves
    instance slots on the class and class-object slots on the metaclass, and it
    normalizes accepted builtin-subclass slot results before a constructor,
    math function, formatter, or bytes parser applies its own range, fallback,
    and diagnostic policy. The bytes count-or-iterable consumer, for example,
    clears only `TypeError` after optional index resolution because CPython
    interprets that error class as a request to try the iterable form; the
    shared protocol helper itself never erases it.
    A scalar constructor may substitute a builtin-subclass backing only when
    the current protocol slot is inherited from a canonical primitive owner.
    A slot owned by a user class, including a copied builtin descriptor, is
    invoked with the original Python receiver so descriptor validation,
    override precedence, and diagnostics remain observable. Each conversion
    stage applies that owner gate independently; an inherited `__float__`, for
    example, may suppress a lower user `__index__` without suppressing an
    earlier user `__complex__`.
31. Interpreter-free built-ins own concrete operations on already-normalized
    values, not Python protocol dispatch. An interpreter-aware adapter binds the
    Python signature and resolves truth, index, iteration, or descriptor
    protocols before calling core storage code. A core fast branch may accept a
    typed `Bool` or `Int`; it must not grow an independent interpretation of an
    arbitrary Python object.
32. Iterator fast paths retain the exact source `Value` that `iter()` observed.
    They must not re-read the register or name that originally supplied it:
    rebinding that location cannot change an existing iterator. Moving cursor
    state out of an iterator object is permitted only when the object is proven
    unaliased; otherwise the specialization must keep or wrap the shared state.
    Cached protocol methods additionally require an identity or mutation
    generation proving that a later class-dictionary update is not hidden.
33. `CallMemo` is an execution optimization for a particular `UserFunction`,
    never a promise inferred from a Python-visible spelling. Compilation may
    prove the current function's direct self-calls only. Runtime lookup verifies
    the actual callee identity and its memo-purity bit, and the key must
    distinguish every bound argument or decline caching when omitted mutable
    defaults, variadic binding, or another incomplete shape would affect the
    call. A cache probe never executes the callee: a hit returns before touching
    in-flight state, while an eligible miss is owned by the explicit VM frame
    until that frame commits its result or cancels during error unwinding.
    Native execution is only the conservative trampoline-gate fallback.
    Every native, call-trampolined, and generator-driven Python frame owns the
    same thread-local `CallDepthGuard`; local trampoline-vector length is not a
    recursion-depth substitute because a full register arena can split one
    Python call chain across nested native VM entries.
34. Cross-jump merging requires semantic control-flow equivalence, not only
    equal instruction bytes. Every corresponding instruction must have the same
    complete active exception-handler stack and the same effective line and
    PEP 657 caret span. If handler-region analysis is inconsistent, the pass
    leaves that candidate unchanged instead of relying on the VM's dynamic
    handler fallback.
35. Python scope errors are classified from the active frame kind and binding
    layout. A suspended or resumed generator is still a function frame, so a
    missing fast local raises `UnboundLocalError`; execution helpers must not use
    a transient function-id optimization field to decide language semantics.
36. Every bytecode specialization and optimizer pass needs a production
    producer-to-consumer path. A unit test that constructs an instruction by
    hand does not prove that the compiler can create the required provenance at
    that pipeline position. When a pass is wholly subsumed by an adjacent pass,
    or its guards are unreachable before its only producer runs, remove the pass
    or reorder it with end-to-end IR evidence; do not retain a parallel analysis
    solely for artificial instruction streams.
37. A primitive built-in provider owns its Python class surface. Its
    `PrimitiveClassAttrs` references the existing `METHODS` inventory and adds
    only typed class/static descriptor overrides plus explicit constructor and
    `__class_getitem__` flags. Interpreter class bootstrap consumes that
    metadata generically and must not relist concrete APIs or briefly install a
    class/static descriptor as an ordinary method. Data-model slot exposure
    remains the separate typed policy in `builtin_methods/slot_tables`.

The `ownership_tests` module in `runtime.rs` follows each domain entry point's
complete recursive `include!` graph, then scans the foundational call, VM, and
generic attribute routers for representative concrete Python API literals.
Adding a new source fragment therefore adds it to the guard automatically.
The tests also recursively reject dependencies from runtime sources to the flat
`builtins` implementation, restricts built-in module-provider access to the
import boundary, rejects generic protocol-service definitions under built-in
modules, keeps cache implementations and concrete iterator adapters out of the
opcode loop, rejects concrete built-in representation decoding in `fast_path`,
and prevents generic iterator services from drifting back into
`builtin_methods`.

The flat `builtins` namespace follows the same rule internally. Its public
registration is assembled from semantic families, while `builtins.rs` lists
the exact runtime services available to each family. Family source files must
not recover the old flattened namespace with `use super::*`; a boundary test
guards that invariant.

One compatibility boundary remains explicit: opaque `BuiltinObject` values
whose Python type has not yet been migrated to a real `PyClass` still produce a
legacy `BuiltinFunction(type_name)` sentinel from `type()`, and the `isinstance`
builtin compares that sentinel at its Python API boundary. Do not spread this
representation into runtime policy. Removing it requires a typed runtime-class
value (or completing the `PyClass` migration), not another local string check.

## File composition

`runtime.rs` declares real Rust child modules for the domains above. Within one
domain, small `include!` fragments intentionally share private implementation
state. An include split is only file organization; crossing a domain module is
the architectural boundary.

In particular, `execution` and `fast_path` are sibling Rust modules. The opcode
loop consumes only the explicitly exported specialization helpers; it is never
composed into the same private scope as their cache implementations. Opcode
argument decoding and cache-site mutation belong to `fast_path`, while exact
built-in method interpretation remains behind a typed `builtin_methods`
service.

The same distinction applies elsewhere:

- parser fragments own grammar families while sharing one token cursor;
- compiler fragments own compilation phases while sharing one register/scope
  builder;
- optimizer fragments own pass families while sharing the optimizer model;
- scope-analysis submodules separately own bindings, declaration ordering, and
  reference/definite-bound analysis;
- `exceptions/control` owns ordinary exception materialisation and chaining,
  while `exceptions/groups` owns PEP 654 matching, splitting, and derivation;
- `builtin_methods/object_protocol` owns object-visible behaviour and `dir()`
  assembly, while `builtin_methods/slot_tables` owns static primitive slot
  metadata;
- built-in module/type files own their Python-facing API, so exact method names
  are expected there;
- `pyrust-core` remains the interpreter-independent value and error substrate,
  split by data-model responsibility.

## Intentionally long state machines

`vm/execute.rs` remains a single opcode dispatch loop. Splitting individual
match arms into artificial modules would not create independent ownership and
could obscure hot-path control flow. Extract code from it when the extracted
policy has its own change reason, data model, or reusable typed interface—not
merely because the loop has many lines. The lexer and a single recursive
compiler/optimizer pass follow the same rule.
