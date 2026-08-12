# Issue #2975: keyed min()/max() must consume their input lazily, and bytes
# constructors must read a bytearray subclass's backing buffer before falling
# back to its user-visible iterator.


def live_mutation(function, mode, changed):
    values = [2] if mode == "grow" else [3, 1, 2]
    calls = []

    def key(value):
        calls.append(value)
        if len(calls) == 1:
            if mode == "grow":
                values.append(changed)
            elif mode == "shrink":
                del values[1:]
            else:
                values[1] = changed
        return value

    result = function(values, key=key)
    print(mode, function.__name__, result, values, calls)


for builtin, replacement in ((min, 0), (max, 9)):
    live_mutation(builtin, "grow", replacement)
    live_mutation(builtin, "shrink", replacement)
    live_mutation(builtin, "replace", replacement)


class KeyFailure(Exception):
    pass


def failing_key(function):
    values = [4]
    calls = []

    def key(value):
        calls.append(value)
        values.append(7)
        raise KeyFailure("key failed")

    try:
        function(values, key=key)
    except Exception as exc:
        print("key-error", function.__name__, type(exc).__name__, str(exc), values, calls)


failing_key(min)
failing_key(max)


events = []


def source():
    for value in (4, 1, 3):
        events.append("yield:" + str(value))
        yield value


def logged_key(value):
    events.append("key:" + str(value))
    return value


print("iterator", min(source(), key=logged_key), events)
print("tie-first", min("first", "second", key=lambda value: 0))


events = []


class IterBytearray(bytearray):
    def __iter__(self):
        events.append("iter")
        return iter([88, 89])


plain = bytearray(b"abc")
print("exact-bytearray", bytes(plain), bytearray(plain))

subclass_value = IterBytearray(b"abc")
subclass_value.extra = "kept"
events.clear()
print("bytearray-subclass-bytes", bytes(subclass_value), events, subclass_value.extra)
events.clear()
print(
    "bytearray-subclass-bytearray",
    bytearray(subclass_value),
    events,
    subclass_value.extra,
)
events.clear()
print("bytearray-subclass-iter", list(subclass_value), events)


class ChildBytearray(IterBytearray):
    pass


child = ChildBytearray(bytes([0, 255]))
events.clear()
child_bytes = bytes(child)
child_bytearray = bytearray(child)
child[:] = b"\x01"
print("nested-copy", child_bytes, child_bytearray, bytes(child), events)


class BytesBytearray(IterBytearray):
    def __bytes__(self):
        events.append("bytes")
        return b"ZZ"


bytes_value = BytesBytearray(b"abc")
events.clear()
print("bytearray-subclass-dunder-bytes", bytes(bytes_value), events)
events.clear()
print("bytearray-subclass-dunder-bytearray", bytearray(bytes_value), events)


class IndexBytearray(IterBytearray):
    def __index__(self):
        events.append("index")
        return 2


index_value = IndexBytearray(b"abc")
events.clear()
print("bytearray-subclass-index-bytes", bytes(index_value), events)
events.clear()
print("bytearray-subclass-index-bytearray", bytearray(index_value), events)

# Intentional divergence: the issue also records a bytearray subclass whose
# __index__ mutates its own backing buffer.  PyRust's RefCell-backed layout
# cannot currently reproduce that write-through case faithfully, so this
# fixture documents but does not execute it; #2975 does not change that policy.


class IterBytes(bytes):
    def __iter__(self):
        events.append("iter")
        return iter([88, 89])


bytes_subclass_value = IterBytes(b"abc")
bytes_subclass_value.extra = "kept"
events.clear()
print("bytes-subclass", bytes(bytes_subclass_value), bytearray(bytes_subclass_value), events, bytes_subclass_value.extra)
events.clear()
print("bytes-subclass-iter", list(bytes_subclass_value), events)


class OverrideBytes(IterBytes):
    def __bytes__(self):
        events.append("bytes")
        return b"override"


override_bytes = OverrideBytes(b"backing")
events.clear()
print("bytes-subclass-dunder-bytes", bytes(override_bytes), events)


class IndexBytes(IterBytes):
    def __index__(self):
        events.append("index")
        return 2


index_bytes = IndexBytes(b"abc")
events.clear()
print("bytes-subclass-index-bytes", bytes(index_bytes), events)
events.clear()
print("bytes-subclass-index-bytearray", bytearray(index_bytes), events)


class RaisingIndexBytes(IterBytes):
    def __index__(self):
        events.append("index")
        raise RuntimeError("index called")


raising_index_bytes = RaisingIndexBytes(b"abc")
events.clear()
print("bytes-subclass-raising-index-bytes", bytes(raising_index_bytes), events)


class Forged:
    pass


forged = Forged()
forged.__builtin_data__ = b"forged"
for constructor in (bytes, bytearray):
    try:
        constructor(forged)
    except Exception as exc:
        print("forged", constructor.__name__, type(exc).__name__, str(exc))
for operation, call in (
    ("iter", lambda: iter(forged)),
    ("keyed-min", lambda: min(forged, key=lambda value: value)),
):
    try:
        call()
    except Exception as exc:
        print("forged", operation, type(exc).__name__, str(exc))


class ForgedSequence:
    def __getitem__(self, index):
        if index < 2:
            return (65, 66)[index]
        raise IndexError


forged_sequence = ForgedSequence()
forged_sequence.__builtin_data__ = b"forged"
print("forged-sequence", bytes(forged_sequence), bytearray(forged_sequence))
print("forged-sequence-min", min(forged_sequence, key=lambda value: value))


class BadBytesBytearray(bytearray):
    def __bytes__(self):
        return "not bytes"


try:
    bytes(BadBytesBytearray(b"abc"))
except Exception as exc:
    print("stable-error", type(exc).__name__, str(exc))
