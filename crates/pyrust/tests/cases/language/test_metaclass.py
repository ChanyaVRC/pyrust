# Metaclass keyword in class header (PEP 3115)
#
# PyRust supports a callable as a metaclass: the class body's namespace is
# captured, then `meta(name, bases_tuple, namespace_dict)` is invoked and
# its return value becomes the class. The 3-arg `type(name, bases, dict)`
# constructor is also implemented so metaclasses can delegate to it.

# --- Function as metaclass that prints construction ---
def Tracker(name, bases, ns):
    print(f"creating {name}")
    return type(name, bases, ns)

class A(metaclass=Tracker):
    x = 1

class B(metaclass=Tracker):
    y = 2

assert A.x == 1
assert B.y == 2
assert A.__name__ == "A"
assert B.__name__ == "B"

# --- Metaclass that injects an attribute via namespace mutation ---
def Inject(name, bases, ns):
    ns["injected"] = 99
    return type(name, bases, ns)

class C(metaclass=Inject):
    pass

assert C.injected == 99

# --- 3-arg type() works directly (without metaclass keyword) ---
D = type("D", (), {"value": 7})
assert D.__name__ == "D"
assert D.value == 7

# --- Metaclass on a class with a base ---
class Base:
    base_attr = "base"

class Sub(Base, metaclass=Tracker):
    sub_attr = "sub"

assert Sub.sub_attr == "sub"
assert Sub.base_attr == "base"

# --- Instance methods survive the metaclass round-trip ---
def Identity(name, bases, ns):
    return type(name, bases, ns)

class WithMethods(metaclass=Identity):
    def greet(self):
        return "hello"

w = WithMethods()
assert w.greet() == "hello"

print("metaclass OK")
