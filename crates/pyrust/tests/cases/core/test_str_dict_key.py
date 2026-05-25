# Parity fixture for PyKey::Str(Value) refactor (issue #505).
#
# Verifies that string-keyed dict and set operations continue to produce
# correct output after the internal representation of PyKey::Str changed
# from String (heap alloc) to Value (O(1) RC bump clone).
#
# The correctness invariants that must hold:
#   1. Lookup: d["key"] finds the right value.
#   2. Deduplication: same string content → same bucket (hash+eq stable).
#   3. Equality: dict/set equality is content-based.
#   4. Iteration: keys come back as str, not some internal type.
#   5. Empty string key works.
#   6. Multi-byte UTF-8 key works.
#   7. str.join over dict keys works (tests string.rs join arm).
#   8. set(str) iteration yields single-character string keys.

# 1. Basic lookup
d = {"a": 1, "b": 2, "c": 3}
print(d["a"], d["b"], d["c"])

# 2. Key deduplication — inserting the same string twice keeps one entry
d2 = {}
d2["x"] = 10
d2["x"] = 20
print(len(d2), d2["x"])

# 3. Dict equality is content-based
d3a = {"hello": 1, "world": 2}
d3b = {"hello": 1, "world": 2}
print(d3a == d3b)

# 4. Iteration yields str keys
for k in {"p": 1, "q": 2}:
    print(type(k).__name__, k)

# 5. Empty string key
d4 = {"": 99}
print(d4[""])

# 6. Multi-byte UTF-8 key (2-byte codepoint)
d5 = {"é": "cafe"}
print(d5["é"])

# 7. str.join over dict keys (exercises the string.rs join Dict arm)
joined = ",".join({"alpha": 1, "beta": 2})
print(joined)

# 8. set(str) — produces single-char string keys
s = set("abc")
chars = sorted(s)
print(chars)

# 9. to_key() round-trip: Value -> PyKey -> back to Value
d6 = {}
key_str = "roundtrip"
d6[key_str] = 42
print(d6[key_str])

# 10. Large tight loop — correctness (not a bench, just must produce "ok")
d7 = {"a": 1, "b": 2, "c": 3}
total = 0
for _ in range(10000):
    total += d7["a"] + d7["b"] + d7["c"]
print(total == 60000)
