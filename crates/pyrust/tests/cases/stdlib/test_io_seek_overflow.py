import io
import sys


def report(label, stream, operation):
    try:
        operation()
    except Exception as error:
        print(
            label,
            type(error).__name__,
            str(error),
            stream.tell(),
            repr(stream.getvalue()),
        )


bytes_write = io.BytesIO()
bytes_write.seek(sys.maxsize)
report("bytes-write", bytes_write, lambda: bytes_write.write(b"x"))

bytes_seek = io.BytesIO()
bytes_seek.seek(sys.maxsize)
report("bytes-seek", bytes_seek, lambda: bytes_seek.seek(1, 1))

bytes_end_seek = io.BytesIO(b"x")
report("bytes-end-seek", bytes_end_seek, lambda: bytes_end_seek.seek(sys.maxsize, 2))

string_write = io.StringIO()
string_write.seek(sys.maxsize)
report("string-write", string_write, lambda: string_write.write("x"))

# Empty writes beyond EOF are a no-op and must not attempt to materialize the
# sparse gap.
bytes_empty = io.BytesIO(b"a")
bytes_empty.seek(sys.maxsize)
print(
    "bytes-empty",
    bytes_empty.write(b""),
    bytes_empty.tell(),
    repr(bytes_empty.getvalue()),
)

string_empty = io.StringIO("a")
string_empty.seek(sys.maxsize)
print(
    "string-empty",
    string_empty.write(""),
    string_empty.tell(),
    repr(string_empty.getvalue()),
)

# Reads and truncation at a sparse position keep the cursor sparse; they must
# neither clamp it to EOF nor materialize the gap.
bytes_sparse = io.BytesIO(b"a")
bytes_sparse.seek(sys.maxsize)
print(
    "bytes-sparse",
    bytes_sparse.read(),
    bytes_sparse.tell(),
    bytes_sparse.truncate(),
    repr(bytes_sparse.getvalue()),
)

string_sparse = io.StringIO("a")
string_sparse.seek(sys.maxsize)
print(
    "string-sparse",
    repr(string_sparse.read()),
    string_sparse.tell(),
    string_sparse.truncate(),
    repr(string_sparse.getvalue()),
)

# The facade accepts heap-backed Python ints that fit in the native boundary
# and reports CPython's conversion errors when they do not.
bytes_offset_big = io.BytesIO()
report(
    "bytes-offset-big",
    bytes_offset_big,
    lambda: bytes_offset_big.seek(10**100),
)
bytes_whence_big = io.BytesIO()
report(
    "bytes-whence-big",
    bytes_whence_big,
    lambda: bytes_whence_big.seek(0, 10**100),
)
string_truncate_big = io.StringIO()
report(
    "string-truncate-big",
    string_truncate_big,
    lambda: string_truncate_big.truncate(10**100),
)

# Keep the ordinary character-indexed overwrite path covered alongside the
# cold overflow branches.
text = io.StringIO("aéz")
text.seek(1)
print("string-normal", text.write("XY"), text.tell(), repr(text.getvalue()))
