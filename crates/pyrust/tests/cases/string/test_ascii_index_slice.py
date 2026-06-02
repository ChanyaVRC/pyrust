# Tests for the ASCII fast path in str indexing / slicing / find / count (#2032).
# An all-ASCII string maps char index == byte index, so indexing/slicing take a
# direct-byte path; non-ASCII strings keep the UTF-8 char-counting path.  Both
# must stay byte-identical to CPython 3.12.

# --- ASCII: indexing ---
s = "abcdefghij"
print(s[0], s[3], s[-1], s[-10])
print(s[2:5], s[2:5:2], s[::-1], s[::2], s[1::3])
print(repr(s[-3:]), repr(s[:100]), repr(s[100:]), repr(s[-100:3]), repr(s[5:2]))
print(repr(""[:]), repr("x"[0]))

# --- ASCII: find / rfind / index / count ---
print(s.find("cd"), s.rfind("a"), s.index("e"))
print(s.find("z"), s.rfind("z"))
print("abababab".count("ab"), "abababab".find("ba", 1), "abababab".rfind("ab"))
print("aaa".count(""), "abc".count("", 1, 2))

# --- ASCII out-of-range index raises IndexError ---
try:
    s[100]
except IndexError as e:
    print("IndexError:", e)
try:
    s[-100]
except IndexError as e:
    print("IndexError:", e)

# --- non-ASCII: indexing must be by code point, not byte ---
u = "αβγδε"  # αβγδε
print(u[0], u[1], u[-1])
print(u[1:4], u[::-1], u[1:4:2], u[::2])
print(u.find("γ"), u.rfind("β"), u.count("β"))
print(u.count("", 1, 4))

# --- emoji (4-byte) ---
e = "a\U0001F600b\U0001F600c"
print(e[1], e[1:4], e.find("\U0001F600"), e.rfind("\U0001F600"), e.count("\U0001F600"))

# --- mixed: ASCII prefix then non-ASCII (byte_to_char_idx prefix is non-ASCII) ---
m = "abcαβγabc"
print(m.find("αβ"), m.rfind("abc"), m[3:6], m[2:7])

# --- non-ASCII out-of-range ---
try:
    u[100]
except IndexError as e:
    print("IndexError:", e)
