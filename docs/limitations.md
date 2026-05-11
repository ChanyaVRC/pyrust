# Known Limitations

PyRust is a minimal educational interpreter. The following features are not yet implemented or are intentionally out of scope.

## Language features

- **No comprehensions or generator expressions** — list/dict/set comprehensions and `(x for x in ...)` are not parsed
- **No `async`/`await`, `yield`, `yield from`** — coroutines and generators are not supported
- **No `match` / `case`** — structural pattern matching (PEP 634) is not implemented
- **No annotated assignment** — `x: int = 1` is not parsed
- **`print` file argument** — `file=` accepts `None` only; real stream redirection is not supported
- **Assigned names are function-local** — a name assigned anywhere in a function body is treated as local for the entire function, matching CPython's scoping rule, but `UnboundLocalError` is not always raised in the same places

## Function signatures

- **No positional-only parameters** — the `/` separator in `def f(a, /, b)` is not supported
- **No annotations with runtime meaning** — parameter and return annotations are ignored

## Classes and object model

- **Single inheritance only** — multiple base classes are not supported
- **No `super()`**
- **No `classmethod`, `staticmethod`, `property`** — descriptor protocol is not implemented
- **No `__getattr__`, `__getattribute__`, `__setattr__`, `__delattr__`** — attribute hooks are not invoked
- **No `__call__`, `__iter__`, `__next__`, `__len__`, `__bool__`** — most dunder methods are not dispatched (exceptions: `__init__`, `@`/`@=`)
- **No `__dict__`, `__class__`** — instance and class namespace inspection is not supported

## Built-in types

- **`set` has no methods** — `set` literals work, but `add`, `remove`, `union`, `intersection`, etc. are not implemented
- **No `frozenset`, `complex`, `bytes`, `bytearray`** — these types are not available
- **Strings** — the following `str` methods are not implemented:
  `casefold`, `center`, `ljust`, `rjust`, `zfill`, `expandtabs`,
  `encode`, `format`, `format_map`, `maketrans`, `translate`,
  `partition`, `rpartition`, `splitlines`, `removeprefix`, `removesuffix`,
  `swapcase`, `title`, `islower`, `isupper`, `istitle`,
  `isascii`, `isdecimal`, `isnumeric`, `isidentifier`, `isprintable`
- **String literal forms** — bytes (`b"..."`), raw (`r"..."`), triple-quoted, and f-strings are not supported

## Built-in functions

Not yet implemented: `all`, `any`, `repr`, `callable`, `delattr`, `format`,
`map`, `filter`, `iter`, `next`, `issubclass`,
`hash`, `chr`, `ord`, `bin`, `oct`, `hex`, `round`, `divmod`, `pow`,
`frozenset`, `complex`, `bytearray`, `bytes`,
`globals`, `locals`, `vars`, `ascii`,
`input`, `open`

## `math` module

Not yet implemented: `tau`, `degrees`, `radians`,
`sinh`, `cosh`, `tanh`, `acosh`, `asinh`, `atanh`,
`gcd`, `lcm`, `factorial`, `comb`, `perm`, `isqrt`,
`trunc`, `fmod`, `modf`, `remainder`,
`fsum`, `hypot`, `dist`, `isclose`, `isfinite`, `copysign`,
`cbrt`, `exp2`, `expm1`, `log1p`,
`erf`, `erfc`, `gamma`, `lgamma`

## Imports and modules

- **Relative imports** — `from . import x` is not supported
- **Standard library** — only `math` and `sys` are available as built-in modules

## Error reporting

- **No tracebacks** — exceptions show an error message but no Python-style stack trace
- **Incomplete exception hierarchy** — only the classes listed in [features.md](features.md) are available; `OSError`, `AttributeError`, `NameError`, etc. are raised as `RuntimeError` internally

## REPL

- **Single-line input only** — compound statements (functions, classes, `if`, `for`, etc.) must be run as a script file
