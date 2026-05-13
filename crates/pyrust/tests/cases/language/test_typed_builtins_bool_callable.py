# Parity tests for the typed-dialect migration of `bool` and `callable` (#400,
# follow-up to #403's Tier-1 fixture).  Each builtin is exercised on:
#   - the happy-path matrix
#   - error paths (wrong arity / keyword arg on positional-only)
#
# The typed-dialect prelude's TypeError wording doesn't match CPython
# byte-for-byte, so error-path assertions only check the *category* of
# error (caught TypeError → True), matching the pattern in
# test_typed_builtins_tier1.py.


# ── bool ─────────────────────────────────────────────────────────────────────
# Happy-path matrix: zero-arg, falsy inputs, truthy inputs.
print("bool-empty", bool())
print("bool-zero", bool(0))
print("bool-one", bool(1))
print("bool-none", bool(None))
print("bool-list-empty", bool([]))
print("bool-list-one", bool([1]))
print("bool-str-empty", bool(""))
print("bool-str-x", bool("x"))

# `x` is positional-only — keyword form must raise TypeError.
try:
    bool(x=1)
except TypeError:
    print("bool-kw", True)


# ── callable ─────────────────────────────────────────────────────────────────
# Builtin function, lambda, plain int.
print("callable-print", callable(print))
print("callable-lambda", callable(lambda: 0))
print("callable-int", callable(42))


# User class with __call__ → instance is callable; without → it isn't.
class WithCall:
    def __call__(self):
        return 0


class NoCall:
    pass


print("callable-with-call", callable(WithCall()))
print("callable-no-call", callable(NoCall()))

# Classes themselves are always callable.
print("callable-class", callable(WithCall))

# `obj` is positional-only — keyword form must raise TypeError.
try:
    callable(obj=1)
except TypeError:
    print("callable-kw", True)

# Wrong arity — both 0 and 2 args produce TypeError under the typed-dialect
# prelude (wording differs from the legacy `"{FN_NAME}() takes exactly one
# argument"` Runtime error — confirmed parity-clean by category).
try:
    callable()
except TypeError:
    print("callable-missing", True)

try:
    callable(1, 2)
except TypeError:
    print("callable-extra", True)
