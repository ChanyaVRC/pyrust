# int, float, str, bool, list, tuple conversions

# --- int ---
print("int-from-str", int('42'))
print("int-from-float", int(3.9))
print("int-from-bool-t", int(True))
print("int-from-bool-f", int(False))
print("int-from-int", int(7))
print("int-zero-args", int())
print("int-whitespace", int('  -5  '))

# --- float ---
print("float-from-str", float('3.14'))
print("float-from-int", float(4))
print("float-from-bool-t", float(True))
print("float-from-bool-f", float(False))
print("float-zero-args", float())
print("float-from-float", float(2.5))

# --- str ---
print("str-from-int", str(42))
print("str-from-float", str(3.14))
print("str-from-bool-t", str(True))
print("str-from-bool-f", str(False))
print("str-from-none", str(None))
print("str-zero-args", str())

# --- bool ---
print("bool-false-int", bool(0))
print("bool-true-int", bool(1))
print("bool-true-neg", bool(-1))
print("bool-false-str", bool(''))
print("bool-true-str", bool('x'))
print("bool-false-list", bool([]))
print("bool-true-list", bool([0]))
print("bool-none", bool(None))
print("bool-zero-args", bool())

# --- list ---
print("list-from-tuple", list((1, 2, 3)))
print("list-from-range", list(range(4)))
print("list-from-str", list('abc'))
print("list-from-set-len", len(list({1, 2, 3})))
print("list-zero-args", list())

# --- tuple ---
print("tuple-from-list", tuple([1, 2, 3]))
print("tuple-from-range", tuple(range(3)))
print("tuple-zero-args", tuple())

# int(str, base) — Issue #99
print("int-base2", int("101", 2))
print("int-base8", int("17", 8))
print("int-base16", int("ff", 16))
print("int-base16-upper", int("FF", 16))
print("int-0x-prefix", int("0xff", 16))
print("int-0b-prefix", int("0b101", 2))
print("int-0o-prefix", int("0o17", 8))
print("int-base36", int("z", 36))
try:
    int("42", 37)
except ValueError:
    print("int-base-high", "ValueError")
try:
    int("42", 1)
except ValueError:
    print("int-base-low", "ValueError")
try:
    int("xyz", 10)
except ValueError:
    print("int-bad-literal", "ValueError")
