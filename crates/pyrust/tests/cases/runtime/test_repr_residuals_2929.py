"""Issue #2929: repr residuals — function/method qualname+address,
`del __module__` audit, and stdlib shim `__module__` seeding.
"""

import asyncio
import collections
import contextlib
import csv
import decimal
import fractions
import functools
import io
import itertools
import json
import pathlib
import pprint
import random
import statistics
import textwrap
import time
import types
import typing


def mask(text):
    out = []
    i = 0
    while i < len(text):
        if text.startswith("0x", i):
            i += 2
            while i < len(text) and text[i] in "0123456789abcdefABCDEF":
                i += 1
            out.append("0xADDR")
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


def show(label, fn):
    try:
        print(f"{label}: OK {fn()!r}")
    except BaseException as e:
        print(f"{label}: {type(e).__name__}: {e}")


# ── 1. Function and bound-method repr() ──────────────────────────────────────


def g():
    pass


def g2():
    pass


class H:
    def m(self):
        pass

    @classmethod
    def cm(cls):
        pass

    @staticmethod
    def sm():
        pass

    class Inner:
        def f(self):
            pass


class HSub(H):
    pass


def outer():
    def inner():
        pass

    lam = lambda: None  # noqa: E731
    return inner, lam


inn1, lam1 = outer()
inn2, _ = outer()
top_lam = lambda: None  # noqa: E731
h = H()
hi = H.Inner()
hs = HSub()

print("g:", mask(repr(g)))
print("str(g) == repr(g):", str(g) == repr(g))
print("f'{g}' == repr(g):", f"{g}" == repr(g))
print("same fn stable addr:", repr(g) == repr(g))
print("distinct fn distinct addr:", repr(g) != repr(g2))
print("distinct closures distinct addr:", repr(inn1) != repr(inn2))
print("H.m:", mask(repr(H.m)))
print("H.Inner.f:", mask(repr(H.Inner.f)))
print("inn:", mask(repr(inn1)))
print("top_lam:", mask(repr(top_lam)))
print("lam:", mask(repr(lam1)))
print("H.__dict__['cm']:", mask(repr(H.__dict__["cm"])))
print("H.__dict__['sm']:", mask(repr(H.__dict__["sm"])))

print("h.m:", mask(repr(h.m)))
print("str(h.m):", mask(str(h.m)))
print("f'{h.m}':", mask(f"{h.m}"))
print("hi.f:", mask(repr(hi.f)))
print("hs.m:", mask(repr(hs.m)))
print("H.cm:", repr(H.cm))
print("h.cm:", repr(h.cm))
print("HSub.cm:", repr(HSub.cm))
print("type(h.m) is types.MethodType:", type(h.m) is types.MethodType)
print("type(H.cm) is types.MethodType:", type(H.cm) is types.MethodType)


class CustomRepr:
    def __repr__(self):
        return "CustomRepr!"

    def m(self):
        pass


cr = CustomRepr()
print("cr.m repr:", repr(cr.m))
print("cr.m str:", str(cr.m))
print("cr.m fstring:", f"{cr.m}")
print("cr.m format:", format(cr.m))
print("[cr.m]:", repr([cr.m]))


class RaisingRepr:
    def __repr__(self):
        raise ValueError("repr-boom")

    def m(self):
        pass


show("RaisingRepr().m repr", lambda: repr(RaisingRepr().m))
show("RaisingRepr().m str", lambda: str(RaisingRepr().m))


class RecRepr:
    def __repr__(self):
        return repr(self.m)

    def m(self):
        pass


try:
    repr(RecRepr().m)
except RecursionError:
    print("RecRepr().m: RecursionError")


class MetaRepr(type):
    def __repr__(cls):
        return f"<meta {cls.__name__}>"


class WithMetaCM(metaclass=MetaRepr):
    @classmethod
    def cm(cls):
        pass


print("WithMetaCM.cm:", repr(WithMetaCM.cm))


# Mutating __qualname__ vs __name__
class MutHolder:
    def m(self):
        pass


mh = MutHolder()
MutHolder.m.__qualname__ = "custom.qual"
print("mut fn qualname:", mask(repr(MutHolder.m)))
print("mut bound qualname:", mask(repr(mh.m)))
MutHolder.m.__name__ = "ignored_name"
print("mut fn after __name__:", mask(repr(MutHolder.m)))
print("mut bound after __name__:", mask(repr(mh.m)))


# ── 2. del __module__ / __qualname__ / __name__ / __doc__ audit ──────────────


class DelMod:
    pass


class DelSub(DelMod):
    pass


class Meta(type):
    pass


