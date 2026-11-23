# Issue #2973: bytearray self-aliasing `+=` / `__iadd__` raises BufferError
# when a resize is required (non-empty buffer), while empty self `+=`,
# distinct buffers, `.extend(ba)`, slice self-assignment, and live `iter(ba)`
# iteration all keep CPython 3.12.3 semantics.

# 1. Non-empty `ba += ba` raises BufferError and leaves `ba` untouched.
ba = bytearray(b"ab")
try:
    ba += ba
    print("self_iadd_nonempty: UNEXPECTED")
except BufferError as exc:
    print("self_iadd_nonempty:", type(exc).__name__, str(exc), bytes(ba))

# 2. Direct `ba.__iadd__(ba)` and `bytearray.__iadd__(ba, ba)` raise BufferError.
ba2 = bytearray(b"xy")
for label, call in (
    ("bound_iadd", lambda: ba2.__iadd__(ba2)),
    ("unbound_iadd", lambda: bytearray.__iadd__(ba2, ba2)),
):
    try:
        call()
        print(label, "UNEXPECTED")
    except BufferError as exc:
        print(label, type(exc).__name__, str(exc), bytes(ba2))

# 3. Empty `ba += ba` requires no resize: succeeds and preserves identity.
empty = bytearray()
orig_empty = empty
empty += empty
print("self_iadd_empty:", empty is orig_empty, bytes(empty))
empty_ret = empty.__iadd__(empty)
print("self_iadd_empty_dunder:", empty_ret is orig_empty, bytes(empty))


# 4. Subclass `b += b`: non-empty raises BufferError, empty succeeds and
#    preserves subclass identity.
class BSub(bytearray):
    pass


sub = BSub(b"ab")
try:
    sub += sub
    print("sub_iadd_nonempty: UNEXPECTED")
except BufferError as exc:
    print("sub_iadd_nonempty:", type(exc).__name__, str(exc), bytes(sub))

sub_empty = BSub()
orig_sub_empty = sub_empty
sub_empty += sub_empty
print("sub_iadd_empty:", sub_empty is orig_sub_empty, type(sub_empty) is BSub, bytes(sub_empty))

# 5. Distinct bytearray / bytes operands still succeed.
d1 = bytearray(b"ab")
d1 += bytearray(d1)
print("distinct_ba:", bytes(d1))

d2 = bytearray(b"ab")
d2 += bytes(d2)
print("distinct_bytes:", bytes(d2))

# 6. `ba.extend(ba)` and slice self-assignments still succeed.
ext = bytearray(b"ab")
ext.extend(ext)
print("extend_self:", bytes(ext))

s1 = bytearray(b"ab")
s1[0:0] = s1
print("slice_prefix:", bytes(s1))

s2 = bytearray(b"ab")
s2[:] = s2
print("slice_all:", bytes(s2))

s3 = bytearray(b"ab")
s3[1:] = s3
print("slice_tail:", bytes(s3))

# 7. `for x in iter(ba)` exercises IterKind::BuiltinIterator live cursor.
live = bytearray(b"ab")
seen = []
for byte in iter(live):
    seen.append(byte)
    if byte == ord("b") and len(live) == 2:
        live.append(ord("c"))
print("live_iter:", seen, bytes(live))
