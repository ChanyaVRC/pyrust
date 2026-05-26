# __getattr__ is called as a fallback when normal attribute lookup fails.
# It is NOT called when the attribute is found via instance dict or class MRO.

class Default:
    def __getattr__(self, name):
        return 42

d = Default()
print(d.anything)   # 42 — __getattr__ called (no such attr)
print(d.foo)        # 42 — __getattr__ called

# Normal attrs are found directly, __getattr__ is not called.
d.x = 100
print(d.x)          # 100 — instance dict, __getattr__ not called

# __getattr__ can raise AttributeError — the exception propagates.
class Strict:
    def __getattr__(self, name):
        raise AttributeError(f"no attribute: {name}")

s = Strict()
try:
    _ = s.missing
except AttributeError as e:
    print(e)          # no attribute: missing

# __getattr__ receives the exact name string.
class Echo:
    def __getattr__(self, name):
        return name

e = Echo()
print(e.hello)      # hello
print(e.world)      # world

# Class methods are found in MRO; __getattr__ is not called for them.
class WithMethod:
    def greet(self):
        return "hi"
    def __getattr__(self, name):
        return "fallback"

w = WithMethod()
print(w.greet())    # hi — method found in class, __getattr__ not called
print(w.unknown)    # fallback — __getattr__ called

# __getattr__ inherited from a base class.
class Base:
    def __getattr__(self, name):
        return f"base:{name}"

class Child(Base):
    pass

c = Child()
print(c.missing)    # base:missing — inherited __getattr__
