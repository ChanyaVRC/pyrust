# f-string lowering via the BuildString join + FormatValue fast path (#1926).
# Exercises every interpolation form to confirm the dedicated opcodes stay
# byte-identical to the previous format()-call + chained-Add lowering.

# Plain literals only (no interpolation): BuildString over literal parts.
assert f"" == ""
assert f"no interp" == "no interp"

# Single interpolation (n == 1 join, returned directly).
i = 42
assert f"{i}" == "42"

# Multiple interpolations + literals: the common BuildString case.
assert f"value={i} squared={i * i}" == "value=42 squared=1764"
assert f"a={i} b={i + 1} c={i + 2}" == "a=42 b=43 c=44"

# FormatValue default conversion on every primitive kind.
assert f"{3.14159}" == "3.14159"
assert f"{True} {False} {None}" == "True False None"
assert f"{[1, 2, 3]}" == "[1, 2, 3]"
assert f"{(1, 2)}" == "(1, 2)"
assert f"{ {'a': 1} }" == "{'a': 1}"
assert f"{b'hi'}" == "b'hi'"

# Conversion flags keep their repr/str/ascii dispatch, then the (now empty)
# spec is rendered through FormatValue.
assert f"{i!r}" == "42"
assert f"{'hi'!s}" == "hi"
assert f"{'héllo'!a}" == "'h\\xe9llo'"

# Explicit / nested format specs stay on the call-based lowering.
x = 3.14159
assert f"{x:>10.3f}" == "     3.142"
w = 8
assert f"{x:{w}.2f}" == "    3.14"
assert f"{255:#x}" == "0xff"
assert f"{42:08d}" == "00000042"

# Debug form f"{x=}" — literal prefix + repr.
assert f"{i=}" == "i=42"
assert f"{i * 2=}" == "i * 2=84"

# Multibyte / non-ASCII parts: byte-length sizing must use bytes, not chars.
s = "héllo"
assert f"café {i} naïve {x:.1f} 日本語" == "café 42 naïve 3.1 日本語"
assert f"[{s}][{s}][{s}]" == "[héllo][héllo][héllo]"

# User __format__ / __str__ / __repr__ dispatch is preserved (FormatValue
# delegates PyInstance operands to the real format builtin).
class C:
    def __format__(self, spec):
        return f"FMT<{spec}>"

    def __str__(self):
        return "STR"

    def __repr__(self):
        return "REPR"


c = C()
assert f"{c}" == "FMT<>"
assert f"{c:abc}" == "FMT<abc>"
assert f"{c!s}" == "STR"
assert f"{c!r}" == "REPR"
assert f"{c=}" == "c=REPR"


# Pure user class with no __format__: empty spec delegates to __str__.
class Greeter:
    def __str__(self):
        return "hi there"


assert f"{Greeter()}" == "hi there"


# __format__ returning a non-str raises TypeError.
class Bad:
    def __format__(self, spec):
        return 42


try:
    f"{Bad()}"
    raised = False
except TypeError:
    raised = True
assert raised, "expected TypeError from non-str __format__"

print("fstring buildstring OK")
