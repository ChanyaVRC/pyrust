"""Direct for-loops promote an overflowing i64 range cursor."""


def collect(label, source):
    values = []
    for value in source:
        values.append(value)
    print(label, values)


collect(
    "positive final increment",
    range(9223372036854775806, 9223372036854775807, 2),
)
collect(
    "negative final increment",
    range(-9223372036854775807, -9223372036854775808, -2),
)
collect(
    "minimum step",
    range(9223372036854775807, -9223372036854775808, -9223372036854775808),
)
