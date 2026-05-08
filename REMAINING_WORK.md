# Remaining Work

This document is an exhaustive inventory of the major gaps that still remain in PyRust relative to the current codebase and a broader Python 3 feature set.

It is intentionally broader than the README limitations section. The goal here is not just to list the next 2 to 3 milestones, but to capture the remaining surface area so future work can be planned explicitly.

## Current Baseline

PyRust currently supports the following core subset:

- [x] Variables and simple assignment
- [x] Numeric, string, tuple, set, list, and dictionary literals
- [x] Arithmetic, boolean, bitwise, matrix-multiplication, membership, identity, and chained comparison operators
- [x] `if`, `while`, `for`, `break`, `continue`, `pass`, `assert`, `del`, `with`
- [x] `def`, `return`, trailing default arguments, keyword arguments, `*args`, `**kwargs`, closures, decorators, `global`, `nonlocal`, `lambda`
- [x] `class`, instance attributes, bound methods, `__init__`, decorators, and single inheritance
- [x] `print`, `len`, and `range`
- [x] REPL mode and script execution
- [x] Exception handling: `raise`, `raise ... from ...`, `try / except / else / finally`, typed and bare raise, tuple except clauses, built-in exception classes
- [x] Import system: `import module`, `from module import name`, aliases, user .py files, built-in `math` and `sys` modules
- [x] List/string slicing, slice assignment, and slice deletion including step slices
- [x] A categorized Python parity suite and interpreter unit tests

Everything below is still missing, only partially supported, or not yet validated deeply enough.

## 1. Missing Statements and Top-Level Syntax

These are statement forms that are not currently represented in the token set, parser, AST, or runtime.

1. Exception-related statements

- [ ] Traceback reporting and richer exception-chain presentation

2. Import-related statements

- [x] `import`
- [x] `from ... import ...`
- [x] Alias handling such as `import x as y`
- [ ] Relative imports
- [ ] Broader standard library (only `math` and `sys` currently)

3. Context-management statements

- [ ] Deeper protocol parity such as `__context__` / exception-chain bookkeeping and more exact traceback objects

4. Async and generator statements

- [ ] `async def`
- [ ] `async for`
- [ ] `async with`
- [ ] `yield`
- [ ] `yield from`

5. Pattern matching

- [ ] `match`
- [ ] `case`
- [ ] Guards
- [ ] Sequence, mapping, and class patterns

6. Other missing statement forms

- [ ] Annotated assignment such as `x: int = 1`
- [ ] Type alias statements

## 2. Missing Expression Forms

These are expression-level gaps in the current lexer/parser/AST.

1. Conditional and anonymous expressions

- [ ] Comprehension forms and generator expressions

2. Additional literal and container forms

- [ ] Empty set construction semantics
- [ ] Bytes literals
- [ ] Triple-quoted strings
- [ ] Raw strings and other string-prefix variants

3. Slicing and extended subscription syntax

- [ ] Slice objects as first-class runtime values

4. Membership and identity expressions

- [ ] More exact parity for edge cases across user-defined objects

5. Additional operators

- [ ] Broader numeric protocol support through special methods beyond `@`

6. Assignment-related expressions not currently modeled

- [ ] Starred assignment targets

7. Call-site features

- [ ] Mixed expansion semantics and related error cases

8. Comparison semantics still not covered fully

- [ ] Full parity for mixed-type comparison behavior

## 3. Missing Function Definition Features

The current function model supports a useful subset, but large parts of Python's signature system are still absent.

1. Parameter kinds

- [ ] Keyword-only parameters
- [ ] Positional-only parameters
- [ ] Bare `*` separator

2. Definition-time features

- [ ] Function annotations
- [ ] Return annotations
- [ ] Generic function syntax and type parameters

3. Callable behavior gaps

- [ ] Generator function semantics
- [ ] Coroutine semantics
- [ ] More precise argument binding and error message parity
- [ ] Support for more built-in callable types

## 4. Missing Class and Object Model Features

The object model is currently intentionally narrow. The following features are still missing.

1. Class definition syntax and decoration

- [ ] Generic class syntax and type parameters

2. Inheritance model expansion

- [ ] Multiple inheritance
- [ ] Full C3 method resolution order
- [ ] `super`
- [ ] Better base-class validation and diagnostics

3. Descriptor and attribute access model

- [ ] Descriptor protocol
- [ ] `staticmethod`
- [ ] `classmethod`
- [ ] `property`
- [ ] `__getattr__`
- [ ] `__getattribute__`
- [ ] `__setattr__`
- [ ] `__delattr__`
- [ ] `__set_name__`

4. Class construction hooks

- [ ] `__new__`
- [ ] `__init_subclass__`
- [ ] Metaclass selection and metaclass hooks
- [ ] `__prepare__`

5. Broader object protocol support

- [ ] `__call__`
- [ ] `__iter__`
- [ ] `__next__`
- [ ] `__len__`
- [ ] `__bool__`
- [ ] `__getitem__`, `__setitem__`, `__delitem__`
- [ ] Numeric special methods
- [ ] Richer string and representation hooks

6. Class and instance namespace behavior still not modeled deeply

- [ ] `__dict__`
- [ ] `__class__`
- [ ] More accurate class-body execution semantics for advanced cases
- [ ] Zero-argument `super` support via implicit `__class__` cell behavior

