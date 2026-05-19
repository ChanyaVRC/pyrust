# Parity fixture for issue #506: StrKey fast-path avoids PyKey::Str allocation
# when probing an IndexMap<PyKey, Value> with a string key.
#
# These cases cover the three sites wired to dict_str_lookup:
#   1. dict[key] subscript GetItem (VM DictLookup arm + eval_index fallback)
#   2. key in dict membership test (eval_in)
#   3. Error paths on missing keys

# ── Basic subscript lookup ──────────────────────────────────────────────────

d = {"hello": 1, "world": 2, 42: 3, True: 4}

print(d["hello"])          # 1
print(d["world"])          # 2
print(d[42])               # 3   (non-string key still works)
print(d[True])             # 4

try:
    print(d["missing"])
except KeyError as e:
    print(f"KeyError: {e}")

# ── Membership test (in operator) ──────────────────────────────────────────

print("hello" in d)        # True
print("world" in d)        # True
print("absent" in d)       # False
print(42 in d)             # True
print("missing" in d)      # False

# ── Dict with only string keys ───────────────────────────────────────────

cfg = {"host": "localhost", "port": "8080", "debug": "true"}
print(cfg["host"])         # localhost
print("port" in cfg)       # True
print("timeout" in cfg)    # False

# ── Mixed-type keys: string lookup must not collide with int keys ─────────

m = {1: "int-one", "1": "str-one", 1.0: "float-one"}
# CPython: 1 == 1.0 == True for dict purposes; "1" is distinct
print(m[1])                # int-one  (or float-one: same bucket)
print(m["1"])              # str-one  (different from the int/float key)
print("1" in m)            # True
print(1.0 in m)            # True

# ── Empty dict edge cases ────────────────────────────────────────────────

empty = {}
print("x" in empty)       # False
try:
    _ = empty["x"]
except KeyError as e:
    print(f"KeyError: {e}")

# ── Nested dict lookup ───────────────────────────────────────────────────

nested = {"outer": {"inner": 99}}
print(nested["outer"]["inner"])    # 99
print("outer" in nested)           # True
print("inner" in nested)           # False

# ── Class __dict__ is separate from Value::Dict ──────────────────────────
# (attr lookup uses IndexMap<String, Value>, not IndexMap<PyKey, Value>)

class C:
    x = 10
    y = 20

c = C()
print(c.x)    # 10
print(c.y)    # 20
d2 = vars(c.__class__)
print("x" in d2)   # True (vars returns a dict-like snapshot)
