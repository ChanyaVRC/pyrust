# Parity Test Cases

Python behavior parity scripts are grouped by concern:

- `language/`: expressions, statements, operators, functions, and collections
- `runtime/`: classes and exception handling behavior
- `stdlib/`: module import behavior and built-in modules

Naming rule:

- Test script files must match `test_*.py` so `cargo test --test parity_compare` can discover them.

Notes:

- These tests are intended to validate Python-language behavior, not byte-for-byte CPython traceback formatting.
- Keep cases small and focused so regressions are easy to localize.
