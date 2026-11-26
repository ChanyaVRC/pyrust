# Comprehensive CPython 3.12 parity tests for `colorsys` (issue #3230).

import colorsys
import sys


def r3(tup):
    return tuple(round(x, 12) for x in tup)


# 1. Module metadata and constants
print("in sys.modules:", "colorsys" in sys.modules)
print("__all__:", colorsys.__all__)
print(
    "constants:",
    round(colorsys.ONE_THIRD, 15),
    round(colorsys.ONE_SIXTH, 15),
    round(colorsys.TWO_THIRD, 15),
)

# 2. Sample RGB palette covering primaries, secondaries, greys, and arbitrary hues
samples = [
    (0.0, 0.0, 0.0),
    (1.0, 1.0, 1.0),
    (0.5, 0.5, 0.5),
    (1.0, 0.0, 0.0),
    (0.0, 1.0, 0.0),
    (0.0, 0.0, 1.0),
    (1.0, 1.0, 0.0),
    (0.0, 1.0, 1.0),
    (1.0, 0.0, 1.0),
    (0.2, 0.4, 0.6),
    (0.8, 0.3, 0.1),
    (0.1, 0.9, 0.4),
    (0.7, 0.2, 0.9),
    (0.95, 0.85, 0.75),
    (0.05, 0.15, 0.25),
    (0.3, 0.3, 0.9),
    (0.9, 0.9, 0.2),
]

for rgb in samples:
    yiq = colorsys.rgb_to_yiq(*rgb)
    rgb_yiq = colorsys.yiq_to_rgb(*yiq)
    hls = colorsys.rgb_to_hls(*rgb)
    rgb_hls = colorsys.hls_to_rgb(*hls)
    hsv = colorsys.rgb_to_hsv(*rgb)
    rgb_hsv = colorsys.hsv_to_rgb(*hsv)
    print("rgb:", rgb)
    print("  yiq:", r3(yiq), "rt:", r3(rgb_yiq))
    print("  hls:", r3(hls), "rt:", r3(rgb_hls))
    print("  hsv:", r3(hsv), "rt:", r3(rgb_hsv))

# 3. gh-106498 high-luminance precision edge case in rgb_to_hls
eps = 1e-16
print("hls gh-106498:", r3(colorsys.rgb_to_hls(1.0, 1.0 - eps, 1.0 - 2 * eps)))

# 4. YIQ clamping out-of-gamut values
print("yiq clamp low:", colorsys.yiq_to_rgb(0.0, -1.0, -1.0))
print("yiq clamp high:", colorsys.yiq_to_rgb(1.0, 1.0, 1.0))
print("yiq clamp mixed:", r3(colorsys.yiq_to_rgb(0.5, 2.0, -2.0)))

# 5. HLS and HSV edge cases: zero saturation, hue wrap-around (negative and >= 1.0)
for h in (-0.25, 0.0, 1.0 / 6.0, 0.25, 0.5, 2.0 / 3.0, 0.75, 5.0 / 6.0, 1.0, 1.25):
    print("hls wrap:", round(h, 6), r3(colorsys.hls_to_rgb(h, 0.5, 0.8)))
    print("hsv wrap:", round(h, 6), r3(colorsys.hsv_to_rgb(h, 0.8, 0.9)))

print("hls s=0:", colorsys.hls_to_rgb(0.3, 0.4, 0.0))
print("hsv s=0:", colorsys.hsv_to_rgb(0.3, 0.0, 0.7))

# 6. Integer inputs accepted cleanly
print("int yiq:", r3(colorsys.rgb_to_yiq(1, 0, 0)))
print("int hls:", r3(colorsys.rgb_to_hls(1, 0, 0)))
print("int hsv:", r3(colorsys.rgb_to_hsv(1, 0, 0)))
print("int inv:", r3(colorsys.hsv_to_rgb(0, 1, 1)), r3(colorsys.hls_to_rgb(0, 1, 0)))

# ---------------------------------------------------------------------------
# Native-port parity (issue #3329): exact IEEE 754 bits, type promotion, and
# error behaviour must match CPython's pure-Python `colorsys`.
# ---------------------------------------------------------------------------

import math
from fractions import Fraction


def bits(tup):
    # Exact representation: repr() of a float round-trips every bit.
    return tuple(repr(x) for x in tup)


def types(tup):
    return tuple(type(x).__name__ for x in tup)


