# Interning should be transparent -- same observable behaviour, just fewer allocations.
# The key invariant: string identity is NOT guaranteed (unlike CPython's sys.intern),
# but value equality must hold in all cases.

# Repeated dict key access with the same short literal
d = {}
for i in range(100):
    d["key"] = i
print(d["key"])  # 99

# String concatenation is not interned; the result is still a valid str
s = "hello" + " " + "world"
print(s)         # hello world
print(type(s))   # <class 'str'>

# Short string equality is preserved across many uses of the same literal
keys = ["name", "age", "city"] * 50
counts = {}
for k in keys:
    counts[k] = counts.get(k, 0) + 1
print(counts["name"])   # 50
print(counts["age"])    # 50
print(counts["city"])   # 50

# Strings longer than 40 bytes are not interned but still work correctly
long_key = "a" * 41
d2 = {long_key: 42}
print(d2[long_key])   # 42
print(len(long_key))  # 41

# Dunder strings are correctly interned (no observable effect, just coverage)
print(__name__)  # __main__
