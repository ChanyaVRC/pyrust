# str.encode() and bytes.decode() keyword argument support (#1342).
#
# CPython 3.12 signatures:
#   str.encode(encoding='utf-8', errors='strict')
#   bytes.decode(encoding='utf-8', errors='strict')
#
# Both parameters must accept keyword form; unknown keywords must still
# raise TypeError.

# ── str.encode() positional (baseline) ─────────────────────────────────
assert "hello".encode() == b"hello"
assert "hello".encode("utf-8") == b"hello"
assert "hello".encode("ascii") == b"hello"
assert "hello".encode("latin-1") == b"hello"

# ── str.encode() keyword-only ───────────────────────────────────────────
assert "hello".encode(encoding="utf-8") == b"hello"
assert "hello".encode(encoding="ascii") == b"hello"
assert "hello".encode(errors="strict") == b"hello"
assert "hello".encode(encoding="utf-8", errors="strict") == b"hello"
assert "hello".encode(encoding="ascii", errors="strict") == b"hello"

# ── str.encode() mixed positional + keyword ─────────────────────────────
assert "hello".encode("utf-8", errors="strict") == b"hello"
assert "hello".encode("ascii", errors="ignore") == b"hello"

# ── str.encode() error-handler kwargs ──────────────────────────────────
# Non-ASCII with errors=ignore or replace
try:
    "caf\xe9".encode(encoding="ascii", errors="strict")
    print("FAIL: should raise UnicodeEncodeError")
except UnicodeEncodeError as e:
    print("ascii strict kw:", str(e))

result_ignore = "caf\xe9".encode(encoding="ascii", errors="ignore")
assert result_ignore == b"caf", repr(result_ignore)

result_replace = "caf\xe9".encode(encoding="ascii", errors="replace")
assert result_replace == b"caf?", repr(result_replace)

# ── str.encode() unknown kwarg raises TypeError ─────────────────────────
try:
    "hello".encode(bad_kwarg="x")
    print("FAIL: unknown kwarg should raise")
except TypeError as e:
    print("unknown kwarg:", str(e))

# ── str.encode() duplicate positional + keyword raises TypeError ─────────
try:
    "hello".encode("utf-8", encoding="ascii")
    print("FAIL: duplicate encoding should raise")
except TypeError as e:
    print("dup encoding:", str(e))

# ── bytes.decode() positional (baseline) ───────────────────────────────
assert b"hello".decode() == "hello"
assert b"hello".decode("utf-8") == "hello"
assert b"hello".decode("ascii") == "hello"

# ── bytes.decode() keyword-only ────────────────────────────────────────
assert b"hello".decode(encoding="utf-8") == "hello"
assert b"hello".decode(encoding="ascii") == "hello"
assert b"hello".decode(errors="strict") == "hello"
assert b"hello".decode(encoding="utf-8", errors="strict") == "hello"

# ── bytes.decode() mixed positional + keyword ───────────────────────────
assert b"hello".decode("utf-8", errors="strict") == "hello"

# ── bytes.decode() error-handler kwargs ────────────────────────────────
bad = bytes([0xFF])
result_replace = bad.decode(encoding="utf-8", errors="replace")
# U+FFFD replacement character
assert len(result_replace) == 1 and ord(result_replace) == 0xFFFD, repr(result_replace)

result_ignore = bad.decode(encoding="utf-8", errors="ignore")
assert result_ignore == "", repr(result_ignore)

# ── bytes.decode() unknown kwarg raises TypeError ───────────────────────
try:
    b"hello".decode(bad="x")
    print("FAIL: unknown kwarg should raise")
except TypeError as e:
    print("decode unknown kwarg:", str(e))

print("encode_decode_kwargs OK")
