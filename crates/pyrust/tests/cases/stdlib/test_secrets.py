import base64
import hmac
import random
import secrets
import sys


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


# 1. Module metadata, __all__, constants, and identity
print("in sys.modules:", "secrets" in sys.modules)
print("__all__:", secrets.__all__)
for name in secrets.__all__:
    assert hasattr(secrets, name), name
print("DEFAULT_ENTROPY:", secrets.DEFAULT_ENTROPY)
print("SystemRandom is random.SystemRandom:", secrets.SystemRandom is random.SystemRandom)
print("compare_digest is hmac.compare_digest:", secrets.compare_digest is hmac.compare_digest)
print("SystemRandom issubclass Random:", issubclass(secrets.SystemRandom, random.Random))


# 2. token_bytes
tb_default = secrets.token_bytes()
print("token_bytes default:", type(tb_default).__name__, len(tb_default))
print("token_bytes None:", len(secrets.token_bytes(None)))
print("token_bytes 0:", secrets.token_bytes(0))
for n in (1, 16, 32, 64):
    tok = secrets.token_bytes(n)
    assert isinstance(tok, bytes) and len(tok) == n
# Two 32-byte tokens should not collide
assert secrets.token_bytes(32) != secrets.token_bytes(32)
print("token_bytes ok")


# 3. token_hex
hex_chars = set("0123456789abcdef")
th_default = secrets.token_hex()
print("token_hex default:", type(th_default).__name__, len(th_default), set(th_default) <= hex_chars)
print("token_hex None:", len(secrets.token_hex(None)))
print("token_hex 0:", repr(secrets.token_hex(0)))
for n in (1, 16, 32, 48):
    tok = secrets.token_hex(n)
    assert isinstance(tok, str) and len(tok) == 2 * n and set(tok) <= hex_chars
assert secrets.token_hex(32) != secrets.token_hex(32)
print("token_hex ok")


# 4. token_urlsafe
urlsafe_chars = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_")
tu_default = secrets.token_urlsafe()
print(
    "token_urlsafe default:",
    type(tu_default).__name__,
    len(tu_default),
    "=" not in tu_default,
    set(tu_default) <= urlsafe_chars,
)
print("token_urlsafe 0:", repr(secrets.token_urlsafe(0)))
for n in (1, 2, 3, 16, 32):
    tok = secrets.token_urlsafe(n)
    assert isinstance(tok, str) and "=" not in tok and set(tok) <= urlsafe_chars
    # Re-pad and verify urlsafe_b64decode recovers n bytes
    pad = "=" * ((4 - len(tok) % 4) % 4)
    decoded = base64.urlsafe_b64decode(tok + pad)
    assert len(decoded) == n
assert secrets.token_urlsafe(32) != secrets.token_urlsafe(32)
print("token_urlsafe ok")


# 5. choice, randbelow, randbits
seq = ["alpha", "beta", "gamma", "delta"]
assert all(secrets.choice(seq) in seq for _ in range(50))
assert secrets.choice(["only"]) == "only"
assert all(secrets.choice("abc") in "abc" for _ in range(30))
print("choice ok")

print("randbelow(1):", secrets.randbelow(1))
assert all(0 <= secrets.randbelow(10) < 10 for _ in range(100))
big_bound = 1 << 100
r_big = secrets.randbelow(big_bound)
assert isinstance(r_big, int) and 0 <= r_big < big_bound
print("randbelow ok")

print("randbits(0):", secrets.randbits(0))
assert all(0 <= secrets.randbits(1) < 2 for _ in range(20))
assert all(0 <= secrets.randbits(8) < 256 for _ in range(50))
r_128 = secrets.randbits(128)
assert isinstance(r_128, int) and 0 <= r_128 < (1 << 128)
print("randbits ok")


# 6. compare_digest via secrets
print(
    "compare_digest:",
    secrets.compare_digest(b"token123", b"token123"),
    secrets.compare_digest(b"token123", b"token124"),
    secrets.compare_digest("token123", "token123"),
    secrets.compare_digest("token123", "token124"),
)


# 7. random.SystemRandom behavior
sr = secrets.SystemRandom()
assert sr.seed(123) is None
assert all(0.0 <= sr.random() < 1.0 for _ in range(50))
assert all(1 <= sr.randint(1, 10) <= 10 for _ in range(50))
assert len(sr.randbytes(16)) == 16
assert sr.randbytes(0) == b""
sample = sr.sample(range(20), 5)
assert len(sample) == 5 and len(set(sample)) == 5
items = [1, 2, 3, 4, 5]
sr.shuffle(items)
assert sorted(items) == [1, 2, 3, 4, 5]
print("SystemRandom ok")


# 8. Error parity
print(
    "errors:",
    expect_exc(secrets.randbelow, 0),
    expect_exc(secrets.randbelow, -5),
    expect_exc(secrets.randbits, -1),
    expect_exc(secrets.token_bytes, -1),
    expect_exc(secrets.token_hex, -1),
    expect_exc(secrets.token_urlsafe, -1),
    expect_exc(secrets.choice, []),
    expect_exc(sr.getstate),
    expect_exc(sr.setstate, None),
)

print("secrets ok")
