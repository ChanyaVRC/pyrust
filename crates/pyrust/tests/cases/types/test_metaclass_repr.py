# Issue #2771: repr(cls) / str(cls) dispatch through the metaclass's
# __repr__ / __str__ when it overrides them.  type(cls).__repr__(cls) already
# worked via direct call; the builtin repr()/str()/f-string/format paths must
# route the same way.

class Meta(type):
    def __repr__(cls):
        return f"<custom {cls.__name__}>"

    def __str__(cls):
        return f"Meta:{cls.__name__}"


class Foo(metaclass=Meta):
    pass


# Direct slot call (already worked before the fix).
print(type(Foo).__repr__(Foo))  # <custom Foo>

# Builtin repr() / str().
print(repr(Foo))  # <custom Foo>
print(str(Foo))   # Meta:Foo
print(Foo)        # Meta:Foo  (print uses str)

# f-string conversions and bare field.
print(f"{Foo!r}")  # <custom Foo>
print(f"{Foo!s}")  # Meta:Foo
print(f"{Foo}")    # Meta:Foo

# str.format conversions.
print("{!r}".format(Foo))  # <custom Foo>
print("{!s}".format(Foo))  # Meta:Foo
print("{}".format(Foo))    # Meta:Foo

# Inside containers: each element's repr is dispatched.
print([Foo])           # [<custom Foo>]
print((Foo,))          # (<custom Foo>,)
print({Foo})           # {<custom Foo>}
print({Foo: 1})        # {<custom Foo>: 1}

# format(cls, "") == str(cls); a non-empty spec raises naming the metaclass.
print(format(Foo, ""))  # Meta:Foo
try:
    format(Foo, ">20")
except TypeError as e:
    print(e)  # unsupported format string passed to Meta.__format__
try:
    f"{Foo:>20}"
except TypeError as e:
    print(e)  # unsupported format string passed to Meta.__format__


# A metaclass overriding only __repr__: str(cls) falls back to it (CPython's
# type.__str__ delegates to type.__repr__).
class ReprOnly(type):
    def __repr__(cls):
        return f"R({cls.__name__})"


class Bar(metaclass=ReprOnly):
    pass


print(repr(Bar))  # R(Bar)
print(str(Bar))   # R(Bar)
print(f"{Bar}")   # R(Bar)


# Inherited metaclass __repr__.
class SubMeta(Meta):
    pass


class Baz(metaclass=SubMeta):
    pass


print(repr(Baz))  # <custom Baz>
print(str(Baz))   # Meta:Baz


# Plain classes (metaclass is `type`) keep the default formatting.
class Plain:
    pass


print(repr(Plain).startswith("<class "))  # True
print(str(Plain).startswith("<class "))   # True
print([int, str])  # [<class 'int'>, <class 'str'>]


# A metaclass __repr__ that returns a non-string raises TypeError.
class BadMeta(type):
    def __repr__(cls):
        return 42


class Bad(metaclass=BadMeta):
    pass


try:
    repr(Bad)
except TypeError as e:
    print("repr non-string:", e)  # repr non-string: __repr__ returned non-string (type int)


# A metaclass with a non-repr/str override does not affect class repr.
class CallMeta(type):
    def __call__(cls, *a, **k):
        return super().__call__(*a, **k)


class Callable(metaclass=CallMeta):
    pass


print(repr(Callable).startswith("<class "))  # True
# format(cls, spec) names the metaclass in the error even when it does not
# override __repr__/__str__ (CPython's object.__format__ uses type(cls)).
try:
    format(Callable, ">5")
except TypeError as e:
    print(e)  # unsupported format string passed to CallMeta.__format__
try:
    f"{Callable:>5}"
except TypeError as e:
    print(e)  # unsupported format string passed to CallMeta.__format__


# A metaclass __format__ override is invoked by format()/f-string for any spec.
class FmtMeta(type):
    def __format__(cls, spec):
        return f"FMT({cls.__name__}|{spec})"


class Formatted(metaclass=FmtMeta):
    pass


print(format(Formatted, "abc"))  # FMT(Formatted|abc)
print(format(Formatted, ""))     # FMT(Formatted|)
print(f"{Formatted:>10}")        # FMT(Formatted|>10)
print("{}".format(Formatted))    # FMT(Formatted|)
# A bare {cls} field is format(cls, ""), which calls __format__ (not str()).
print(f"{Formatted}")            # FMT(Formatted|)
# str(cls) does NOT call __format__; FmtMeta has no __str__/__repr__ so the
# default class formatting is used.
print(str(Formatted).startswith("<class "))  # True
