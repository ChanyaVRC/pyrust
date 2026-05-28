big = 99999999999999999999
neg = -big

# Basic decimal, hex, octal
print("%d" % big)
print("%d" % neg)
print("%x" % big)
print("%x" % neg)
print("%o" % big)
print("%o" % neg)

# Uppercase hex
print("%X" % big)
print("%X" % neg)

# Width and alignment
print("%30d" % big)
print("%-30d" % big)
print("%+d" % big)
print("% d" % big)

# Zero-fill
print("%030d" % big)
print("%030x" % big)
print("%030X" % big)
print("%030o" % big)

# Hash flag (base prefix)
print("%#x" % big)
print("%#x" % neg)
print("%#X" % big)
print("%#X" % neg)
print("%#o" % big)
print("%#o" % neg)
print("%#+x" % big)
print("% #x" % big)
print("%#030x" % big)

# Negative with zero-fill
print("%030d" % neg)
print("%030x" % neg)
print("%#030x" % neg)

# Values that fit in i64 still work (no regression)
print("%d" % 42)
print("%x" % 255)
print("%o" % 8)
print("%d" % -1)
print("%x" % -1)

# i64 boundary values
print("%d" % 9223372036854775807)
print("%d" % -9223372036854775808)
print("%x" % 9223372036854775807)
print("%x" % -9223372036854775808)

# Float truncation: small floats use Small path
print("%d" % 1.5)
print("%d" % -1.5)

# Float truncation: large finite float beyond i64 range uses Big path
print("%d" % 1e20)
print("%d" % -1e20)
print("%d" % 1e30)

# Float NaN/inf raise ValueError/OverflowError (matching CPython)
try:
    print("%d" % float("nan"))
except ValueError as e:
    print("ValueError:", str(e))

try:
    print("%d" % float("inf"))
except OverflowError as e:
    print("OverflowError:", str(e))

try:
    print("%d" % float("-inf"))
except OverflowError as e:
    print("OverflowError:", str(e))
