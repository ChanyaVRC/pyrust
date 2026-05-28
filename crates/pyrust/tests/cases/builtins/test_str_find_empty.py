# Parity fixture for str.find/rfind/index/rindex with empty needle and
# start/end arguments beyond the string length.
#
# CPython 3.12 semantics: when start > len(s), the search window is empty
# (inverted) and the empty needle is "not found".  The boundary start==len(s)
# is still "found" at position len(s).

s = "hello"  # len 5, ASCII

# ── find / rfind (return -1 on miss) ──────────────────────────────────────────

# Boundary: start == len(s) — found at len(s)
print(s.find("", 5))   # 5
print(s.rfind("", 5))  # 5

# Past end: start > len(s) — not found
print(s.find("", 6))   # -1
print(s.find("", 10))  # -1
print(s.rfind("", 6))  # -1
print(s.rfind("", 10)) # -1

# ── index / rindex (raise ValueError on miss) ─────────────────────────────────

try:
    s.index("", 6)
    print("no error")
except ValueError:
    print("ValueError")

try:
    s.rindex("", 6)
    print("no error")
except ValueError:
    print("ValueError")

# ── count (return 0 on miss) ──────────────────────────────────────────────────

print(s.count("", 6))  # 0 (window is empty)
print(s.count("", 5))  # 1 (boundary: one empty match at position 5)
print(s.count("", 0))  # 6 (len + 1)

# ── negative start that normalises to a valid index still works ───────────────

print(s.find("h", -100))  # 0  (clamps to 0)
print(s.find("h", -5))    # 0

# ── end beyond len is clamped, does not affect start-vs-end comparison ────────

print(s.find("o", 0, 100))   # 4  (end clamped to 5)
print(s.find("o", 5, 100))   # -1 (start == len, no 'o' in empty tail)
print(s.find("o", 6, 100))   # -1 (start > len)

# ── Unicode (non-ASCII): same semantics, different code path ──────────────────

u = "helo"   # 4 Unicode chars; é is 2 bytes in UTF-8
print(len(u))          # 4
print(u.find("", 4))   # 4
print(u.find("", 5))   # -1
print(u.rfind("", 5))  # -1

try:
    u.index("", 5)
    print("no error")
except ValueError:
    print("ValueError")

try:
    u.rindex("", 5)
    print("no error")
except ValueError:
    print("ValueError")
