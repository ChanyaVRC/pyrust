# PEP 654: BaseExceptionGroup.subgroup() / split() / derive() (issue #2041)

eg = ExceptionGroup("g", [ValueError(1), TypeError(2), ValueError(3)])

# subgroup with an exception type
print(eg.subgroup(ValueError).exceptions)   # (ValueError(1), ValueError(3))

# subgroup with no match returns None
print(eg.subgroup(KeyError))                 # None

# subgroup with a tuple of types
print(eg.subgroup((ValueError, KeyError)).exceptions)

# subgroup with a predicate
print(eg.subgroup(lambda e: isinstance(e, ValueError)).exceptions)

# split partitions into (match, rest)
m, r = eg.split(TypeError)
print(m.exceptions, r.exceptions)

# split returns a 2-tuple
res = eg.split(ValueError)
print(type(res).__name__, len(res))

# all-match: rest is None; subgroup builds a NEW group (not identity)
allm = ExceptionGroup("g", [ValueError(1), ValueError(2)])
mm, rr = allm.split(ValueError)
print(mm.exceptions, rr)
print(allm.subgroup(ValueError) is allm)     # False

# condition matching the whole group → identity preserved
print(eg.subgroup(Exception) is eg)          # True
m2, r2 = eg.split(Exception)
print(m2 is eg, r2)                          # True None

# .exceptions on a sub-group is always a tuple
print(type(eg.subgroup(ValueError).exceptions).__name__)

# nested structure is preserved
inner = ExceptionGroup("inner", [ValueError(1), TypeError(2)])
outer = ExceptionGroup("outer", [inner, KeyError(3)])
sub = outer.subgroup(ValueError)
print(repr(sub))
print(sub.exceptions[0].exceptions)

# deeply nested
deep = ExceptionGroup("L1", [
    ExceptionGroup("L2", [
        ExceptionGroup("L3", [ValueError(1), TypeError(2)]),
        KeyError(3),
    ]),
    ValueError(4),
])
def show(g, indent=0):
    print(" " * indent + type(g).__name__ + ":" + g.message)
    for e in g.exceptions:
        if isinstance(e, BaseExceptionGroup):
            show(e, indent + 2)
        else:
            print(" " * (indent + 2) + repr(e))
show(deep.subgroup(ValueError))

# matched leaf exceptions keep their identity (and thus traceback/cause/context)
v = ValueError(1)
v.__cause__ = KeyError("cause")
v.__context__ = RuntimeError("ctx")
g2 = ExceptionGroup("g", [v, TypeError(2)])
leaf = g2.subgroup(ValueError).exceptions[0]
print(leaf is v, repr(leaf.__cause__), repr(leaf.__context__))

# group metadata (__cause__, __notes__) copied onto the derived sub-group
g2.__cause__ = KeyError("eg cause")
g2.add_note("note1")
sub2 = g2.subgroup(ValueError)
print(repr(sub2.__cause__), getattr(sub2, "__notes__", None))

# derive() builds a new group with the same message
d = eg.derive([TypeError(9)])
print(type(d).__name__, d.message, d.exceptions)

# a subclass's overridden derive() is used to build sub-groups
class MyEG(ExceptionGroup):
    def derive(self, excs):
        return MyEG(self.message + "!", excs)

me = MyEG("g", [ValueError(1), TypeError(2)])
s = me.subgroup(ValueError)
print(type(s).__name__, s.message, s.exceptions)

# BaseExceptionGroup with a BaseException leaf splits without promotion
beg = BaseExceptionGroup("b", [KeyboardInterrupt(), ValueError(1)])
ki = beg.subgroup(KeyboardInterrupt)
print(type(ki).__name__, ki.exceptions)
ve = beg.subgroup(ValueError)
print(type(ve).__name__, ve.exceptions)

# invalid conditions raise TypeError
for bad in (int, 42):
    try:
        eg.subgroup(bad)
    except TypeError as e:
        print("TypeError:", e)

# wrong argument count
try:
    eg.subgroup()
except TypeError as e:
    print("TypeError:", e)
