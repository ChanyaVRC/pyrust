"""
Python 3.12 ``struct`` module wrapper backed by native primitives in
``struct_mod.rs`` (issue #2807).
"""

__all__ = [
    "calcsize",
    "pack",
    "pack_into",
    "unpack",
    "unpack_from",
    "iter_unpack",
    "Struct",
    "error",
]


class error(Exception):
    pass


def _iter_unpack_gen(fmt, buf, size, total_len):
    offset = 0
    while offset < total_len:
        yield _unpack_from(error, fmt, buf, offset)
        offset += size


def calcsize(format, /):
    return _parse_format(error, format)[1]


def pack(format, /, *v):
    return _pack(error, format, v)


def unpack(format, buffer, /):
    return _unpack(error, format, buffer)


def pack_into(format, buffer, offset, /, *v):
    return _pack_into(error, format, buffer, offset, v)


def unpack_from(format, /, buffer, offset=0):
    return _unpack_from(error, format, buffer, offset)


def iter_unpack(format, buffer, /):
    size, total_len = _check_iter_unpack(error, format, buffer)
    return _iter_unpack_gen(format, buffer, size, total_len)


def _clearcache():
    return None


class Struct:
    def __init__(self, format):
        normalized, size = _parse_format(error, format)
        self._format = normalized
        self._size = size

    @property
    def format(self):
        return self._format

    @property
    def size(self):
        return self._size

    def pack(self, *v):
        return _pack(error, self._format, v)

    def unpack(self, buffer, /):
        return _unpack(error, self._format, buffer)

    def pack_into(self, buffer, offset, *v):
        return _pack_into(error, self._format, buffer, offset, v)

    def unpack_from(self, /, buffer, offset=0):
        return _unpack_from(error, self._format, buffer, offset)

    def iter_unpack(self, buffer, /):
        size, total_len = _check_iter_unpack(error, self._format, buffer)
        return _iter_unpack_gen(self._format, buffer, size, total_len)
