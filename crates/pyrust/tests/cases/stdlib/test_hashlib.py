import array
import hashlib
import io
import os
import sys


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


def expect_exc_type(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return type(e).__name__


# 1. Module metadata, __all__, algorithms_guaranteed / algorithms_available
print("in sys.modules:", "hashlib" in sys.modules)
print("__all__:", hashlib.__all__)
for name in hashlib.__all__:
    assert hasattr(hashlib, name), name
print("all exported ok")

print("guaranteed type:", type(hashlib.algorithms_guaranteed).__name__)
print("available type:", type(hashlib.algorithms_available).__name__)
print("guaranteed sorted:", sorted(hashlib.algorithms_guaranteed))
print("guaranteed <= available:", hashlib.algorithms_guaranteed <= hashlib.algorithms_available)


# 2. Known RFC / NIST test vectors (empty, "abc", multi-block)
long_msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
very_long = b"a" * 1000

fixed_algos = [
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "blake2b",
    "blake2s",
]

for algo in fixed_algos:
    ctor = getattr(hashlib, algo)
    h0 = ctor()
    h_abc = ctor(b"abc")
    h_long = ctor(long_msg)
    h_vlong = ctor(very_long)
    print(algo, "attrs:", h0.name, h0.digest_size, h0.block_size)
    print(algo, "empty:", h0.hexdigest())
    print(algo, "abc:", h_abc.hexdigest())
    print(algo, "long:", h_long.hexdigest())
    print(algo, "vlong:", h_vlong.hexdigest())
    assert len(h_abc.digest()) == h_abc.digest_size
    assert h_abc.digest().hex() == h_abc.hexdigest()
    # hashlib.new parity
    assert hashlib.new(algo, b"abc").hexdigest() == h_abc.hexdigest()
    assert hashlib.new(algo, bytearray(b"abc")).hexdigest() == h_abc.hexdigest()
    assert hashlib.new(algo, array.array("B", b"abc")).hexdigest() == h_abc.hexdigest()


# 3. Streaming .update(), non-mutating .digest()/.hexdigest(), and .copy()
for algo in fixed_algos:
    ctor = getattr(hashlib, algo)
    h = ctor()
    h.update(b"ab")
    snap = h.copy()
    d_mid = h.hexdigest()
    h.update(b"c")
    # Calling digest/hexdigest before more updates must not corrupt state.
    assert snap.hexdigest() == d_mid
    snap.update(b"xyz")
    assert h.hexdigest() == ctor(b"abc").hexdigest()
    assert snap.hexdigest() == ctor(b"abxyz").hexdigest()
    # Chunked streaming across block boundaries
    h_chunked = ctor()
    for i in range(0, len(very_long), 13):
        h_chunked.update(very_long[i : i + 13])
    assert h_chunked.hexdigest() == ctor(very_long).hexdigest()
print("streaming and copy ok")


# 4. SHAKE128 / SHAKE256 (XOF)
for algo in ("shake_128", "shake_256"):
    ctor = getattr(hashlib, algo)
    h = ctor(b"abc")
    print(algo, "attrs:", h.name, h.digest_size, h.block_size)
    print(algo, "len0:", h.digest(0), h.hexdigest(0))
    print(algo, "len16:", h.hexdigest(16))
    print(algo, "len64:", h.hexdigest(64))
    print(algo, "len200:", h.hexdigest(200))
    # Prefix property and non-mutating digest
    assert h.digest(200)[:64] == h.digest(64)
    assert h.digest(64)[:16] == h.digest(16)
    c = h.copy()
    h.update(b"def")
    assert c.hexdigest(32) == ctor(b"abc").hexdigest(32)
    assert h.hexdigest(32) == ctor(b"abcdef").hexdigest(32)
    assert hashlib.new(algo, b"abc").hexdigest(32) == ctor(b"abc").hexdigest(32)
    print(algo, "missing len:", expect_exc_type(h.digest), expect_exc_type(h.hexdigest))


# 5. hashlib.new aliases and usedforsecurity
for alias in ("MD5", "SHA1", "sha-1", "SHA-1", "SHA224", "SHA-224", "SHA256", "SHA-256", "sha-256", "SHA384", "SHA-384", "SHA512", "SHA-512", "sha512_224", "sha512_256"):
    h = hashlib.new(alias, b"abc")
    print("alias:", alias, h.name, h.digest_size, h.hexdigest())

print(
    "usedforsecurity:",
    hashlib.md5(b"abc", usedforsecurity=False).hexdigest(),
    hashlib.sha256(string=b"abc", usedforsecurity=False).hexdigest(),
    hashlib.new("sha256", b"abc", usedforsecurity=False).hexdigest(),
    hashlib.blake2b(b"abc", usedforsecurity=False).hexdigest(),
)


# 6. BLAKE2b / BLAKE2s parameters & class constants
print(
    "blake2b consts:",
    hashlib.blake2b.MAX_DIGEST_SIZE,
    hashlib.blake2b.MAX_KEY_SIZE,
    hashlib.blake2b.PERSON_SIZE,
    hashlib.blake2b.SALT_SIZE,
    isinstance(hashlib.blake2b(), hashlib.blake2b),
)
print(
    "blake2s consts:",
    hashlib.blake2s.MAX_DIGEST_SIZE,
    hashlib.blake2s.MAX_KEY_SIZE,
    hashlib.blake2s.PERSON_SIZE,
    hashlib.blake2s.SALT_SIZE,
    isinstance(hashlib.blake2s(), hashlib.blake2s),
)

for sz in (1, 16, 20, 32, 48, 64):
    h = hashlib.blake2b(b"abc", digest_size=sz)
    assert h.digest_size == sz and len(h.digest()) == sz
    print("blake2b sz", sz, h.hexdigest())

for sz in (1, 8, 16, 20, 32):
    h = hashlib.blake2s(b"abc", digest_size=sz)
    assert h.digest_size == sz and len(h.digest()) == sz
    print("blake2s sz", sz, h.hexdigest())

print(
    "blake2b keyed:",
    hashlib.blake2b(b"message", key=b"secret-key", digest_size=32).hexdigest(),
    hashlib.blake2b(b"message", key=bytearray(b"secret-key"), salt=b"salt1234", person=b"my-app", digest_size=24).hexdigest(),
    hashlib.blake2b(b"", key=b"k" * 64).hexdigest(),
)
print(
    "blake2s keyed:",
    hashlib.blake2s(b"message", key=b"secret-key", digest_size=16).hexdigest(),
    hashlib.blake2s(b"message", key=bytearray(b"secret-key"), salt=b"salt1234", person=b"app12345", digest_size=20).hexdigest(),
    hashlib.blake2s(b"", key=b"k" * 32).hexdigest(),
)
print(
    "blake2b tree:",
    hashlib.blake2b(
        b"abc",
        digest_size=32,
        key=b"k",
        salt=b"s" * 16,
        person=b"p" * 16,
        fanout=2,
        depth=2,
        leaf_size=1024,
        node_offset=1,
        node_depth=1,
        inner_size=32,
        last_node=True,
    ).hexdigest(),
)
print(
    "blake2s tree:",
    hashlib.blake2s(
        b"abc",
        digest_size=16,
        key=b"k",
        salt=b"s" * 8,
        person=b"p" * 8,
        fanout=2,
        depth=2,
        leaf_size=1024,
        node_offset=1,
        node_depth=1,
        inner_size=16,
        last_node=True,
    ).hexdigest(),
)

# Verify blake2 keyed streaming matches single-shot
b2k = hashlib.blake2b(key=b"secret", digest_size=32)
b2k.update(b"hel")
b2k.update(b"lo")
assert b2k.hexdigest() == hashlib.blake2b(b"hello", key=b"secret", digest_size=32).hexdigest()
b2sk = hashlib.blake2s(key=b"secret", digest_size=16)
b2sk.update(b"hel")
b2sk.update(b"lo")
assert b2sk.hexdigest() == hashlib.blake2s(b"hello", key=b"secret", digest_size=16).hexdigest()
print("blake2 keyed streaming ok")


# 7. pbkdf2_hmac (RFC 6070 + SHA-256 + SHA-512 + custom dklen)
print("pbkdf2 sha1 1:", hashlib.pbkdf2_hmac("sha1", b"password", b"salt", 1, 20).hex())
print("pbkdf2 sha1 2:", hashlib.pbkdf2_hmac("sha1", b"password", b"salt", 2, 20).hex())
print("pbkdf2 sha1 4096:", hashlib.pbkdf2_hmac("sha1", b"password", b"salt", 4096, 20).hex())
print("pbkdf2 sha1 long:", hashlib.pbkdf2_hmac("sha1", b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 25).hex())
print("pbkdf2 sha256 1:", hashlib.pbkdf2_hmac("sha256", b"password", b"salt", 1).hex())
print("pbkdf2 sha256 2 dk16:", hashlib.pbkdf2_hmac("sha256", b"password", b"salt", 2, 16).hex())
print("pbkdf2 sha256 dk50:", hashlib.pbkdf2_hmac("sha256", b"password", b"salt", 5, 50).hex())
print("pbkdf2 sha512 1:", hashlib.pbkdf2_hmac("sha512", bytearray(b"password"), bytearray(b"salt"), 1).hex())


# 8. file_digest (BytesIO, custom readinto/readable, real binary file, errors)
print("file_digest BytesIO str:", hashlib.file_digest(io.BytesIO(b"hello world"), "sha256").hexdigest())
print("file_digest BytesIO fn:", hashlib.file_digest(io.BytesIO(b"hello world"), hashlib.sha256).hexdigest())
print("file_digest small buf:", hashlib.file_digest(io.BytesIO(very_long), "md5", _bufsize=37).hexdigest())


class CustomBinaryReader:
    def __init__(self, data):
        self._data = data
        self._pos = 0

    def readable(self):
        return True

    def readinto(self, b):
        rem = self._data[self._pos : self._pos + len(b)]
        n = len(rem)
        b[:n] = rem
        self._pos += n
        return n


print("file_digest custom:", hashlib.file_digest(CustomBinaryReader(b"streamed-bytes"), "sha1", _bufsize=4).hexdigest())

tmp_path = f"_pyrust_hashlib_test_{os.getpid()}.bin"
try:
    with open(tmp_path, "wb") as f:
        f.write(b"file digest payload 1234567890")
    with open(tmp_path, "rb") as f:
        print("file_digest real file:", hashlib.file_digest(f, "sha256").hexdigest())
finally:
    if os.path.exists(tmp_path):
        os.remove(tmp_path)


# 9. Error parity
print(
    "str errors:",
    expect_exc(hashlib.sha256, "abc"),
    expect_exc(hashlib.sha256().update, "abc"),
    expect_exc(hashlib.new, "sha256", "abc"),
    expect_exc(hashlib.blake2b, "abc"),
    expect_exc(hashlib.blake2s, "abc"),
)
print(
    "type errors:",
    expect_exc(hashlib.sha256, 123),
    expect_exc(hashlib.new, 123),
    expect_exc(hashlib.new, "not_a_real_hash"),
    expect_exc_type(hashlib.blake2b, data=b"a"),
    expect_exc(hashlib.blake2b, key="abc"),
    expect_exc(hashlib.pbkdf2_hmac, 123, b"p", b"s", 1),
    expect_exc(hashlib.pbkdf2_hmac, "sha256", "p", b"s", 1),
    expect_exc(hashlib.pbkdf2_hmac, "sha256", b"p", "s", 1),
)
print(
    "value errors:",
    expect_exc(hashlib.blake2b, digest_size=0),
    expect_exc(hashlib.blake2b, digest_size=65),
    expect_exc(hashlib.blake2s, digest_size=0),
    expect_exc(hashlib.blake2s, digest_size=33),
    expect_exc(hashlib.blake2b, key=b"x" * 65),
    expect_exc(hashlib.blake2b, salt=b"x" * 17),
    expect_exc(hashlib.blake2b, person=b"x" * 17),
    expect_exc(hashlib.blake2b, fanout=-1),
    expect_exc(hashlib.blake2b, depth=0),
    expect_exc(hashlib.blake2b, leaf_size=-1),
    expect_exc(hashlib.blake2b, node_offset=-1),
    expect_exc(hashlib.blake2b, node_depth=256),
    expect_exc(hashlib.blake2b, inner_size=65),
    expect_exc(hashlib.blake2s, inner_size=33),
    expect_exc(hashlib.blake2s, node_offset=2**48),
    expect_exc(hashlib.pbkdf2_hmac, "sha256", b"p", b"s", 0),
    expect_exc(hashlib.pbkdf2_hmac, "sha256", b"p", b"s", -1),
    expect_exc(hashlib.pbkdf2_hmac, "sha256", b"p", b"s", 1, 0),
    expect_exc(hashlib.pbkdf2_hmac, "sha256", b"p", b"s", 1, -1),
    expect_exc_type(hashlib.file_digest, 123, "sha256"),
    expect_exc_type(hashlib.file_digest, io.StringIO("abc"), "sha256"),
    expect_exc(hashlib.file_digest, io.BytesIO(b"abc"), "nope"),
)

# Read-only attributes on hash objects
h_ro = hashlib.sha256()
for attr in ("name", "digest_size", "block_size"):
    print("readonly", attr, expect_exc_type(setattr, h_ro, attr, 1))

# pbkdf2_hmac with unknown/shake hash raises ValueError (or subclass)
for bad in ("nope", "shake_128"):
    try:
        hashlib.pbkdf2_hmac(bad, b"p", b"s", 1)
        print("pbkdf2 bad unexpected:", bad)
    except ValueError:
        print("pbkdf2 bad ValueError ok:", bad)

# 10. C-level argument parsing, error wording, and call shapes (CPython 3.12.3
# reports these from `_hashlib` / `_blake2` Argument Clinic code).
def e(f, *a, **k):
    try:
        r = f(*a, **k)
        return ("OK", r if not hasattr(r, "hexdigest") else type(r).__name__)
    except BaseException as x:
        return (type(x).__name__, str(x))


print(e(hashlib.new,'md5',b'',foo=1))
print(e(hashlib.new,'blake2b',b'',digest_size=8))
print(e(hashlib.new,'blake2b',b'',foo=1))
print(e(hashlib.new))
print(e(hashlib.new,'md5',b'a',b'b'))
print(e(hashlib.new,'md5',data=b'a'))
print(e(hashlib.new,'md5',string=b'a'))
print(e(hashlib.new,name='md5'))
print(e(hashlib.new,'md5',None))
print(e(hashlib.new,'md5',usedforsecurity=0))
print(e(hashlib.md5,b'a',b'b'))
print(e(hashlib.md5,foo=1))
print(e(hashlib.md5,string=b'a',usedforsecurity=None))
print(e(hashlib.md5,b'a',True))
print(e(hashlib.md5,None))
print(e(hashlib.md5,'a'))
print(e(hashlib.md5,memoryview(b'abc')))
print(e(hashlib.md5,memoryview(b'abc')[::2]))
print(e(hashlib.md5,io.BytesIO(b'a')))
print(e(hashlib.sha256().update))
print(e(hashlib.sha256().update,b'a',b'b'))
print(e(hashlib.sha256().update,data=b'a'))
print(e(hashlib.sha256().digest,1))
print(e(hashlib.sha256().copy,1))
print(e(hashlib.shake_128().digest,-1))
print(e(hashlib.shake_128().digest,2**70))
print(e(hashlib.shake_128().digest,'a'))
print(e(hashlib.shake_128().digest,1.5))
print(e(hashlib.shake_128().digest,length=3))
print(e(hashlib.shake_128().hexdigest,-1))
print(e(hashlib.shake_128().digest,True))
print(e(hashlib.shake_128().digest,1,2))
print(e(hashlib.pbkdf2_hmac))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's'))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's',1,None))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's',1,2,3))
print(e(hashlib.pbkdf2_hmac,hash_name='sha256',password=b'p',salt=b's',iterations=1,dklen=4))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's',1.5))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's',2**70))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's',1,2**70))
print(e(hashlib.pbkdf2_hmac,'nope',b'p',b's',1))
print(e(hashlib.pbkdf2_hmac,'SHA256',b'p',b's',1,4))
print(e(hashlib.pbkdf2_hmac,'sha3_256',b'p',b's',1,4))
print(e(hashlib.pbkdf2_hmac,'blake2b',b'p',b's',1,4))
print(e(hashlib.pbkdf2_hmac,'shake_128',b'p',b's',1,4))
print(e(hashlib.pbkdf2_hmac,'sha256','p',b's',1))
print(e(hashlib.pbkdf2_hmac,'sha256',b'p',b's','a'))
print(e(hashlib.scrypt))
print(e(hashlib.scrypt,b'p'))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,dklen=10).__repr__())
print(e(hashlib.scrypt,b'p',salt=b's',n=15,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=1,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=0,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=0))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,dklen=0))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,dklen=-1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,maxmem=-1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,maxmem=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=2**64,r=1,p=1))
print(e(hashlib.scrypt,'p',salt=b's',n=16,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt='s',n=16,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=None,n=16,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=None,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=None,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=None))
print(e(hashlib.scrypt,b'p',b's',n=16,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16.0,r=1,p=1))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,dklen=None))
print(e(hashlib.scrypt,b'p',salt=b's',n=16,r=1,p=1,maxmem=None))
print(hashlib.scrypt(b'password',salt=b'NaCl',n=1024,r=8,p=16,dklen=64).hex())
print(hashlib.scrypt(b'',salt=b'',n=16,r=1,p=1,dklen=64).hex())
print(e(hashlib.file_digest))
print(e(hashlib.file_digest,io.BytesIO(b'a')))
print(e(hashlib.file_digest,io.BytesIO(b'a'),'md5',_bufsize=0))
print(e(hashlib.file_digest,io.BytesIO(b'a'),'md5',_bufsize=-1))
print(e(hashlib.file_digest,io.BytesIO(b'a'),'md5',1))
print(e(hashlib.file_digest,io.BytesIO(b'a'),digest='md5'))
print(e(hashlib.file_digest,io.BytesIO(b'a'),None))
print(e(hashlib.file_digest,io.BytesIO(b'a'),1))
print(e(hashlib.file_digest,io.BytesIO(b'a'),lambda: hashlib.md5()))
print(e(hashlib.file_digest,'x','md5'))

