import types


def closure_cell():
    value = 1

    def inner():
        return value

    return inner.__closure__[0]


try:
    raise RuntimeError("probe")
except RuntimeError as exc:
    traceback = exc.__traceback__
    frame = traceback.tb_frame


class Slotted:
    __slots__ = ("member",)


alias_cases = (
    ("CodeType", (lambda: None).__code__),
    ("CellType", closure_cell()),
    ("WrapperDescriptorType", object.__init__),
    ("MethodWrapperType", object().__str__),
    ("MethodDescriptorType", str.join),
    ("ClassMethodDescriptorType", dict.__dict__["fromkeys"]),
    ("TracebackType", traceback),
    ("FrameType", frame),
    ("GetSetDescriptorType", int.real),
    ("MemberDescriptorType", Slotted.member),
)

for name, value in alias_cases:
    alias = getattr(types, name, None)
    print(name, alias is type(value))


public_helpers = (
    "new_class",
    "resolve_bases",
    "prepare_class",
    "get_original_bases",
    "DynamicClassAttribute",
)
print([hasattr(types, name) for name in public_helpers])


class Base:
    pass


bases = (Base,)
print(types.resolve_bases(bases) is bases)


class Proxy:
    def __mro_entries__(self, original):
        print(original is proxy_bases)
        return (Base,)


proxy = Proxy()
proxy_bases = (proxy,)
resolved = types.resolve_bases(proxy_bases)
print(resolved == (Base,), resolved is proxy_bases)


class BadProxy:
    def __mro_entries__(self, original):
        return [Base]


try:
    types.resolve_bases((BadProxy(),))
except TypeError as exc:
    print(type(exc).__name__, str(exc))


events = []


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        events.append(("prepare", name, bases, dict(kwargs)))
        return {"prepared": True}

    def __new__(mcls, name, bases, namespace, **kwargs):
        events.append(
            (
                "new",
                name,
                bases,
                namespace.get("prepared"),
                namespace.get("payload"),
                dict(kwargs),
            )
        )
        return super().__new__(mcls, name, bases, namespace)


keywords = {"metaclass": Meta, "flag": 7}
meta, namespace, remaining = types.prepare_class("Prepared", (Base,), keywords)
print(meta is Meta, namespace, remaining, keywords)


def exec_body(namespace):
    namespace["payload"] = 11


Created = types.new_class(
    "Created", (Base,), {"metaclass": Meta, "flag": 9}, exec_body
)
print(Created.__name__, Created.__bases__ == (Base,), Created.payload)
print(events)


class DerivedMeta(Meta):
    pass


class WithDerivedMeta(metaclass=DerivedMeta):
    pass


winner, _, _ = types.prepare_class("Winner", (WithDerivedMeta,), {"metaclass": Meta})
print(winner is DerivedMeta)


class OtherMeta(type):
    pass


class WithOtherMeta(metaclass=OtherMeta):
    pass


try:
    types.prepare_class("Conflict", (WithDerivedMeta, WithOtherMeta))
except TypeError as exc:
    print(type(exc).__name__, str(exc))


class OriginalProxy:
    def __mro_entries__(self, original):
        return (Base,)


original_proxy = OriginalProxy()
Original = types.new_class("Original", (original_proxy,))
print(Original.__bases__ == (Base,))
print(types.get_original_bases(Original) == (original_proxy,))
print(types.get_original_bases(Base) == (object,))

try:
    types.get_original_bases(1)
except TypeError as exc:
    print(type(exc).__name__, str(exc))


class Dynamic:
    def __init__(self):
        self._value = 3

    @types.DynamicClassAttribute
    def value(self):
        return self._value

    @value.setter
    def value(self, new_value):
        self._value = new_value

    @value.deleter
    def value(self):
        del self._value


dynamic = Dynamic()
print(dynamic.value)
dynamic.value = 8
print(dynamic.value)
try:
    Dynamic.value
except AttributeError:
    print("class-attribute-error")
del dynamic.value
print(hasattr(dynamic, "_value"))


from types import (  # noqa: E402
    CellType,
    ClassMethodDescriptorType,
    CodeType,
    DynamicClassAttribute,
    FrameType,
    GetSetDescriptorType,
    MemberDescriptorType,
    MethodDescriptorType,
    MethodWrapperType,
    TracebackType,
    WrapperDescriptorType,
    get_original_bases,
    new_class,
    prepare_class,
    resolve_bases,
)

print(
    CellType is types.CellType,
    ClassMethodDescriptorType is types.ClassMethodDescriptorType,
    CodeType is types.CodeType,
    FrameType is types.FrameType,
    GetSetDescriptorType is types.GetSetDescriptorType,
    MemberDescriptorType is types.MemberDescriptorType,
    MethodDescriptorType is types.MethodDescriptorType,
    MethodWrapperType is types.MethodWrapperType,
    TracebackType is types.TracebackType,
    WrapperDescriptorType is types.WrapperDescriptorType,
    DynamicClassAttribute is types.DynamicClassAttribute,
    get_original_bases is types.get_original_bases,
    new_class is types.new_class,
    prepare_class is types.prepare_class,
    resolve_bases is types.resolve_bases,
)
