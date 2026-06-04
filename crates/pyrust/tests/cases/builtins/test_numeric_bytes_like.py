# Issue #2077: int() and float() parse bytes and bytearray as ASCII numeric
# strings, identically to a str operand (whitespace, sign, PEP 515
# underscores, base, inf/nan), and raise ValueError (not TypeError) with the
# operand's repr in the message on malformed input.


def show(label, fn):
    try:
        print(label, "=>", repr(fn()))
    except Exception as e:
        print(label, "=>", type(e).__name__ + ":", e)


# --- int(bytes-like[, base]) ---
show("int(b'42')", lambda: int(b"42"))
show("int(bytearray(b'10'))", lambda: int(bytearray(b"10")))
show("int(b'10', 2)", lambda: int(b"10", 2))
show("int(bytearray(b'1010'), 2)", lambda: int(bytearray(b"1010"), 2))
show("int(b'1_0')", lambda: int(b"1_0"))
show("int(b'  10  ')", lambda: int(b"  10  "))
show("int(b'  -5  ')", lambda: int(b"  -5  "))
show("int(b'0xff', 16)", lambda: int(b"0xff", 16))
show("int(b'0b101', 0)", lambda: int(b"0b101", 0))
show("int(bytearray(b'0o17'), 0)", lambda: int(bytearray(b"0o17"), 0))
show("int(b'99999999999999999999')", lambda: int(b"99999999999999999999"))

# Malformed: ValueError with the bytes repr (int() always uses the b'…' repr,
# even for a bytearray operand).
show("int(b'xx')", lambda: int(b"xx"))
show("int(bytearray(b'zz'))", lambda: int(bytearray(b"zz")))
show("int(b'')", lambda: int(b""))
show("int(b'1.5')", lambda: int(b"1.5"))

# --- float(bytes-like) ---
show("float(b'3.14')", lambda: float(b"3.14"))
show("float(bytearray(b'2.5'))", lambda: float(bytearray(b"2.5")))
show("float(b'  1.5  ')", lambda: float(b"  1.5  "))
show("float(b'inf')", lambda: float(b"inf"))
show("float(bytearray(b'  nan  '))", lambda: float(bytearray(b"  nan  ")))
show("float(b'1_000.5')", lambda: float(b"1_000.5"))
show("float(b'1e3')", lambda: float(b"1e3"))

# Malformed: ValueError; float() uses the operand's own repr (b'…' vs
# bytearray(b'…')).
show("float(b'abc')", lambda: float(b"abc"))
show("float(bytearray(b'xx'))", lambda: float(bytearray(b"xx")))
show("float(b'')", lambda: float(b""))
