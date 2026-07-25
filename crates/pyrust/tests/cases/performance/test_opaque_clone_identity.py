z = complex(1.25, -2.5)
z_aliases = [z] * 10000
pair = (z, 10**100)
pair_aliases = [pair] * 10000

print(
    "opaque-aliases",
    len(z_aliases),
    z_aliases[0] is z,
    z_aliases[-1] is z,
    pair_aliases[0] is pair,
    pair_aliases[-1][0] is z,
)
