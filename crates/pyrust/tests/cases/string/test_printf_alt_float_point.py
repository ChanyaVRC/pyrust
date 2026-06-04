# printf %#.0f / %#.0e / %#.0g alt-form keeps the decimal point (#2029).
#
# The '#' (alternate form) flag forces a decimal point for %f/%e/%g even at
# precision 0: "%#.0f" % 3.0 -> '3.' (was '3'), "%#.0e" % 3.0 -> '3.e+00'.
# str.format already did this; the printf path didn't.  Non-alt and non-finite
# values are unchanged (no spurious point).
for fmt in ['%#.0f', '%#.0e', '%#.0E', '%#.0g', '%#.0G',
            '%.0f', '%.0e',                # non-alt: no point
            '%#.1f', '%#.2e',              # nonzero precision: point already present
            '%# .0f', '%#+.0f']:           # alt combined with space / plus flags
    for v in [3.0, 0.0, -2.0, 1e20, 0.5,
              float('inf'), float('-inf'), float('nan')]:
        print(fmt, repr(v), '->', repr(fmt % v))
