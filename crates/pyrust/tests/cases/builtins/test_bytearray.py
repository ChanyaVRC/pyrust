# Parity fixture for bytearray built-in type.
# Covers issues #1002, #1424 (constructor and mutation) and #1476 (six missing
# methods: partition, rpartition, swapcase, isascii, istitle, fromhex).

# ── Construction ────────────────────────────────────────────────────────────
print(bytearray())
print(bytearray(5))
print(bytearray(b"hello"))
print(bytearray([72, 101, 108, 108, 111]))
print(bytearray('hi', 'utf-8'))
print(bytearray(bytearray(b"clone me")))

# ── Type identity ────────────────────────────────────────────────────────────
print(type(bytearray()).__name__)
print(type(bytearray()) is bytearray)
print(isinstance(bytearray(), bytearray))
print(isinstance(b"bytes", bytearray))

# ── repr ─────────────────────────────────────────────────────────────────────
print(repr(bytearray()))
print(repr(bytearray(b"hello")))
print(repr(bytearray(b"\x00\xff\x0a")))

# ── Comparison ──────────────────────────────────────────────────────────────
print(bytearray(b"hello") == bytearray(b"hello"))
print(bytearray(b"hello") == b"hello")
print(b"hello" == bytearray(b"hello"))
print(bytearray(b"hello") == bytearray(b"world"))
print(bytearray(b"abc") < bytearray(b"abd"))

# ── len / bool / contains ────────────────────────────────────────────────────
print(len(bytearray()))
print(len(bytearray(b"hello")))
print(bool(bytearray()))
print(bool(bytearray(b"x")))
print(104 in bytearray(b"hello"))
print(200 in bytearray(b"hello"))

# ── Indexing and slicing ─────────────────────────────────────────────────────
ba = bytearray(b"hello")
print(ba[0])
print(ba[-1])
print(ba[1:3])
print(ba[::2])

# ── Iteration ────────────────────────────────────────────────────────────────
print(list(bytearray(b"abc")))
print([b for b in bytearray(b"hi")])

# ── Mutable item assignment ──────────────────────────────────────────────────
ba = bytearray(b"hello")
ba[0] = 72
print(ba)
ba[-1] = 33
print(ba)

# ── Slice assignment ─────────────────────────────────────────────────────────
ba = bytearray(b"hello world")
ba[0:5] = b"HELLO"
print(ba)

# ── Delete item and slice ────────────────────────────────────────────────────
ba = bytearray(b"hello")
del ba[0]
print(ba)
ba = bytearray(b"hello")
del ba[1:3]
print(ba)

# ── append, extend, insert, pop, remove, reverse, clear, copy ───────────────
ba = bytearray(b"hello")
ba.append(33)
print(ba)

ba = bytearray(b"hello")
ba.extend(b" world")
print(ba)

ba = bytearray(b"hllo")
ba.insert(1, 101)
print(ba)

ba = bytearray(b"hello")
print(ba.pop())
print(ba)

ba = bytearray(b"hello")
ba.remove(108)
print(ba)

ba = bytearray(b"hello")
ba.reverse()
print(ba)

ba = bytearray(b"hello")
ba.clear()
print(ba)

ba = bytearray(b"hello")
ba2 = ba.copy()
ba2[0] = 72
print(ba)
print(ba2)

# ── Shared read methods (same semantics as bytes) ────────────────────────────
ba = bytearray(b"Hello World")
print(ba.upper())
print(ba.lower())
print(ba.title())
print(ba.capitalize())
print(ba.swapcase())   # issue #1476
print(ba.replace(b"World", b"pyrust"))
print(ba.strip())
print(ba.lstrip(b"H"))
print(ba.rstrip(b"d"))
print(ba.split(b" "))
print(ba.rsplit(b" ", 1))
print(ba.startswith(b"Hello"))
print(ba.endswith(b"World"))
print(ba.find(b"World"))
print(ba.rfind(b"l"))
print(ba.index(b"World"))
print(ba.count(b"l"))
print(ba.center(15))
print(ba.ljust(15, b"-"))
print(ba.rjust(15, b"-"))
print(ba.zfill(15))
print(ba.hex())
print(ba.decode('ascii'))
print(bytearray(b"  hello  ").strip())
print(bytearray(b"hello\nworld").splitlines())
print(bytearray(b" ").join([b"a", b"b", b"c"]))

# ── isascii / istitle / isdigit / isalpha / isalnum / isspace / isupper / islower
print(bytearray(b"Hello World").isascii())   # True  — issue #1476
print(bytearray(b"\xff").isascii())           # False — issue #1476
print(bytearray(b"Hello World").istitle())    # True  — issue #1476
print(bytearray(b"hello").istitle())          # False — issue #1476
print(bytearray(b"123").isdigit())
print(bytearray(b"abc").isalpha())
print(bytearray(b"abc123").isalnum())
print(bytearray(b"HELLO").isupper())
print(bytearray(b"hello").islower())
print(bytearray(b"   ").isspace())

# ── partition / rpartition  (issue #1476) ────────────────────────────────────
ba = bytearray(b"Hello World")
print(ba.partition(b" "))
print(ba.rpartition(b" "))
ba2 = bytearray(b"nospace")
print(ba2.partition(b" "))
print(ba2.rpartition(b" "))

# ── fromhex  (issue #1476) ───────────────────────────────────────────────────
print(bytearray.fromhex("48656c6c6f"))
print(bytearray.fromhex("deadbeef"))
print(bytearray.fromhex("4865 6c6c 6f"))   # spaces allowed
print(bytearray(b"").fromhex("48656c6c6f"))  # instance call also works

# ── Reference semantics (mutable shared backing) ─────────────────────────────
a = bytearray(b"hello")
b = a
b[0] = 72
print(a)   # bytearray(b'Hello') — shared backing
print(b)
