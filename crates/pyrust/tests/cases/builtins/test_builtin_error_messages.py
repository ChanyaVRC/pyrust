# Parity fixture for CPython 3.12 error message wording in divmod(), ord(), chr().
# Issue #1339: three error messages diverged from CPython's exact wording.

# ── divmod() arity errors ─────────────────────────────────────────────────────

try:
    divmod()
except TypeError as e:
    print(e)  # divmod expected 2 arguments, got 0

try:
    divmod(1)
except TypeError as e:
    print(e)  # divmod expected 2 arguments, got 1

try:
    divmod(1, 2, 3)
except TypeError as e:
    print(e)  # divmod expected 2 arguments, got 3

# ── divmod() happy path (regression guard) ────────────────────────────────────

print(divmod(10, 3))    # (3, 1)
print(divmod(10.5, 3))  # (3.0, 1.5)

# ── ord() type errors ─────────────────────────────────────────────────────────

try:
    ord(42)
except TypeError as e:
    print(e)  # ord() expected string of length 1, but int found

try:
    ord([])
except TypeError as e:
    print(e)  # ord() expected string of length 1, but list found

try:
    ord(3.14)
except TypeError as e:
    print(e)  # ord() expected string of length 1, but float found

try:
    ord(None)
except TypeError as e:
    print(e)  # ord() expected string of length 1, but NoneType found

# ── ord() happy path (regression guard) ──────────────────────────────────────

print(ord('A'))    # 65
print(ord('€'))    # 8364
print(ord(b'A'))   # 65

# ── chr() out-of-range errors ─────────────────────────────────────────────────

try:
    chr(-1)
except ValueError as e:
    print(e)  # chr() arg not in range(0x110000)

try:
    chr(1114112)
except ValueError as e:
    print(e)  # chr() arg not in range(0x110000)

# ── chr() happy path (regression guard) ──────────────────────────────────────

print(chr(65))      # A
print(chr(0))       # (null character)
print(chr(1114111)) # last valid codepoint