# 11. Hash object types: identity, readonly attributes, pickling, subclassing.
h_md5 = hashlib.md5()
h_shake = hashlib.shake_128()
h_b2b = hashlib.blake2b()
h_b2s = hashlib.blake2s()
for obj in (h_md5, h_shake, h_b2b, h_b2s):
    t = type(obj)
    print("type:", t.__module__, t.__name__, [a for a in dir(t) if not a.startswith("_")])
print("mro:", [c.__name__ for c in type(h_shake).__mro__], [c.__name__ for c in type(h_md5).__mro__])
print("repr prefixes:", repr(h_md5)[:25], repr(h_shake)[:25], repr(h_b2b)[:20], repr(h_b2s)[:20])
print("same class:", type(hashlib.sha1()) is type(hashlib.sha3_256()), type(hashlib.sha1()) is type(h_shake))
print("blake2 module:", hashlib.blake2b.__module__, hashlib.blake2s.__module__)
print(
    "no create:",
    expect_exc(type(h_md5)),
    expect_exc(type(h_shake)),
)
print("no subclass:", expect_exc(type, "B", (hashlib.blake2b,), {}), expect_exc(type, "S", (hashlib.blake2s,), {}))
import copy
import pickle

print(
    "no pickle:",
    expect_exc(pickle.dumps, h_md5),
    expect_exc(copy.copy, h_shake),
    expect_exc(pickle.dumps, h_b2b),
    expect_exc(copy.copy, h_b2s),
)
print("no dict:", expect_exc_type(setattr, h_md5, "extra", 1), expect_exc_type(setattr, h_b2b, "extra", 1))
print("eq/hash:", h_md5 == h_md5, h_md5 == hashlib.md5(), isinstance(hash(h_md5), int))
print("blake2 attrs:", h_b2b.name, h_b2b.digest_size, h_b2b.block_size, h_b2s.name, h_b2s.digest_size, h_b2s.block_size)
print(
    "blake2 copy:",
    type(h_b2b.copy()) is hashlib.blake2b,
    hashlib.blake2b(b"abc", digest_size=20).copy().digest_size,
    hashlib.blake2s(b"abc", digest_size=12).copy().hexdigest(),
)

