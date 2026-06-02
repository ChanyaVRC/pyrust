# bytes/bytearray % args — PEP 461 printf-style formatting (#1883)


def show(fn):
    try:
        print(repr(fn()))
    except Exception as e:
        print(type(e).__name__, str(e))


# Issue repro rows
show(lambda: b"%d-%s" % (5, b"x"))
show(lambda: b"%d" % 42)
show(lambda: b"%s" % b"hi")
show(lambda: b"%b" % b"raw")
show(lambda: b"%x" % 255)
show(lambda: b"%c" % 65)
show(lambda: b"%5.2f" % 3.14159)
show(lambda: b"%(k)s" % {b"k": b"v"})

# Numeric / float conversions
show(lambda: b"%i" % 7)
show(lambda: b"%u" % 7)
show(lambda: b"%o" % 64)
show(lambda: b"%X" % 255)
show(lambda: b"%e" % 1234.5)
show(lambda: b"%E" % 1234.5)
show(lambda: b"%f" % 3.14159)
show(lambda: b"%F" % 3.14159)
show(lambda: b"%g" % 0.0001)
show(lambda: b"%G" % 1e20)

# %b / %s accept bytes-like (bytes, bytearray) and __bytes__ objects
show(lambda: b"%b" % bytearray(b"ba"))
show(lambda: b"%s" % bytearray(b"ba"))


class HasBytes:
    def __bytes__(self):
        return b"custom"


show(lambda: b"%s" % HasBytes())
show(lambda: b"%b" % HasBytes())

# %a — ascii repr encoded to bytes
show(lambda: b"%a" % "caf\xe9")
show(lambda: b"%a" % b"raw")
show(lambda: b"%a" % 42)
show(lambda: b"%a" % [1, 2])

# %c — int in range(256) or a single byte
show(lambda: b"%c" % 65)
show(lambda: b"%c" % 200)
show(lambda: b"%c" % 0)
show(lambda: b"%c" % b"A")
show(lambda: b"%c" % bytes([65]))

# Flags / width / precision
show(lambda: b"%-10s|" % b"hi")
show(lambda: b"%05d" % 42)
show(lambda: b"%+d" % 42)
show(lambda: b"% d" % 42)
show(lambda: b"%#x" % 255)
show(lambda: b"%#o" % 64)
show(lambda: b"%.3s" % b"hello")
show(lambda: b"%10.2f" % 3.14159)
show(lambda: b"%-5c|" % 65)
show(lambda: b"%+.2e" % 12.5)

# Star width / precision
show(lambda: b"%*d" % (5, 42))
show(lambda: b"%.*f" % (3, 3.14159))

# Literal percent
show(lambda: b"100%%")

# Result type: bytearray % args -> bytearray; bytes -> bytes
ba = bytearray(b"%d") % 5
print(type(ba).__name__, repr(ba))
bb = b"%s" % b"hi"
print(type(bb).__name__, repr(bb))

# Error cases
show(lambda: b"%s" % "x")  # str rejected (PEP 461), message names %b
show(lambda: b"%b" % "x")
show(lambda: b"%c" % 256)  # OverflowError
show(lambda: b"%c" % -1)  # OverflowError
show(lambda: b"%c" % b"AB")  # multi-byte TypeError
show(lambda: b"%c" % "A")  # str rejected
show(lambda: b"%d %d" % (1,))  # not enough arguments
show(lambda: b"%d" % (1, 2))  # not all arguments converted
show(lambda: b"%(z)s" % {b"k": b"v"})  # KeyError with bytes key
show(lambda: b"%w" % 1)  # unsupported conversion
