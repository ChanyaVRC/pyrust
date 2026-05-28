# bytes.decode() and str.encode() argument count checks include kwargs (#1562).
#
# When positional + keyword argument totals exceed the 2-arg limit, pyrust must
# raise the same TypeError CPython 3.12 does instead of the duplicate-binding
# error triggered by the per-argument checks that follow.

# ── bytes.decode: 2 positional + 1 keyword (3 total) ───────────────────────
try:
    b"x".decode("utf-8", "strict", errors="strict")
    print("FAIL: should raise")
except TypeError as e:
    print("decode 2pos+1kw:", e)

# ── bytes.decode: 1 positional + 2 keywords (3 total) ──────────────────────
try:
    b"x".decode("utf-8", encoding="utf-8", errors="strict")
    print("FAIL: should raise")
except TypeError as e:
    print("decode 1pos+2kw:", e)

# ── bytes.decode: 0 positional + 3 keywords (3 total) ──────────────────────
try:
    b"x".decode(encoding="utf-8", errors="strict", bad="x")
    print("FAIL: should raise")
except TypeError as e:
    print("decode 0pos+3kw:", e)

# ── str.encode: 2 positional + 1 keyword (3 total) ─────────────────────────
try:
    "x".encode("utf-8", "strict", errors="strict")
    print("FAIL: should raise")
except TypeError as e:
    print("encode 2pos+1kw:", e)

# ── str.encode: 1 positional + 2 keywords (3 total) ────────────────────────
try:
    "x".encode("utf-8", encoding="utf-8", errors="strict")
    print("FAIL: should raise")
except TypeError as e:
    print("encode 1pos+2kw:", e)

# ── str.encode: 0 positional + 3 keywords (3 total) ────────────────────────
try:
    "x".encode(encoding="utf-8", errors="strict", bad="x")
    print("FAIL: should raise")
except TypeError as e:
    print("encode 0pos+3kw:", e)

# ── Valid combinations must still succeed ───────────────────────────────────
assert b"x".decode("utf-8", "strict") == "x"
assert b"x".decode(errors="strict") == "x"
assert b"x".decode("utf-8", errors="strict") == "x"
assert "x".encode("utf-8", "strict") == b"x"
assert "x".encode(errors="strict") == b"x"
assert "x".encode("utf-8", errors="strict") == b"x"

print("decode_encode_arg_count OK")
