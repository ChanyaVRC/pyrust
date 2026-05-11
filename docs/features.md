# Implemented Features

## Execution modes

- REPL mode (`cargo run`)
- Script mode (`cargo run -- path/to/script.py`)

## Values and types

- Numeric literals: integers (arbitrary precision), floats (`1.0`, `1e10`, `float('inf')`)
- String literals; `True`, `False`, `None`
- Container literals: list `[1, 2]`, tuple `(1, 2)`, set `{1, 2}`, dict `{"k": v}`
- Indexing for list, tuple, string, and dictionary (`x[0]`, `d["k"]`)
- Slicing for lists and strings, including step slices (`x[1:5]`, `x[::2]`, `x[::-1]`)

## Operators

- Arithmetic: `+ - * / // % ** @`
- Bitwise: `& | ^ ~ << >>`
- Comparison: `== != < <= > >=`, chained comparisons, `in`, `not in`, `is`, `is not`
- Boolean: `and or not`

## Statements and control flow

- Assignment forms: simple, unpacking, augmented (`+=` etc.), index/slice assignment, `del`
- Statement sequencing with semicolons
- `if / elif / else`, `while [else]`, `for ... in ... [else]`
- `break`, `continue`, `pass`, `assert`
- `global`, `nonlocal`
- `raise`, `raise ... from ...`, `try / except / else / finally`
- `with` (context managers)
- Comments with `#`

## Functions

- `def name(args): ...`, `return`
- Positional arguments, trailing default arguments, keyword arguments
- Keyword-only parameters (`def f(a, *, b): ...`, `def f(*args, b): ...`)
- `*args`, `**kwargs`; call-site `*args` / `**kwargs` expansion
- Lexical closures, decorators
- `lambda`; ternary expressions

## Classes

- `class Name: ...`, single inheritance, `__init__`, instance attributes, bound methods
- Decorators on class definitions
- Special-method dispatch for `@` / `@=`

## Exception handling

- Built-in exception classes: `Exception`, `RuntimeError`, `TypeError`, `ValueError`,
  `AssertionError`, `IndexError`, `KeyError`, `StopIteration`, `RecursionError`, `SystemExit`
- Tuple exception clauses: `except (TypeError, ValueError)`

## Imports

- `import module`, `from module import name [as alias]`, `import module as alias`, star imports
- User `.py` file imports
- Built-in modules: `math` (see below), `sys` (`sys.exit`, `sys.argv`, `sys.path`)

## Built-in functions

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

## Built-in type methods

### `list`

| Method | Signature |
|---|---|
| `append` | `append(x)` |
| `pop` | `pop([i])` |
| `insert` | `insert(i, x)` |
| `extend` | `extend(iterable)` |
| `remove` | `remove(x)` |
| `clear` | `clear()` |
| `copy` | `copy()` |
| `reverse` | `reverse()` |
| `sort` | `sort(*, reverse=False)` |
| `index` | `index(x[, i[, j]])` |
| `count` | `count(x)` |

### `dict`

| Method | Signature |
|---|---|
| `get` | `get(key[, default])` |
| `keys` | `keys()` |
| `values` | `values()` |
| `items` | `items()` |
| `update` | `update(other)` |
| `pop` | `pop(key[, default])` |
| `popitem` | `popitem()` |
| `setdefault` | `setdefault(key[, default])` |
| `clear` | `clear()` |
| `copy` | `copy()` |

### `str`

| Method | Signature |
|---|---|
| `split` | `split([sep[, maxsplit]])` |
| `rsplit` | `rsplit([sep[, maxsplit]])` |
| `join` | `join(iterable)` |
| `strip` | `strip([chars])` |
| `lstrip` | `lstrip([chars])` |
| `rstrip` | `rstrip([chars])` |
| `upper` | `upper()` |
| `lower` | `lower()` |
| `capitalize` | `capitalize()` |
| `replace` | `replace(old, new[, count])` |
| `startswith` | `startswith(prefix)` |
| `endswith` | `endswith(suffix)` |
| `find` | `find(sub[, start[, end]])` |
| `rfind` | `rfind(sub[, start[, end]])` |
| `index` | `index(sub[, start[, end]])` |
| `rindex` | `rindex(sub[, start[, end]])` |
| `count` | `count(sub[, start[, end]])` |
| `isdigit` | `isdigit()` |
| `isalpha` | `isalpha()` |
| `isalnum` | `isalnum()` |
| `isspace` | `isspace()` |

### `tuple`

| Method | Signature |
|---|---|
| `index` | `index(x[, i[, j]])` |
| `count` | `count(x)` |

## `math` module

### Implemented

| Name | Description |
|---|---|
| `pi`, `e`, `inf`, `nan` | Constants |
| `floor(x)`, `ceil(x)` | Floor / ceiling |
| `sqrt(x)`, `fabs(x)` | Square root, absolute value |
| `exp(x)` | eˣ |
| `log(x[, base])`, `log2(x)`, `log10(x)` | Logarithms |
| `pow(x, y)` | xʸ |
| `sin(x)`, `cos(x)`, `tan(x)` | Trigonometric |
| `asin(x)`, `acos(x)`, `atan(x)`, `atan2(y, x)` | Inverse trigonometric |
| `isnan(x)`, `isinf(x)` | Float classification |

### Not yet implemented

`tau`, `degrees`, `radians`, `sinh`, `cosh`, `tanh`, `acosh`, `asinh`, `atanh`,
`gcd`, `lcm`, `factorial`, `comb`, `perm`, `isqrt`,
`trunc`, `fmod`, `modf`, `remainder`, `fsum`, `hypot`, `dist`,
`isclose`, `isfinite`, `copysign`,
`cbrt`, `exp2`, `expm1`, `log1p`,
`erf`, `erfc`, `gamma`, `lgamma`

## Optimizer

PyRust applies a 15-pass peephole optimizer to every compiled function.
See [optimizer.md](optimizer.md) for a full description of each pass.
