# Parity fixture for __init_subclass__ (issue #1047).
# CPython 3.12 calls __init_subclass__ on the parent class after the new
# subclass is created.  The hook receives the new subclass as `cls`.

# --- Basic: hook fires at class creation ---
class Base:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"created: {cls.__name__}")

class Child(Base):
    pass

class GrandChild(Child):
    pass

# --- Registration pattern ---
class Registry:
    _subs = []
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        Registry._subs.append(cls.__name__)

class A(Registry):
    pass

class B(Registry):
    pass

print(Registry._subs)

# --- cls is the new subclass, not the base ---
class Checker:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        print(cls is not Checker)

class Sub(Checker):
    pass

# --- No error when base has no __init_subclass__ ---
class Plain:
    pass

class PlainChild(Plain):
    pass

print("plain ok")

# --- No error for top-level class (no base) ---
class Standalone:
    pass

print("standalone ok")

# --- super() chaining across multiple levels ---
log = []

class Root:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(("Root", cls.__name__))

class Mid(Root):
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(("Mid", cls.__name__))

class Leaf(Mid):
    pass

class DeepLeaf(Leaf):
    pass

print(log)