# 12. hashlib.new aliases accepted through OpenSSL's case-insensitive name table.
for n in ("sha512/224", "SHA-512/224", "Sha1", "BLAKE2B", "blake2b512", "blake2s256", "sha1-160", "shake-128", "SHAKE_128", "SHAKE128", "SHA3-224", "sha-512/256", "SHA2-256"):
    r = e(hashlib.new, n)
    print("new alias:", n, r if r[0] != "OK" else hashlib.new(n).name)

# 13. scrypt (RFC 7914 vectors) and its validation order.
print("scrypt rfc1:", hashlib.scrypt(b"", salt=b"", n=16, r=1, p=1, dklen=64).hex())
print("scrypt rfc2:", hashlib.scrypt(b"password", salt=b"NaCl", n=1024, r=8, p=16, dklen=64).hex())
print("scrypt default dklen:", len(hashlib.scrypt(b"pw", salt=b"s", n=16, r=1, p=1)))
print("scrypt bytearray:", hashlib.scrypt(bytearray(b"pw"), salt=memoryview(b"salt"), n=32, r=2, p=1, dklen=16).hex())
print("scrypt maxmem:", hashlib.scrypt(b"pw", salt=b"s", n=16, r=1, p=1, maxmem=2**20, dklen=8).hex())
for kwargs in (
    dict(n=16, r=1, p=1, dklen=0),
    dict(n=16, r=1, p=1, dklen=-1),
    dict(n=16, r=1, p=1, maxmem=-1),
    dict(n=16, r=1, p=1, maxmem=1),
    dict(n=0, r=1, p=1),
    dict(n=1, r=1, p=1),
    dict(n=15, r=1, p=1),
    dict(n=16, r=0, p=1),
    dict(n=16, r=1, p=0),
    dict(n=2**16, r=1, p=1),
    dict(n=16, r=-1, p=1),
    dict(n=16, r=1, p=2**31),
    dict(n=16, r=2**31, p=1),
    dict(r=1, p=1),
    dict(n=16, p=1),
    dict(n=16, r=1),
    dict(n=16.0, r=1, p=1),
    dict(n=None, r=1, p=1),
    dict(n=16, r="1", p=1),
    dict(n=16, r=1, p=1, dklen=None),
    dict(n=16, r=1, p=1, maxmem=None),
    dict(n=16, r=1, p=1, salt=None),
    dict(n=16, r=1, p=1, salt="s"),
    dict(n=16, r=1, p=1, extra=1),
):
    kw = dict(kwargs)
    kw.setdefault("salt", b"s")
    print("scrypt err:", sorted(kwargs), e(hashlib.scrypt, b"p", **kw))
