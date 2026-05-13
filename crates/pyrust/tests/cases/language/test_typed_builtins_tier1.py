# Parity tests for the Tier-1 typed-dialect migration (#400):
# repr / ascii / hash / id / hex.  Each builtin is exercised on:
#   - the happy path
#   - wrong-type rejection (TypeError)
#   - missing positional arg (TypeError)
#   - extra positional arg (TypeError)
#   - keyword arg (positional-only — TypeError)
#
# The TypeError-wording from the typed-dialect prelude doesn't match
# CPython byte-for-byte (e.g. "missing required argument: 'obj'" vs
# CPython's "takes exactly one argument (0 given)"), so the script
# only asserts the *category* of error (caught TypeError → True) and
# prints `True` either way.  Happy-path outputs are full equality.

# ── repr ─────────────────────────────────────────────────────────────────────
# Happy path — repr accepts any object via the PyValue wrapper.
print("repr-int", repr(42))
print("repr-str", repr("hi"))
print("repr-list", repr([1, 2]))
print("repr-none", repr(None))

# Custom __repr__ still dispatches.
class WithRepr:
    def __repr__(self):
        return "custom-repr"

print("repr-user", repr(WithRepr()))

# repr's prelude — wrong-arity / kwarg rejection.
try:
    repr()
except TypeError:
    print("repr-missing", True)

try:
    repr(1, 2)
except TypeError:
    print("repr-extra", True)

try:
    repr(obj=1)
except TypeError:
    print("repr-kw", True)


# ── ascii ────────────────────────────────────────────────────────────────────
print("ascii-str", ascii("café"))
print("ascii-int", ascii(7))
print("ascii-list", ascii(["a", "é"]))

try:
    ascii()
except TypeError:
    print("ascii-missing", True)

try:
    ascii(1, 2)
except TypeError:
    print("ascii-extra", True)

try:
    ascii(obj="x")
except TypeError:
    print("ascii-kw", True)


# ── hash ─────────────────────────────────────────────────────────────────────
# Equality (not exact value) — CPython hashes differ from pyrust's.
print("hash-int-eq", hash(0) == hash(0))
print("hash-bool-int", hash(True) == hash(1))
print("hash-str-stable", hash("abc") == hash("abc"))
print("hash-float-int", hash(1.0) == hash(1))

try:
    hash([1])
except TypeError:
    print("hash-list", True)

try:
    hash({})
except TypeError:
    print("hash-dict", True)

try:
    hash({1, 2})
except TypeError:
    print("hash-set", True)

try:
    hash()
except TypeError:
    print("hash-missing", True)

try:
    hash(1, 2)
except TypeError:
    print("hash-extra", True)

try:
    hash(obj=1)
except TypeError:
    print("hash-kw", True)


# ── id ───────────────────────────────────────────────────────────────────────
# We can't compare the numeric id against CPython, but we can check that
# id() returns a stable int for the same binding and a TypeError on
# missing args.
class _IdProbe:
    pass

x = _IdProbe()
print("id-int-type", type(id(x)) is int)
print("id-stable", id(x) == id(x))

try:
    id()
except TypeError:
    print("id-missing", True)

try:
    id(1, 2)
except TypeError:
    print("id-extra", True)

try:
    id(obj=1)
except TypeError:
    print("id-kw", True)


# ── hex ──────────────────────────────────────────────────────────────────────
print("hex-pos", hex(255))
print("hex-zero", hex(0))
print("hex-neg", hex(-1))
print("hex-bool-true", hex(True))
print("hex-bool-false", hex(False))

try:
    hex("nope")
except TypeError:
    print("hex-type-str", True)

try:
    hex(3.14)
except TypeError:
    print("hex-type-float", True)

try:
    hex([])
except TypeError:
    print("hex-type-list", True)

try:
    hex()
except TypeError:
    print("hex-missing", True)

try:
    hex(1, 2)
except TypeError:
    print("hex-extra", True)

try:
    hex(x=1)
except TypeError:
    print("hex-kw", True)
