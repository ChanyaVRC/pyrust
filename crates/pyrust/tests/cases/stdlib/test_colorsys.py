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
