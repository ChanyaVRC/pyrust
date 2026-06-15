# Issue #2498: float format-spec rendering with grouping / zero-pad / sign /
# precision must stay byte-identical to CPython 3.12 after the allocation-light
# rewrite of the grouping helpers (group_digits / regroup_with_zero_pad).

# The headline repro: sign + zero-pad + comma grouping + precision.
v = 3.14159
print(f"{v:+015,.3f}")

# Zero-pad interleaved with comma grouping across widths.
print(f"{1234567.89:020,.2f}")
print(f"{-1234567.89:+020,.2f}")
print(f"{0.0:015,.3f}")
print(f"{0.5:020,.4f}")

# '_' grouping interleaved with zero-fill.
print(f"{12345.678:015_.3f}")
print(f"{12345.678:020_.3f}")

# Grouping without zero-pad (no regroup path).
print(f"{1234567.89:,.2f}")
print(f"{1234567.89:_.2f}")

# No grouping (must not regress) and bare-spec floats.
print(f"{v:.3f}")
print(f"{v:.2f}")
print(f"{v:015.3f}")
print(f"{1234567.89}")

# Sign variants on signed / unsigned / zero values.
print(f"{-0.0:+}")
print(f"{0.0:+}")
print(f"{3.5: }")
print(f"{-3.5:+}")

# Non-finite values: sign still applies, grouping/precision ignored.
print(f"{float('inf'):+}")
print(f"{float('-inf'):+}")
print(f"{float('nan'):.3f}")
# NB: f"{inf:+015,.3f}" exposes a *separate*, pre-existing bug (grouping is
# wrongly applied to the zero-fill of non-finite values); see the follow-up
# issue filed alongside this PR. Out of scope for this perf change.

# Percent and exponent presentation types with grouping + zero-pad.
print(f"{0.5:%}")
print(f"{12.5:%}")
print(f"{1234.5:015,.2%}")
print(f"{12345.678:020,.3e}")
print(f"{12345.678:.3g}")

# Boundary widths: width equal to / just under the grouped length.
print(f"{1234567.0:11,.0f}")
print(f"{1234567.0:015,.0f}")

# Integer grouping shares the same helpers — verify decimal and non-decimal
# bases (group_size 3 vs 4) still match after the byte-based rewrite.
print(f"{-12345:08,}")
print(f"{12345:_}")
print(f"{0xABCDEF:_x}")
print(f"{0xABCDEF:#012_x}")
print(f"{1234:>+012,}")
print(f"{255:#010_b}")
