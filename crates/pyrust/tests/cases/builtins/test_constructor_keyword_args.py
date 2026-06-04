# Issue #2180: int/round/complex/str/bytes/bytearray accept exactly the
# keyword arguments CPython 3.12 documents (and reject positional-only-as-kw).
import functools


def show(label, fn):
    try:
        print(label, "=>", repr(fn()))
    except Exception as e:
        print(label, "=>", type(e).__name__ + ":", e)


# --- int: x positional-only, base keyword-or-positional ---
show("int('10', base=2)", lambda: int("10", base=2))
show("int('10', base=0)", lambda: int("10", base=0))
show("int('ff', base=16)", lambda: int("ff", base=16))
show("int(x='5')", lambda: int(x="5"))            # positional-only → invalid kw
show("int(base=2)", lambda: int(base=2))          # missing string argument
show("int('10', foo=2)", lambda: int("10", foo=2))
show("int('10', 2, base=2)", lambda: int("10", 2, base=2))
show("partial(int, base=16)('ff')", lambda: functools.partial(int, base=16)("ff"))

# --- round: number and ndigits both keyword-or-positional ---
show("round(3.14, ndigits=1)", lambda: round(3.14, ndigits=1))
show("round(number=3.14)", lambda: round(number=3.14))
show("round(number=2.6, ndigits=0)", lambda: round(number=2.6, ndigits=0))
show("round(3.14, foo=1)", lambda: round(3.14, foo=1))
show("round(3.14, 1, ndigits=1)", lambda: round(3.14, 1, ndigits=1))

# --- complex: real and imag both keyword ---
show("complex(real=1, imag=2)", lambda: complex(real=1, imag=2))
show("complex(real=5)", lambda: complex(real=5))
show("complex(imag=5)", lambda: complex(imag=5))
show("complex(real='1+2j')", lambda: complex(real="1+2j"))
show("complex('1+2j', imag=1)", lambda: complex("1+2j", imag=1))
show("complex(1, real=2)", lambda: complex(1, real=2))
show("complex(1, 2, foo=3)", lambda: complex(1, 2, foo=3))
show("complex(real=1, imag=2, foo=3)", lambda: complex(real=1, imag=2, foo=3))

# --- str: object/encoding/errors keyword ---
show("str(object=5)", lambda: str(object=5))
show("str(b'x', encoding='utf-8')", lambda: str(b"x", encoding="utf-8"))
show("str(object=b'x', encoding='utf-8')", lambda: str(object=b"x", encoding="utf-8"))
show("str(encoding='utf-8')", lambda: str(encoding="utf-8"))
show("str(b'x', errors='strict')", lambda: str(b"x", errors="strict"))
show("str('x', object='y')", lambda: str("x", object="y"))

# --- bytes / bytearray: source/encoding/errors keyword ---
show("bytes(source=b'x')", lambda: bytes(source=b"x"))
show("bytes('ab', encoding='utf-8')", lambda: bytes("ab", encoding="utf-8"))
show("bytes(source='ab', encoding='utf-8')", lambda: bytes(source="ab", encoding="utf-8"))
show("bytes('ab', errors='strict')", lambda: bytes("ab", errors="strict"))
show("bytes(errors='strict')", lambda: bytes(errors="strict"))
show("bytes(encoding='utf-8')", lambda: bytes(encoding="utf-8"))
show("bytearray(source=b'x')", lambda: bytearray(source=b"x"))
show("bytearray('ab', encoding='utf-8')", lambda: bytearray("ab", encoding="utf-8"))

# --- float takes NO keyword arguments in 3.12 ---
show("float(x=1.5)", lambda: float(x=1.5))

# --- list/tuple stay keyword-free (must not regress) ---
show("list(iterable=[1])", lambda: list(iterable=[1]))
show("tuple(iterable=[1])", lambda: tuple(iterable=[1]))
