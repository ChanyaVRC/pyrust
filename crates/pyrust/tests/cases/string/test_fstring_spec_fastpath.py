# Parity fixture for the f-string format-spec fast path (FormatValueSpec opcode).
# `f"{value:spec}"` is lowered to a dedicated opcode that dispatches through
# `__format__` directly (like CPython's FORMAT_VALUE) instead of an explicit
# `format(value, spec)` builtin call.  Behaviour must stay byte-for-byte with
# CPython 3.12 across every spec form.

# ── numeric specs: sign / zero-pad / thousands / alt-form ─────────────────────
v = 3.14159
print(f"{v:.2f}|")
print(f"{v:+,.2f}|")
print(f"{v:>10.3f}|")
print(f"{v:010.2f}|")
print(f"{0.1234:.1%}|")
print(f"{0.1234:>6.1%}|")
print(f"{42:>6}|")
print(f"{42:<6}|")
print(f"{42:^7}|")
print(f"{42:08}|")
print(f"{-5:+}|")
print(f"{1234567:,}|")
print(f"{255:#x}|")
print(f"{255:#b}|")
print(f"{255:#o}|")

# ── nested specs: width / precision pulled from variables ─────────────────────
w = 8
p = 2
print(f"{v:{w}.{p}f}|")
print(f"{42:{w}}|")
print(f"{'hi':>{w}}|")

# ── string specs: padding / truncation / alignment ───────────────────────────
print(f"{'hi':>5}|")
print(f"{'hello':.3}|")
print(f"{'hello':.3s}|")
print(f"{'x':^7}|")
print(f"{'x':*^7}|")

# ── bool / None ──────────────────────────────────────────────────────────────
print(f"{True:d}|")
print(f"{True:>6}|")
print(f"{False:05}|")

# ── complex ──────────────────────────────────────────────────────────────────
print(f"{1 + 2j:.2f}|")

# ── !r / !s / !a conversions combined with a spec ────────────────────────────
print(f"{'ab'!r:>6}|")
print(f"{3.14159!s:>10}|")
print(f"{'café'!a:>8}|")

# ── debug form with a spec (f"{x=:spec}") ────────────────────────────────────
x = 42
print(f"{x=:>5}")
print(f"{x=:#x}")

# ── user __format__ must still receive the spec ──────────────────────────────
class C:
    def __format__(self, spec):
        return f"C<{spec}>"

print(f"{C():custom}|")
print(f"{C():>10}|")
print(f"{C()}|")

# ── __format__ that returns a non-str raises TypeError ───────────────────────
class Bad:
    def __format__(self, spec):
        return 123

try:
    f"{Bad():x}"
except TypeError as e:
    print("TypeError:", e)

# ── a shadowed `format` builtin must NOT affect f-string spec dispatch ────────
# (CPython's FORMAT_VALUE calls PyObject_Format directly, ignoring the name.)
format = lambda *a, **k: "SHADOW"
print(f"{7:>4}|")
print(f"{C():spec}|")
del format

# ── invalid specs raise the right exception class + message ──────────────────
try:
    eval("f'{42:Z}'")
except ValueError as e:
    print("ValueError:", e)
try:
    eval("f'{3.14:d}'")
except ValueError as e:
    print("ValueError:", e)
try:
    eval("f'{[1, 2]:>5}'")
except TypeError as e:
    print("TypeError:", e)
