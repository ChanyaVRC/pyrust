# Exact built-in classes use the ordinary Call opcode.  A warm call-site cache
# must remain observationally invisible when the same PC later sees unrelated
# callable representations, including same-named user classes and subclasses.


def identity(value):
    return value


def warm_exact_classes():
    for _ in range(4):
        z = zip((), ())
        m = map(identity, ())
        f = filter(None, ())
        e = enumerate(())
        s = slice(0)
        r = reversed(())
    return (
        list(z),
        list(m),
        list(f),
        list(e),
        (s.start, s.stop, s.step),
        list(r),
    )


print("exact-six", warm_exact_classes())


def user_two(left, right):
    return ("user", left, right)


class SameNameZip:
    def __init__(self, left, right):
        self.value = ("same-name", left, right)


SameNameZip.__name__ = "zip"


class ZipSubclass(zip):
    pass


class CustomMeta(type):
    def __call__(cls, left, right):
        return ("custom-meta", left, right)


class MetaTarget(metaclass=CustomMeta):
    pass


def call_two_at_one_site(callable_value, left, right):
    return callable_value(left, right)


zip_alias = zip
same_pc_results = []
for callable_value, left, right in [
    (zip, (), ()),
    (zip_alias, (), ()),
    (user_two, 1, 2),
    (SameNameZip, 3, 4),
    (ZipSubclass, (), ()),
    (MetaTarget, 5, 6),
    (zip, (), ()),
]:
    result = call_two_at_one_site(callable_value, left, right)
    if type(result) is tuple:
        same_pc_results.append(result)
    elif type(result) is SameNameZip:
        same_pc_results.append(result.value)
    else:
        same_pc_results.append((type(result) is ZipSubclass, list(result)))

print("same-pc", same_pc_results)


def error_type(thunk):
    try:
        thunk()
        return "ok"
    except Exception as error:
        return type(error).__name__


# Wrong positional arity must miss any vectorcall entry and retain the normal
# constructor error path.
print(
    "wrong-arity",
    error_type(lambda: map(identity)),
    error_type(lambda: filter(None)),
    error_type(lambda: enumerate()),
    error_type(lambda: slice()),
    error_type(lambda: reversed()),
)

# Keyword and splat opcodes deliberately remain outside the positional cache.
print("zip-keyword", list(zip([1], [2], strict=True)))
print("enumerate-keyword", list(enumerate(["x"], start=4)))
print("slice-keyword", error_type(lambda: slice(stop=4)))
print("zip-splat", list(zip(*([1, 2], [3, 4]))))
print("map-splat", list(map(*(identity, [5, 6]))))

# strict zip validates lazily, after yielding the common prefix.
strict_zip = zip([1, 2], [3], strict=True)
print("strict-first", next(strict_zip))
print("strict-tail", error_type(lambda: next(strict_zip)))


index_calls = []


class StartIndex:
    def __index__(self):
        index_calls.append("index")
        return 4


print("enumerate-index", list(enumerate(["x", "y"], StartIndex())), index_calls)


class ReverseProtocol:
    def __reversed__(self):
        return iter(["protocol", "result"])


class SequenceProtocol:
    def __len__(self):
        return 3

    def __getitem__(self, index):
        if index < 0 or index >= 3:
            raise IndexError
        return index + 10


print("reversed-special", list(reversed(ReverseProtocol())))
print("reversed-sequence", list(reversed(SequenceProtocol())))


# A primitive-cache miss must continue through the three pre-existing special
# class adapters, even when the same PC was warmed by a positive primitive
# identity immediately beforehand.
import types
import typing


def call_one_at_one_site(callable_value, argument):
    return callable_value(argument)


call_one_at_one_site(int, 0)
proxy = call_one_at_one_site(types.MappingProxyType, {"answer": 42})
alias = call_two_at_one_site(types.GenericAlias, list, int)
pair_type = call_two_at_one_site(typing.NamedTuple, "CachePair", [("value", int)])
pair = pair_type(7)
print(
    "primitive-miss-tail",
    proxy["answer"],
    alias.__origin__ is list,
    alias.__args__ == (int,),
    pair.value,
    pair._fields,
)