## 5. Missing Built-ins and Runtime Types

PyRust currently exposes only a very small built-in surface.

1. Built-in constants and types not surfaced as built-ins

- [ ] `object`
- [ ] `type`
- [ ] `int`
- [ ] `float`
- [ ] `str`
- [ ] `bool`
- [ ] `list`
- [ ] `dict`
- [ ] `tuple`
- [ ] `set`
- [ ] Exception classes

2. Built-in functions not yet present

- [ ] Common conversion and inspection functions such as `repr`, `type`, `isinstance`, `issubclass`, `iter`, `next`
- [ ] Numeric helpers such as `abs`, `min`, `max`, `sum`
- [ ] Container helpers such as `enumerate`, `zip`, `sorted`, `reversed`, `any`, `all`
- [ ] I/O helpers such as `open`

3. Runtime value types not yet modeled directly

- [ ] Bytes
- [ ] Richer exception objects with traceback/context metadata
- [ ] Slice objects
- [ ] Iterators and generators as dedicated runtime objects
- [ ] Coroutines and async iterators

## 6. Missing Collection and Data Semantics

The current container support is useful but still shallow compared with Python.

1. Lists

- [ ] List methods
- [ ] In-place update semantics

2. Dictionaries

- [ ] Dictionary methods
- [ ] More complete key and hashing behavior
- [ ] Better `KeyError`-like exception behavior once exceptions exist

3. Strings

- [ ] Richer string literal syntax
- [ ] String methods
- [ ] Escape-sequence coverage review

4. Tuples and sets

- [ ] Hashability rules involving tuples and sets

5. Iteration model

- [ ] Iterator protocol support instead of only eagerly materializing iterable values
- [ ] Custom iterable objects
- [ ] Better behavior for user-defined sequence and mapping types

## 7. Missing Error Model and Diagnostics

The current interpreter reports runtime and parse failures, but the error model is still much simpler than Python's.

1. Structured exception hierarchy

- [ ] Richer exception hierarchy and traceback/context attachment

2. Better syntax diagnostics

- [ ] Line and column information
- [ ] More CPython-like parse messages for invalid syntax
- [ ] Better differentiation between parse-time and runtime failures

3. Better runtime diagnostics

- [ ] Tracebacks
- [ ] Source-aware runtime messages
- [ ] More accurate wording for argument, attribute, and type errors

## 8. Missing REPL and CLI Features

The current command-line interface works, but it is still minimal.

1. REPL experience

- [ ] Multi-line input for blocks
- [ ] Better prompt handling for continued input
- [ ] Better display of runtime failures in interactive mode
- [ ] More CPython-like expression echo behavior

2. CLI behavior

- [ ] More robust argument parsing
- [ ] Better file execution diagnostics
- [ ] Optional modes for running parity checks or debug output

## 9. Missing Testing and Validation Coverage

The project already validates a small parity workload and some unit tests, but there is still a lot to add.

1. Unit tests

- [ ] Parser coverage for invalid syntax
- [ ] More runtime tests for binding, truthiness, indexing, and attribute lookup
- [ ] Focused regressions for every bug fixed during development

2. Parity tests against CPython

- [ ] Add negative tests, not only success cases
- [ ] Add parity coverage for errors and diagnostics where practical

3. Documentation sync

- [ ] Keep README aligned with every feature slice
- [ ] Document partially supported features explicitly
- [ ] Record intentional non-goals to avoid roadmap drift

## 10. Partially Supported Features That Still Need Follow-Up

These areas already exist, but are still incomplete enough that they should remain on the roadmap.

1. `print`

- [x] `sep` and `end` are supported
- [ ] `file` currently only accepts `None`
- [ ] `flush` is validated as a boolean but does not provide real stream-control behavior

2. Class support

- [x] Single inheritance works
- [ ] Inheritance is still limited to one base class
- [ ] Broader Python object-model behavior is still absent

3. Function calling

- [x] Positional arguments, keyword arguments, trailing defaults, `*args`, `**kwargs`, decorators, and call-site expansion work
- [ ] Signature shape is still restricted compared with full Python

4. REPL

- [x] Basic single-line interactive execution works
- [ ] Compound multi-line editing does not

## 11. Suggested Development Order

If the goal is to keep shipping coherent vertical slices, a practical order is:

1. [ ] Exceptions and traceback-capable error propagation
2. [x] Import and module support
3. [ ] Decorator syntax plus richer class behavior (`super`, `staticmethod`, `classmethod`)
4. [ ] Function signature completeness (keyword-only, positional-only, bare `*`, annotations)
5. [ ] Richer object and exception protocol support
6. [ ] Comprehensions and additional expression forms
7. [ ] REPL and CLI improvements
8. [ ] Broader built-ins and protocol support

## 12. Out of Scope Unless the Project Goal Changes

These areas should remain explicitly deferred unless PyRust is intentionally expanded far beyond its current educational scope.

- [ ] Full CPython compatibility
- [ ] Full standard library compatibility
- [ ] Bytecode execution
- [ ] JIT or optimizer work
- [ ] Advanced metaprogramming parity
- [ ] Full async ecosystem behavior
- [ ] Full descriptor edge-case parity
- [ ] Full multiple inheritance and metaclass corner-case parity