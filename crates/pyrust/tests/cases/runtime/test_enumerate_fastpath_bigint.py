MAX_I64 = 2**63 - 1


def collect_pairs(iterable, start):
    result = []
    for pair in enumerate(iterable, start):
        result.append(pair)
    return result


def collect_unpacked(iterable, start):
    result = []
    for index, value in enumerate(iterable, start):
        result.append((index, value))
    return result


# bytearray is represented by NativeIterSource::Materialized. Its second index
# crosses i64::MAX and must promote to an arbitrary-precision Python int.
print(collect_pairs(bytearray((10, 20)), MAX_I64))
print(collect_unpacked(bytearray((10, 20)), MAX_I64))

# list uses NativeIterSource::Indexed and must retain the same boundary result.
print(collect_pairs([10, 20], MAX_I64))

# The ordinary compact-counter path remains unchanged.
print(collect_pairs(bytearray((10, 20)), 5))
