# Parity fixture for the allocation-free format-spec parser (#2357).
# `parse_format_spec` walks the borrowed spec `&str` in place instead of
# collecting a throwaway `Vec<char>`.  These cases stress the parser's
# cursor logic directly: the same spec re-parsed at many sites, a spec built
# dynamically vs. as a constant (results must be identical), specs that parse
# fine but are invalid only for some value types, multibyte fill characters
# (where byte offset != char offset), grouping separators, and empty /
# zero-width specs.  Every line must stay byte-for-byte with CPython 3.12.

# ── same constant spec re-parsed at many call sites ──────────────────────────
# (a tight loop re-parses an identical spec; results must not drift.)
for i in range(5):
    print(f"{i:>8}|")
for v in (1.5, 2.25, 3.125):
    print(f"{v:.2f}|")

# ── dynamic spec vs constant spec — identical render ─────────────────────────
v = 3.14159
w = 8
p = 2
const = f"{v:8.2f}"
dyn = f"{v:{w}.{p}f}"
print(const == dyn, repr(const), repr(dyn))
# the spec text itself, built dynamically, then applied via format()
spec = f"{'>'}{w}.{p}f"
print(format(v, spec) == f"{v:>8.2f}")

# ── a spec valid for one type but a TypeError/ValueError for another ──────────
# `.2f` parses fine, then renders for float but is rejected for str/int.
print(f"{3.14:.2f}|")
try:
    eval("f'{\"s\":.2f}'")
except ValueError as e:
    print("str .2f ->", type(e).__name__, e)
try:
    eval("f'{42:.2f}'")
except ValueError as e:
    print("int .2f ->", type(e).__name__, e)
# the reverse: `d` is fine for int, a ValueError for float.
print(f"{42:d}|")
try:
    eval("f'{3.14:d}'")
except ValueError as e:
    print("float d ->", type(e).__name__, e)

# ── thousands / underscore grouping, incl. on a string (rejected) ────────────
print(f"{1234567:,}|")
print(f"{1234567:_}|")
print(f"{1234.5678:,.2f}|")
print(f"{1234567:_x}|")
try:
    eval("f'{\"hello\":,}'")
except ValueError as e:
    # message wording differs from CPython pre-#2357 (out of scope); class only.
    print("str , ->", type(e).__name__)

# ── multibyte fill characters: byte offset != char index after fill+align ────
print(f"{'x':é>6}|")
print(f"{42:🦀^9}|")
print(f"{7:€=8}|")
# a multibyte char that is NOT followed by an align is not a fill — it falls
# through to the "no fill/align" branch (and is an invalid spec here).
try:
    eval("f'{42:é}'")
except ValueError as e:
    # message wording differs from CPython pre-#2357 (out of scope); class only.
    print("bare multibyte ->", type(e).__name__)

# ── empty / zero-width / minimal specs ───────────────────────────────────────
print(f"{42:}|")          # empty spec == str()
print(f"{42:0}|")         # lone zero-pad flag, width 0
print(f"{42:00}|")        # zero-pad flag then width 0
print(f"{42:1}|")         # width 1
print(f"{'':>5}|")        # empty string padded
print(f"{42:>0}|")        # explicit align, width 0

# ── precision boundary: '.' with no digits is a ValueError ───────────────────
try:
    eval("f'{3.14:.}'")
except ValueError as e:
    print("dot no digits ->", type(e).__name__, e)
print(f"{3.14159:.0f}|")  # zero precision is valid
print(f"{'hello':.0}|")   # zero precision truncates string to empty

# ── trailing junk after the type char is rejected ────────────────────────────
try:
    eval("f'{42:dd}'")
except ValueError as e:
    # message wording differs from CPython pre-#2357 (out of scope); class only.
    print("double type ->", type(e).__name__)