# 7. Exact (unrounded) results over a grid -- any reassociation of the float
#    arithmetic would show up here.
grid = [i / 8 for i in range(9)]
for r in grid:
    for g in grid:
        for b in grid:
            yiq = colorsys.rgb_to_yiq(r, g, b)
            hls = colorsys.rgb_to_hls(r, g, b)
            hsv = colorsys.rgb_to_hsv(r, g, b)
            print("grid", r, g, b, bits(yiq), bits(colorsys.yiq_to_rgb(*yiq)),
                  bits(hls), bits(colorsys.hls_to_rgb(*hls)),
                  bits(hsv), bits(colorsys.hsv_to_rgb(*hsv)))

# 8. Awkward, non-dyadic values
for v in (0.1, 0.2, 0.3, 0.7, 1 / 3, 2 / 3, 0.123456789, 0.987654321):
    for w in (0.05, 0.45, 0.55, 0.95):
        print("odd", v, w,
              bits(colorsys.rgb_to_yiq(v, w, 1 - v)),
              bits(colorsys.rgb_to_hls(v, w, 1 - v)),
              bits(colorsys.rgb_to_hsv(v, w, 1 - v)),
              bits(colorsys.hls_to_rgb(v, w, 1 - w)),
              bits(colorsys.hsv_to_rgb(v, w, 1 - w)))

# 9. Achromatic (grayscale) inputs keep the caller's object for l / v
for x in (0, 1, 0.0, 0.25, 1.0, True, False, -0.5, 7):
    print("gray", repr(x),
          colorsys.rgb_to_hls(x, x, x), colorsys.rgb_to_hsv(x, x, x),
          colorsys.hls_to_rgb(0.4, x, 0), colorsys.hsv_to_rgb(0.4, 0, x))
    print("gray types", types(colorsys.rgb_to_hls(x, x, x)),
          types(colorsys.rgb_to_hsv(x, x, x)),
          types(colorsys.hls_to_rgb(0.4, x, 0)),
          types(colorsys.hsv_to_rgb(0.4, 0, x)))

# 10. Boundary hues (every sextant edge, +/- epsilon, wrap-around)
eps = 2.0 ** -40
for k in range(-13, 14):
    h = k / 12
    for hh in (h - eps, h, h + eps):
        print("hue", repr(hh),
              bits(colorsys.hls_to_rgb(hh, 0.3, 0.6)),
              bits(colorsys.hls_to_rgb(hh, 0.7, 0.6)),
              bits(colorsys.hsv_to_rgb(hh, 0.6, 0.8)))
for h in (-2.5, -1.0, -1e-9, 1e-9, 3.7, 10.0, 1e6, -1e6, 1 / 6, 1 / 3, 2 / 3):
    print("hue2", repr(h), bits(colorsys.hls_to_rgb(h, 0.5, 1.0)),
          bits(colorsys.hsv_to_rgb(h, 1.0, 1.0)))
print("_v", colorsys._v(0.1, 0.9, 0.0), colorsys._v(0.1, 0.9, 0.2),
      colorsys._v(0.1, 0.9, 0.6), colorsys._v(0.1, 0.9, 0.9),
      colorsys._v(0.1, 0.9, -0.1), colorsys._v(0.1, 0.9, 1.5))

# 11. Negative and out-of-range inputs (no clamping outside yiq_to_rgb)
for rgb in ((-0.5, 0.25, 2.0), (2.0, 3.0, 4.0), (-1.0, -2.0, -3.0),
            (1.5, -0.5, 0.5), (-1.0, 1.0, 0.0), (100.0, 0.0, -100.0)):
    for fn in (colorsys.rgb_to_yiq, colorsys.rgb_to_hls, colorsys.rgb_to_hsv):
        try:
            print("oor", fn.__name__, rgb, bits(fn(*rgb)))
        except Exception as e:
            print("oor", fn.__name__, rgb, type(e).__name__, e)
for args in ((0.5, 1.5, 0.5), (0.5, -0.5, 0.5), (0.5, 0.5, 2.0), (0.5, 0.5, -1.0),
             (1.5, 0.5, 0.5), (5.0, 0.2, 0.2)):
    for fn in (colorsys.hls_to_rgb, colorsys.hsv_to_rgb, colorsys.yiq_to_rgb):
        try:
            print("oor", fn.__name__, args, bits(fn(*args)))
        except Exception as e:
            print("oor", fn.__name__, args, type(e).__name__, e)

# 12. yiq_to_rgb clamping is per-channel and independent
print("yiq clamp:", colorsys.yiq_to_rgb(0.5, 5.0, 0.0),
      colorsys.yiq_to_rgb(0.5, -5.0, 0.0), colorsys.yiq_to_rgb(0.5, 0.0, 5.0),
      colorsys.yiq_to_rgb(0.5, 0.0, -5.0), colorsys.yiq_to_rgb(-3, 0, 0),
      colorsys.yiq_to_rgb(3, 0, 0))

