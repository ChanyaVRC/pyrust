# Parity fixture for __init_subclass__ through built-in exception class MROs
# (issue #1378).
#
# CPython invariant: every class (including built-in exception classes) has
# `object` as the terminal of its MRO, so `object.__init_subclass__` must be
# reachable via `super().__init_subclass__(**kwargs)` from any hook.

import builtins

# --- hasattr via inherited object attr ---
for exc_name in ["BaseException", "Exception", "ValueError", "TypeError",
                 "OSError", "RuntimeError", "AttributeError"]:
    cls = getattr(builtins, exc_name)
    print(hasattr(cls, "__init_subclass__"))   # True for all

# object itself
print(hasattr(object, "__init_subclass__"))    # True

# User class with no explicit base also inherits from object
class A:
    pass
print(hasattr(A, "__init_subclass__"))         # True

# --- super().__init_subclass__(**kwargs) chains up through exception base ---
class MyBase(Exception):
    def __init_subclass__(cls, **kw):
        super().__init_subclass__(**kw)
        print(f"MyBase.subclass: {cls.__name__}")

class MyChild(MyBase): pass      # MyBase.subclass: MyChild

# --- chained hooks through a user + exception hierarchy ---
class Mid(MyBase):
    def __init_subclass__(cls, **kw):
        super().__init_subclass__(**kw)
        print(f"Mid.subclass: {cls.__name__}")

class Leaf(Mid): pass            # MyBase.subclass: Leaf, Mid.subclass: Leaf

# --- ValueError hierarchy ---
class MyValBase(ValueError):
    def __init_subclass__(cls, **kw):
        super().__init_subclass__(**kw)
        print(f"MyValBase.subclass: {cls.__name__}")

class MyValChild(MyValBase): pass   # MyValBase.subclass: MyValChild

# --- object.__init_subclass__ rejects unexpected keyword args ---
try:
    object.__init_subclass__(bogus=1)
except TypeError:
    print("ok: TypeError for unexpected kwargs")

# --- object.__init_subclass__ accepts no args ---
object.__init_subclass__()
print("ok: zero-arg call to object.__init_subclass__")
