# Adjacent string literal implicit concatenation (CPython 3.12 semantics).
# All output must be ASCII only (Windows CI uses cp1252).

x = 42

# str + str
s = "hello " "world"
print(s)

# three str tokens
s2 = "a" "b" "c"
print(s2)

# fstr + str
s3 = f"value={x}" " end"
print(s3)

# str + fstr
s4 = "start " f"x={x}"
print(s4)

# fstr + fstr
s5 = f"x={x}" f" y={x + 1}"
print(s5)

# str + fstr + str
s6 = "pre" f"[{x}]" "post"
print(s6)

# fstr + fstr + fstr
s7 = f"a={x}" f" b={x + 1}" f" c={x + 2}"
print(s7)

# multiline in parentheses (common docstring/print pattern)
s8 = (
    "first "
    "second "
    f"third={x}"
)
print(s8)

# fstr with format spec adjacent to str
s9 = f"{x:.2f}" " done"
print(s9)

# empty string pieces
s10 = "" "hello" ""
print(s10)

# bytes + bytes
b1 = b"hello " b"world"
print(b1)

# three bytes tokens
b2 = b"a" b"b" b"c"
print(b2)

# in function calls (common real-world pattern)
print(
    "first line "
    "second line"
)

# result type check: str+fstr produces str
t = "hello " f"{x}"
print(type(t).__name__)

# result type check: fstr+str produces str
t2 = f"{x}" " world"
print(type(t2).__name__)
