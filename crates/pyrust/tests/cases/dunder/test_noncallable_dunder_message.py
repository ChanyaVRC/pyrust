# Issue #1963: when an implicit-protocol dunder is looked up and the attribute
# EXISTS but is NOT callable (e.g. `__len__ = 5`), CPython raises
# `TypeError: '<valuetype>' object is not callable` keyed on the resolved
# value's type — not a custom "class attribute is not callable" message.


def show(label, fn):
    try:
        fn()
        print(label, "NO ERROR")
    except TypeError as e:
        print(label, str(e))


# len / iter / dir / next / contains / getitem / repr / str / bool / hash / call
class Len:
    __len__ = 5


show("len", lambda: len(Len()))


class Iter:
    __iter__ = 5


show("iter", lambda: iter(Iter()))


class ForLoop:
    __iter__ = 5


def run_for():
    for _ in ForLoop():
        pass


show("for", run_for)


class Dir:
    __dir__ = 5


show("dir", lambda: dir(Dir()))


class Nx:
    __iter__ = lambda self: self
    __next__ = 5


show("next", lambda: next(iter(Nx())))


class Contains:
    __contains__ = 5


show("contains", lambda: 0 in Contains())


class GetItem:
    __getitem__ = 5


show("getitem", lambda: GetItem()[0])


class Repr:
    __repr__ = 5


show("repr", lambda: repr(Repr()))


class Str:
    __str__ = 5


show("str", lambda: str(Str()))


class Bool:
    __bool__ = 5


show("bool", lambda: bool(Bool()))


class Hash:
    __hash__ = 5


show("hash", lambda: hash(Hash()))


class Call:
    __call__ = 5


show("call", lambda: Call()())


# The reported type tracks the resolved value, not the owning class.
class LenStr:
    __len__ = "x"


show("len-str", lambda: len(LenStr()))


class LenList:
    __len__ = [1]


show("len-list", lambda: len(LenList()))


class Plain:
    pass


class LenInstance:
    __len__ = Plain()


show("len-instance", lambda: len(LenInstance()))


# Regression: callable dunders still work, missing dunders keep their own
# (correct) message.
class Good:
    def __len__(self):
        return 3


print("good-len", len(Good()))


class NoLen:
    pass


show("missing-len", lambda: len(NoLen()))
