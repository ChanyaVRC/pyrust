# Parity Test Cases

Python behavior parity scripts, run against both CPython 3.12 and pyrust by
`cargo test --release --test parity_compare` (outputs are diffed after
normalization).

## Harness

- Discovery: every `test_*.py` under this tree, recursively. Helper modules a
  fixture imports must NOT match `test_*.py` — prefix them with `_`.
- `PYRUST_PARITY_FILTER=<substring>` runs only fixtures whose path contains the
  substring (the per-fixture debugging loop).
- `PYRUST_PARITY_JOBS=<n>` overrides the worker count (defaults to the
  machine's parallelism; `1` restores serial order). The CPython and pyrust
  runs of one fixture always stay sequential, so a fixture may use scratch
  files if their names are unique to that fixture.
- `PYRUST_PYTHON=/path/to/python` overrides interpreter discovery.

## Directory taxonomy

Group by the *mechanism under test*, not by the syntax that happens to appear
in the script:

- `language/`: statements, control flow, operators, functions, f-strings
- `scoping/`: name resolution, closures, global/nonlocal
- `arithmetic/`, `ops/`: numeric semantics and operator dispatch
- `sequences/`, `builtins/`, `collections/`, `dunder/`, `method_dispatch/`:
  built-in types, their methods, and data-model protocol dispatch
- `classes/`, `pep695/`, `typing/`: class machinery, generics, typing surface
- `exceptions/`, `tracebacks/`: raising, handling, groups, traceback and
  PEP 657 presentation
- `generators/`, `async/`: generator/coroutine protocols and the event loop
- `optimizer/`, `compiler/`, `vm/`, `runtime/`, `performance/`: fixtures that
  pin interpreter-internal transformations (guarded optimizations, codegen
  shapes, iterator machinery) — name the mechanism in the file name
- `stdlib/`, `itertools/`: built-in module behavior
- `parser/`: syntax errors and grammar edges

When adding a fixture for a guarded optimization or a cached fast path, pin
BOTH sides: the fast path's result and at least one deopt/fallback edge.

## Naming and style

- Files must match `test_*.py`; name them `test_<mechanism>_<property>.py`.
- Start each fixture with a short comment saying what it pins and, when
  relevant, the PR/issue that introduced the machinery.
- Keep cases small and sectioned with `print` markers so a mismatch localizes
  by itself; thousands of loop iterations at most — parity runs measure
  correctness, not performance.
- Output must be platform-stable: no raw pointers/ids, no path-flavor
  (PosixPath/WindowsPath) assumptions, no dict-order assumptions beyond
  CPython's insertion-order guarantee, no locale/timezone dependence.
- These tests validate Python-language behavior, not byte-for-byte CPython
  traceback formatting (the harness strips caret underlines and `File`
  headers; assert on `tb_lineno` and messages instead).
