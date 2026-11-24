import array
import hashlib
import hmac
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


# 1. Module metadata & constants
print("in sys.modules:", "hmac" in sys.modules)
print("digest_size:", hmac.digest_size)
print("trans lens:", len(hmac.trans_36), len(hmac.trans_5C))
print("trans sample:", hmac.trans_36[0], hmac.trans_5C[0], hmac.trans_36[0x36], hmac.trans_5C[0x5C])


# 2. RFC 2104 (MD5) & RFC 4231 (SHA-256 / SHA-384 / SHA-512) test vectors
rfc2104_md5 = [
    (b"\x0b" * 16, b"Hi There"),
    (b"Jefe", b"what do ya want for nothing?"),
    (b"\xaa" * 16, b"\xdd" * 50),
    (bytes(range(1, 26)), b"\xcd" * 50),
    (b"\x0c" * 16, b"Test With Truncation"),
    (b"\xaa" * 80, b"Test Using Larger Than Block-Size Key - Hash Key First"),
    (
        b"\xaa" * 80,
        b"Test Using Larger Than Block-Size Key and Larger Than One Block-Size Data",
    ),
]

for idx, (k, m) in enumerate(rfc2104_md5):
    h1 = hmac.new(k, m, "md5")
    h2 = hmac.HMAC(k, m, hashlib.md5)
    d3 = hmac.digest(k, m, "md5")
    d4 = hmac.digest(k, m, hashlib.md5)
    assert h1.digest() == h2.digest() == d3 == d4
    assert h1.digest().hex() == h1.hexdigest()
    print(f"md5[{idx}]:", h1.hexdigest())

rfc4231 = [
    (b"\x0b" * 20, b"Hi There"),
    (b"Jefe", b"what do ya want for nothing?"),
    (b"\xaa" * 20, b"\xdd" * 50),
    (bytes(range(1, 26)), b"\xcd" * 50),
    (b"\x0c" * 20, b"Test With Truncation"),
    (b"\xaa" * 131, b"Test Using Larger Than Block-Size Key - Hash Key First"),
    (
        b"\xaa" * 131,
        b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.",
    ),
]

for algo in ("sha224", "sha256", "sha384", "sha512", "sha1", "sha3_256", "blake2b", "blake2s"):
    ctor = getattr(hashlib, algo)
    for idx, (k, m) in enumerate(rfc4231):
        h1 = hmac.new(k, m, algo)
        h2 = hmac.new(k, m, ctor)
        d3 = hmac.digest(k, m, algo)
        d4 = hmac.digest(k, m, ctor)
        assert h1.digest() == h2.digest() == d3 == d4
        if idx in (0, 1, 5):
            print(f"{algo}[{idx}]:", h1.hexdigest())


# 3. Attributes, streaming .update(), non-mutating .digest(), and .copy()
h = hmac.new(b"secret", digestmod="sha256")
print("attrs:", h.name, h.digest_size, h.block_size)
h.update(b"hello ")
c = h.copy()
mid = h.hexdigest()
h.update(b"world")
assert c.hexdigest() == mid
c.update(b"there")
assert h.hexdigest() == hmac.new(b"secret", b"hello world", "sha256").hexdigest()
assert c.hexdigest() == hmac.new(b"secret", b"hello there", "sha256").hexdigest()
assert c.name == "hmac-sha256" and c.digest_size == 32
print("streaming and copy ok")

# bytearray key & msg
h_ba = hmac.new(bytearray(b"secret"), bytearray(b"hello world"), "sha256")
assert h_ba.hexdigest() == h.hexdigest()
assert hmac.digest(bytearray(b"secret"), bytearray(b"hello world"), "sha256") == h.digest()


# 4. Custom callable and PEP 247 module-like digestmod
class CustomModule:
    def new(self, data=b""):
        return hashlib.sha256(data)


custom_mod = CustomModule()
print(
    "custom digestmod:",
    hmac.new(b"k", b"msg", lambda d=b"": hashlib.sha256(d)).hexdigest()
    == hmac.new(b"k", b"msg", "sha256").hexdigest(),
    hmac.new(b"k", b"msg", custom_mod).hexdigest()
    == hmac.new(b"k", b"msg", "sha256").hexdigest(),
    hmac.digest(b"k", b"msg", lambda d=b"": hashlib.sha256(d))
    == hmac.digest(b"k", b"msg", "sha256"),
    hmac.digest(b"k", b"msg", custom_mod) == hmac.digest(b"k", b"msg", "sha256"),
)


# 5. compare_digest (bytes, bytearray, array, ASCII str, and type rules)
print(
    "compare_digest:",
    hmac.compare_digest(b"abc", b"abc"),
    hmac.compare_digest(b"abc", b"abd"),
    hmac.compare_digest(b"abc", b"ab"),
    hmac.compare_digest(b"", b""),
    hmac.compare_digest(bytearray(b"abc"), b"abc"),
    hmac.compare_digest(b"abc", bytearray(b"abc")),
    hmac.compare_digest(array.array("B", b"abc"), b"abc"),
    hmac.compare_digest("abc", "abc"),
    hmac.compare_digest("abc", "abd"),
    hmac.compare_digest("", ""),
)
print(
    "compare_digest errors:",
    expect_exc(hmac.compare_digest, "abc", b"abc"),
    expect_exc(hmac.compare_digest, b"abc", "abc"),
    expect_exc(hmac.compare_digest, "\u00e9", "\u00e9"),
    expect_exc(hmac.compare_digest, "a", "\u00e9"),
    expect_exc(hmac.compare_digest, 1, 1),
    expect_exc(hmac.compare_digest, "a", 1),
    expect_exc(hmac.compare_digest, b"a", 1),
    expect_exc(hmac.compare_digest, 1, b"a"),
)


# 6. HMAC & digest error parity
print(
    "hmac errors:",
    expect_exc(hmac.new, b"k", b"m"),
    expect_exc(hmac.new, b"k", b"m", None),
    expect_exc(hmac.new, "k", b"m", "sha256"),
    expect_exc(hmac.new, b"k", "m", "sha256"),
    expect_exc(hmac.new, b"k", b"m", "not_a_hash"),
    expect_exc_type(hmac.new, b"k", b"m", "shake_128"),
    expect_exc(hmac.digest, "k", b"m", "sha256"),
    expect_exc(hmac.digest, b"k", "m", "sha256"),
    expect_exc(hmac.digest, b"k", b"m", "not_a_hash"),
    expect_exc_type(hmac.digest, b"k", b"m", "shake_128"),
)

print("hmac ok")
