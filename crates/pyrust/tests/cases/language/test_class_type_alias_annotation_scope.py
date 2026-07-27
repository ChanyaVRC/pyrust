# PEP 695 class-body aliases own a lazy annotation scope. Ordinary names first
# consult the live class namespace, then fall back to type parameters, enclosing
# scopes, globals, and builtins. Methods defined in the same class do not share
# that lookup rule.

events = []


def mark(value):
    events.append(value)
    return value


class Later:
    x = 1
    type Alias = mark(x)
    x = 2


print("lazy", events)
Later.x = 3
print("live", Later.Alias.__value__, events)
Later.x = 4
print("cached", Later.Alias.__value__, events)


class During:
    x = 5
    type Alias = x
    x = 6
    observed = Alias.__value__


print("during", During.observed)

x = 10


class Deleted:
    x = 11
    type Alias = x


del Deleted.x
print("delete-fallback", Deleted.Alias.__value__)


class GenericShadow:
    T = "class"
    type Alias[T] = T


print(
    "class-shadows-type-param",
    GenericShadow.Alias.__value__,
)


class GenericFallback:
    type Alias[T] = T


print(
    "type-param-fallback",
    GenericFallback.Alias.__value__ is GenericFallback.Alias.__type_params__[0],
)


class SelfRef:
    type Alias = Alias


print("self", SelfRef.Alias.__value__ is SelfRef.Alias)


class ClassRef:
    type Alias = __class__


print("class", ClassRef.Alias.__value__ is ClassRef)

method_name = "global"


class MethodScope:
    method_name = "class"

    def read(self):
        return method_name


print("method", MethodScope().read())


def make_nested():
    captured = 30

    class Nested:
        type Alias = captured

    captured = 31
    return Nested


Nested = make_nested()
print("enclosing-cell", Nested.Alias.__value__)


def extract_alias():
    class Temporary:
        x = 32
        type Alias = x

    return Temporary.Alias


Extracted = extract_alias()
print("extracted-owner", Extracted.__value__)


class Retry:
    type Alias = missing


try:
    Retry.Alias.__value__
except NameError:
    print("retry-first", "NameError")
Retry.missing = 40
print("retry-second", Retry.Alias.__value__)


class Meta(type):
    def __new__(meta, name, bases, namespace):
        namespace["x"] = 21
        namespace["seen"] = namespace["Alias"].__value__
        return super().__new__(meta, name, bases, namespace)


class Prepared(metaclass=Meta):
    x = 20
    type Alias = x


print("prepared", Prepared.seen)
