"""Python 3.12 ``hashlib`` module backed by native primitives in ``hashlib.rs``."""

__all__ = (
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "blake2b",
    "blake2s",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
    "new",
    "algorithms_guaranteed",
    "algorithms_available",
    "file_digest",
    "pbkdf2_hmac",
)

algorithms_guaranteed = {
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "blake2b",
    "blake2s",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
}

algorithms_available = set(algorithms_guaranteed) | {
    "sha512_224",
    "sha512_256",
}


class _Hash:
    __slots__ = ("_state", "_name", "_digest_size", "_block_size")

    def __init__(self, state, name, digest_size, block_size):
        self._state = state
        self._name = name
        self._digest_size = digest_size
        self._block_size = block_size

    @property
    def name(self):
        return self._name

    @property
    def digest_size(self):
        return self._digest_size

    @property
    def block_size(self):
        return self._block_size

    def update(self, data, /):
        _hash_update(self._state, data)

    def digest(self, /):
        return _hash_digest(self._state)

    def hexdigest(self, /):
        return _hash_hexdigest(self._state)

    def copy(self, /):
        return _Hash(
            _hash_copy(self._state),
            self._name,
            self._digest_size,
            self._block_size,
        )

    def __repr__(self):
        return f"<{self._name} _hashlib.HASH object @ {hex(id(self))}>"


class _ShakeHash:
    __slots__ = ("_state", "_name", "_block_size")

    def __init__(self, state, name, block_size):
        self._state = state
        self._name = name
        self._block_size = block_size

    @property
    def name(self):
        return self._name

    @property
    def digest_size(self):
        return 0

    @property
    def block_size(self):
        return self._block_size

    def update(self, data, /):
        _hash_update(self._state, data)

    def digest(self, length, /):
        return _hash_digest_xof(self._state, length)

    def hexdigest(self, length, /):
        return _hash_hexdigest_xof(self._state, length)

    def copy(self, /):
        return _ShakeHash(
            _hash_copy(self._state),
            self._name,
            self._block_size,
        )

    def __repr__(self):
        return f"<{self._name} _hashlib.HASHXOF object @ {hex(id(self))}>"


class blake2b:
    MAX_DIGEST_SIZE = 64
    MAX_KEY_SIZE = 64
    PERSON_SIZE = 16
    SALT_SIZE = 16

    __slots__ = ("_state", "_digest_size")

    def __init__(
        self,
        data=b"",
        /,
        *,
        digest_size=64,
        key=b"",
        salt=b"",
        person=b"",
        fanout=1,
        depth=1,
        leaf_size=0,
        node_offset=0,
        node_depth=0,
        inner_size=0,
        last_node=False,
        usedforsecurity=True,
    ):
        bool(usedforsecurity)
        self._state = _new_blake2(
            True,
            data,
            digest_size,
            key,
            salt,
            person,
            fanout,
            depth,
            leaf_size,
            node_offset,
            node_depth,
            inner_size,
            last_node,
        )
        self._digest_size = int(digest_size)

    @property
    def name(self):
        return "blake2b"

    @property
    def digest_size(self):
        return self._digest_size

    @property
    def block_size(self):
        return 128

    def update(self, data, /):
        _hash_update(self._state, data)

    def digest(self, /):
        return _hash_digest(self._state)

    def hexdigest(self, /):
        return _hash_hexdigest(self._state)

    def copy(self, /):
        other = blake2b.__new__(blake2b)
        other._state = _hash_copy(self._state)
        other._digest_size = self._digest_size
        return other

    def __repr__(self):
        return f"<blake2b _blake2.blake2b object @ {hex(id(self))}>"


