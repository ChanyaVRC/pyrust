# Issues #1981 / #2163: the instance `__dict__` proxy is a live, fully-writable
# mapping over the instance's attribute store.  Every mutation entry point —
# subscript store, `__setitem__`, `update(mapping, **kwargs)`, `setdefault`,
# `pop`, `popitem`, `clear`, `del` — must reflect on the instance, and reads of
# `o.__dict__` must observe attribute writes (bidirectional aliasing).


class C:
    pass


# Identity: o.__dict__ is o.__dict__ and vars(o) is o.__dict__.
o = C()
print(o.__dict__ is o.__dict__)
o.x = 1
print(vars(o) is o.__dict__)

# Subscript store / read write through both directions.
o.__dict__["a"] = 5
print(o.a, "a" in o.__dict__)
o.b = 7
print(o.__dict__["b"])

# __setitem__ / __getitem__ / __delitem__ / __contains__ / __len__ methods.
o2 = C()
o2.__dict__.__setitem__("y", 9)
print(o2.y, o2.__dict__.__getitem__("y"))
print(o2.__dict__.__contains__("y"), o2.__dict__.__len__())
o2.__dict__.__delitem__("y")
print("y" in o2.__dict__)

# update(mapping), update(**kwargs), and the combined form.
o3 = C()
o3.__dict__.update({"p": 1})
o3.__dict__.update(q=2)
o3.__dict__.update({"r": 3}, s=4)
print(sorted(o3.__dict__), o3.p, o3.q, o3.r, o3.s)

# setdefault returns existing / inserts and writes through.
o4 = C()
print(o4.__dict__.setdefault("k", 10), o4.k)
print(o4.__dict__.setdefault("k", 99))

# pop / popitem.
o5 = C()
o5.a = 1
o5.b = 2
o5.c = 3
print(o5.__dict__.pop("b"), sorted(o5.__dict__))
print(o5.__dict__.popitem(), sorted(o5.__dict__))

# clear wipes all attributes.
o6 = C()
o6.m = 1
o6.n = 2
o6.__dict__.clear()
print(list(o6.__dict__), o6.__dict__)

# get / keys / values / items / copy.
o7 = C()
o7.a = 1
o7.b = 2
print(o7.__dict__.get("a"), o7.__dict__.get("z", "default"))
print(sorted(o7.__dict__.keys()))
print(sorted(o7.__dict__.values()))
print(sorted(o7.__dict__.items()))
d = o7.__dict__.copy()
print(type(d).__name__, sorted(d.items()))
d["c"] = 9  # copy is detached
print("c" in o7.__dict__)

# Error paths matching CPython's slot-wrapper messages.
o8 = C()
try:
    o8.__dict__.__setitem__("a")
except TypeError as e:
    print("setitem:", repr(str(e)))
try:
    o8.__dict__.__getitem__()
except TypeError as e:
    print("getitem:", str(e))
try:
    o8.__dict__.__delitem__()
except TypeError as e:
    print("delitem:", str(e))
try:
    o8.__dict__.popitem()
except KeyError as e:
    print("popitem empty:", e)
try:
    o8.__dict__["missing"]
except KeyError as e:
    print("missing key:", e)
