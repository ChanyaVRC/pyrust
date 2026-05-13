# Parity tests for the Tier-2 typed-dialect migration (#400):
# bin / oct.  Each builtin is exercised on:
#   - the happy path (int / bool / negative)
#   - wrong-type rejection (TypeError) — str / float / None / list
#   - missing positional arg (TypeError)
#   - extra positional arg (TypeError)
#   - keyword arg (positional-only — TypeError)
#
# The TypeError-wording from the typed-dialect prelude doesn't match
# CPython byte-for-byte, so the script only asserts the *category* of
# error (caught TypeError -> True) and prints `True` either way.
# Happy-path outputs are full equality.
#
# Bignum cases (`bin(2**100)` / `oct(2**100)`) are intentionally omitted:
# CPython supports them, but pyrust raises `OverflowError` via
# `PyInt::expect_i64` (deliberate divergence, see the docstring on
# `bin` / `oct` / `hex` in `builtins.rs`).
# TODO: bignum support -- tracked under #400.

# -- bin ---------------------------------------------------------------------
print("bin-zero", bin(0))
print("bin-one", bin(1))
print("bin-255", bin(255))
print("bin-neg", bin(-1))
print("bin-bool-true", bin(True))
print("bin-bool-false", bin(False))

try:
    bin("x")
except TypeError:
    print("bin-type-str", True)

try:
    bin(1.0)
except TypeError:
    print("bin-type-float", True)

try:
    bin(None)
except TypeError:
    print("bin-type-none", True)

try:
    bin([])
except TypeError:
    print("bin-type-list", True)

try:
    bin()
except TypeError:
    print("bin-missing", True)

try:
    bin(1, 2)
except TypeError:
    print("bin-extra", True)

try:
    bin(x=1)
except TypeError:
    print("bin-kw", True)


# -- oct ---------------------------------------------------------------------
print("oct-zero", oct(0))
print("oct-one", oct(1))
print("oct-255", oct(255))
print("oct-neg", oct(-1))
print("oct-bool-true", oct(True))
print("oct-bool-false", oct(False))

try:
    oct("x")
except TypeError:
    print("oct-type-str", True)

try:
    oct(1.0)
except TypeError:
    print("oct-type-float", True)

try:
    oct(None)
except TypeError:
    print("oct-type-none", True)

try:
    oct([])
except TypeError:
    print("oct-type-list", True)

try:
    oct()
except TypeError:
    print("oct-missing", True)

try:
    oct(1, 2)
except TypeError:
    print("oct-extra", True)

try:
    oct(x=1)
except TypeError:
    print("oct-kw", True)
