# str %-formatting fast-path coverage.
#
# The formatter grew a fast path that writes `%s` (plain str) and `%d/%i/%u`
# (plain int, no +/space flag, no width) straight into the output buffer,
# skipping the per-conversion temporary String.  This fixture pins that the
# fast path is byte-for-byte identical to CPython and that neighbouring
# specifiers / flags / width / precision (which take the general path) are
# unaffected.

# --- fast-path cases (exercised directly by the optimisation) ---
print("%d-%s" % (123, "x"))
print("%s" % "hello")
print("%d" % 456)
print("%i" % -789)
print("%u" % 0)
print("%d/%d/%d" % (1, 22, 333))
print("%s%s%s" % ("a", "b", "c"))
print("prefix %d middle %s suffix" % (42, "z"))
print("%d" % -2147483648)
# empty and unicode literal runs around conversions
print("" % ())
print("héllo %s wörld %d" % ("ünïcödé", 7))
print("%d%%" % 50)  # %% literal must not consume the fast path

# --- values that must NOT take the str/int fast path ---
print("%s" % 123)          # int through %s -> str()
print("%s" % 3.5)          # float through %s
print("%s" % [1, 2])       # list through %s
print("%s" % None)
print("%s" % True)         # bool -> "True"
print("%d" % True)         # bool -> 1 (separate ValueKind)
print("%d" % 3.9)          # float truncates toward zero -> 3
print("%d" % -3.9)         # -> -3

# --- flags force the general path ---
print("%+d" % 5)
print("% d" % 5)
print("%+d" % -5)
print("% d" % -5)

# --- width / precision force the general path ---
print("[%5d]" % 42)
print("[%-5d]" % 42)
print("[%05d]" % 42)
print("[%8s]" % "hi")
print("[%-8s]" % "hi")
print("[%.3s]" % "hello")
print("[%5.3s]" % "hello")
print("[%*d]" % (6, 3))
print("[%.*f]" % (2, 3.14159))

# --- other specifiers around the fast path ---
print("%x %X %o" % (255, 255, 64))
print("%#x %#o" % (255, 64))
print("%r" % "quote")
print("%c%c%c" % (72, 105, 33))
print("%e" % 12345.678)
print("%g" % 0.0001)
print("%f" % 2.5)

# --- big int through %d fast-path guard (BigInt is a distinct kind) ---
print("%d" % (10 ** 30))
print("%d-%d" % (10 ** 40, 7))

# --- mapping mode unaffected ---
print("%(name)s is %(age)d" % {"name": "Bob", "age": 30})

# --- single non-tuple arg wrapped as one-element positional ---
print("%s" % "solo")
print("%d" % 99)
