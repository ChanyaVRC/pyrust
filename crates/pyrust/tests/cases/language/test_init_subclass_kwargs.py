# Parity fixture for PEP 487 class keyword args forwarded to __init_subclass__
# (issue #1080).  CPython 3.12 forwards all non-metaclass keyword arguments
# in the class header to __init_subclass__ on the direct base.

# --- Basic: single keyword forwarded ---
class Base:
    def __init_subclass__(cls, tag=None, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"tag={tag}")

class Sub(Base, tag="hello"):
    pass

# --- Default used when no kwarg supplied ---
class NoKwarg(Base):
    pass

# --- Multiple keywords ---
class Multi:
    def __init_subclass__(cls, key1=None, key2=None, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"key1={key1} key2={key2}")

class Child(Multi, key1="a", key2="b"):
    pass

# --- Registry pattern (the motivating use-case) ---
class Registrar:
    def __init_subclass__(cls, /, registry=None, **kwargs):
        super().__init_subclass__(**kwargs)
        if registry is not None:
            registry.append(cls.__name__)

items = []

class A(Registrar, registry=items):
    pass

class B(Registrar, registry=items):
    pass

print(items)

# --- Kwarg value can be an expression (not just a literal) ---
x = 99

class WithExpr:
    def __init_subclass__(cls, val=None, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"val={val}")

class Computed(WithExpr, val=x):
    pass

# --- Chained super().__init_subclass__(**kwargs) forwards kwargs up ---
log = []

class Root:
    def __init_subclass__(cls, flavor=None, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(f"Root flavor={flavor}")

class Mid(Root):
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append("Mid")

class Leaf(Mid, flavor="vanilla"):
    pass

print(log)

# --- metaclass= is NOT forwarded to __init_subclass__ ---
# (metaclass is consumed by the class machinery; only other kwargs are forwarded)
class BaseMetaCheck:
    def __init_subclass__(cls, extra=None, **kwargs):
        # metaclass= should not appear in kwargs; only 'extra' should
        print(f"extra={extra}")

class WithExtra(BaseMetaCheck, extra="yes"):
    pass
