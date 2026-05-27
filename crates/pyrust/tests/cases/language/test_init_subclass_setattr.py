# Parity fixture for __init_subclass__ attribute mutation (issue #1252).
# The hook must be able to set attributes on `cls` without panicking.
# Previously, the class RefCell was still borrowed during the hook call.

# --- Repro: cls.flag = True inside __init_subclass__ ---
class Base:
    def __init_subclass__(cls):
        cls.flag = True

class Sub(Base):
    pass

print(Sub.flag)  # True

# --- Method assignment inside the hook ---
class Base2:
    def __init_subclass__(cls):
        cls.greet = lambda: "hello"

class Sub2(Base2):
    pass

print(Sub2.greet())  # hello

# --- Multiple attribute assignments ---
class Base3:
    def __init_subclass__(cls):
        cls.x = 10
        cls.y = 20

class Sub3(Base3):
    pass

print(Sub3.x, Sub3.y)  # 10 20

# --- Keyword args AND attribute mutation combined ---
class Base4:
    def __init_subclass__(cls, tag=None, **kwargs):
        super().__init_subclass__(**kwargs)
        cls.tag = tag

class Sub4(Base4, tag="v1"):
    pass

print(Sub4.tag)  # v1

# --- Exception from hook propagates as Python exception (not Rust panic) ---
class Base5:
    def __init_subclass__(cls):
        raise ValueError("rejected")

try:
    class Sub5(Base5):
        pass
except ValueError as e:
    print(e)  # rejected

# --- Attribute mutation in a chained hook (super().__init_subclass__) ---
class Root:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        cls.from_root = True

class Mid(Root):
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        cls.from_mid = True

class Leaf(Mid):
    pass

print(Leaf.from_root)  # True
print(Leaf.from_mid)   # True

# --- Regular class creation without __init_subclass__ is unaffected ---
class Plain:
    x = 99

class PlainChild(Plain):
    pass

print(PlainChild.x)  # 99
