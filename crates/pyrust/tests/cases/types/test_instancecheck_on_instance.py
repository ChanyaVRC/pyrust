# Issue #2525: isinstance()/issubclass() must consult
# type(arg2).__instancecheck__ / __subclasscheck__ when arg2 is a plain
# instance whose *type* defines the hook, instead of rejecting it with a
# TypeError for "arg 2 must be a type".


class InstMeta:
    def __instancecheck__(self, instance):
        return instance == 42


class SubMeta:
    def __subclasscheck__(self, sub):
        return sub is int


x = InstMeta()
print(isinstance(42, x))
print(isinstance(7, x))

y = SubMeta()
print(issubclass(int, y))
print(issubclass(str, y))


# The hook is looked up on the *type*, so the instance is the receiver and
# the candidate is the second positional: type(x).__instancecheck__(x, obj).
class TaggedMeta:
    def __instancecheck__(self, instance):
        return (self.tag, instance)


tagged = TaggedMeta()
tagged.tag = "T"
print(isinstance("obj", tagged))


# Non-bool return values are coerced through truthiness, like CPython.
class TruthyMeta:
    def __instancecheck__(self, instance):
        return [1]


print(isinstance(0, TruthyMeta()))


class FalsyMeta:
    def __instancecheck__(self, instance):
        return []


print(isinstance(0, FalsyMeta()))


# The hook is resolved along the class MRO, not only the immediate type.
class Base:
    def __instancecheck__(self, instance):
        return True


class Derived(Base):
    pass


print(isinstance("anything", Derived()))


# A plain instance whose type defines no hook still raises the TypeError.
try:
    isinstance(42, object())
except TypeError as e:
    print("isinstance TypeError:", e)

try:
    issubclass(int, object())
except TypeError as e:
    print("issubclass TypeError:", e)


# Ordinary class-based checks are unaffected.
print(isinstance(5, int))
print(isinstance(5, (str, int)))
print(issubclass(bool, int))
