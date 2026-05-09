# Layout A -> B -> B chain: slice of slice of slice must not corrupt pointer arithmetic
s = "hello world"
a = s[2:9]       # "llo wor"
assert a == "llo wor", repr(a)
b = a[1:5]       # "lo w"
assert b == "lo w", repr(b)
c = b[1:3]       # "o "
assert c == "o ", repr(c)

# Zero-length slices must survive
empty = s[3:3]
assert empty == ""
empty2 = a[0:0]
assert empty2 == ""

# Slice at boundary
last = s[10:]
assert last == "d"
first = s[:1]
assert first == "h"

# Unicode: byte offsets must align with char boundaries
u = "こんにちは"
u1 = u[1:3]   # "んに"
assert u1 == "んに", repr(u1)
u2 = u1[0:1]  # "ん"
assert u2 == "ん", repr(u2)

print("str slice safety OK")