print("scrypt shapes:", e(hashlib.scrypt), e(hashlib.scrypt, b"p"), e(hashlib.scrypt, b"p", b"s", n=16, r=1, p=1), e(hashlib.scrypt, "p", salt=b"s", n=16, r=1, p=1), e(hashlib.scrypt, b"p", n=16, r=1, p=1))

# 14. Case-insensitive OpenSSL names also reach pbkdf2_hmac.
print("pbkdf2 names:", [hashlib.pbkdf2_hmac(n, b"p", b"s", 1, 4).hex() for n in ("sha256", "SHA256", "sha-256", "Sha1", "sha3_256", "blake2b")])

# 15. file_digest argument binding and reader protocol variants.
class NotReadable:
    def __repr__(self):
        return "<NotReadable>"

    def readinto(self, b):
        return 0

    def readable(self):
        return False


class NoReadable:
    def __repr__(self):
        return "<NoReadable>"

    def readinto(self, b):
        return 0


class RaisingAttr:
    @property
    def getbuffer(self):
        raise RuntimeError("boom")


class Updater:
    def __init__(self):
        self.chunks = []

    def update(self, data):
        self.chunks.append(bytes(data))


class OneByteReader:
    def __init__(self, data):
        self._data = data
        self._pos = 0

    def readable(self):
        return True

    def readinto(self, b):
        chunk = self._data[self._pos : self._pos + 1]
        n = len(chunk)
        b[:n] = chunk
        self._pos += n
        return n


