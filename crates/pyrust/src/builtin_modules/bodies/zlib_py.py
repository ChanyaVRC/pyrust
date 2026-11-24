"""Python 3.12 ``zlib`` module wrapper backed by pure-Rust RFC 1950/1951/1952
primitives in ``zlib.rs`` (issue #3186).
"""

MAX_WBITS = 15
DEFLATED = 8
DEF_MEM_LEVEL = 8
DEF_BUF_SIZE = 16384

Z_NO_COMPRESSION = 0
Z_BEST_SPEED = 1
Z_BEST_COMPRESSION = 9
Z_DEFAULT_COMPRESSION = -1

Z_DEFAULT_STRATEGY = 0
Z_FILTERED = 1
Z_HUFFMAN_ONLY = 2
Z_RLE = 3
Z_FIXED = 4

Z_NO_FLUSH = 0
Z_PARTIAL_FLUSH = 1
Z_SYNC_FLUSH = 2
Z_FULL_FLUSH = 3
Z_FINISH = 4
Z_BLOCK = 5
Z_TREES = 6

ZLIB_VERSION = "1.3.1"
ZLIB_RUNTIME_VERSION = "1.3.1"


class error(Exception):
    pass


def crc32(data, value=0, /):
    """Compute a CRC-32 checksum of data."""
    return _crc32(data, value)


def adler32(data, value=1, /):
    """Compute an Adler-32 checksum of data."""
    return _adler32(data, value)


def compress(data, /, level=Z_DEFAULT_COMPRESSION, wbits=MAX_WBITS):
    """Returns a bytes object containing compressed data."""
    return _compress(error, data, level, wbits)


def decompress(data, /, wbits=MAX_WBITS, bufsize=DEF_BUF_SIZE):
    """Returns a bytes object containing the uncompressed data."""
    return _decompress(error, data, wbits, bufsize)


class _Compress:
    """Streaming compressor returned by ``zlib.compressobj()``."""

    def __init__(self, state):
        self._state = state

    def compress(self, data, /):
        return _compressobj_compress(error, self._state, data)

    def flush(self, mode=Z_FINISH, /):
        return _compressobj_flush(error, self._state, mode)

    def copy(self):
        return _Compress(_compressobj_copy(self._state))


class _Decompress:
    """Streaming decompressor returned by ``zlib.decompressobj()``."""

    def __init__(self, state, eof=False, unused_data=b"", unconsumed_tail=b""):
        self._state = state
        self._eof = eof
        self._unused_data = unused_data
        self._unconsumed_tail = unconsumed_tail

    @property
    def eof(self):
        return self._eof

    @property
    def unused_data(self):
        return self._unused_data

    @property
    def unconsumed_tail(self):
        return self._unconsumed_tail

    def decompress(self, data, /, max_length=0):
        out, eof, unused, tail = _decompressobj_decompress(
            error, self._state, data, max_length
        )
        self._eof = eof
        self._unused_data = unused
        self._unconsumed_tail = tail
        return out

    def flush(self, length=DEF_BUF_SIZE, /):
        out, eof, unused, tail = _decompressobj_flush(error, self._state, length)
        self._eof = eof
        self._unused_data = unused
        self._unconsumed_tail = tail
        return out

    def copy(self):
        state, eof, unused, tail = _decompressobj_copy(self._state)
        return _Decompress(state, eof, unused, tail)


def compressobj(
    level=Z_DEFAULT_COMPRESSION,
    method=DEFLATED,
    wbits=MAX_WBITS,
    memLevel=DEF_MEM_LEVEL,
    strategy=Z_DEFAULT_STRATEGY,
    zdict=None,
):
    """Return a compressor object."""
    state = _compressobj_new(level, method, wbits, memLevel, strategy, zdict)
    return _Compress(state)


def decompressobj(wbits=MAX_WBITS, zdict=b""):
    """Return a decompressor object."""
    state = _decompressobj_new(wbits, zdict)
    return _Decompress(state)
