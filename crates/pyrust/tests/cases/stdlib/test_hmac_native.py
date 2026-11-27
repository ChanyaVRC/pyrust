import array
import hashlib
import hmac
import warnings


def run(fn, *args, **kwargs):
    try:
        return ("OK", fn(*args, **kwargs))
    except BaseException as e:
        return (type(e).__name__, str(e))


def kind(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
        return "OK"
    except BaseException as e:
        return type(e).__name__


# 1. Module surface
print("surface:", sorted(n for n in ("HMAC", "new", "digest", "compare_digest", "trans_5C", "trans_36", "digest_size") if hasattr(hmac, n)))
print("digest_size:", hmac.digest_size, "blocksize:", hmac.HMAC.blocksize, "module:", hmac.HMAC.__module__)
print("trans:", hmac.trans_36 == bytes(x ^ 0x36 for x in range(256)), hmac.trans_5C == bytes(x ^ 0x5C for x in range(256)), type(hmac.trans_36).__name__)
print("hmac.new is HMAC:", type(hmac.new(b"k", b"m", "sha256")) is hmac.HMAC)


# 2. Every hash name, str vs constructor vs one-shot
names = ["md5", "sha1", "sha224", "sha256", "sha384", "sha512", "sha3_224", "sha3_256", "sha3_384", "sha3_512", "blake2b", "blake2s"]
keys = [b"", b"k", b"Jefe", b"\xaa" * 20, b"\x0b" * 64, b"\xaa" * 65, b"\xaa" * 131, b"\xaa" * 137, b"\xaa" * 200]
for name in names:
    ctor = getattr(hashlib, name)
    row = []
    for key in keys:
        msg = b"what do ya want for nothing?" * 7
        h_name = hmac.new(key, msg, name)
        h_ctor = hmac.HMAC(key, msg, ctor)
        h_kw = hmac.new(key, digestmod=name)
        h_kw.update(msg)
        d_name = hmac.digest(key, msg, name)
        d_ctor = hmac.digest(key, msg, ctor)
        assert h_name.digest() == h_ctor.digest() == h_kw.digest() == d_name == d_ctor, (name, key)
        assert h_name.hexdigest() == h_name.digest().hex()
        row.append(h_name.hexdigest()[:12])
    h = hmac.new(b"key", b"msg", name)
    print(name, h.name, h.digest_size, h.block_size, row)

# OpenSSL spellings and aliases accepted by hashlib.new
for alias in ("SHA256", "Sha1", "sha-256", "sha3-256", "SHA512_224", "sha512_256", "blake2b512", "MD5"):
    print("alias", alias, run(lambda a=alias: (hmac.new(b"k", b"m", a).hexdigest()[:16], hmac.new(b"k", b"m", a).name, hmac.digest(b"k", b"m", a).hex()[:16])))


# 3. RFC 2202 / RFC 4231 vectors
rfc = [
    (b"\x0b" * 20, b"Hi There"),
    (b"Jefe", b"what do ya want for nothing?"),
    (b"\xaa" * 20, b"\xdd" * 50),
    (bytes(range(1, 26)), b"\xcd" * 50),
    (b"\xaa" * 131, b"Test Using Larger Than Block-Size Key - Hash Key First"),
]
for key, msg in rfc:
    print("rfc:", hmac.new(key, msg, "sha256").hexdigest(), hmac.new(key, msg, "sha1").hexdigest())


# 4. Streaming, copy(), non-mutating digest()
h = hmac.new(b"secret", digestmod="sha256")
first = h.digest()
assert h.digest() == first == h.digest()
h.update(b"hello ")
mid = h.hexdigest()
assert h.hexdigest() == mid
c = h.copy()
h.update(b"world")
assert c.hexdigest() == mid
c.update(b"there")
assert h.hexdigest() == hmac.new(b"secret", b"hello world", "sha256").hexdigest()
assert c.hexdigest() == hmac.new(b"secret", b"hello there", "sha256").hexdigest()
h.update(b"")
assert h.hexdigest() == hmac.new(b"secret", b"hello world", "sha256").hexdigest()
print("stream:", h.hexdigest()[:16], c.hexdigest()[:16], first.hex()[:16])
print("copy attrs:", c.name, c.digest_size, run(getattr, c, "block_size"), type(c) is type(h), c is h)

# bytearray / bytes subclass / memoryview / array messages
class MyBytes(bytes):
    pass

class MyByteArray(bytearray):
    pass

ref = hmac.new(b"secret", b"hello world", "sha256").hexdigest()
print("msg kinds:", [
    hmac.new(b"secret", m, "sha256").hexdigest() == ref
    for m in (b"hello world", bytearray(b"hello world"), memoryview(b"hello world"), MyBytes(b"hello world"), MyByteArray(b"hello world"), array.array("B", b"hello world"))
])
print("key kinds:", [
    run(hmac.new, k, b"hello world", "sha256")[0]
    for k in (b"secret", bytearray(b"secret"), MyBytes(b"secret"), MyByteArray(b"secret"), memoryview(b"secret"), array.array("B", b"secret"), "secret", 123, None)
])
print("key errors:", run(hmac.new, memoryview(b"k"), b"m", "sha256"), run(hmac.new, "k", b"m", "sha256"), run(hmac.new, None, b"m", "sha256"), run(hmac.new, [1], b"m", "sha256"))
print("digest key kinds:", [
    run(hmac.digest, k, b"hello world", "sha256")[0]
    for k in (b"secret", bytearray(b"secret"), memoryview(b"secret"), array.array("B", b"secret"), "secret", 123, None)
], run(hmac.digest, "k", b"m", "sha256"), run(hmac.digest, b"k", "m", "sha256"), run(hmac.digest, 1, "m", "nope"))


# 5. digestmod errors
print("missing digestmod:", run(hmac.new, b"k"), run(hmac.new, b"k", b"m"), run(hmac.new, b"k", b"m", ""), run(hmac.new, b"k", b"m", None), run(hmac.new, b"k", b"m", 0), run(hmac.HMAC, b"k"), run(hmac.new, b"k", digestmod=b""))
print("bad digestmod:", run(hmac.new, b"k", b"m", "not_a_hash"), run(hmac.digest, b"k", b"m", "not_a_hash"), run(hmac.new, b"k", b"m", 1), run(hmac.new, b"k", b"m", object()), run(hmac.digest, b"k", b"m", None), run(hmac.digest, b"k", b"m", 1))
print("xof:", kind(hmac.new, b"k", b"m", "shake_128"), kind(hmac.new, b"k", b"m", hashlib.shake_256), kind(hmac.digest, b"k", b"m", "shake_256"), kind(hmac.digest, b"k", b"m", hashlib.shake_128))
print("msg errors:", run(hmac.new, b"k", "m", "sha256"), run(hmac.new, b"k", 5, "sha256"), run(hmac.new, b"k", b"m", "sha256").__class__.__name__)
print("update errors:", run(hmac.new(b"k", digestmod="sha256").update, "s"), run(hmac.new(b"k", digestmod="sha256").update, 1), run(hmac.new(b"k", digestmod="sha256").update))
print("arity:", run(hmac.new), run(hmac.HMAC), run(hmac.digest, b"k", b"m"), run(hmac.digest), run(hmac.new, b"k", b"m", "sha256", 1), run(hmac.new(b"k", digestmod="md5").copy, 1), run(hmac.new(b"k", digestmod="md5").digest, 1))
print("kw errors:", run(hmac.new, key=b"k", digestmod="md5")[0], run(hmac.new, b"k", foo=1), run(hmac.digest, key=b"k", msg=b"m", digest="sha1")[0])


# 6. Custom callable digestmod (a pure-Python hash object)
class PyHash:
    """A SHA-256 with a configurable block_size attribute."""

    name = "pysha"

    def __init__(self, data=b"", block_size=64):
        self._h = hashlib.sha256(data)
        self.block_size = block_size
        self.digest_size = 32
        self.calls = []

    def update(self, data):
        self.calls.append(len(data))
        self._h.update(data)

    def digest(self):
        return self._h.digest()

    def hexdigest(self):
        return self._h.hexdigest()

    def copy(self):
        other = PyHash.__new__(PyHash)
        other._h = self._h.copy()
        other.block_size = self.block_size
        other.digest_size = self.digest_size
        other.calls = list(self.calls)
        return other


expected = hmac.new(b"key", b"message", "sha256")
for label, factory in (
    ("lambda", lambda d=b"": PyHash(d)),
    ("class", PyHash),
    ("hashlib.new-lambda", lambda d=b"": hashlib.new("sha256", d)),
):
    h = hmac.new(b"key", b"message", factory)
    print("custom", label, h.hexdigest() == expected.hexdigest(), h.name, h.digest_size, h.block_size, type(h).__name__)
    print("custom", label, "digest():", hmac.digest(b"key", b"message", factory) == expected.digest())
    print("custom", label, "long key:", hmac.new(b"k" * 100, b"m", factory).hexdigest() == hmac.new(b"k" * 100, b"m", "sha256").hexdigest(), hmac.digest(b"k" * 100, b"m", factory) == hmac.digest(b"k" * 100, b"m", "sha256"))

h = hmac.new(b"key", b"message", PyHash)
d1 = h.digest()
d2 = h.digest()
h.update(b" more")
c = h.copy()
c.update(b" copy")
print("custom state:", d1 == d2, h.hexdigest() == hmac.new(b"key", b"message more", "sha256").hexdigest(), c.hexdigest() == hmac.new(b"key", b"message more copy", "sha256").hexdigest(), h.digest() != c.digest())

# block_size handling: big, small (warns, falls back to 64), missing (warns)
class BigBlock(PyHash):
    def __init__(self, data=b""):
        super().__init__(data, block_size=128)


class SmallBlock(PyHash):
    def __init__(self, data=b""):
        super().__init__(data, block_size=8)


class NoBlock:
    name = "noblock"
    digest_size = 32

    def __init__(self, data=b""):
        self._h = hashlib.sha256(data)

    def update(self, data):
        self._h.update(data)

    def digest(self):
        return self._h.digest()

    def hexdigest(self):
        return self._h.hexdigest()

    def copy(self):
        other = NoBlock()
        other._h = self._h.copy()
        return other


def hmac_by_hand(key, msg, blocksize, hashfn):
    if len(key) > blocksize:
        key = hashfn(key).digest()
    key = key.ljust(blocksize, b"\0")
    inner = hashfn(bytes(b ^ 0x36 for b in key) + msg).digest()
    return hashfn(bytes(b ^ 0x5C for b in key) + inner).digest()


with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    big = hmac.new(b"k" * 100, b"msg", BigBlock)
    small = hmac.new(b"k" * 100, b"msg", SmallBlock)
    noblock = hmac.new(b"k" * 100, b"msg", NoBlock)
    small_short = hmac.new(b"key", b"msg", SmallBlock)
print("block sizes:", big.block_size, small.block_size, noblock.block_size)
print("block digests:", big.digest() == hmac_by_hand(b"k" * 100, b"msg", 128, hashlib.sha256), small.digest() == hmac_by_hand(b"k" * 100, b"msg", 64, hashlib.sha256), noblock.digest() == hmac_by_hand(b"k" * 100, b"msg", 64, hashlib.sha256), small_short.digest() == hmac_by_hand(b"key", b"msg", 64, hashlib.sha256))
print("warnings:", [(w.category.__name__, str(w.message)) for w in caught])
with warnings.catch_warnings():
    warnings.simplefilter("error")
    print("warning as error:", run(hmac.new, b"k", b"m", SmallBlock), run(hmac.new, b"k", b"m", NoBlock))
print("one-shot block sizes:", hmac.digest(b"k" * 100, b"msg", BigBlock) == hmac_by_hand(b"k" * 100, b"msg", 128, hashlib.sha256), hmac.digest(b"k" * 100, b"msg", SmallBlock) == hmac_by_hand(b"k" * 100, b"msg", 8, hashlib.sha256), hmac.digest(b"k" * 100, b"msg", NoBlock) == hmac_by_hand(b"k" * 100, b"msg", 64, hashlib.sha256))

# the default block size of a subclass is honoured
class Block32(hmac.HMAC):
    blocksize = 32

with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    sub = Block32(b"k" * 50, b"msg", NoBlock)
print("subclass blocksize:", sub.block_size, sub.digest() == hmac_by_hand(b"k" * 50, b"msg", 32, hashlib.sha256), [str(w.message) for w in caught])

# errors from a bad custom digestmod
print("custom errors:", run(hmac.new, b"k", b"m", lambda d=b"": 5))
print("custom raise:", run(hmac.new, b"k", b"m", lambda *a: 1 / 0), run(hmac.digest, b"k", b"m", lambda *a: 1 / 0))


# 7. PEP 247 module-like digestmod
class Pep247:
    def __init__(self):
        self.seen = []

    def new(self, data=b""):
        self.seen.append(data)
        return hashlib.sha256(data)

    digest_size = 32


mod = Pep247()
h = hmac.new(b"key", b"message", mod)
print("pep247:", h.hexdigest() == expected.hexdigest(), h.name, h.digest_size, h.block_size, mod.seen)
mod2 = Pep247()
print("pep247 digest():", hmac.digest(b"key", b"message", mod2) == expected.digest(), mod2.seen)
mod3 = Pep247()
print("pep247 long key:", hmac.new(b"k" * 100, b"m", mod3).hexdigest() == hmac.new(b"k" * 100, b"m", "sha256").hexdigest(), [len(s) for s in mod3.seen])

import types
mod_obj = types.SimpleNamespace(new=lambda d=b"": hashlib.sha1(d))
print("namespace module:", hmac.new(b"k", b"m", mod_obj).hexdigest() == hmac.new(b"k", b"m", "sha1").hexdigest(), hmac.digest(b"k", b"m", mod_obj) == hmac.digest(b"k", b"m", "sha1"))
print("no new:", run(hmac.new, b"k", b"m", types.SimpleNamespace()), run(hmac.digest, b"k", b"m", types.SimpleNamespace()))


# 8. Subclassing and instance attributes
class Tagged(hmac.HMAC):
    def __init__(self, tag, *a, **k):
        super().__init__(*a, **k)
        self.tag = tag


t = Tagged("T", b"key", b"message", "sha256")
print("subclass:", t.tag, t.hexdigest() == expected.hexdigest(), t.name, type(t.copy()).__name__, hasattr(t.copy(), "tag"))

h = hmac.new(b"k", b"m", "sha256")
print("attrs:", h.digest_size, h.block_size, run(setattr, h, "foo", 1), run(getattr, h, "missing"))
print("name readonly:", run(setattr, h, "name", "x"))
print("class attrs:", hmac.HMAC.blocksize, hasattr(hmac.HMAC, "digest"), hmac.HMAC.__name__, hmac.HMAC.__qualname__)


# 9. One-shot digest() equals HMAC objects
for name in names:
    assert hmac.digest(b"k", b"m", name) == hmac.new(b"k", b"m", name).digest()
print("oneshot ok")
print("digest kw:", hmac.digest(key=b"k", msg=b"m", digest="sha256").hex()[:16], hmac.digest(b"k", b"m", digest=hashlib.sha256).hex()[:16])
print("digest memoryview:", hmac.digest(memoryview(b"k"), memoryview(b"m"), "sha256").hex()[:16], hmac.digest(bytearray(b"k"), array.array("B", b"m"), hashlib.sha1).hex()[:16])


# 10. compare_digest
class Weird:
    pass

print(
    "compare_digest:",
    hmac.compare_digest(b"abc", b"abc"),
    hmac.compare_digest(b"abc", b"abd"),
    hmac.compare_digest(b"abc", b"ab"),
    hmac.compare_digest(b"ab", b"abc"),
    hmac.compare_digest(b"", b""),
    hmac.compare_digest(bytearray(b"abc"), b"abc"),
    hmac.compare_digest(b"abc", bytearray(b"abd")),
    hmac.compare_digest(memoryview(b"abc"), b"abc"),
    hmac.compare_digest(memoryview(b"abc"), memoryview(b"abc")),
    hmac.compare_digest(array.array("B", b"abc"), b"abc"),
    hmac.compare_digest(MyBytes(b"abc"), MyByteArray(b"abc")),
    hmac.compare_digest("abc", "abc"),
    hmac.compare_digest("abc", "abd"),
    hmac.compare_digest("abc", "ab"),
    hmac.compare_digest("", ""),
    hmac.compare_digest("a\x00b", "a\x00b"),
)
print(
    "compare_digest errors:",
    run(hmac.compare_digest, "abc", b"abc"),
    run(hmac.compare_digest, b"abc", "abc"),
    run(hmac.compare_digest, "é", "é"),
    run(hmac.compare_digest, "a", "é"),
    run(hmac.compare_digest, "é", "a"),
    run(hmac.compare_digest, 1, 1),
    run(hmac.compare_digest, "a", 1),
    run(hmac.compare_digest, 1, "a"),
    run(hmac.compare_digest, b"a", 1),
    run(hmac.compare_digest, 1, b"a"),
    run(hmac.compare_digest, None, None),
    run(hmac.compare_digest, [1], [1]),
    run(hmac.compare_digest, Weird(), b"a"),
    run(hmac.compare_digest, bytearray(b"a"), "a"),
)
print("compare_digest arity:", run(hmac.compare_digest), run(hmac.compare_digest, b"a"), run(hmac.compare_digest, b"a", b"a", b"a"), run(hmac.compare_digest, a=b"a", b=b"a"), run(hmac.compare_digest, b"a", b=b"a"))

# digests produced by this module compare equal to themselves and not to others
d = hmac.new(b"k", b"m", "sha256")
print("compare own:", hmac.compare_digest(d.digest(), hmac.digest(b"k", b"m", "sha256")), hmac.compare_digest(d.hexdigest(), hmac.new(b"k", b"m", "sha256").hexdigest()), hmac.compare_digest(d.digest(), hmac.digest(b"k", b"n", "sha256")))



# 11. copy / pickle protocol, uninitialised instances, odd buffers
import copy
import pickle

h = hmac.new(b"k", b"m", "sha256")
print("copy.copy:", copy.copy(h).hexdigest() == h.hexdigest(), kind(copy.deepcopy, h), kind(pickle.dumps, h), run(pickle.dumps, h))


class Bare(hmac.HMAC):
    def __init__(self):
        pass


bare = Bare()
print("bare:", run(bare.digest), run(bare.hexdigest), run(bare.update, b"x"), run(bare.copy), run(getattr, bare, "name"))
print("unbound:", run(hmac.HMAC.update, 5, b"x"), run(hmac.HMAC.digest, b"x"))

print("compare strided:", run(hmac.compare_digest, memoryview(b"abcd")[::2], b"ac"), run(hmac.compare_digest, b"ac", memoryview(b"abcd")[::2]), run(hmac.compare_digest, "a", memoryview(b"abcd")[::2]))
print("compare cast:", run(hmac.compare_digest, memoryview(bytearray(b"abcd")).cast("H"), b"abcd"))

h2 = hmac.HMAC(b"k", b"m", "md5")
h2.__init__(b"k2", b"m2", "sha1")
print("re-init:", h2.name, h2.digest_size, h2.hexdigest() == hmac.new(b"k2", b"m2", "sha1").hexdigest())


class StrName(str):
    pass


print("str subclass digestmod:", hmac.new(b"k", b"m", StrName("sha256")).hexdigest()[:8], hmac.digest(b"k", b"m", StrName("sha256")).hex()[:8])
print("long key:", hmac.new(b"k" * 100000, b"m", "sha256").hexdigest()[:8])
print("non-hash callables:", run(hmac.new, b"k", b"m", len), run(hmac.digest, b"k", b"m", len), run(hmac.new, b"k", b"m", hmac.new)[0])


class FloatBlock(PyHash):
    def __init__(self, data=b""):
        super().__init__(data)
        self.block_size = 64.0


class StrBlock(PyHash):
    def __init__(self, data=b""):
        super().__init__(data)
        self.block_size = "x"


print("odd block sizes:", kind(hmac.new, b"k", b"m", FloatBlock), run(hmac.new, b"k", b"m", StrBlock), run(hmac.digest, b"k", b"m", FloatBlock)[0], run(hmac.digest, b"k", b"m", StrBlock))
print("generic key types:", run(hmac.digest, "k", b"m", PyHash), run(hmac.digest, memoryview(b"k"), b"m", PyHash), run(hmac.digest, 5, b"m", PyHash), run(hmac.digest, bytearray(b"k" * 100), b"m", PyHash)[0])

print("hmac native ok")