updater = Updater()
hashlib.file_digest(OneByteReader(b"xyz"), lambda: updater, _bufsize=4)
print("file_digest chunks:", updater.chunks)
print(
    "file_digest shapes:",
    e(hashlib.file_digest),
    e(hashlib.file_digest, io.BytesIO(b"a")),
    e(hashlib.file_digest, io.BytesIO(b"a"), "md5", 1),
    e(hashlib.file_digest, io.BytesIO(b"a"), digest="md5"),
    e(hashlib.file_digest, io.BytesIO(b"a"), "md5", bogus=1),
    e(hashlib.file_digest, fileobj=io.BytesIO(b"a"), digest="md5"),
    e(hashlib.file_digest, io.BytesIO(b"a"), None),
    e(hashlib.file_digest, io.BytesIO(b"a"), 1),
    e(hashlib.file_digest, "x", "md5"),
    e(hashlib.file_digest, NotReadable(), "md5"),
    e(hashlib.file_digest, NoReadable(), "md5"),
    e(hashlib.file_digest, RaisingAttr(), "md5"),
    e(hashlib.file_digest, io.BytesIO(b"a"), "nope"),
    e(hashlib.file_digest, io.BytesIO(b"a"), "md5", _bufsize=0),
)
print(
    "file_digest digest kinds:",
    hashlib.file_digest(io.BytesIO(b"abc"), "sha1").name,
    hashlib.file_digest(io.BytesIO(b"abc"), hashlib.sha3_256).hexdigest(),
    hashlib.file_digest(io.BytesIO(b"abc"), lambda: hashlib.blake2s(digest_size=8)).hexdigest(),
    hashlib.file_digest(io.BytesIO(b""), "sha256").hexdigest(),
)

# 16. algorithms sets are plain mutable sets owned by the module.
print("sets:", type(hashlib.algorithms_guaranteed).__name__, hashlib.algorithms_guaranteed is hashlib.algorithms_available, len(hashlib.algorithms_guaranteed))
print("module attrs:", sorted(n for n in ("md5", "scrypt", "pbkdf2_hmac", "new", "file_digest", "HASH", "HASHXOF", "_Hash", "_ShakeHash", "_new_hash", "_pbkdf2_hmac") if hasattr(hashlib, n)))

print("hashlib ok")
