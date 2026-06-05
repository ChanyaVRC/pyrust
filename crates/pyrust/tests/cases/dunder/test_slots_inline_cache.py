# Issue #2207: __slots__ slot reads use a dedicated InstanceAttr-style inline
# cache (SlotAttr) instead of routing through the full member_descriptor
# data-descriptor dispatch on every read.  The cache must preserve every
# unset-slot / del / __class__-swap / __getattr__ semantic exactly.


class S:
    __slots__ = ("x", "y")


s = S()

# Unset slot read -> AttributeError (the cache must NOT serve a stale value).
try:
    s.x
except AttributeError as e:
    print(e)  # 'S' object has no attribute 'x'

s.x = 10
# Warm the inline cache: many repeated reads of the same site.
total = 0
for _ in range(1000):
    total += s.x
print(total)  # 10000
print(s.x)  # 10
s.x = 20
print(s.x)  # 20

# del then read: cache must fall through to AttributeError, not return 20.
del s.x
try:
    s.x
except AttributeError as e:
    print(e)  # 'S' object has no attribute 'x'
s.x = 5
print(s.x)  # 5

# Reading a second, still-unset slot while the first is cached.
try:
    s.y
except AttributeError as e:
    print(e)  # 'S' object has no attribute 'y'
s.y = 7
print(s.y)  # 7

# The class attribute is still a member_descriptor (cache must not shadow it).
print(type(S.x).__name__)  # member_descriptor
print("x" in S.__dict__)  # True


# __getattr__ fallback for an unset slot must still fire through the cache miss.
class G:
    __slots__ = ("z",)

    def __getattr__(self, name):
        return "fallback-" + name


g = G()
for _ in range(50):
    print(g.z)  # fallback-z (repeated, exercising the miss path under warming)
    break
print(g.z)  # fallback-z
g.z = 3
acc = 0
for _ in range(1000):
    acc += g.z
print(acc)  # 3000
del g.z
print(g.z)  # fallback-z


# Inheritance: a subclass slot read also hits the cache; base + own slots both
# resolve to the right storage.
class Sub(S):
    __slots__ = ("w",)


t = Sub()
t.x = 1
t.w = 2
sub_total = 0
for _ in range(1000):
    sub_total += t.x + t.w
print(sub_total)  # 3000
try:
    Sub().w
except AttributeError as e:
    print(e)  # 'Sub' object has no attribute 'w'


# __class__ reassignment between two slotted classes invalidates the cache
# (class pointer guard) and reads the new class's identically-named slot.
class A:
    __slots__ = ("v",)


class B:
    __slots__ = ("v",)


a = A()
a.v = 100
for _ in range(20):
    a.v  # warm cache on A
a.__class__ = B
print(a.v)  # 100
print(type(a).__name__)  # B
