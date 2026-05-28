# Parity fixture: None displays as "None" (not "NoneType") in TypeError messages
# for bytes.decode(), str.encode(), and str.replace() argument type errors.
#
# CPython 3.12 uses the singleton display name "None" rather than the class name
# "NoneType" in these specific "argument '<param>' must be str, not <name>"
# messages. Other builtins (abs, sorted, etc.) continue to use "NoneType".
#
# See issue #1561.

def capture(fn):
    try:
        fn()
        return "no error"
    except TypeError as e:
        return str(e)

# --- bytes.decode() positional encoding ---
msg = capture(lambda: b"x".decode(None))
assert msg == "decode() argument 'encoding' must be str, not None", repr(msg)
print(msg)

# --- bytes.decode() keyword encoding ---
msg = capture(lambda: b"x".decode(encoding=None))
assert msg == "decode() argument 'encoding' must be str, not None", repr(msg)
print(msg)

# --- bytes.decode() positional errors ---
msg = capture(lambda: b"x".decode("utf-8", None))
assert msg == "decode() argument 'errors' must be str, not None", repr(msg)
print(msg)

# --- bytes.decode() keyword errors ---
msg = capture(lambda: b"x".decode(errors=None))
assert msg == "decode() argument 'errors' must be str, not None", repr(msg)
print(msg)

# --- str.encode() positional encoding ---
msg = capture(lambda: "x".encode(None))
assert msg == "encode() argument 'encoding' must be str, not None", repr(msg)
print(msg)

# --- str.encode() keyword encoding ---
msg = capture(lambda: "x".encode(encoding=None))
assert msg == "encode() argument 'encoding' must be str, not None", repr(msg)
print(msg)

# --- str.encode() positional errors ---
msg = capture(lambda: "x".encode("utf-8", None))
assert msg == "encode() argument 'errors' must be str, not None", repr(msg)
print(msg)

# --- str.encode() keyword errors ---
msg = capture(lambda: "x".encode(errors=None))
assert msg == "encode() argument 'errors' must be str, not None", repr(msg)
print(msg)

# --- str.replace() argument 1 ---
msg = capture(lambda: "x".replace(None, "a"))
assert msg == "replace() argument 1 must be str, not None", repr(msg)
print(msg)

# --- str.replace() argument 2 ---
msg = capture(lambda: "x".replace("a", None))
assert msg == "replace() argument 2 must be str, not None", repr(msg)
print(msg)

# --- str.removeprefix() / removesuffix() ---
msg = capture(lambda: "x".removeprefix(None))
assert msg == "removeprefix() argument must be str, not None", repr(msg)
print(msg)

msg = capture(lambda: "x".removesuffix(None))
assert msg == "removesuffix() argument must be str, not None", repr(msg)
print(msg)

# --- str.maketrans() argument 2 and 3 ---
msg = capture(lambda: str.maketrans("a", None))
assert msg == "maketrans() argument 2 must be str, not None", repr(msg)
print(msg)

msg = capture(lambda: str.maketrans("a", "b", None))
assert msg == "maketrans() argument 3 must be str, not None", repr(msg)
print(msg)

# --- Non-None arguments still use the class name ---
msg = capture(lambda: b"x".decode(42))
assert "not int" in msg, repr(msg)

msg = capture(lambda: "x".encode(42))
assert "not int" in msg, repr(msg)

msg = capture(lambda: "x".replace(42, "a"))
assert "not int" in msg, repr(msg)

msg = capture(lambda: "x".removeprefix(42))
assert "not int" in msg, repr(msg)

msg = capture(lambda: str.maketrans("a", 42))
assert "not int" in msg, repr(msg)

# --- Other builtins still use "NoneType" for None ---
msg = capture(lambda: abs(None))
assert "NoneType" in msg, repr(msg)

msg = capture(lambda: sorted(None))
assert "NoneType" in msg, repr(msg)

# --- type(None).__name__ is still "NoneType" (type system unchanged) ---
assert type(None).__name__ == "NoneType"
