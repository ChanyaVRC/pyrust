"""Native exception fields keep their CPython setter/deleter contracts."""


def report(label, operation):
    try:
        value = operation()
    except Exception as error:
        print(label, type(error).__name__, str(error))
    else:
        print(label, "ok", value)


# BaseException.args preserves an exact tuple but materializes every other
# iterable as a new exact tuple.
error = Exception()
exact = (1, 2)
error.args = exact
print("args-exact", error.args is exact)
source_list = [3, 4]
error.args = source_list
print("args-list", type(error.args).__name__, error.args, error.args is source_list)
report("args-noniterable", lambda: setattr(error, "args", 1))


class Index:
    calls = 0

    def __index__(self):
        type(self).calls += 1
        return 7


# characters_written is an optional index-protocol field. Its absent getter
# raises AttributeError; a successful deletion makes it absent again.
blocking = BlockingIOError(1, "blocked")
report("characters-absent", lambda: blocking.characters_written)
blocking.characters_written = Index()
print(
    "characters-index",
    blocking.characters_written,
    type(blocking.characters_written).__name__,
    Index.calls,
)
del blocking.characters_written
report("characters-after-delete", lambda: blocking.characters_written)
report(
    "characters-delete-again",
    lambda: object.__delattr__(blocking, "characters_written"),
)
report(
    "characters-overflow",
    lambda: setattr(blocking, "characters_written", 1 << 100),
)


class ConstructorIndex:
    calls = 0

    def __index__(self):
        type(self).calls += 1
        return 9


# Exact BlockingIOError (including an exact OSError remapped by errno) gives
# its third argument a constructor-only character-count meaning. The original
# object remains in args while the native field stores one normalized int.
constructor_index = ConstructorIndex()
constructed = BlockingIOError(
    1,
    "blocked",
    constructor_index,
    "ignored filename",
    "ignored filename2",
)
print(
    "constructor-index",
    constructed.characters_written,
    type(constructed.characters_written).__name__,
    ConstructorIndex.calls,
    len(constructed.args),
    constructed.args[2] is constructor_index,
    constructed.filename,
    constructed.filename2,
)

constructor_filename = object()
constructed = BlockingIOError(1, "blocked", constructor_filename)
print(
    "constructor-filename",
    constructed.filename is constructor_filename,
    len(constructed.args),
    hasattr(constructed, "characters_written"),
)

constructed = BlockingIOError(1, "blocked", None, 4, 5)
print(
    "constructor-none",
    len(constructed.args),
    constructed.args[2] is None,
    constructed.filename is None,
    constructed.filename2 is None,
    hasattr(constructed, "characters_written"),
)


class BlockingSubclass(BlockingIOError):
    pass


constructed_subclass = BlockingSubclass(1, "blocked", 3)
print(
    "constructor-subclass",
    len(constructed_subclass.args),
    constructed_subclass.filename,
    hasattr(constructed_subclass, "characters_written"),
)


class RemappedIndex:
    calls = 0

    def __index__(self):
        type(self).calls += 1
        return 10


remapped_index = RemappedIndex()
remapped = OSError(11, "blocked", remapped_index)
print(
    "constructor-remap",
    type(remapped).__name__,
    remapped.characters_written,
    RemappedIndex.calls,
    len(remapped.args),
    remapped.args[2] is remapped_index,
    remapped.filename,
    remapped.filename2,
)


class RemapIntSubclass(int):
    index_calls = 0

    def __index__(self):
        type(self).index_calls += 1
        return 2


subclass_errno = RemapIntSubclass(11)
subclass_remapped = OSError(subclass_errno, "blocked", 12)
print(
    "errno-int-subclass",
    type(subclass_remapped).__name__,
    subclass_remapped.errno is subclass_errno,
    subclass_remapped.characters_written,
    RemapIntSubclass.index_calls,
)

bool_remapped = OSError(True, "blocked", 3)
print(
    "errno-bool",
    type(bool_remapped).__name__,
    bool_remapped.errno is True,
    bool_remapped.filename,
)


class ErrnoIndexOnly:
    calls = 0

    def __index__(self):
        type(self).calls += 1
        return 11


index_only_errno = ErrnoIndexOnly()
index_only_error = OSError(index_only_errno, "blocked", 3)
print(
    "errno-index-only",
    type(index_only_error).__name__,
    index_only_error.errno is index_only_errno,
    index_only_error.filename,
    ErrnoIndexOnly.calls,
    hasattr(index_only_error, "characters_written"),
)

huge_errno = 1 << 100
huge_errno_error = OSError(huge_errno, "blocked", 3)
print(
    "errno-huge",
    type(huge_errno_error).__name__,
    huge_errno_error.errno is huge_errno,
    huge_errno_error.filename,
)

report(
    "constructor-float",
    lambda: BlockingIOError(1, "blocked", 1.5),
)


class HugeConstructorIndex:
    def __index__(self):
        return 1 << 100


report(
    "constructor-overflow",
    lambda: BlockingIOError(1, "blocked", HugeConstructorIndex()),
)


class IntSubclass(int):
    pass


# UnicodeError.start/end require a real integer and do not invoke an arbitrary
# __index__ provider.
unicode_error = UnicodeDecodeError("utf-8", b"x", 0, 1, "bad")
before = Index.calls
report("unicode-index-provider", lambda: setattr(unicode_error, "start", Index()))
print("unicode-index-calls", Index.calls - before)
unicode_error.start = IntSubclass(5)
print("unicode-int-subclass", unicode_error.start, type(unicode_error.start).__name__)
report("unicode-overflow", lambda: setattr(unicode_error, "end", 1 << 100))
report("unicode-delete", lambda: object.__delattr__(unicode_error, "start"))


# Exception-group structural fields are read-only native members.
group = ExceptionGroup("group", [ValueError("leaf")])
for field in ("message", "exceptions"):
    report(
        "group-set-" + field,
        lambda field=field: setattr(group, field, "replacement"),
    )
    report(
        "group-del-" + field,
        lambda field=field: object.__delattr__(group, field),
    )


# winerror is a native OSError member only on Windows.
import os

if os.name != "nt":
    os_error = OSError(2, "missing")
    report("posix-winerror-get", lambda: os_error.winerror)
    report(
        "posix-winerror-delete",
        lambda: object.__delattr__(os_error, "winerror"),
    )
