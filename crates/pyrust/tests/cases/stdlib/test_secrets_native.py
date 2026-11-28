"""Parity fixture for the 100% native `secrets` module (issue #3495).

Everything printed here must be byte-for-byte identical to CPython 3.12.3.
Random values are never printed: only types, lengths, bounds, character sets
and exact exception types / messages.
"""

import base64
import hmac
import random
import secrets
import sys


def run(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", type(res).__name__)
    except Exception as e:
        return (type(e).__name__, str(e))


# 1. Module metadata and re-exports
print("in sys.modules:", "secrets" in sys.modules)
print("__all__:", secrets.__all__)
for name in secrets.__all__:
    assert hasattr(secrets, name), name
print("DEFAULT_ENTROPY:", secrets.DEFAULT_ENTROPY)
print("SystemRandom is random.SystemRandom:", secrets.SystemRandom is random.SystemRandom)
print("compare_digest is hmac.compare_digest:", secrets.compare_digest is hmac.compare_digest)
print("_sysrand:", type(secrets._sysrand).__name__, isinstance(secrets._sysrand, random.SystemRandom))
print("base64 is base64:", secrets.base64 is base64)
print("randbits self:", secrets.randbits.__self__ is secrets._sysrand)
print("choice self:", secrets.choice.__self__ is secrets._sysrand)
print("names:", [n for n in vars(secrets) if not n.startswith("__")])
print("module name:", secrets.__name__)
print("function modules:", {f.__module__ for f in (secrets.randbelow, secrets.token_bytes, secrets.token_hex, secrets.token_urlsafe)})
print("function names:", [f.__name__ for f in (secrets.randbelow, secrets.token_bytes, secrets.token_hex, secrets.token_urlsafe)])

# 2. choice
seq = ["alpha", "beta", "gamma", "delta"]
assert all(secrets.choice(seq) in seq for _ in range(50))
assert secrets.choice(["only"]) == "only"
assert all(secrets.choice("abc") in "abc" for _ in range(30))
assert secrets.choice(range(5, 6)) == 5
print("choice empty list:", run(secrets.choice, []))
print("choice empty str:", run(secrets.choice, ""))
print("choice none:", run(secrets.choice, None))
print("choice int:", run(secrets.choice, 5))

# 3. randbelow
print("randbelow(1):", secrets.randbelow(1))
assert all(0 <= secrets.randbelow(100) < 100 for _ in range(200))
assert {secrets.randbelow(2) for _ in range(200)} == {0, 1}
big = 1 << 100
r_big = secrets.randbelow(big)
assert type(r_big) is int and 0 <= r_big < big
print("randbelow(0):", run(secrets.randbelow, 0))
print("randbelow(-5):", run(secrets.randbelow, -5))
print("randbelow(-big):", run(secrets.randbelow, -big))
print("randbelow(False):", run(secrets.randbelow, False))
print("randbelow(True):", secrets.randbelow(True))
print("randbelow('5'):", run(secrets.randbelow, "5"))
print("randbelow(None):", run(secrets.randbelow, None))
print("randbelow(2.5):", run(secrets.randbelow, 2.5))
print("randbelow(-2.5):", run(secrets.randbelow, -2.5))
print("randbelow():", run(secrets.randbelow))
print("randbelow(1, 2):", run(secrets.randbelow, 1, 2))
print("randbelow(kw):", secrets.randbelow(exclusive_upper_bound=1))
print("randbelow(bad kw):", run(secrets.randbelow, n=3))

# 4. randbits
print("randbits(0):", secrets.randbits(0))
assert all(secrets.randbits(1) in (0, 1) for _ in range(40))
assert all(0 <= secrets.randbits(16) < (1 << 16) for _ in range(100))
r128 = secrets.randbits(128)
assert type(r128) is int and 0 <= r128 < (1 << 128)
assert any(secrets.randbits(64) >= (1 << 63) for _ in range(200))
print("randbits(-1):", run(secrets.randbits, -1))
print("randbits('1'):", run(secrets.randbits, "1"))
print("randbits(1.5):", run(secrets.randbits, 1.5))

# 5. token_bytes
tb = secrets.token_bytes()
print("token_bytes default:", type(tb).__name__, len(tb))
print("token_bytes None:", len(secrets.token_bytes(None)))
print("token_bytes 0:", secrets.token_bytes(0))
for n in (1, 16, 32, 64, 1000):
    tok = secrets.token_bytes(n)
    assert type(tok) is bytes and len(tok) == n
assert secrets.token_bytes(32) != secrets.token_bytes(32)
print("token_bytes kw:", len(secrets.token_bytes(nbytes=7)))
print("token_bytes(-1):", run(secrets.token_bytes, -1))
print("token_bytes('4'):", run(secrets.token_bytes, "4"))
print("token_bytes(2.0):", run(secrets.token_bytes, 2.0))
print("token_bytes(True):", len(secrets.token_bytes(True)))
print("token_bytes(1, 2):", run(secrets.token_bytes, 1, 2))
print("token_bytes(bad kw):", run(secrets.token_bytes, n=1))

# 6. token_hex
hex_chars = set("0123456789abcdef")
th = secrets.token_hex()
print("token_hex default:", type(th).__name__, len(th), set(th) <= hex_chars)
print("token_hex None:", len(secrets.token_hex(None)))
print("token_hex 0:", repr(secrets.token_hex(0)))
for n in (1, 16, 32, 48):
    tok = secrets.token_hex(n)
    assert type(tok) is str and len(tok) == 2 * n and set(tok) <= hex_chars
    assert bytes.fromhex(tok) and len(bytes.fromhex(tok)) == n
assert secrets.token_hex(32) != secrets.token_hex(32)
print("token_hex(-1):", run(secrets.token_hex, -1))
print("token_hex('4'):", run(secrets.token_hex, "4"))
print("token_hex(1, 2):", run(secrets.token_hex, 1, 2))

# 7. token_urlsafe
urlsafe_chars = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_")
tu = secrets.token_urlsafe()
print("token_urlsafe default:", type(tu).__name__, len(tu), "=" not in tu, set(tu) <= urlsafe_chars)
print("token_urlsafe None:", len(secrets.token_urlsafe(None)))
print("token_urlsafe 0:", repr(secrets.token_urlsafe(0)))
for n in (1, 2, 3, 4, 16, 32, 100):
    tok = secrets.token_urlsafe(n)
    assert type(tok) is str and "=" not in tok and set(tok) <= urlsafe_chars
    assert len(tok) == (n * 4 + 2) // 3
    pad = "=" * ((4 - len(tok) % 4) % 4)
    assert len(base64.urlsafe_b64decode(tok + pad)) == n
assert secrets.token_urlsafe(32) != secrets.token_urlsafe(32)
print("token_urlsafe(-1):", run(secrets.token_urlsafe, -1))
print("token_urlsafe('4'):", run(secrets.token_urlsafe, "4"))

# 8. compare_digest
print(
    "compare_digest:",
    secrets.compare_digest(b"token123", b"token123"),
    secrets.compare_digest(b"token123", b"token124"),
    secrets.compare_digest("token123", "token123"),
    secrets.compare_digest("token123", "token124"),
    secrets.compare_digest(b"", b""),
    secrets.compare_digest(b"ab", b"abc"),
    secrets.compare_digest(bytearray(b"ab"), b"ab"),
)
print("compare_digest non-ascii:", run(secrets.compare_digest, "caf\xe9", "caf\xe9"))
print("compare_digest mixed:", run(secrets.compare_digest, "abc", b"abc"))
print("compare_digest int:", run(secrets.compare_digest, 1, 2))

# 9. Globals are resolved at call time
saved = secrets.DEFAULT_ENTROPY
secrets.DEFAULT_ENTROPY = 5
print("patched DEFAULT_ENTROPY:", len(secrets.token_bytes()), len(secrets.token_hex()), len(secrets.token_urlsafe()))
secrets.DEFAULT_ENTROPY = 0
print("zero DEFAULT_ENTROPY:", secrets.token_bytes(), repr(secrets.token_hex()), repr(secrets.token_urlsafe()))
secrets.DEFAULT_ENTROPY = saved
print("restored DEFAULT_ENTROPY:", len(secrets.token_bytes()))


class Fixed(random.SystemRandom):
    def randbytes(self, n):
        return b"\xfb\xff" * (n // 2) + b"\xfb" * (n % 2)

    def _randbelow(self, n):
        return n - 1


saved_sysrand = secrets._sysrand
secrets._sysrand = Fixed()
print("patched _sysrand:", secrets.token_bytes(4), secrets.token_hex(3), secrets.token_urlsafe(4), secrets.randbelow(10))
secrets._sysrand = saved_sysrand

saved_tb = secrets.token_bytes
secrets.token_bytes = lambda nbytes=None: b"\x01\x02\x03" if nbytes is None else b"\xff" * nbytes
print("patched token_bytes:", secrets.token_hex(), secrets.token_hex(2), secrets.token_urlsafe(), secrets.token_urlsafe(2))
secrets.token_bytes = saved_tb
print("restored token_bytes:", len(secrets.token_hex(4)))

# 10. Independent of the global `random` generator state
random.seed(1234)
a = secrets.token_bytes(16)
random.seed(1234)
b = secrets.token_bytes(16)
print("seed independent:", a != b)
state = random.getstate()
secrets.randbelow(1000)
secrets.token_hex(8)
print("random state untouched:", random.getstate() == state)

print("secrets native ok")
