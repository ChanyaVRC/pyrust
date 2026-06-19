# `obj.__dict__ = d` must store `d` by reference (issue #1981): the assigned
# dict becomes the instance's live backing store, so identity holds, mutations
# alias both ways, and non-str keys round-trip.


class W:
    pass


# --- Identity: __dict__ assignment stores the dict itself ---
w = W()
d = {"m": 7}
w.__dict__ = d
print(w.__dict__ is d)  # True
print(w.__dict__ is w.__dict__)  # True (same object every read)


# --- Aliasing: mutate d -> visible as attribute, and vice-versa ---
d["n"] = 8
print(w.n)  # 8
w.x = 5
print(d["x"])  # 5
print(sorted(d.items()))  # [('m', 7), ('n', 8), ('x', 5)]

# delete via attribute removes from the live dict; identity preserved
del w.m
print("m" in d)  # False
print(w.__dict__ is d)  # True


# --- Non-str keys: stored, shown, not attribute-accessible ---
w2 = W()
w2.__dict__ = {1: 2}
print(w2.__dict__)  # {1: 2}
w2.__dict__[3.5] = "f"
print(sorted(w2.__dict__.items(), key=repr))  # [(1, 2), (3.5, 'f')]


# --- vars(obj) is the same live dict ---
w3 = W()
e = {"p": 1}
w3.__dict__ = e
print(vars(w3) is e)  # True
e["q"] = 2
print(sorted(k for k in dir(w3) if not k.startswith("_")))  # ['p', 'q']


# --- Two instances can share one backing dict ---
shared = {"k": 1}
a = W()
b = W()
a.__dict__ = shared
b.__dict__ = shared
a.k2 = 2
print(b.k2)  # 2
print(a.__dict__ is b.__dict__)  # True


# --- Replacing with a non-dict raises TypeError ---
try:
    W().__dict__ = [1, 2]
except TypeError as ex:
    print("TypeError:", ex)


# --- __slots__ with '__dict__': slot values survive a __dict__ replacement ---
class S:
    __slots__ = ("x", "__dict__")


s = S()
s.x = 10
slot_dict = {"y": 20}
s.__dict__ = slot_dict
print(s.__dict__ is slot_dict)  # True
print(s.x)  # 10 (slot independent of __dict__)
print(s.y)  # 20
slot_dict["z"] = 30
print(s.z)  # 30 (aliased)
s.x = 11
print(s.x, "z" in s.__dict__)  # 11 True (slot write does not touch __dict__)


# --- Normal instances unaffected (no __dict__ replacement) ---
class N:
    def __init__(self):
        self.a = 1


n = N()
n.b = 2
print(sorted(n.__dict__.items()))  # [('a', 1), ('b', 2)]
n.__dict__["c"] = 3
print(n.c)  # 3 (proxy write-back still works for unreplaced __dict__)
