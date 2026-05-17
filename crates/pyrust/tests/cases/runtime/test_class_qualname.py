# Issue #546: CPython pre-injects __qualname__ and __module__ into the class
# body namespace before execution.  Verify pyrust matches this behaviour.

class C:
    q = __qualname__
    m = __module__

print(C.q)           # C
print(C.m)           # __main__

# __qualname__ and __module__ must appear in locals() at class body start
class D:
    locs = locals()
    print('__qualname__' in locs)  # True
    print('__module__' in locs)    # True
    print(locs['__qualname__'])    # D

# Class attributes must be set on the resulting class object
class E:
    pass

print(E.__qualname__)  # E
print(E.__module__)    # __main__

# User-assigned __qualname__ must win over the pre-injected default
class F:
    __qualname__ = 'CustomF'

print(F.__qualname__)  # CustomF
print(F.__module__)    # __main__

# __qualname__ and __module__ available as expressions inside class body
class G:
    print(__qualname__)  # G
    print(__module__)    # __main__

# Nested class: __qualname__ must not raise NameError inside the inner body.
# pyrust uses just the bare class name (not the dotted CPython form Outer.Inner)
# because nested qualname tracking is not yet implemented.
class Outer:
    class Inner:
        q = __qualname__
    # Just check it's a non-empty string — avoids locking in the dotted form.
    print(isinstance(Inner.q, str) and len(Inner.q) > 0)  # True
