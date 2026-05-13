# Parity tests for the chr / ord typed-dialect migration (#400, PR #406).
#
# Coverage matrix:
#   - chr: int, bool, negative (ValueError), >0x10FFFF (ValueError),
#          non-int (TypeError).  Bignum -> OverflowError is documented
#          in the `chr` docstring (PR #406) but isn't exercised here
#          because pyrust integers are i64-bounded; see the comment
#          on the chr-bignum block below.
#   - ord: 1-char str, 1-byte bytes (NEW CPython parity feature -
#          legacy rejected bytes outright), multi-char str
#          (TypeError), empty bytes (TypeError), non-str/bytes
#          (TypeError).
#
# CPython and pyrust diverge on TypeError wording, so error branches
# print only the exception class name (True).

# -- chr ----------------------------------------------------------------------
print("chr-int-0", chr(0))
print("chr-int-65", chr(65))
print("chr-int-max", ord(chr(0x10FFFF)) == 0x10FFFF)
print("chr-bool-true", chr(True))
print("chr-bool-false", chr(False) == chr(0))

try:
    chr(-1)
except ValueError:
    print("chr-neg", True)

try:
    chr(0x110000)
except ValueError:
    print("chr-overflow-range", True)

# Note: bignum chr() (e.g. `chr(2**100)`) raises OverflowError on
# both CPython and modern pyrust via the PyInt expect_i64 path (the
# legacy pyrust body raised ValueError via the range check — see the
# `chr` docstring in builtins.rs).  We can't exercise it here because
# pyrust's integer arithmetic is i64-bounded (`2**100` overflows to
# 0) and large literals are lex-rejected, so the parity comparison
# would diverge structurally rather than just on wording.

# Non-int: TypeError wording differs across implementations, class only.
try:
    chr("a")
except TypeError:
    print("chr-type-str", True)

try:
    chr(3.14)
except TypeError:
    print("chr-type-float", True)


# -- ord ----------------------------------------------------------------------
print("ord-str-a", ord("a"))
print("ord-str-A", ord("A"))
print("ord-str-unicode", ord("é"))

# NEW: 1-byte bytes input (legacy rejected this).
print("ord-bytes-a", ord(b"a"))
print("ord-bytes-zero", ord(bytes([0])))

try:
    ord("ab")
except TypeError:
    print("ord-multichar", True)

try:
    ord(b"")
except TypeError:
    print("ord-bytes-empty", True)

try:
    ord(b"ab")
except TypeError:
    print("ord-bytes-multi", True)

try:
    ord(42)
except TypeError:
    print("ord-type-int", True)

try:
    ord([])
except TypeError:
    print("ord-type-list", True)
