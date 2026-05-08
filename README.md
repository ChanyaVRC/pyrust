# PyRust

A tiny Python-like interpreter implemented in Rust.

## Implemented Features

- REPL mode (`cargo run`)
- Script mode (`cargo run -- path/to/script.py`)
- Variables and assignment (`x = 10`)
- Numeric, string, tuple, set, list, and dictionary literals
- Indexing for list, tuple, string, and dictionary (`x[0]`, `d["k"]`)
- Slicing for lists and strings, including step slices (`x[1:5]`, `x[::2]`, `x[::-1]`)
- Assignment forms: unpacking, augmented assignment, index assignment, slice assignment, and `del`
- Arithmetic operators: `+ - * / // % ** @`
- Bitwise operators: `& | ^ ~ << >>`
- Comparison operators: `== != < <= > >=`, chained comparisons, `in`, `not in`, `is`, `is not`
- Boolean operators: `and or not`
- Expressions: ternary expressions and `lambda`
- Statement sequencing with semicolons
- Control flow: `if / elif / else`, `while [else]`, `for ... in ... [else]`, `break`, `continue`, `pass`, `global`, `nonlocal`, `raise`, `raise ... from ...`, `try / except / else / finally`, `assert`, `with`
- Function definitions: `def name(args): ...`, `return`, trailing default arguments, keyword arguments, lexical closures, `*args`, `**kwargs`, decorators, and call-site `*args` / `**kwargs`
- Class support: `class Name: ...`, instance attributes, bound methods, `__init__`, single inheritance, decorators, and special-method dispatch for `@` / `@=`
- Built-in exception classes: `Exception`, `RuntimeError`, `TypeError`, `ValueError`, `AssertionError`
- Import system: `import module`, `from module import name [as alias]`, `import module as alias`, star imports, user `.py` file imports, built-in modules `math` and `sys`
- Built-in constants: `True`, `False`, `None`
- Built-in functions: `print(..., sep=..., end=..., file=None, flush=...)`, `len(...)`, `range(...)`
- Comments with `#`

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

- No comprehensions
- Import system supports `math` and `sys` built-ins and user `.py` files; relative imports and most stdlib modules are not available
- REPL supports single-line input only (multi-line blocks should be run as a script)
- Function signatures are still a subset of Python: no positional-only parameters, keyword-only parameters, bare `*`, or annotations with runtime meaning
- Classes currently support a narrow object model: single inheritance only, with no multiple inheritance, descriptors, `classmethod`, `staticmethod`, or `super`
- Exceptions currently support built-in exception classes, `raise`, `raise ... from ...`, tuple catches, and `try / except / else / finally`, but still do not provide tracebacks or a Python-complete exception hierarchy
- Assigned names are treated as function-local across the whole function body
- `print` supports `sep`, `end`, `file=None`, and `flush=False/True`

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

## CI/CD

- CI: GitHub Actions runs on push to `master`/`main` and on pull requests.
	- OS matrix: `ubuntu-latest`, `windows-latest`
	- `cargo fmt --all --check`
	- `cargo build`
	- `cargo test`
	- `cargo test --test parity_compare`
- CD: A GitHub Release is published automatically when a tag matching `v*` is pushed.
	- Builds and uploads:
		- `pyrust-linux-x86_64`
		- `pyrust-windows-x86_64.exe`

### Create a release

```bash
git tag v0.1.0
git push origin v0.1.0
```
