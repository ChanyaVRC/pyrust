import io

# Seek-whence constants.
print(io.SEEK_SET)
print(io.SEEK_CUR)
print(io.SEEK_END)
print(io.DEFAULT_BUFFER_SIZE)
print(callable(io.open))
print(io.open is open)

# seek using the constants.
bio = io.BytesIO(b"hello world")
bio.seek(6, io.SEEK_SET)
print(bio.read(5))
bio.seek(0, io.SEEK_SET)
bio.seek(0, io.SEEK_END)
print(bio.tell())
bio.seek(-5, io.SEEK_CUR)
print(bio.read())

# ABC classes exist.
print(hasattr(io, "IOBase"))
print(hasattr(io, "RawIOBase"))
print(hasattr(io, "BufferedIOBase"))
print(hasattr(io, "TextIOBase"))
print(io.IOBase.__module__)

# Hierarchy: each ABC derives from IOBase.
print(issubclass(io.RawIOBase, io.IOBase))
print(issubclass(io.BufferedIOBase, io.IOBase))
print(issubclass(io.TextIOBase, io.IOBase))

# Concrete classes are wired into the hierarchy.
sio = io.StringIO("hello")
bio2 = io.BytesIO(b"hello")
print(isinstance(sio, io.TextIOBase))
print(isinstance(bio2, io.BufferedIOBase))
print(isinstance(sio, io.IOBase))
print(isinstance(bio2, io.IOBase))

# Negative cases: no cross-wiring.
print(isinstance(sio, io.BufferedIOBase))
print(isinstance(bio2, io.TextIOBase))
print(issubclass(io.StringIO, io.BufferedIOBase))
print(issubclass(io.BytesIO, io.TextIOBase))

print("io module ok")

# --- Native ABC family details (issue #3342) ---------------------------------

# Exact types of the constants.
print([type(v).__name__ for v in (io.SEEK_SET, io.SEEK_CUR, io.SEEK_END, io.DEFAULT_BUFFER_SIZE)])

# Names, modules, bases and MRO of every ABC and concrete stream.  CPython's
# hidden C-level `_io._*Base` helper classes are not part of the public
# surface, so only the public names are compared.
def public_names(classes):
    return [c.__name__ for c in classes if not c.__name__.startswith('_') and c is not object]

for cls in (io.IOBase, io.RawIOBase, io.BufferedIOBase, io.TextIOBase):
    print(cls.__name__, cls.__qualname__, cls.__module__, public_names(cls.__bases__))
    print(public_names(cls.__mro__))
    print(repr(cls))
print(io.StringIO.__module__, io.BytesIO.__module__)

# Docstrings of the ABCs.
print(io.RawIOBase.__doc__)
print(io.TextIOBase.__doc__)
print(io.BufferedIOBase.__doc__)
print(io.IOBase.__doc__)

# Cross-hierarchy issubclass / isinstance checks.
print(issubclass(io.IOBase, io.RawIOBase), issubclass(io.RawIOBase, io.TextIOBase))
print(issubclass(io.TextIOBase, io.BufferedIOBase), issubclass(io.BufferedIOBase, io.RawIOBase))
print(isinstance(io.StringIO(), io.RawIOBase), isinstance(io.BytesIO(), io.RawIOBase))
print(issubclass(io.StringIO, io.IOBase), issubclass(io.BytesIO, io.IOBase))

# The ABCs can be subclassed and the subclasses join the hierarchy.
class MyText(io.TextIOBase):
    pass

class MyBuf(io.BufferedIOBase):
    pass

class MyRaw(io.RawIOBase):
    pass

print(isinstance(MyText(), io.TextIOBase), isinstance(MyText(), io.IOBase))
print(isinstance(MyBuf(), io.BufferedIOBase), isinstance(MyBuf(), io.IOBase))
print(isinstance(MyRaw(), io.RawIOBase), isinstance(MyRaw(), io.IOBase))
print(isinstance(MyText(), io.BufferedIOBase), isinstance(MyRaw(), io.TextIOBase))

# Subclassing the concrete streams keeps the ABC hierarchy.
class MyStringIO(io.StringIO):
    pass

class MyBytesIO(io.BytesIO):
    pass

print(isinstance(MyStringIO("x"), io.TextIOBase), isinstance(MyBytesIO(b"x"), io.BufferedIOBase))
print(MyStringIO("abc").read(), MyBytesIO(b"abc").read())

# `io.open` is the built-in `open`; the module exposes the documented names.
import builtins
print(io.open is builtins.open)
for name in ("IOBase", "RawIOBase", "BufferedIOBase", "TextIOBase", "StringIO", "BytesIO",
             "UnsupportedOperation", "SEEK_SET", "SEEK_CUR", "SEEK_END",
             "DEFAULT_BUFFER_SIZE", "open"):
    print(name, hasattr(io, name))
print(io.UnsupportedOperation.__module__, io.UnsupportedOperation.__name__)
print(issubclass(io.UnsupportedOperation, OSError), issubclass(io.UnsupportedOperation, ValueError))

# Importing again returns the same objects.
import io as io2
print(io2 is io, io2.IOBase is io.IOBase, io2.TextIOBase is io.TextIOBase)
print("io native ABC ok")
