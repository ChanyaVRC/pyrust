# PyRust

A tiny Python-like interpreter implemented in Rust.

## Implemented Features

### Execution modes
- REPL mode (`cargo run`)
- Script mode (`cargo run -- path/to/script.py`)

### Values and types
- Numeric literals: integers (arbitrary precision), floats (`1.0`, `1e10`, `float('inf')`)
- String, bytes-free string literals; `True`, `False`, `None`
- Container literals: list `[1, 2]`, tuple `(1, 2)`, set `{1, 2}`, dict `{"k": v}`
- Indexing for list, tuple, string, and dictionary (`x[0]`, `d["k"]`)
- Slicing for lists and strings, including step slices (`x[1:5]`, `x[::2]`, `x[::-1]`)

### Operators
- Arithmetic: `+ - * / // % ** @`
- Bitwise: `& | ^ ~ << >>`
- Comparison: `== != < <= > >=`, chained comparisons, `in`, `not in`, `is`, `is not`
- Boolean: `and or not`

### Statements and control flow
- Assignment forms: simple, unpacking, augmented (`+=` etc.), index/slice assignment, `del`
- Statement sequencing with semicolons
- `if / elif / else`, `while [else]`, `for ... in ... [else]`
- `break`, `continue`, `pass`, `assert`
- `global`, `nonlocal`
- `raise`, `raise ... from ...`, `try / except / else / finally`
- `with` (context managers)
- Comments with `#`

### Functions
- `def name(args): ...`, `return`
- Positional arguments, trailing default arguments, keyword arguments
- Keyword-only parameters (`def f(a, *, b): ...`, `def f(*args, b): ...`)
- `*args`, `**kwargs`; call-site `*args` / `**kwargs` expansion
- Lexical closures, decorators
- `lambda`; ternary expressions

### Classes
- `class Name: ...`, single inheritance, `__init__`, instance attributes, bound methods
- Decorators on class definitions
- Special-method dispatch for `@` / `@=`

### Exception handling
- Built-in exception classes: `Exception`, `RuntimeError`, `TypeError`, `ValueError`, `AssertionError`, `IndexError`, `KeyError`, `StopIteration`, `RecursionError`, `SystemExit`
- Tuple exception clauses: `except (TypeError, ValueError)`

### Imports
- `import module`, `from module import name [as alias]`, `import module as alias`, star imports
- User `.py` file imports
- Built-in modules: `math` (see below), `sys` (`sys.exit`, `sys.argv`, `sys.path`)

### Built-in functions

| Function | Notes |
|---|---|
| `print(*args, sep=" ", end="\n", file=None, flush=False)` | |
| `len(x)` | |
| `range(stop)` / `range(start, stop[, step])` | |
| `abs(x)` | |
| `min(iterable, *, key=None)` / `min(a, b, ..., key=None)` | |
| `max(iterable, *, key=None)` / `max(a, b, ..., key=None)` | |
| `sum(iterable, start=0)` | |
| `enumerate(iterable, start=0)` | |
| `zip(*iterables)` | |
| `reversed(seq)` | |
| `sorted(iterable, *, key=None, reverse=False)` | |
| `isinstance(obj, classinfo)` | supports built-in types and user classes |
| `type(obj)` | returns a type object |
| `id(obj)` | |
| `hasattr(obj, name)` | |
| `getattr(obj, name[, default])` | |
| `setattr(obj, name, value)` | |
| `int(x)` / `int(x, base)` | |
| `float(x)` | |
| `str(x)` | |
| `bool(x)` | |
| `list(iterable)` | |
| `tuple(iterable)` | |
| `set(iterable)` | |
| `dict(**kwargs)` / `dict(mapping)` | |

### Built-in type methods

**`list`**: `append`, `pop([i])`, `insert(i, x)`, `extend(iterable)`, `remove(x)`, `clear`, `copy`, `reverse`, `sort(*, reverse=False)`, `index(x[, i[, j]])`, `count(x)`

**`dict`**: `get(key[, default])`, `keys()`, `values()`, `items()`, `update(other)`, `pop(key[, default])`, `popitem()`, `setdefault(key[, default])`, `clear`, `copy`

**`str`**: `split([sep[, maxsplit]])`, `rsplit([sep[, maxsplit]])`, `join(iterable)`, `strip([chars])`, `lstrip([chars])`, `rstrip([chars])`, `upper()`, `lower()`, `capitalize()`, `replace(old, new[, count])`, `startswith(prefix)`, `endswith(suffix)`, `find(sub[, start[, end]])`, `rfind(sub[, start[, end]])`, `index(sub[, start[, end]])`, `rindex(sub[, start[, end]])`, `count(sub[, start[, end]])`, `isdigit()`, `isalpha()`, `isalnum()`, `isspace()`

