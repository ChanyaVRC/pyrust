# mappingproxy.keys()/values()/items() return live dict_keys/dict_values/
# dict_items views, not snapshot lists (issue #2751).

# --- dict-backed proxy: type identity ---
d = {"a": 1, "b": 2, "c": 3}
mp = d.keys().mapping
print(type(mp.keys()).__name__)
print(type(mp.values()).__name__)
print(type(mp.items()).__name__)

# --- liveness: the view reflects later mutation of the backing dict ---
kv = mp.keys()
d["c2"] = 4
print(list(kv))
print(len(kv))
print("c2" in kv)

vv = mp.values()
print(sorted(vv))

iv = mp.items()
print(("c2", 4) in iv)

# --- mutation guard while iterating values ---
dg = {"a": 1, "b": 2, "c": 3}
mpg = dg.keys().mapping
it = iter(mpg.values())
next(it)
dg["q"] = 9
try:
    next(it)
    print("no error")
except RuntimeError as e:
    print(e)

# --- mutation guard while iterating items ---
dh = {"a": 1, "b": 2, "c": 3}
mph = dh.keys().mapping
it2 = iter(mph.items())
next(it2)
dh["r"] = 9
try:
    next(it2)
    print("no error")
except RuntimeError as e:
    print(e)

# --- mutation guard while iterating keys ---
dk = {"a": 1, "b": 2, "c": 3}
mpk = dk.keys().mapping
it3 = iter(mpk.keys())
next(it3)
dk["s"] = 9
try:
    next(it3)
    print("no error")
except RuntimeError as e:
    print(e)

# --- class-backed proxy: type identity and view membership ---
class C:
    x = 1
    y = 2


v = vars(C)
print(type(v.keys()).__name__)
print(type(v.values()).__name__)
print(type(v.items()).__name__)
print("x" in v.keys())
print(2 in v.values())
print(("y", 2) in v.items())

# --- methods take no positional arguments ---
for name in ("keys", "values", "items", "copy"):
    try:
        getattr(mp, name)(1)
        print("no error")
    except TypeError as e:
        print(e)
