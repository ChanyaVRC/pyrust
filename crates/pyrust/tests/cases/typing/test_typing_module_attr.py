import typing

# Real-class markers: __module__ == "typing", reprs as <class 'typing.Name'>.
print(typing.Generic.__module__)
print(repr(typing.Generic))
print(typing.Protocol.__module__)
print(repr(typing.Protocol))

# Special-form markers: __module__ == "typing", reprs as typing.Name.
print(typing.ClassVar.__module__)
print(repr(typing.ClassVar))
print(typing.Final.__module__)
print(repr(typing.Final))
print(typing.Literal.__module__)
print(repr(typing.Literal))
print(typing.Callable.__module__)
print(repr(typing.Callable))

# NamedTuple is a real-class marker in pyrust; its __module__ is "typing"
# (repr differs from CPython's function form, so only the module is asserted).
print(typing.NamedTuple.__module__)


# A user subclass keeps its own module / repr, not "typing".
class Stack(typing.Generic[typing.TypeVar("T")]):
    pass


print(Stack.__module__)
