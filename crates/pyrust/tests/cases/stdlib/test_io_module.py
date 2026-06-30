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
