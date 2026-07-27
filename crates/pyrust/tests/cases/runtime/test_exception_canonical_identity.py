# Built-in exception protocol slots are selected by canonical class identity,
# never by the mutable/reusable Python-visible class name.

BuiltinBaseException = BaseException
BuiltinException = Exception
BuiltinBaseExceptionGroup = BaseExceptionGroup
BuiltinExceptionGroup = ExceptionGroup
BuiltinKeyError = KeyError
BuiltinOSError = OSError
BuiltinSyntaxError = SyntaxError
BuiltinSystemExit = SystemExit


class Exception:
    pass


ordinary = Exception()
print("ordinary repr:", repr(ordinary).startswith("<__main__.Exception object at "))
try:
    raise ordinary
except TypeError as error:
    print("ordinary raise:", error)


class SyntaxError(BuiltinException):
    pass


fake_syntax = SyntaxError("plain")
print(
    "fake syntax:",
    fake_syntax.args,
    hasattr(fake_syntax, "filename"),
    str(fake_syntax),
)


class OSError(BuiltinException):
    pass


fake_os = OSError(2, "plain")
print(
    "fake os:",
    fake_os.__class__ is OSError,
    hasattr(fake_os, "errno"),
    str(fake_os),
)


class SystemExit(BuiltinException):
    pass


fake_exit = SystemExit(4)
print("fake exit:", hasattr(fake_exit, "code"), str(fake_exit))
fake_exit.code = "ordinary-code"
print("fake exit vars:", vars(fake_exit))


class AttributeError(BuiltinException):
    pass


fake_attr = AttributeError("plain")
print(
    "fake attr:",
    hasattr(fake_attr, "name"),
    hasattr(fake_attr, "obj"),
)


class UnicodeDecodeError(BuiltinException):
    pass


fake_unicode = UnicodeDecodeError("plain")
print(
    "fake unicode:",
    fake_unicode.args,
    hasattr(fake_unicode, "encoding"),
)


class KeyError(BuiltinException):
    pass


fake_key = KeyError("plain")
print("fake key:", str(fake_key))


# A genuine subclass keeps its inherited protocol even if the leaf's visible
# name no longer resembles the built-in.
class RenamedSyntax(BuiltinSyntaxError):
    pass


RenamedSyntax.__name__ = "NotSyntaxByName"
real_syntax = RenamedSyntax("bad", ("demo.py", 3, 2, "x"))
print(
    "renamed real:",
    real_syntax.filename,
    real_syntax.lineno,
    str(real_syntax),
)


# Interpreter-dispatched exception rendering must use the canonical family.
class Payload:
    def __str__(self):
        return "payload-str"

    def __repr__(self):
        return "payload-repr"


print(
    "fake render:",
    str(KeyError(Payload())),
    str(OSError(Payload())),
    str(SyntaxError(Payload())),
)


class RenamedKey(BuiltinKeyError):
    pass


RenamedKey.__name__ = "NotKeyByName"
print("renamed key:", str(RenamedKey(Payload())))


# Structured descriptors belong to real exception families, not same-named
# ordinary classes. Exercise both `del obj.attr` and object.__delattr__ paths.
fake_syntax.filename = "fake.py"
print("fake syntax vars:", vars(fake_syntax))
del fake_syntax.filename
print("fake syntax del:", hasattr(fake_syntax, "filename"))
fake_syntax.filename = "fake-again.py"
object.__delattr__(fake_syntax, "filename")
print("fake syntax object del:", hasattr(fake_syntax, "filename"))

del real_syntax.filename
print("real syntax del:", hasattr(real_syntax, "filename"), real_syntax.filename)
real_syntax.filename = "real-again.py"
object.__delattr__(real_syntax, "filename")
print(
    "real syntax object del:",
    hasattr(real_syntax, "filename"),
    real_syntax.filename,
)


class RenamedExit(BuiltinSystemExit):
    pass


# Use the genuine SystemExit family for its hidden native `code` slot, even
# when the leaf's visible name is unrelated.
RenamedExit.__name__ = "NotSystemExitByName"
real_exit = RenamedExit(9)
real_exit.marker = "visible"
print("renamed exit vars:", vars(real_exit))


class BaseException:
    pass


fake_base = BaseException()
fake_base.args = ("ordinary",)
fake_base.note = 1
print("fake BaseException vars:", vars(fake_base))
del fake_base.args
print("fake BaseException del:", hasattr(fake_base, "args"))
fake_base.args = ("ordinary-again",)
object.__delattr__(fake_base, "args")
print("fake BaseException object del:", hasattr(fake_base, "args"))
try:
    BaseException(1)
except TypeError as error:
    print("fake BaseException args:", error)


class RenamedException(BuiltinException):
    pass


RenamedException.__name__ = "NotExceptionByName"
real_exception = RenamedException("one", "two")
print("renamed exception args:", real_exception.args)
try:
    del real_exception.args
except TypeError as error:
    print("renamed exception del:", error)
try:
    object.__delattr__(real_exception, "args")
except TypeError as error:
    print("renamed exception object del:", error)


# A BaseException-only subclass merely named Exception is not an Exception.
# ExceptionGroup must reject it, while BaseExceptionGroup must not auto-promote.
class Exception(BuiltinBaseException):
    pass


base_only = Exception("base-only")
try:
    BuiltinExceptionGroup("invalid", [base_only])
except TypeError as error:
    print("fake Exception leaf rejected:", error)
base_group = BuiltinBaseExceptionGroup("base", [base_only])
print(
    "fake Exception no promotion:",
    type(base_group) is BuiltinBaseExceptionGroup,
)


# A renamed real ExceptionGroup subclass keeps group validation.
class RenamedGroup(BuiltinExceptionGroup):
    pass


RenamedGroup.__name__ = "NotGroupByName"
renamed_group = RenamedGroup("renamed", [ValueError("value")])
print("renamed group:", type(renamed_group) is RenamedGroup)
try:
    RenamedGroup("invalid", [KeyboardInterrupt()])
except TypeError as error:
    print("renamed group rejects base:", error)


# A regular Exception merely named BaseExceptionGroup remains a leaf when a
# real exception group recursively splits its children.
class BaseExceptionGroup(BuiltinException):
    pass


fake_group = BaseExceptionGroup("leaf")
outer_group = BuiltinExceptionGroup("outer", [fake_group])
matched, rest = outer_group.split(BaseExceptionGroup)
print(
    "fake group remains leaf:",
    matched.exceptions[0] is fake_group,
    rest is None,
)
