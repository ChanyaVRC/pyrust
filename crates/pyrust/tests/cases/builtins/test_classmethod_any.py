# classmethod() and staticmethod() accept any object as their argument,
# not just UserFunction values.  Regression test for issue #1318.

# Construction with a non-callable must not raise TypeError.
classmethod(42)
staticmethod(42)
print("construction ok")

# __func__ returns the wrapped value.
cm = classmethod(42)
sm = staticmethod(42)
print(cm.__func__)
print(sm.__func__)
print(cm == cm, hash(cm) == hash(cm))
print(sm == sm, hash(sm) == hash(sm))

# isinstance() recognises both wrapper kinds.
print(isinstance(classmethod(42), classmethod))
print(isinstance(staticmethod(42), staticmethod))

# Class-level descriptor: staticmethod(non_fn) returns the wrapped value
# when accessed as a class or instance attribute.
class C:
    s = staticmethod(42)

print(C.s)
print(C().s)

# Classmethod wrapping a UserFunction: __get__ must return a bound method
# that prepends cls when called.
class D:
    m = classmethod(lambda cls: cls.__name__)

print(D.m())
print(D().m())

# Builtin functions also receive the class. This exercises the generic
# descriptor marker used by runtime-owned builtin classmethods.
class E:
    m = classmethod(isinstance)


print(E.m(type))
print(E().m(type))
e = E()
for _ in range(3):
    bound = e.m
    print(bound(type))
print(type(E.m).__name__, E.m.__func__ is isinstance, E.m.__self__ is E)
print(type(E().m).__name__, E().m.__func__ is isinstance, E().m.__self__ is E)

# Arbitrary callable objects use the same generic bound-method representation.
class F:
    t = classmethod(type)
    noncallable = classmethod(42)


print(F.t() is type)
print(F().t() is type)
print(
    type(F.noncallable).__name__,
    F.noncallable.__func__,
    F.noncallable.__self__ is F,
)

# Explicit __get__ on a UserFunction classmethod descriptor.
cm2 = classmethod(lambda cls: cls.__name__ + " via get")
print(cm2.__get__(None, D)())

# Explicit __get__ on a UserFunction staticmethod descriptor.
sm2 = staticmethod(lambda: "static result")
print(sm2.__get__(None, D)())

# Non-function wrapped in classmethod: __func__ still accessible.
cm3 = classmethod("not a function")
print(cm3.__func__)

# staticmethod(None): __func__ returns None.
sm3 = staticmethod(None)
print(sm3.__func__)
