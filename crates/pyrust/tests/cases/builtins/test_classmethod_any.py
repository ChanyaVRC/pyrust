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