**`tuple`**: `index(x[, i[, j]])`, `count(x)`

### `math` module

`floor`, `ceil`, `sqrt`, `fabs`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `log(x[, base])`, `log2`, `log10`, `pow`, `isnan`, `isinf`, `pi`, `e`, `inf`, `nan`

### Optimizer passes

PyRust applies a peephole optimizer to each compiled function. See [docs/optimizer.md](docs/optimizer.md) for details.

Pipeline (in order):

1. Jump threading
2. BinOp–constant fusion
3. Constant tuple folding
4. Constant folding
5. Algebraic simplification
6. Unary constant folding
7. Constant branch elimination
8. Compare–jump fusion
9. `not` inversion
10. `BinOpInPlace` downgrade
11. Dead code elimination
12. Dead store elimination
13. Copy propagation
14. Trivial nop removal
15. Constant pool compaction

## Example

```text
if True:
	nums = [1, 2, 3]
	d = {"a": 10, "b": 20}
	i = 0
	total = 0
	while i < len(nums):
		total = total + nums[i]
		i = i + 1
	print("sum", total)
	print("dict", d["b"])
```

## Notes

This is a minimal educational interpreter, not a full CPython implementation.

Current limitations:

- No comprehensions or generator expressions
- No `async`/`await`, `yield`, `yield from`
- No `match` / `case`
- Import system supports `math` and `sys` built-ins and user `.py` files; relative imports and most stdlib modules are not available
- REPL supports single-line input only (multi-line blocks should be run as a script)
- Function signatures do not support positional-only parameters (`/`) or annotations with runtime meaning
- Classes support single inheritance only; no multiple inheritance, descriptors, `classmethod`, `staticmethod`, or `super`
- Exceptions do not provide tracebacks or the full CPython exception hierarchy
- Assigned names are treated as function-local across the whole function body

## Language References Used

- https://docs.python.org/3/reference/expressions.html
- https://docs.python.org/3/reference/compound_stmts.html
- https://docs.python.org/3/library/stdtypes.html

## Run

```bash
cargo run
```

## Run a script

```bash
cargo run -- examples/demo.py
```

## Run on Windows

```powershell
cargo run
cargo run -- examples/demo.py
```

For parity testing on Windows, install Python and run:

```powershell
cargo test --test parity_compare
```

If needed, override the Python executable path:

```powershell
$env:PYRUST_PYTHON = "C:\\Python312\\python.exe"
cargo test --test parity_compare
```

## Verify Python Output Parity

```bash
cargo test --test parity_compare
```

This runs all `tests/cases/**/test_*.py` with Python and PyRust and compares outputs.

## Compare Execution Speed (Python vs PyRust)

```bash
cargo build
python tools/benchmark_compare.py --iterations 3 --top 12
```

On Windows (PowerShell):

```powershell
cargo build
python tools/benchmark_compare.py --iterations 3 --top 12
```

Optional override for a custom PyRust binary path:

```bash
PYRUST_BIN=target/debug/pyrust python tools/benchmark_compare.py
```

### Latest Benchmark on master

<!-- BENCHMARK_SNAPSHOT_START -->
![Latest benchmark snapshot](https://chanyavrc.github.io/pyrust/benchmark.svg)

Live page: https://chanyavrc.github.io/pyrust/

This image is regenerated by GitHub Actions on every push to `master`.
<!-- BENCHMARK_SNAPSHOT_END -->

## CI/CD

- CI: GitHub Actions runs on push to `master` and on pull requests.
	- OS matrix: `ubuntu-latest`, `windows-latest`
	- Jobs run in parallel:
		- `fmt` (Ubuntu): `cargo fmt --all --check`
		- `unit_and_integration` (Ubuntu/Windows): `cargo build`, `cargo test`
		- `parity` (Ubuntu/Windows): `cargo build`, `cargo test --test parity_compare`
		- `benchmark` (Ubuntu): `python tools/benchmark_compare.py --iterations 2 --top 12`
		- `benchmark-readme` (Ubuntu): publishes `benchmark.svg` to GitHub Pages (no README commit)
- CD: A GitHub Release is published automatically when a tag matching `v*` is pushed.
	- Builds and uploads:
		- `pyrust-linux-x86_64`
		- `pyrust-windows-x86_64.exe`

### Create a release

```bash
git tag v0.1.0
git push origin v0.1.0
```
