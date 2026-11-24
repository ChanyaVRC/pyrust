"""CPython 3.12 parity tests for dataclasses hardening (issue #3155)."""

from collections import OrderedDict, namedtuple
from dataclasses import (
    InitVar,
    KW_ONLY,
    asdict,
    astuple,
    dataclass,
    field,
    fields,
    is_dataclass,
    make_dataclass,
    replace,
)
from types import MappingProxyType
import typing
from typing import ClassVar


# ── 1. InitVar ────────────────────────────────────────────────────────────────
@dataclass
class Scaled:
    value: int
    factor: InitVar[int] = 2
    offset: InitVar[int] = 0

    def __post_init__(self, factor, offset):
        self.value = self.value * factor + offset


s1 = Scaled(5)
s2 = Scaled(5, 3, 1)
print("InitVar:", s1, s2, s1 == Scaled(10, 1, 0))
print("InitVar fields:", [f.name for f in fields(Scaled)])
print("InitVar asdict/astuple:", asdict(s2), astuple(s2))
print("InitVar in __dict__:", "factor" in s2.__dict__, "offset" in s2.__dict__)
print("InitVar match_args:", Scaled.__match_args__)
print("InitVar repr:", repr(InitVar[int]), InitVar[int].type is int)


@dataclass
class WithReqInitVar:
    x: int
    delta: InitVar[int]

    def __post_init__(self, delta):
        self.x += delta


r1 = WithReqInitVar(10, 5)
print("req InitVar hasattr:", hasattr(r1, "delta"))
print("replace with InitVar:", replace(r1, delta=2))
try:
    replace(r1, x=20)
    print("replace missing InitVar NO ERROR")
except ValueError:
    print("replace missing InitVar ValueError")

try:
    @dataclass
    class BadInitVar:
        x: InitVar[int] = field(default_factory=int)
    print("InitVar default_factory NO ERROR")
except TypeError:
    print("InitVar default_factory TypeError")


@dataclass(slots=True)
class SlottedInitVar:
    x: int
    bump: InitVar[int] = 1

    def __post_init__(self, bump):
        self.x += bump


print("SlottedInitVar:", SlottedInitVar(4, 3), SlottedInitVar.__slots__)


# ── 2. ClassVar exclusion ─────────────────────────────────────────────────────
@dataclass
class Config:
    kind: ClassVar[str] = "cfg"
    bare: ClassVar = 123
    qualified: typing.ClassVar[int] = 456
    name: str = "default"
    count: int = 1


cfg = Config("app", 7)
print("ClassVar instance:", cfg, Config.kind, Config.bare, Config.qualified)
print("ClassVar fields:", [f.name for f in fields(Config)])
print("ClassVar asdict/astuple:", asdict(cfg), astuple(cfg))
print("ClassVar match_args:", Config.__match_args__)
print("ClassVar eq:", Config("app", 7) == Config("app", 7))


@dataclass
class SubConfig(Config):
    count: ClassVar[int] = 99


print("SubConfig fields:", [f.name for f in fields(SubConfig)], SubConfig.count)


# ── 3. field(metadata=...) ────────────────────────────────────────────────────
@dataclass
class Measured:
    length: float = field(default=1.0, metadata={"unit": "m", "precision": 2})
    plain: int = 0


m_fields = fields(Measured)
print("metadata type:", type(m_fields[0].metadata) is MappingProxyType)
print("metadata items:", sorted(m_fields[0].metadata.items()))
print("default metadata:", type(m_fields[1].metadata) is MappingProxyType, len(m_fields[1].metadata))
try:
    m_fields[0].metadata["unit"] = "cm"
    print("metadata mutate NO ERROR")
except TypeError:
    print("metadata read-only TypeError")

try:
    field(metadata=42)
    print("bad metadata NO ERROR")
except TypeError:
    print("bad metadata TypeError")


# ── 4. make_dataclass() ───────────────────────────────────────────────────────
Pt = make_dataclass(
    "Pt",
    [
        "x",
        ("y", int),
        ("z", int, field(default=5, metadata={"axis": "z"})),
    ],
    order=True,
    frozen=True,
    namespace={"norm2": lambda self: self.x * self.x + self.y * self.y + self.z * self.z},
)
pt1 = Pt(1, 2)
pt2 = Pt(1, 2, 6)
print("make_dataclass:", pt1, pt1.norm2(), pt1 < pt2, Pt.__module__)
print("make_dataclass fields:", [(f.name, dict(f.metadata)) for f in fields(Pt)])
print("make_dataclass is_dataclass:", is_dataclass(Pt), is_dataclass(pt1))

SubPt = make_dataclass("SubPt", [("w", int, 0)], bases=(Pt,), frozen=True, module="custom.mod")
print("SubPt:", SubPt(1, 2, 3, 4), SubPt.__module__)

SlottedMD = make_dataclass("SlottedMD", [("a", int)], slots=True, weakref_slot=True)
print("SlottedMD:", SlottedMD(7), SlottedMD.__slots__)

for label, bad_fields in [
    ("bad-ident", [("1x", int)]),
    ("keyword", [("class", int)]),
    ("dup", [("a", int), ("a", str)]),
    ("bad-tuple", [("a",)]),
]:
    try:
        make_dataclass("Bad", bad_fields)
        print(label, "NO ERROR")
    except TypeError:
        print(label, "TypeError")


# ── 5. asdict(dict_factory=...) & astuple(tuple_factory=...) ──────────────────
Pair = namedtuple("Pair", ["left", "right"])


@dataclass
class Leaf:
    a: int
    b: list = field(default_factory=lambda: [1, 2])


@dataclass
class Tree:
    leaf: Leaf
    pair: Pair
    items: list


tree = Tree(Leaf(10), Pair(Leaf(20), 30), [Leaf(40)])

od = asdict(tree, dict_factory=OrderedDict)
print("asdict OrderedDict:", type(od) is OrderedDict, type(od["leaf"]) is OrderedDict)
print("asdict namedtuple:", type(od["pair"]) is Pair, type(od["pair"].left) is OrderedDict)
print("asdict list item:", type(od["items"][0]) is OrderedDict, od["items"][0]["a"])

# Deep-copy isolation
od["leaf"]["b"].append(99)
print("asdict deepcopy isolated:", tree.leaf.b)

as_list = astuple(tree, tuple_factory=list)
print("astuple list:", type(as_list) is list, type(as_list[0]) is list, as_list)
print("astuple namedtuple:", type(as_list[1]) is Pair, type(as_list[1].left) is list)

print("dataclasses hardening ok")
