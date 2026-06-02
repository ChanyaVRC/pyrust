# Dict-literal construction (BuildDict VM handler).
# Covers empty / small / large literals, duplicate-key dedup
# (last value wins), and insertion-order preservation.

# Empty.
print({})

# Small literals.
print({1: 2, 3: 4})
print({"a": 1, "b": 2})

# Duplicate keys: last value wins, position of first occurrence kept.
print({1: 1, 1: 2})
print({"a": 1, "b": 2, "a": 3})

# Mixed int/str keys preserve insertion order.
print(list({3: "c", 1: "a", 2: "b"}.keys()))

# Large literal of known size (exercises the capacity hint).
big = {
    0: "z", 1: "y", 2: "x", 3: "w", 4: "v", 5: "u", 6: "t", 7: "s",
    8: "r", 9: "q", 10: "p", 11: "o", 12: "n", 13: "m", 14: "l", 15: "k",
    16: "j", 17: "i", 18: "h", 19: "g",
}
print(len(big))
print(big[0], big[19])
print(list(big.keys()))

# Equality is order-independent.
print({1: 2, 3: 4} == {3: 4, 1: 2})
