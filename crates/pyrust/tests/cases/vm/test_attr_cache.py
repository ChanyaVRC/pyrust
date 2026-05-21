# Parity fixture for the GetAttr / CallMethod inline attr cache (#347).
#
# Tests the following correctness properties:
#   1. Basic method call hits the cache and returns the correct result.
#   2. Different instances of the same class get correct per-instance bindings
#      (guards against the stale-receiver bug).
#   3. Instance attrs that shadow class attrs bypass the cache (correctness).
#   4. Monkey-patching a class invalidates the cache (mutation_version bump).
#   5. Inheriting from a base class: cache entries keyed on the leaf class
#      pointer do not bleed between subclasses.
#   6. Plain (non-method) class attributes are cached and returned correctly.
#   7. Static methods are returned unbound (no receiver prepended).
#   8. Class methods receive the class, not the instance, as the first arg.


# ── 1. Basic method call in a hot loop ──────────────────────────────────────

class Foo:
    def bar(self):
        return 42

f = Foo()
s = 0
for i in range(10_000):
    s += f.bar()
print("loop-result", s)          # 420000


# ── 2. Different instances, same class — correct per-instance binding ────────

class Counter:
    def __init__(self, n):
        self.n = n
    def get(self):
        return self.n

a = Counter(1)
b = Counter(2)
c = Counter(3)

# Force the cache to prime on `a`, then verify `b` and `c` are correct.
results = []
for obj in [a, b, c, a, b]:
    results.append(obj.get())
print("per-instance", results)   # [1, 2, 3, 1, 2]


# ── 3. Instance attr shadow bypasses cache ───────────────────────────────────

class Shadow:
    def value(self):
        return "class"

s1 = Shadow()
s2 = Shadow()
s2.value = lambda: "instance"   # shadow with an instance attr

# Prime the cache on s1 (class method).
print("shadow-class", s1.value())       # class
# s2 has an instance attr — must not return the cached class method.
print("shadow-instance", s2.value())   # instance
# s1 is still correct after s2's shadow was accessed.
print("shadow-class-again", s1.value()) # class


# ── 4. Monkey-patching the class invalidates the cache ───────────────────────

class Patchable:
    def greet(self):
        return "hello"

p = Patchable()
print("pre-patch", p.greet())       # hello

# Prime the cache, then mutate the class.
for _ in range(5):
    p.greet()

Patchable.greet = lambda self: "world"
print("post-patch", p.greet())      # world

# One more call confirms the patched method stays cached.
print("post-patch-2", p.greet())    # world


# ── 5. Subclass cache isolation ──────────────────────────────────────────────

class Base:
    def name(self):
        return "base"

class Child(Base):
    def name(self):
        return "child"

base_obj = Base()
child_obj = Child()

# Alternate between the two so both primes the cache at the same call site.
for _ in range(3):
    print("base", base_obj.name())   # base
    print("child", child_obj.name()) # child


# ── 6. Plain class attribute (non-callable) ──────────────────────────────────

class Config:
    version = "1.0"

cfg = Config()
# GetAttr on a non-method class attr — must return the attr value, not bind it.
for _ in range(3):
    print("version", cfg.version)   # 1.0

# Reassign on the class — must invalidate.
Config.version = "2.0"
print("version-patched", cfg.version)  # 2.0


# ── 7. Static method ─────────────────────────────────────────────────────────

class Utils:
    @staticmethod
    def add(x, y):
        return x + y

u = Utils()
# GetAttr on a static method — must NOT prepend the instance as receiver.
for _ in range(3):
    print("static", u.add(3, 4))   # 7


# ── 8. Class method ──────────────────────────────────────────────────────────

class Registered:
    count = 0
    @classmethod
    def bump(cls):
        cls.count += 1
        return cls.count

r = Registered()
print("classmethod-1", r.bump())   # 1
print("classmethod-2", r.bump())   # 2
print("classmethod-3", r.bump())   # 3


# ── 9. CallMethod path: method called via the fused opcode ──────────────────

class MathObj:
    def __init__(self, v):
        self.v = v
    def mul(self, factor):
        return self.v * factor

objs = [MathObj(i) for i in range(1, 6)]
results = [o.mul(10) for o in objs]
print("callmethod", results)   # [10, 20, 30, 40, 50]


# ── 10. Base-class monkey-patch invalidation ─────────────────────────────────
# A method resolved from a base class is cached on the derived-class call site.
# Patching the base class must invalidate the cache so the next call returns
# the new implementation.  This verifies that Base.mutation_version is bumped
# by SetAttr and that the version check in the fast path catches the change.

class AnimalBase:
    def speak(self):
        return "base"

class Dog(AnimalBase):
    pass

dog = Dog()
for _ in range(3):
    print("pre-patch speak:", dog.speak())   # base

AnimalBase.speak = lambda self: "patched"
print("post-patch speak:", dog.speak())      # patched
