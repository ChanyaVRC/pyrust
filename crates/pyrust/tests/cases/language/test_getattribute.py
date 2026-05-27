# Parity fixture for user-defined __getattribute__ dispatch (issue #1254).
# CPython calls type(obj).__getattribute__(obj, name) for every attribute
# access on an instance; pyrust was skipping this and going directly to the
# instance dict / class attr lookup.

# --- Basic interception ---

class Logger:
    def __getattribute__(self, name):
        print("get:" + name)
        return object.__getattribute__(self, name)

obj = Logger()
obj.x = 1
print(obj.x)
# get:x
# 1

# --- Synthetic value (attribute that does not exist on the instance) ---

class Magic:
    def __getattribute__(self, name):
        if name == "magic":
            return 42
        return object.__getattribute__(self, name)

m = Magic()
print(m.magic)
# 42

# --- Class attribute forwarded via object.__getattribute__ ---

class WithClassAttr:
    x = "class_x"
    def __getattribute__(self, name):
        return object.__getattribute__(self, name)

wca = WithClassAttr()
print(wca.x)
# class_x

# --- __getattribute__ raising AttributeError falls through to __getattr__ ---

class Fallback:
    def __getattribute__(self, name):
        if name == "missing":
            raise AttributeError(name)
        return object.__getattribute__(self, name)

    def __getattr__(self, name):
        return "fallback:" + name

f = Fallback()
f.real = 99
print(f.real)
print(f.missing)
# 99
# fallback:missing

# --- Non-AttributeError from __getattribute__ propagates unchanged ---

class Raiser:
    def __getattribute__(self, name):
        raise ValueError("bad:" + name)

r = Raiser()
try:
    _ = r.anything
except ValueError as e:
    print(str(e))
# bad:anything

# --- Inheriting __getattribute__ from a parent class ---

class Base:
    def __getattribute__(self, name):
        return "intercepted:" + name

class Child(Base):
    pass

c = Child()
print(c.foo)
# intercepted:foo

# --- object.__getattribute__ direct call works without infinite recursion ---

class Safe:
    val = 99
    def __getattribute__(self, name):
        return object.__getattribute__(self, name)

s = Safe()
print(s.val)
# 99

# --- object.__getattribute__ called directly on a plain instance ---

class Plain:
    y = 7

p = Plain()
print(object.__getattribute__(p, "y"))
# 7

# --- Classes without a custom __getattribute__ behave as before ---

class Normal:
    z = 5

n = Normal()
n.w = 10
print(n.z)
print(n.w)
# 5
# 10
