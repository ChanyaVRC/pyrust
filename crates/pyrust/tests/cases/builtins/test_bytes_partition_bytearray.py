# bytes.partition / bytes.rpartition echo the *original* separator object as the
# middle element of the result tuple (#2680). When the separator is a bytearray
# (or bytes subclass), the matched middle keeps that type and identity; the outer
# parts stay plain bytes. A no-match leaves an empty bytes middle.

sep = bytearray(b'b')

# Match: middle element is the bytearray separator (same object).
r = b'abc'.partition(sep)
print(r)
print(type(r[1]).__name__)
print(r[1] is sep)

# No-match: all three parts are plain bytes (middle is b'', not bytearray).
r2 = b'abc'.partition(bytearray(b'z'))
print(r2)
print([type(x).__name__ for x in r2])

# rpartition: matches the last occurrence; middle echoes the bytearray.
r3 = b'abcbc'.rpartition(sep)
print(r3)
print(type(r3[1]).__name__)
print(r3[1] is sep)

# rpartition no-match: all bytes.
r4 = b'abc'.rpartition(bytearray(b'z'))
print(r4)
print([type(x).__name__ for x in r4])

# Plain bytes separator still returns a bytes middle.
r5 = b'abc'.partition(b'b')
print(r5)
print(type(r5[1]).__name__)


# bytes subclass separator keeps its own subclass type and identity.
class B(bytes):
    pass


bs = B(b'b')
r6 = b'abc'.partition(bs)
print(r6)
print([type(x).__name__ for x in r6])
print(r6[1] is bs)

# bytearray receiver: every part is bytearray regardless of separator type.
ba = bytearray(b'abc')
r7 = ba.partition(bytearray(b'b'))
print(r7)
print([type(x).__name__ for x in r7])
