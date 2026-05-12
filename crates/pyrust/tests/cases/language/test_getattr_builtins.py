# getattr / hasattr on built-in types: list, str, dict, tuple, set
#
# Note: PyRust lists/sets are value-typed (assigning a list copies it).
# Therefore, mutation via a stored bound method does not propagate back to
# the original; only read-only methods are tested here for those types.
# See also CLAUDE.md and pyrust-core/src/lib.rs for value semantics.
# This divergence from CPython is tracked in issue #305.

# --- list (read-only methods) ---
x = [1, 2, 3, 4, 5]
assert getattr(x, "count")(3) == 1
assert getattr(x, "index")(2) == 1
assert getattr(x, "copy")() == [1, 2, 3, 4, 5]

assert hasattr(x, "append")
assert hasattr(x, "extend")
assert hasattr(x, "pop")
assert hasattr(x, "reverse")
assert hasattr(x, "sort")
assert not hasattr(x, "foobar")

# --- str ---
s = "Hello"
assert getattr(s, "upper")() == "HELLO"
assert getattr(s, "lower")() == "hello"
assert getattr(s, "startswith")("He") == True
assert getattr(s, "replace")("l", "L") == "HeLLo"
assert getattr(s, "split")("l") == ["He", "", "o"]
assert hasattr(s, "split")
assert hasattr(s, "upper")
# Note: hasattr(s, "format") is not yet supported via the bound-method path
# because str.format is dispatched at the VM level, not through string::call.
assert not hasattr(s, "append")

# bound method retains the receiver across calls (read-only)
upper = getattr(s, "upper")
assert upper() == "HELLO"
assert upper() == "HELLO"

# --- dict (shared state — mutation propagates) ---
d = {"a": 1, "b": 2}
assert getattr(d, "get")("a") == 1
assert getattr(d, "get")("missing", -1) == -1
assert hasattr(d, "keys")
assert hasattr(d, "pop")
assert not hasattr(d, "append")

# Dict mutation via bound method works because dicts are Rc-shared
popper = getattr(d, "pop")
popper("a")
assert "a" not in d

# --- tuple ---
t = (1, 2, 3, 2)
assert getattr(t, "count")(2) == 2
assert getattr(t, "index")(3) == 2
assert hasattr(t, "count")
assert not hasattr(t, "append")

# --- set (read-only methods) ---
ss = {1, 2, 3}
assert getattr(ss, "copy")() == {1, 2, 3}
assert getattr(ss, "isdisjoint")({4, 5}) == True
assert hasattr(ss, "add")
assert not hasattr(ss, "append")

# --- AttributeError on missing attribute ---
try:
    _ = getattr([], "foobar")
    print("FAIL: expected AttributeError")
except AttributeError:
    pass

# --- getattr with default ---
assert getattr([], "foobar", "default") == "default"
assert getattr("hi", "missing", None) is None

# --- bound-method dispatch hot path (issue #276) ---
# Stored bound method invoked many times in a loop. Exercises the
# `pyrust_builtins::bound_method::as_bound_method` arm of
# `call_function_expanded` repeatedly so any regression in the recent
# refactor (drop of `name.to_string()`) would surface here.
total = 0
counter = getattr("ababab", "count")
for _ in range(50):
    total += counter("a")
assert total == 150

# Same shape for dict.get, exercising the dict arm.
d = {"k": 7}
getter = getattr(d, "get")
acc = 0
for _ in range(50):
    acc += getter("k")
assert acc == 350

print("getattr/hasattr builtins OK")
