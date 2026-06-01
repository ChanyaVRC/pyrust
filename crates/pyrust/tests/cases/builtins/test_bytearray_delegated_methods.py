# Exercises the bytearray methods that delegate to the shared bytes
# implementation and wrap the result (collapsed into multi-pattern match
# arms in bytearray.rs). Covers happy paths, not-found, negative/limit args,
# and empty inputs to guard the dedup against behavioural drift.

# --- methods returning a new bytearray ---
print(bytearray(b"hello world").replace(b"o", b"0"))
print(bytearray(b"aaa").replace(b"a", b"bb", 2))
print(bytearray(b"hello").replace(b"x", b"y"))

print(bytearray(b"  hi  ").strip())
print(bytearray(b"xxhixx").strip(b"x"))
print(bytearray(b"").strip())
print(bytearray(b"  hi  ").lstrip())
print(bytearray(b"xxhi").lstrip(b"x"))
print(bytearray(b"  hi  ").rstrip())
print(bytearray(b"hixx").rstrip(b"x"))

print(bytearray(b"foobar").removeprefix(b"foo"))
print(bytearray(b"foobar").removeprefix(b"xyz"))
print(bytearray(b"foobar").removesuffix(b"bar"))
print(bytearray(b"foobar").removesuffix(b"xyz"))

print(bytearray(b"hi").center(8))
print(bytearray(b"hi").center(8, b"*"))
print(bytearray(b"hi").center(1))
print(bytearray(b"hi").ljust(6, b"-"))
print(bytearray(b"hi").rjust(6, b"-"))

print(bytearray(b"42").zfill(5))
print(bytearray(b"-42").zfill(5))
print(bytearray(b"42").zfill(1))

print(bytearray(b"hello").translate(bytes.maketrans(b"el", b"ip")))
print(bytearray(b"hello").translate(None, b"l"))

print(bytearray(b"a\tbc\tdef").expandtabs())
print(bytearray(b"a\tbc\tdef").expandtabs(4))

# verify the wrapped results are genuine bytearrays (mutable)
r = bytearray(b"abc").replace(b"a", b"x")
print(type(r).__name__)
r.append(0x21)
print(r)

# --- methods returning a list of bytearray ---
print(bytearray(b"a b c").split())
print(bytearray(b"a,b,c").split(b","))
print(bytearray(b"a,b,c").split(b",", 1))
print(bytearray(b"").split())
print(bytearray(b"a b c").rsplit())
print(bytearray(b"a,b,c").rsplit(b",", 1))
print(bytearray(b"a\nb\nc").splitlines())
print(bytearray(b"a\nb\n").splitlines(True))
print(bytearray(b"a\r\nb\rc").splitlines())

parts = bytearray(b"a,b,c").split(b",")
print([type(p).__name__ for p in parts])
parts[0].append(0x21)
print(parts[0])
