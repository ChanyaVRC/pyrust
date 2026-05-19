# Parity fixture for default __repr__ of user-defined class instances.
# Issue #566: repr() should emit <module.qualname object at 0xADDR>.
# The exact hex address differs per run / interpreter, so we test the format
# pattern and the consistency properties rather than the byte value.


class Foo:
    pass


f = Foo()
r = repr(f)

# Must start with the module-qualified class name.
print(r.startswith("<__main__.Foo object at 0x"))  # True
print(r.endswith(">"))                              # True

# Two distinct instances must have different repr (different addresses).
f2 = Foo()
print(repr(f) != repr(f2))                          # True

# repr() on the same object is stable within a run.
print(repr(f) == repr(f))                           # True

# Custom __repr__ is not disturbed.
class Bar:
    def __repr__(self):
        return "custom repr for Bar"

print(repr(Bar()))                                   # custom repr for Bar

# __module__ set explicitly in the class body.
class WithModule:
    __module__ = "mypackage.sub"

wm = WithModule()
r2 = repr(wm)
print(r2.startswith("<mypackage.sub.WithModule object at 0x"))  # True
print(r2.endswith(">"))                              # True

# str() of an instance with neither __str__ nor __repr__ uses the same format.
class Plain:
    pass

s = str(Plain())
print(s.startswith("<__main__.Plain object at 0x"))  # True

# Address part must consist of hex digits (CPython uses %p which is uppercase
# on MSVC Windows; normalise before checking so the test is platform-neutral).
hex_part = r.split("0x")[1].rstrip(">")
print(all(c in "0123456789abcdef" for c in hex_part.lower()) and len(hex_part) > 0)  # True

# Built-in type instances are unaffected.
print(repr(42))                                      # 42
print(repr("hi"))                                    # 'hi'
