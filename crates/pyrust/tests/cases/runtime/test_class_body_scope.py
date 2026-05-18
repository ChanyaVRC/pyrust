# Regression test for issue #384: module-level names must be visible
# inside a class body when used as expressions (constructor calls,
# attribute values, list literals initialised from globals, etc.).
#
# Python's class-body scoping rule (LEGB with the class scope skipped):
# free-variable reads in a class body resolve first in the class namespace,
# then directly in module globals (env chain), then builtins.

# --- Basic case: class body references module-scope name ---------------------

class Desc:
    pass

class W:
    d = Desc()
    name = "hello"
    items = [1, 2, 3]

print(type(W.d).__name__)   # Desc
print(W.name)               # hello
print(W.items)              # [1, 2, 3]

# --- Module-scope name used in arithmetic -----------------------------------

x = 10
y = 20

class Math:
    total = x + y

print(Math.total)           # 30

# --- NameError for genuinely undefined name ---------------------------------

class Safe:
    try:
        val = _no_such_name_xyz_
    except NameError:
        val = "NameError caught"

print(Safe.val)             # NameError caught

# --- Nested class body: inner body sees outer class -------------------------

class Outer:
    class Inner:
        pass
    inner_instance = Inner()

print(type(Outer.inner_instance).__name__)  # Inner

# --- Class defined before the one that references it -----------------------

class Base:
    pass

class Derived(Base):
    sentinel = Base()

print(type(Derived.sentinel).__name__)  # Base
