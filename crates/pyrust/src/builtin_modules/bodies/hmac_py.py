"""HMAC (Keyed-Hashing for Message Authentication) module (RFC 2104 / RFC 4231)."""

import hashlib as _hashlib
import warnings as _warnings

trans_5C = bytes((x ^ 0x5C) for x in range(256))
trans_36 = bytes((x ^ 0x36) for x in range(256))

# The size of the digests returned by HMAC depends on the underlying
# hashing module used.  Use digest_size from the instance of HMAC instead.
digest_size = None

_BUILTIN_HASH_NAMES = {
    _hashlib.md5: "md5",
    _hashlib.sha1: "sha1",
    _hashlib.sha224: "sha224",
    _hashlib.sha256: "sha256",
    _hashlib.sha384: "sha384",
    _hashlib.sha512: "sha512",
    _hashlib.sha3_224: "sha3_224",
    _hashlib.sha3_256: "sha3_256",
    _hashlib.sha3_384: "sha3_384",
    _hashlib.sha3_512: "sha3_512",
    _hashlib.blake2b: "blake2b",
    _hashlib.blake2s: "blake2s",
}


def compare_digest(a, b, /):
    """Return 'a == b' using constant-time comparison."""
    return _compare_digest(a, b)


class HMAC:
    """RFC 2104 HMAC class. Also complies with RFC 4231."""

    blocksize = 64  # 512-bit HMAC; can be changed in subclasses.

    __slots__ = ("_inner", "_outer", "block_size", "digest_size")

    def __init__(self, key, msg=None, digestmod=""):
        if not isinstance(key, (bytes, bytearray)):
            raise TypeError(
                "key: expected bytes or bytearray, but got %r" % type(key).__name__
            )

        if not digestmod:
            raise TypeError("Missing required argument 'digestmod'.")

        if callable(digestmod):
            digest_cons = digestmod
        elif isinstance(digestmod, str):
            digest_cons = lambda d=b"": _hashlib.new(digestmod, d)
        else:
            digest_cons = lambda d=b"": digestmod.new(d)

        self._outer = digest_cons()
        self._inner = digest_cons()
        self.digest_size = self._inner.digest_size

        if self.digest_size == 0:
            raise ValueError(f"unsupported hash type {self._inner.name}")

        if hasattr(self._inner, "block_size"):
            blocksize = self._inner.block_size
            if blocksize < 16:
                _warnings.warn(
                    "block_size of %d seems too small; using our default of %d."
                    % (blocksize, self.blocksize),
                    RuntimeWarning,
                    2,
                )
                blocksize = self.blocksize
        else:
            _warnings.warn(
                "No block_size attribute on given digest object; Assuming %d."
                % (self.blocksize),
                RuntimeWarning,
                2,
            )
            blocksize = self.blocksize

        if len(key) > blocksize:
            key = digest_cons(key).digest()

        self.block_size = blocksize

        key = bytes(key).ljust(blocksize, b"\0")
        self._outer.update(key.translate(trans_5C))
        self._inner.update(key.translate(trans_36))
        if msg is not None:
            self.update(msg)

    @property
    def name(self):
        return f"hmac-{self._inner.name}"

    def update(self, msg):
        """Feed data from msg into this hashing object."""
        self._inner.update(msg)

    def copy(self):
        """Return a separate copy of this hashing object."""
        other = self.__class__.__new__(self.__class__)
        other.digest_size = self.digest_size
        other.block_size = self.block_size
        other._inner = self._inner.copy()
        other._outer = self._outer.copy()
        return other

    def _current(self):
        h = self._outer.copy()
        h.update(self._inner.digest())
        return h

    def digest(self):
        """Return the hash value of this hashing object as bytes."""
        return self._current().digest()

    def hexdigest(self):
        """Return the hash value as a string of hexadecimal digits."""
        return self._current().hexdigest()


def new(key, msg=None, digestmod=""):
    """Create a new hashing object and return it."""
    return HMAC(key, msg, digestmod)


def digest(key, msg, digest):
    """Fast inline implementation of HMAC."""
    if isinstance(digest, str):
        fast = _hmac_digest_fast(key, msg, digest)
        if fast is not None:
            return fast
    else:
        try:
            fast_name = _BUILTIN_HASH_NAMES.get(digest)
        except TypeError:
            fast_name = None
        if fast_name is not None:
            fast = _hmac_digest_fast(key, msg, fast_name)
            if fast is not None:
                return fast

    if callable(digest):
        digest_cons = digest
    elif isinstance(digest, str):
        digest_cons = lambda d=b"": _hashlib.new(digest, d)
    else:
        digest_cons = lambda d=b"": digest.new(d)

    inner = digest_cons()
    outer = digest_cons()
    if inner.digest_size == 0:
        raise ValueError(f"unsupported hash type {inner.name}")
    blocksize = getattr(inner, "block_size", 64)
    if len(key) > blocksize:
        key = digest_cons(key).digest()
    key = bytes(key) + b"\x00" * (blocksize - len(key))
    inner.update(key.translate(trans_36))
    outer.update(key.translate(trans_5C))
    inner.update(msg)
    outer.update(inner.digest())
    return outer.digest()
