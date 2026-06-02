# str.join builds the result in a single allocation by borrowing each element
# (#1995). Covers separator/element shapes, unicode, str-subclass elements
# (#1927, accepted), dict keys, and the non-str TypeError parity message.

# --- basic separators / shapes ---
print(",".join(["a", "b", "c"]))   # a,b,c
print("".join(["a", "b", "c"]))    # abc
print(",".join([]))                # (empty)
print(",".join(["only"]))          # only
print("".join([]))                 # (empty)
print(",".join(("x", "y", "z")))   # x,y,z (tuple)

# --- str iterable: chars joined by sep ---
print("-".join("abc"))             # a-b-c
print("".join("abc"))              # abc
print("-".join(""))                # (empty)
print("-".join("x"))               # x

# --- unicode / multibyte elements and separator ---
print("、".join(["日", "本", "語"]))   # 日、本、語
print("🔥".join(["a", "b"]))           # a🔥b
print("".join(["αβ", "γδ"]))           # αβγδ

# --- dict keys ---
print(",".join({"k1": 1, "k2": 2}))   # k1,k2
print(",".join({}))                    # (empty)

# --- generator argument ---
print(",".join(str(i) for i in range(5)))   # 0,1,2,3,4

# --- str-subclass elements join by their str value (#1927) ---
class S(str):
    pass

print(",".join([S("p"), S("q"), "r"]))   # p,q,r
print(repr(",".join([S("hi")])))         # 'hi'  (result is plain str)

# --- non-str element: exact CPython TypeError with index + type ---
try:
    ",".join(["a", 1, "c"])
except TypeError as e:
    print(e)                              # sequence item 1: expected str instance, int found

try:
    ",".join([None])
except TypeError as e:
    print(e)                              # sequence item 0: expected str instance, NoneType found

try:
    ",".join({1: 2})
except TypeError as e:
    print(e)                              # sequence item 0: expected str instance, int found

try:
    ",".join(5)
except TypeError as e:
    print(e)                              # can only join an iterable
