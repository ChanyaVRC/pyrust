# Parity tests for round, divmod, pow, hash, chr, ord, bin, oct, hex,
# issubclass, delattr

# ── round ─────────────────────────────────────────────────────────────────────
print("round-int", round(5))
print("round-int-neg", round(-3))
print("round-float-basic", round(3.6))
print("round-float-basic-neg", round(-2.7))
# Banker's rounding: 0.5 rounds to nearest even
print("round-half-even-0.5", round(0.5))
print("round-half-even-1.5", round(1.5))
print("round-half-even-2.5", round(2.5))
print("round-ndigits-0", round(3.14, 0))
print("round-ndigits-1", round(3.14159, 1))
print("round-ndigits-2", round(3.14159, 2))
print("round-ndigits-neg", round(1234.5, -2))

# ── divmod ────────────────────────────────────────────────────────────────────
print("divmod-int", divmod(17, 5))
print("divmod-int-neg", divmod(-17, 5))
print("divmod-int-neg2", divmod(17, -5))
print("divmod-float", divmod(10.5, 3.0))

try:
    divmod(5, 0)
    print("divmod-zero-no-error")
except ZeroDivisionError:
    print("divmod-zero-error", True)

# ── pow ───────────────────────────────────────────────────────────────────────
print("pow-int-pos", pow(2, 10))
print("pow-int-zero-exp", pow(5, 0))
print("pow-float", pow(2.0, 0.5))
print("pow-neg-exp", pow(2, -1))
print("pow-3arg", pow(2, 10, 100))
print("pow-3arg-b", pow(3, 4, 7))

try:
    pow(2, 2, 0)
    print("pow-zero-mod-no-error")
except ValueError:
    print("pow-zero-mod-error", True)

# ── hash ──────────────────────────────────────────────────────────────────────
# Test that hash returns an int and that equal objects have equal hashes
print("hash-int", hash(42) == hash(42))
print("hash-zero", hash(0) == hash(0))
print("hash-bool-true", hash(True) == hash(1))
print("hash-bool-false", hash(False) == hash(0))
# hash of a string is consistent
print("hash-str-consistent", hash("hello") == hash("hello"))
print("hash-str-diff", hash("hello") != hash("world"))
# hash of an int float should equal hash of int (CPython compat)
print("hash-float-int", hash(1.0) == hash(1))
print("hash-float-int-2", hash(42.0) == hash(42))

try:
    hash([1, 2, 3])
    print("hash-list-no-error")
except TypeError:
    print("hash-list-error", True)

try:
    hash({"a": 1})
    print("hash-dict-no-error")
except TypeError:
    print("hash-dict-error", True)

# ── chr ───────────────────────────────────────────────────────────────────────
print("chr-65", chr(65))
print("chr-97", chr(97))
print("chr-48", chr(48))
print("chr-0", ord(chr(0)) == 0)
print("chr-max", ord(chr(1114111)) == 1114111)

try:
    chr(-1)
    print("chr-neg-no-error")
except ValueError:
    print("chr-neg-error", True)

try:
    chr(1114112)
    print("chr-overflow-no-error")
except ValueError:
    print("chr-overflow-error", True)

# ── ord ───────────────────────────────────────────────────────────────────────
print("ord-A", ord("A"))
print("ord-a", ord("a"))
print("ord-0", ord("0"))
print("ord-unicode", ord(chr(1114111)) == 1114111)

try:
    ord("")
    print("ord-empty-no-error")
except TypeError:
    print("ord-empty-error", True)

try:
    ord("ab")
    print("ord-long-no-error")
except TypeError:
    print("ord-long-error", True)

try:
    ord(65)
    print("ord-int-no-error")
except TypeError:
    print("ord-int-error", True)

# ── bin ───────────────────────────────────────────────────────────────────────
print("bin-0", bin(0))
print("bin-1", bin(1))
print("bin-10", bin(10))
print("bin-neg", bin(-5))
print("bin-255", bin(255))
print("bin-bool-true", bin(True))
print("bin-bool-false", bin(False))

try:
    bin(3.14)
    print("bin-float-no-error")
except TypeError:
    print("bin-float-error", True)

# ── oct ───────────────────────────────────────────────────────────────────────
print("oct-0", oct(0))
print("oct-8", oct(8))
print("oct-neg", oct(-8))
print("oct-255", oct(255))
print("oct-bool-true", oct(True))

try:
    oct(1.5)
    print("oct-float-no-error")
except TypeError:
    print("oct-float-error", True)

# ── hex ───────────────────────────────────────────────────────────────────────
print("hex-0", hex(0))
print("hex-255", hex(255))
print("hex-neg", hex(-255))
print("hex-256", hex(256))
print("hex-bool-true", hex(True))

try:
    hex(1.5)
    print("hex-float-no-error")
except TypeError:
    print("hex-float-error", True)

# ── issubclass ────────────────────────────────────────────────────────────────
class Animal:
    pass

class Dog(Animal):
    pass

class Cat(Animal):
    pass

class GoldenRetriever(Dog):
    pass

print("issubclass-same", issubclass(Dog, Dog))
print("issubclass-direct", issubclass(Dog, Animal))
print("issubclass-indirect", issubclass(GoldenRetriever, Animal))
print("issubclass-false", issubclass(Cat, Dog))
print("issubclass-tuple", issubclass(Dog, (Cat, Animal)))
print("issubclass-tuple-false", issubclass(Dog, (Cat, Cat)))

try:
    issubclass("not_a_class", Animal)
    print("issubclass-non-class-no-error")
except TypeError:
    print("issubclass-non-class-error", True)

# ── delattr ───────────────────────────────────────────────────────────────────
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

p = Point(1, 2)
print("delattr-before", p.x)
delattr(p, "x")

try:
    _ = p.x
    print("delattr-after-no-error")
except AttributeError:
    print("delattr-after-error", True)

# delattr missing attribute raises AttributeError
try:
    delattr(p, "z")
    print("delattr-missing-no-error")
except AttributeError:
    print("delattr-missing-error", True)