class WithMeta(metaclass=Meta):
    pass


class MetaWithMod(type):
    __module__ = "meta_mod"


class WithMetaMod(metaclass=MetaWithMod):
    pass


show("del DelMod.__module__", lambda: delattr(DelMod, "__module__"))
DelMod.__module__ = "foo"
show("del DelMod.__module__ after set", lambda: delattr(DelMod, "__module__"))
show("del DelSub.__module__", lambda: delattr(DelSub, "__module__"))
show("del DelMod.__qualname__", lambda: delattr(DelMod, "__qualname__"))
show("del DelMod.__name__", lambda: delattr(DelMod, "__name__"))
show("del DelMod.__doc__", lambda: delattr(DelMod, "__doc__"))

show("del WithMeta.__qualname__", lambda: delattr(WithMeta, "__qualname__"))
show("del WithMeta.__name__", lambda: delattr(WithMeta, "__name__"))
show("del WithMeta.__module__ #1", lambda: delattr(WithMeta, "__module__"))
show("del WithMeta.__module__ #2", lambda: delattr(WithMeta, "__module__"))
print("WithMeta.__module__ after del:", repr(WithMeta.__module__))
print("repr(WithMeta) after del:", repr(WithMeta))
print("repr(WithMeta()) after del:", mask(repr(WithMeta())))
show("del WithMetaMod.__module__", lambda: delattr(WithMetaMod, "__module__"))
print("WithMetaMod.__module__ after del:", repr(WithMetaMod.__module__))

show("del int.__module__", lambda: delattr(int, "__module__"))
show("del object.__module__", lambda: delattr(object, "__module__"))
show("del type.__module__", lambda: delattr(type, "__module__"))
show("del int.__qualname__", lambda: delattr(int, "__qualname__"))
show("del int.__name__", lambda: delattr(int, "__name__"))


def fn_del():
    pass


show("del fn_del.__module__", lambda: delattr(fn_del, "__module__"))
print("fn_del.__module__ after del:", repr(fn_del.__module__))
show("del fn_del.__qualname__", lambda: delattr(fn_del, "__qualname__"))
show("del fn_del.__name__", lambda: delattr(fn_del, "__name__"))

d_inst = DelMod()
show("del d_inst.__module__ unset", lambda: delattr(d_inst, "__module__"))
d_inst.__module__ = "inst_mod"
show("del d_inst.__module__ set", lambda: delattr(d_inst, "__module__"))
print("d_inst.__module__ after del:", repr(d_inst.__module__))


# ── 3. Stdlib shim class __module__ & repr() seeding ─────────────────────────

stdlib_classes = [
    ("csv.Error", csv.Error),
    ("csv.Dialect", csv.Dialect),
    ("csv.excel", csv.excel),
    ("csv.excel_tab", csv.excel_tab),
    ("csv.unix_dialect", csv.unix_dialect),
    ("csv.DictReader", csv.DictReader),
    ("csv.DictWriter", csv.DictWriter),
    ("pprint.PrettyPrinter", pprint.PrettyPrinter),
    ("textwrap.TextWrapper", textwrap.TextWrapper),
    ("time.struct_time", time.struct_time),
    ("io.BytesIO", io.BytesIO),
    ("io.StringIO", io.StringIO),
    ("io.IOBase", io.IOBase),
    ("io.RawIOBase", io.RawIOBase),
    ("io.BufferedIOBase", io.BufferedIOBase),
    ("io.TextIOBase", io.TextIOBase),
    ("io.UnsupportedOperation", io.UnsupportedOperation),
    ("asyncio.CancelledError", asyncio.CancelledError),
    ("asyncio.InvalidStateError", asyncio.InvalidStateError),
    ("asyncio.Future", asyncio.Future),
    ("asyncio.Task", asyncio.Task),
    ("json.JSONDecodeError", json.JSONDecodeError),
    ("collections.Counter", collections.Counter),
    ("collections.OrderedDict", collections.OrderedDict),
    ("collections.ChainMap", collections.ChainMap),
    ("contextlib.ExitStack", contextlib.ExitStack),
    ("decimal.Decimal", decimal.Decimal),
    ("fractions.Fraction", fractions.Fraction),
    ("functools.partial", functools.partial),
    ("itertools.chain", itertools.chain),
    ("pathlib.Path", pathlib.Path),
    ("random.Random", random.Random),
    ("statistics.NormalDist", statistics.NormalDist),
    ("typing.TypeVar", typing.TypeVar),
]

for label, cls in stdlib_classes:
    print(f"stdlib {label}: {cls.__module__!r} {cls.__qualname__!r} {cls!r}")
