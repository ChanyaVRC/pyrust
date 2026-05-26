"""
Parity test for generator introspection attributes (issue #1270).

Verifies __name__, __qualname__, gi_running, gi_yieldfrom, gi_frame, gi_code
are accessible on generator objects without raising AttributeError.
"""


def gen():
    yield 1
    yield 2


# Basic name attributes
g = gen()
print(g.__name__)
print(g.__qualname__)

# gi_running is False when accessed from outside
print(g.gi_running)

# gi_yieldfrom is None when not delegating
print(g.gi_yieldfrom)

# gi_frame and gi_code don't raise AttributeError
# (CPython returns frame/code objects; pyrust returns None — both are valid)
print(g.gi_frame is None or g.gi_frame is not None)
print(g.gi_code is None or g.gi_code is not None)

# After exhaustion, gi_frame must be None in both CPython and pyrust
list(g)
print(g.gi_frame is None)


# Qualname for nested generator
def outer():
    def inner():
        yield 42

    return inner()


g2 = outer()
print(g2.__name__)
print(g2.__qualname__)


# gi_yieldfrom during yield from delegation
def sub_gen():
    yield 10
    yield 20


def delegating():
    yield from sub_gen()


g3 = delegating()
# Before first advance: suspended at YieldFrom with pc pointing at the instruction
# gi_yieldfrom should be None (not yet entered body)
print(g3.gi_yieldfrom is None)

# After first advance: suspended inside yield from, gi_yieldfrom is the sub-iterator
next(g3)
sub = g3.gi_yieldfrom
print(sub is not None)
print(type(sub).__name__)


# dir() includes all expected attributes
attrs = dir(gen())
for attr in ["__name__", "__qualname__", "gi_running", "gi_yieldfrom", "gi_frame", "gi_code"]:
    print(attr, attr in attrs)


# AttributeError for unknown attributes
try:
    gen().nonexistent_attr
except AttributeError as e:
    print("AttributeError raised")


# Qualname for a class method generator
class MyClass:
    def my_gen(self):
        yield 1


g4 = MyClass().my_gen()
print(g4.__name__)
print(g4.__qualname__)