# 13. Integer / bool / mixed-type arguments and result types
for args in ((1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 0), (1, 1, 1), (0, 0, 0),
             (True, False, True), (1, 0.5, 0), (0.5, 1, 0), (0, 0.5, 1), (2, 3, 5)):
    for fn in (colorsys.rgb_to_yiq, colorsys.rgb_to_hls, colorsys.rgb_to_hsv,
               colorsys.yiq_to_rgb, colorsys.hls_to_rgb, colorsys.hsv_to_rgb):
        try:
            res = fn(*args)
            print("int", fn.__name__, args, bits(res), types(res))
        except Exception as e:
            print("int", fn.__name__, args, type(e).__name__, e)

# 14. Non-float numeric types follow Python promotion rules
fr = (Fraction(1, 2), Fraction(1, 4), Fraction(3, 4))
for fn in (colorsys.rgb_to_hls, colorsys.rgb_to_hsv, colorsys.rgb_to_yiq,
           colorsys.hls_to_rgb, colorsys.hsv_to_rgb):
    res = fn(*fr)
    print("fraction", fn.__name__, res, types(res))
print("bigint", colorsys.rgb_to_hsv(10**30, 0, 0), colorsys.rgb_to_hls(10**30, 5, 0))
print("gray fraction", colorsys.rgb_to_hsv(*(Fraction(1, 3),) * 3))

# 15. Special floats
inf, nan = math.inf, math.nan
for args in ((inf, 0.0, 0.0), (-inf, 0.0, 0.0), (nan, 0.0, 0.0), (0.0, nan, 0.0),
             (0.0, 0.0, nan), (inf, inf, inf), (nan, nan, nan), (0.0, -0.0, 0.0)):
    for fn in (colorsys.rgb_to_yiq, colorsys.rgb_to_hls, colorsys.rgb_to_hsv,
               colorsys.yiq_to_rgb, colorsys.hls_to_rgb, colorsys.hsv_to_rgb):
        try:
            print("special", fn.__name__, args, fn(*args))
        except Exception as e:
            print("special", fn.__name__, args, type(e).__name__, e)

# 16. Errors: wrong operand types and argument counts / keywords
for fn, args in ((colorsys.rgb_to_yiq, ("a", 0, 0)), (colorsys.rgb_to_hls, (None, 0, 0)),
                 (colorsys.rgb_to_hsv, ("a", "b", "c")), (colorsys.hls_to_rgb, ("x", 0.5, 0.5)),
                 (colorsys.hsv_to_rgb, (0.5, 0.5, "v")), (colorsys.yiq_to_rgb, (1, [], 0))):
    try:
        print("err", fn.__name__, fn(*args))
    except Exception as e:
        print("err", fn.__name__, type(e).__name__, e)
try:
    colorsys.hsv_to_rgb(math.nan, 0.5, 0.5)
except Exception as e:
    print("nan hue:", type(e).__name__, e)
try:
    colorsys.hsv_to_rgb(math.inf, 0.5, 0.5)
except Exception as e:
    print("inf hue:", type(e).__name__, e)
print("kwargs:", colorsys.rgb_to_hls(r=1.0, g=0.5, b=0.0),
      colorsys.hsv_to_rgb(h=0.1, s=0.2, v=0.3), colorsys.hls_to_rgb(0.1, s=0.2, l=0.3))
for call in (lambda: colorsys.rgb_to_yiq(1, 2), lambda: colorsys.rgb_to_hsv(1, 2, 3, 4),
             lambda: colorsys.rgb_to_hls(), lambda: colorsys.hsv_to_rgb(1, 2, x=3)):
    try:
        call()
    except TypeError:
        print("TypeError raised")

# 17. Result shape and module surface
res = colorsys.rgb_to_hls(0.1, 0.5, 0.9)
print("shape:", type(res).__name__, len(res))
print("names:", sorted(n for n in dir(colorsys) if not n.startswith("__")))
print("all resolvable:", all(callable(getattr(colorsys, n)) for n in colorsys.__all__))
print("doc head:", colorsys.__doc__.splitlines()[0])
print("fn names:", [f.__name__ for f in (colorsys.rgb_to_yiq, colorsys.hsv_to_rgb)])
print("fn module:", colorsys.rgb_to_hls.__module__)
ns = {}
exec("from colorsys import *", ns)
print("star import:", sorted(k for k in ns if not k.startswith("__")))
