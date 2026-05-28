# str % args — printf-style string formatting (#1018, #1393)

# Basic %s
print("%s" % "hello")
print("hello %s" % "world")
print("%s %s" % ("a", "b"))

# Single non-tuple arg (implicit single-element)
print("%d" % 42)
print("%s" % 99)

# Tuple args
print("%s %d" % ("x", 7))
print("%d + %d = %d" % (1, 2, 3))

# Named args (dict)
print("%(name)s" % {"name": "Alice"})
print("%(x)d + %(y)d" % {"x": 1, "y": 2})

# Literal percent
print("100%%")
print("%%" % ())
print("50%% done" % ())

# %r
print("%r" % "hello")
print("%r" % 42)

# Hex / octal
print("%x" % 255)
print("%X" % 255)
print("%o" % 8)

# Width and precision
print("%-10s|" % "left")
print("%10s|" % "right")
print("%05d" % 42)
print("%.2f" % 3.14159)
print("%8.3f" % 3.14159)

# Plus sign and space flags
print("%+d" % 42)
print("%+d" % -42)
print("% d" % 42)
print("% d" % -42)

# %c from int
print("%c" % 65)

# Zero fill with width
print("%010d" % 42)
print("%010d" % -42)

# dict as single positional arg (no %(key) in format → not a mapping)
print("%s" % {"a": 1})
print("%r" % {"a": 1})

# Hash flag (#) for hex and octal
print("%#x" % 255)
print("%#X" % 255)
print("%#o" % 8)
print("%#o" % 0)
print("%#x" % 0)

# Hash + zero-fill: zeros go between prefix and digits
print("%#010x" % 255)
print("%#010X" % 255)
print("%#010o" % 8)

# Hash with negative
print("%#x" % -255)
print("%#o" % -8)

# NaN and Inf in %f and %F
print("%f" % float("nan"))
print("%F" % float("nan"))
print("%f" % float("inf"))
print("%F" % float("inf"))
print("%f" % -float("inf"))
print("%F" % -float("inf"))

# Plus/space flags with NaN and Inf
print("%+f" % float("nan"))
print("% f" % float("nan"))
print("%+f" % float("inf"))
print("% f" % float("inf"))

# Error: wrong type for %o/%x (CPython says "an integer is required")
try:
    "%o" % "hello"
except TypeError as e:
    print("TypeError:", e)

try:
    "%x" % "hello"
except TypeError as e:
    print("TypeError:", e)

# Error: float for %o/%x/%X (CPython 3.12 rejects, unlike %d which truncates)
try:
    "%o" % 1.5
except TypeError as e:
    print("TypeError:", e)

try:
    "%x" % 1.5
except TypeError as e:
    print("TypeError:", e)

try:
    "%X" % 1.5
except TypeError as e:
    print("TypeError:", e)

# Error: wrong type for %f/%g (CPython says "must be real number")
try:
    "%f" % "hello"
except TypeError as e:
    print("TypeError:", e)

try:
    "%g" % "hello"
except TypeError as e:
    print("TypeError:", e)

# Not all args consumed: TypeError
try:
    "%s" % ("a", "b")
except TypeError as e:
    print("TypeError:", e)

# Not enough args: TypeError
try:
    "%s %s" % ("a",)
except TypeError as e:
    print("TypeError:", e)
