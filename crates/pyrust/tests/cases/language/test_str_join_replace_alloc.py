# str.join / str.replace allocation-reduction refactor: results must stay
# byte-identical to CPython 3.12 across all iterable/argument shapes.

# --- join over every iterable kind ---
print(",".join(["a", "b", "c"]))
print("".join([]))
print("-".join(("x", "y", "z")))
print("".join("abcdef"))                 # str iterable -> chars
print(",".join({"k1": 1, "k2": 2}))      # dict -> keys, insertion order
print("".join(str(i) for i in range(6))) # generator (needs collect)
print("|".join(["single"]))
print("é".join(["a", "b", "c"]))         # multibyte separator
print(len("x".join(["ab" * 5] * 40)))    # >16 parts: SmallVec spills to heap
try:
    ",".join(["a", 1, "c"])
except TypeError as e:
    print("TypeError:", e)
try:
    "".join([None])
except TypeError as e:
    print("TypeError:", e)

# --- replace across sign of length change, counts, empty, multibyte ---
print("the a the b the".replace("the", "X"))     # shrink
print("aaaa".replace("a", "bb"))                 # grow (result > source cap)
print("aaa".replace("aa", "b"))                  # overlapping (non-overlap match)
print("abc".replace("", "-"))                    # empty from -> insert between
print("hello".replace("l", "L", 1))              # count limit
print("hello".replace("l", "L", 0))              # count 0 -> unchanged
print("hello".replace("z", "Q"))                 # no match -> unchanged
print("héllo wörld".replace("ö", "o"))           # multibyte
print("".replace("", "x"))                       # empty string
print("x".replace("x", ""))                      # to empty
print(repr("mississippi".replace("ss", "S")))
print("a.b.c.d.e".replace(".", "/", 2))          # partial count
print(("the " * 100).replace("the", "a")[:20])   # many occurrences

# translate also routed through the sliced string::call.
print("hello".translate(str.maketrans("lo", "LO")))