class blake2s:
    MAX_DIGEST_SIZE = 32
    MAX_KEY_SIZE = 32
    PERSON_SIZE = 8
    SALT_SIZE = 8

    __slots__ = ("_state", "_digest_size")

    def __init__(
        self,
        data=b"",
        /,
        *,
        digest_size=32,
        key=b"",
        salt=b"",
        person=b"",
        fanout=1,
        depth=1,
        leaf_size=0,
        node_offset=0,
        node_depth=0,
        inner_size=0,
        last_node=False,
        usedforsecurity=True,
    ):
        bool(usedforsecurity)
        self._state = _new_blake2(
            False,
            data,
            digest_size,
            key,
            salt,
            person,
            fanout,
            depth,
            leaf_size,
            node_offset,
            node_depth,
            inner_size,
            last_node,
        )
        self._digest_size = int(digest_size)

    @property
    def name(self):
        return "blake2s"

    @property
    def digest_size(self):
        return self._digest_size

    @property
    def block_size(self):
        return 64

    def update(self, data, /):
        _hash_update(self._state, data)

    def digest(self, /):
        return _hash_digest(self._state)

    def hexdigest(self, /):
        return _hash_hexdigest(self._state)

    def copy(self, /):
        other = blake2s.__new__(blake2s)
        other._state = _hash_copy(self._state)
        other._digest_size = self._digest_size
        return other

    def __repr__(self):
        return f"<blake2s _blake2.blake2s object @ {hex(id(self))}>"


def _make_hash(name, string, usedforsecurity):
    bool(usedforsecurity)
    state, canonical, digest_size, block_size, is_shake = _new_hash(name, string)
    if is_shake:
        return _ShakeHash(state, canonical, block_size)
    return _Hash(state, canonical, digest_size, block_size)


def md5(string=b"", *, usedforsecurity=True):
    return _make_hash("md5", string, usedforsecurity)


def sha1(string=b"", *, usedforsecurity=True):
    return _make_hash("sha1", string, usedforsecurity)


def sha224(string=b"", *, usedforsecurity=True):
    return _make_hash("sha224", string, usedforsecurity)


def sha256(string=b"", *, usedforsecurity=True):
    return _make_hash("sha256", string, usedforsecurity)


def sha384(string=b"", *, usedforsecurity=True):
    return _make_hash("sha384", string, usedforsecurity)


def sha512(string=b"", *, usedforsecurity=True):
    return _make_hash("sha512", string, usedforsecurity)


def sha3_224(string=b"", *, usedforsecurity=True):
    return _make_hash("sha3_224", string, usedforsecurity)


def sha3_256(string=b"", *, usedforsecurity=True):
    return _make_hash("sha3_256", string, usedforsecurity)


def sha3_384(string=b"", *, usedforsecurity=True):
    return _make_hash("sha3_384", string, usedforsecurity)


def sha3_512(string=b"", *, usedforsecurity=True):
    return _make_hash("sha3_512", string, usedforsecurity)


def shake_128(string=b"", *, usedforsecurity=True):
    return _make_hash("shake_128", string, usedforsecurity)


def shake_256(string=b"", *, usedforsecurity=True):
    return _make_hash("shake_256", string, usedforsecurity)


def new(name, data=b"", *, usedforsecurity=True):
    bool(usedforsecurity)
    if not isinstance(name, str):
        raise TypeError("name must be a string")
    if name == "blake2b":
        return blake2b(data, usedforsecurity=usedforsecurity)
    if name == "blake2s":
        return blake2s(data, usedforsecurity=usedforsecurity)
    return _make_hash(name, data, usedforsecurity)


def pbkdf2_hmac(hash_name, password, salt, iterations, dklen=None):
    return _pbkdf2_hmac(hash_name, password, salt, iterations, dklen)


def file_digest(fileobj, digest, /, *, _bufsize=2**18):
    if isinstance(digest, str):
        digestobj = new(digest)
    else:
        digestobj = digest()

    if hasattr(fileobj, "getbuffer"):
        digestobj.update(fileobj.getbuffer())
        return digestobj

    if (
        hasattr(fileobj, "readinto")
        and hasattr(fileobj, "readable")
        and fileobj.readable()
    ):
        buf = bytearray(_bufsize)
        while True:
            size = fileobj.readinto(buf)
            if size == 0:
                break
            if size == _bufsize:
                digestobj.update(buf)
            else:
                digestobj.update(buf[:size])
        return digestobj

    mode = getattr(fileobj, "mode", None)
    if (
        isinstance(mode, str)
        and "b" in mode
        and ("r" in mode or "+" in mode)
        and hasattr(fileobj, "read")
        and not getattr(fileobj, "closed", False)
    ):
        while True:
            chunk = fileobj.read(_bufsize)
            if not chunk:
                break
            digestobj.update(chunk)
        return digestobj

    raise ValueError(
        f"'{fileobj!r}' is not a file-like object in binary reading mode."
    )
