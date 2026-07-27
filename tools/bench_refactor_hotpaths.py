"""Focused workloads for the runtime-responsibility refactor.

Run one workload with:

    pyrust tools/bench_refactor_hotpaths.py <workload> <size>

The script intentionally prints nothing so `hyperfine --shell=none` measures
execution rather than terminal I/O.  Supported workloads are listed below.
"""

import sys


workload = sys.argv[1]
size = int(sys.argv[2])


if workload == "startup-noop":
    pass
elif workload == "dict-update":
    target = {}
    target.update((value, value) for value in range(size))
elif workload == "set-isub":
    left = set(range(size))
    left -= set(range(size))
elif workload == "deque-popleft":
    from collections import deque

    values = deque(range(size))
    while values:
        values.popleft()
elif workload == "lru-hits":
    from functools import lru_cache

    @lru_cache(maxsize=size)
    def cached(value):
        return value

    for value in range(size):
        cached(value)
    for _ in range(2_000):
        cached(0)
elif workload == "lru-hot":
    from functools import lru_cache

    @lru_cache(maxsize=1)
    def cached(value):
        return value

    cached(1)
    total = 0
    for _ in range(size):
        total += cached(1)
    if total != size:
        raise AssertionError("lru_cache hit lost a value")
elif workload == "lru-wrapper-build":
    from functools import lru_cache

    wrapper = None
    for _ in range(size):
        wrapper = lru_cache(maxsize=16)(lambda value: value)
    if size and wrapper(7) != 7:
        raise AssertionError("lru_cache wrapper construction lost its callable")
elif workload == "counter-small-updates":
    from collections import Counter

    counts = Counter({value: 1 for value in range(size)})
    for _ in range(1_000):
        counts.update([0])
elif workload == "counter-bigint-updates":
    from collections import Counter

    counts = Counter(anchor=2**100)
    delta = {"anchor": 2**80}
    for _ in range(size):
        counts.update(delta)
    if counts["anchor"] != 2**100 + size * 2**80:
        raise AssertionError("Counter bigint update lost precision")
elif workload == "counter-iter-first":
    from collections import Counter

    counts = Counter({value: value for value in range(size)})
    total = 0
    for _ in range(2_000):
        total += next(iter(counts))
    if total != 0:
        raise AssertionError("Counter iterator returned the wrong first key")
elif workload == "counter-iter-full":
    from collections import Counter

    counts = Counter({value: value for value in range(size)})
    total = 0
    for value in counts:
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("Counter iterator lost a key")
elif workload == "counter-elements-first":
    from collections import Counter

    counts = Counter()
    counts["item"] = size
    # `iter(...)` keeps this workload runnable against the pre-fix PyRust
    # snapshot, whose non-conforming `elements()` returned a list.
    iterator = iter(counts.elements())
    next(iterator)
elif workload == "counter-most-common-small":
    from collections import Counter

    counts = Counter(
        {value: (value * 48_271) % size for value in range(size)}
    )
    for _ in range(5):
        top = counts.most_common(10)
    if len(top) != min(10, size):
        raise AssertionError("Counter.most_common() returned the wrong prefix")
elif workload == "counter-keep-positive":
    from collections import Counter

    counts = Counter({value: -1 for value in range(size)})
    counts += Counter()
    if counts:
        raise AssertionError("Counter cleanup retained a nonpositive count")
elif workload == "unicode-first":
    iterator = iter("é" * size)
    next(iterator)
elif workload == "bytes-first":
    iterator = iter(bytes([42]) * size)
    next(iterator)
elif workload == "range-first":
    iterator = iter(range(size))
    next(iterator)
elif workload == "range-for-full":
    total = 0
    for value in range(size):
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("range loop lost a value")
elif workload == "while-counted-module":
    index = 0
    total = 0
    while index < size:
        total += index
        index += 1
    if size and total != size * (size - 1) // 2:
        raise AssertionError("counted while loop lost a value")
elif workload == "while-counted-short-reentry":
    outer = 0
    total = 0
    while outer < size:
        index = 0
        while index < 4:
            total += index
            index += 1
        outer += 1
    if total != size * 6:
        raise AssertionError("short counted while loop lost a value")
elif workload == "range-index-hot":
    values = range(1024)
    total = 0
    for index in range(size):
        total += values[index & 1023]
    cycles, remainder = divmod(size, 1024)
    expected = cycles * (1023 * 1024 // 2) + remainder * (remainder - 1) // 2
    if total != expected:
        raise AssertionError("range indexing returned the wrong value")
elif workload == "reversed-range-first":
    iterator = reversed(range(size))
    next(iterator)
elif workload == "reversed-user-first":
    class Sequence:
        def __len__(self):
            return size

        def __getitem__(self, index):
            if index < 0 or index >= size:
                raise IndexError
            return index

    iterator = reversed(Sequence())
    if next(iterator) != size - 1:
        raise AssertionError("reversed user sequence returned the wrong first item")
elif workload == "reversed-list-full":
    values = list(range(size))
    total = 0
    for value in reversed(values):
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("reversed list iterator lost a value")
elif workload == "deque-repeat-bounded":
    from collections import deque

    values = deque(range(32), maxlen=32)
    repeated = values * size
    if len(repeated) != 32:
        raise AssertionError("bounded deque repeat lost its maxlen")
elif workload == "deque-index-protocol":
    from collections import deque

    class Index:
        def __index__(self):
            return 1

    values = deque((10, 20, 30))
    index = Index()
    total = 0
    for _ in range(size):
        total += values[index]
    if total != size * 20:
        raise AssertionError("deque __index__ dispatch returned the wrong item")
elif workload == "cmp-key-build":
    from functools import cmp_to_key

    factory = cmp_to_key(lambda left, right: left - right)
    values = [factory(value) for value in range(size)]
    if len(values) != size:
        raise AssertionError("cmp_to_key factory lost values")
elif workload == "groupby-groups":
    from itertools import groupby

    total = 0
    for _, group in groupby(range(size)):
        total += next(group)
    if size and total < 0:
        raise AssertionError("groupby produced an invalid total")
elif workload == "product-yields":
    from itertools import product

    iterator = product(range(2), repeat=20)
    for _ in range(size):
        value = next(iterator)
    if size and len(value) != 20:
        raise AssertionError("product returned the wrong tuple width")
elif workload == "combinations-yields":
    from itertools import combinations

    iterator = combinations(range(40), 8)
    for _ in range(size):
        value = next(iterator)
    if size and len(value) != 8:
        raise AssertionError("combinations returned the wrong tuple width")
elif workload == "combinations-replacement-yields":
    from itertools import combinations_with_replacement

    iterator = combinations_with_replacement(range(20), 8)
    for _ in range(size):
        value = next(iterator)
    if size and len(value) != 8:
        raise AssertionError(
            "combinations_with_replacement returned the wrong tuple width"
        )
elif workload == "permutations-yields":
    from itertools import permutations

    total = 0
    # Each repetition yields 8P5 = 6,720 tuples.  This isolates cursor
    # advancement while keeping the returned values live long enough to be
    # consumed by Python code.
    for _ in range(size):
        for item in permutations(range(8), 5):
            total += item[0]
    if size and total <= 0:
        raise AssertionError("permutations produced an invalid total")
elif workload == "contextmanager-build":
    from contextlib import contextmanager

    @contextmanager
    def managed(value):
        yield value

    managers = [managed(value) for value in range(size)]
    if len(managers) != size:
        raise AssertionError("contextmanager factory lost values")
elif workload == "abc-instancecheck":
    from collections.abc import Iterable

    class IterOnly:
        def __iter__(self):
            return iter(())

    value = IterOnly()
    for _ in range(size):
        matched = isinstance(value, Iterable)
    if size and not matched:
        raise AssertionError("Iterable structural check lost its result")
elif workload == "generic-alias-build":
    from typing import Generic, TypeVar

    T = TypeVar("T")

    class Box(Generic[T]):
        pass

    alias = None
    for _ in range(size):
        alias = Box[int]
    if size and alias.__origin__ is not Box:
        raise AssertionError("Generic alias construction lost its origin")
elif workload == "path-build":
    from pathlib import Path

    root = Path("/tmp")
    values = [root / str(value) for value in range(size)]
    if len(values) != size:
        raise AssertionError("Path construction lost values")
elif workload == "math-sqrt-calls":
    from math import sqrt

    total = 0.0
    for value in range(size):
        total += sqrt((value & 63) + 1.0)
    if size and total <= 0.0:
        raise AssertionError("math.sqrt calls produced an invalid total")
elif workload == "math-prod-streaming":
    from math import prod

    values = [1] * size
    if prod(values) != 1:
        raise AssertionError("math.prod streaming fold lost a factor")
elif workload == "math-fsum-streaming":
    from math import fsum

    values = [1.0] * size
    if fsum(values) != float(size):
        raise AssertionError("math.fsum streaming fold lost a term")
elif workload == "math-sumprod-streaming":
    from math import sumprod

    left = [1.0] * size
    right = [2.0] * size
    if sumprod(left, right) != float(size * 2):
        raise AssertionError("math.sumprod streaming fold lost a pair")
elif workload == "math-module-reloads":
    import math
    import sys

    for _ in range(size):
        del sys.modules["math"]
        import math
    if math.sqrt(4.0) != 2.0:
        raise AssertionError("reloaded math module lost its native callables")
elif workload == "math-module-cached-imports":
    import math

    for _ in range(size):
        import math
    if math.sqrt(4.0) != 2.0:
        raise AssertionError("cached math import lost its native callables")
elif workload == "filesystem-module-attr":
    import _bench_refactor_filesystem_namespace as module

    def read_module_attrs(count, target):
        total = 0
        for _ in range(count):
            total += target.value
        return total


    total = read_module_attrs(size, module)
    if total != size * 7:
        raise AssertionError("filesystem module attribute lookup lost a value")
elif workload == "filesystem-module-global-calls":
    import _bench_refactor_filesystem_namespace as module

    def call_module_function(count, read):
        total = 0
        for _ in range(count):
            total += read()
        return total


    total = call_module_function(size, module.read)
    if total != size * 7:
        raise AssertionError("filesystem module global lookup lost a value")
elif workload == "filesystem-module-reloads":
    import sys
    import _bench_refactor_filesystem_namespace as module

    def reload_filesystem_module(count, initial):
        loaded = initial
        for _ in range(count):
            del sys.modules["_bench_refactor_filesystem_namespace"]
            import _bench_refactor_filesystem_namespace as loaded
        return loaded


    module = reload_filesystem_module(size, module)
    if module.read() != 7:
        raise AssertionError("reloaded filesystem module lost its namespace")
elif workload == "builtin-global-calls":
    values = (1, 2, 3, 4)
    total = 0
    for _ in range(size):
        total += len(values)
    if total != size * 4:
        raise AssertionError("builtin global lookup returned the wrong callable")
elif workload == "builtin-global-cache-hit":
    def call_builtin_global(count):
        values = (1, 2, 3, 4)
        total = 0
        for _ in range(count):
            total += len(values)
        return total


    total = call_builtin_global(size)
    if total != size * 4:
        raise AssertionError("isolated builtin global cache returned the wrong value")
elif workload == "list-append-calls":
    values = []
    for value in range(size):
        values.append(value)
    if len(values) != size:
        raise AssertionError("list.append lost a value")
elif workload == "dict-get-calls":
    values = {"key": 7}
    total = 0
    for _ in range(size):
        total += values.get("key", 0)
    if total != size * 7:
        raise AssertionError("dict.get returned the wrong value")
elif workload == "set-add-calls":
    values = set()
    for value in range(size):
        values.add(value & 255)
    if len(values) != min(size, 256):
        raise AssertionError("set.add lost a value")
elif workload == "str-upper-calls":
    value = "pyrust"
    result = ""
    for _ in range(size):
        result = value.upper()
    if size and result != "PYRUST":
        raise AssertionError("str.upper returned the wrong value")
elif workload == "bytes-upper-calls":
    value = b"pyrust"
    result = b""
    for _ in range(size):
        result = value.upper()
    if size and result != b"PYRUST":
        raise AssertionError("bytes.upper returned the wrong value")
elif workload == "str-splitlines-bool":
    value = "alpha\nbeta\ngamma"
    result = None
    for _ in range(size):
        result = value.splitlines(True)
    if size and result != ["alpha\n", "beta\n", "gamma"]:
        raise AssertionError("str.splitlines lost a line ending")
elif workload == "bytes-splitlines-bool":
    value = b"alpha\nbeta\ngamma"
    result = None
    for _ in range(size):
        result = value.splitlines(True)
    if size and result != [b"alpha\n", b"beta\n", b"gamma"]:
        raise AssertionError("bytes.splitlines lost a line ending")
elif workload == "bytearray-splitlines-bool":
    value = bytearray(b"alpha\nbeta\ngamma")
    result = None
    for _ in range(size):
        result = value.splitlines(True)
    if size and result != [
        bytearray(b"alpha\n"),
        bytearray(b"beta\n"),
        bytearray(b"gamma"),
    ]:
        raise AssertionError("bytearray.splitlines lost a line ending")
elif workload == "callmemo-scalar-hot":
    def square(value):
        return value * value


    total = 0
    for _ in range(size):
        total += square(7)
    if total != size * 49:
        raise AssertionError("CallMemo returned a stale scalar result")
elif workload == "leaf-binop-dynamic-calls":
    def run_leaf_calls(count):
        def add(left, right):
            return left + right

        increment = 1
        total = 0
        for value in range(count):
            left = value & 255
            # Keep both arguments in pre-existing registers so this workload
            # exercises the guard-before-Move hot shape, rather than merely
            # timing an ineligible argument-expression layout.
            total += add(left, increment)
        return total


    total = run_leaf_calls(size)
    cycles, remainder = divmod(size, 256)
    expected = cycles * (255 * 256 // 2 + 256) + remainder * (
        remainder - 1
    ) // 2 + remainder
    if total != expected:
        raise AssertionError("guarded dynamic leaf-call inline lost a value")
elif workload == "leaf-binop-constant-calls":
    def run_leaf_calls(count):
        def add(left, right):
            return left + right

        total = 0
        for _ in range(count):
            total += add(20, 22)
        return total


    total = run_leaf_calls(size)
    if total != size * 42:
        raise AssertionError("guarded constant leaf-call inline lost a value")
elif workload == "callmemo-high-arity-hot":
    def add8(a, b, c, d, e, f, g, h):
        return a + b + c + d + e + f + g + h


    total = 0
    for _ in range(size):
        total += add8(1, 2, 3, 4, 5, 6, 7, 8)
    if total != size * 36:
        raise AssertionError("high-arity CallMemo returned a stale result")
elif workload == "callmemo-recursive-miss-chain":
    def descend(value, floor):
        if value <= floor:
            return 0
        return descend(value - 1, floor) + 1


    depth = 24
    repeated_hits = 13
    total = 0
    for seed in range(size):
        floor = seed * (depth + 1)
        top = floor + depth
        total += descend(top, floor)
        for _ in range(repeated_hits):
            total += descend(top, floor)
    if total != size * depth * (repeated_hits + 1):
        raise AssertionError("recursive CallMemo miss chain returned a stale result")
elif workload == "user-next-full":
    class Counter:
        def __init__(self, stop):
            self.current = 0
            self.stop = stop

        def __iter__(self):
            return self

        def __next__(self):
            if self.current >= self.stop:
                raise StopIteration
            value = self.current
            self.current += 1
            return value


    total = 0
    for value in Counter(size):
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("user iterator lost a value")
elif workload == "user-global-load":
    global_value = 7

    def read_global():
        return global_value


    total = 0
    for _ in range(size):
        total += read_global()
    if total != size * global_value:
        raise AssertionError("user global lookup returned the wrong value")
elif workload == "global-scalar-cache-hit":
    global_cache_scalar = 7

    def sum_global_scalar(count):
        total = 0
        for _ in range(count):
            total += global_cache_scalar
        return total


    total = sum_global_scalar(size)
    if total != size * global_cache_scalar:
        raise AssertionError("scalar global cache returned the wrong value")
elif workload == "global-user-function-cache-hit":
    def global_cache_callee():
        return 1


    def call_global_function(count):
        total = 0
        for _ in range(count):
            total += global_cache_callee()
        return total


    total = call_global_function(size)
    if total != size:
        raise AssertionError("user-function global cache returned the wrong value")
elif workload == "global-dict-cache-hit":
    global_cache_mapping = {"value": 7}

    def sum_global_mapping(count):
        total = 0
        for _ in range(count):
            total += global_cache_mapping["value"]
        return total


    total = sum_global_mapping(size)
    if total != size * 7:
        raise AssertionError("dict global cache returned the wrong value")
elif workload == "global-heap-string-load":
    global_cache_text = "pyrust heap-backed global string payload"

    def sum_global_text_lengths(count):
        total = 0
        for _ in range(count):
            total += len(global_cache_text)
        return total


    total = sum_global_text_lengths(size)
    if total != size * len(global_cache_text):
        raise AssertionError("heap-string global lookup returned the wrong value")
elif workload == "global-class-construct":
    class GlobalCacheClass:
        pass

    def construct_global_class(count):
        result = None
        for _ in range(count):
            result = GlobalCacheClass()
        return result


    result = construct_global_class(size)
    if size and type(result) is not GlobalCacheClass:
        raise AssertionError("class global cache constructed the wrong type")
elif workload == "keyword-call-hot":
    def combine(first, second, third):
        return first + second + third


    total = 0
    for _ in range(size):
        total += combine(first=1, second=2, third=3)
    if total != size * 6:
        raise AssertionError("keyword-call cache bound the wrong arguments")
elif workload == "variadic-mixed-expanded":
    def combine(first, second, marker=0, tail=0):
        return first + second + marker + tail


    positional = (1, 2)
    keywords = {"tail": 4}
    total = 0
    for _ in range(size):
        # The literal keyword between `*args` and `**kwargs` deliberately uses
        # the source-order-sensitive generic expanded-call lowering.
        total += combine(*positional, marker=3, **keywords)
    if total != size * 10:
        raise AssertionError("generic expanded call bound the wrong arguments")
elif workload == "native-classmethod-calls":
    total = 0
    for _ in range(size):
        total += int.from_bytes(b"\x01", "big")
    if total != size:
        raise AssertionError("native classmethod calls produced an invalid total")
elif workload == "native-classmethod-cache-hit":
    def call_native_classmethod(count):
        total = 0
        for _ in range(count):
            total += int.from_bytes(b"\x01", "big")
        return total


    total = call_native_classmethod(size)
    if total != size:
        raise AssertionError("isolated native classmethod cache returned the wrong value")
elif workload == "native-classmethod-bind":
    bound = None
    for _ in range(size):
        bound = int.from_bytes
    if size and bound(b"\x01", "big") != 1:
        raise AssertionError("native classmethod binding lost its callable")
elif workload == "native-classmethod-bind-cache-hit":
    def bind_native_classmethod(count):
        bound = None
        for _ in range(count):
            bound = int.from_bytes
        return bound


    bound = bind_native_classmethod(size)
    if size and bound(b"\x01", "big") != 1:
        raise AssertionError("isolated native classmethod binding lost its callable")
elif workload == "user-method-calls":
    class Reader:
        def read(self):
            return 1

    reader = Reader()
    total = 0
    for _ in range(size):
        total += reader.read()
    if total != size:
        raise AssertionError("user method cache returned the wrong callable")
elif workload == "user-method-cache-hit":
    class Reader:
        def read(self):
            return 1

    def call_user_method(count, reader):
        total = 0
        for _ in range(count):
            total += reader.read()
        return total


    total = call_user_method(size, Reader())
    if total != size:
        raise AssertionError("isolated user method cache returned the wrong callable")
elif workload == "user-method-bind":
    class Reader:
        def read(self):
            return 1

    def bind_user_method(count, reader):
        bound = None
        for _ in range(count):
            bound = reader.read
        return bound


    bound = bind_user_method(size, Reader())
    if size and bound() != 1:
        raise AssertionError("user method binding lost its callable")
elif workload == "instance-attr-read":
    class Reader:
        pass

    reader = Reader()
    reader.value = 7

    def read_instance_attr(count, target):
        total = 0
        for _ in range(count):
            total += target.value
        return total


    total = read_instance_attr(size, reader)
    if total != size * 7:
        raise AssertionError("instance attribute cache returned the wrong value")
elif workload == "class-attr-read":
    class Reader:
        value = 7

    def read_class_attr(count, target):
        total = 0
        for _ in range(count):
            total += target.value
        return total


    total = read_class_attr(size, Reader())
    if total != size * 7:
        raise AssertionError("class attribute cache returned the wrong value")
elif workload == "native-staticmethod-calls":
    table = None
    for _ in range(size):
        table = bytes.maketrans(b"a", b"b")
    if size and len(table) != 256:
        raise AssertionError("native staticmethod calls returned an invalid table")
elif workload == "wrapped-classmethod-bind":
    class Checker:
        check = classmethod(isinstance)

    matched = False
    for _ in range(size):
        matched = Checker.check(type)
    if size and not matched:
        raise AssertionError("wrapped classmethod lost its bound class")
elif workload == "slot-read":
    class Slotted:
        __slots__ = ("value",)

    instance = Slotted()
    instance.value = 7
    total = 0
    for _ in range(size):
        total += instance.value
    if total != size * 7:
        raise AssertionError("slot reads returned the wrong value")
elif workload == "enumerate-indexed":
    values = list(range(256))
    total = 0
    for _ in range(size):
        for index, value in enumerate(values, 5):
            total += index + value
    expected = size * (sum(range(5, 261)) + sum(range(256)))
    if total != expected:
        raise AssertionError("indexed enumerate lost an index or value")
elif workload == "enumerate-materialized":
    values = bytearray(range(256))
    total = 0
    for _ in range(size):
        for index, value in enumerate(values, 5):
            total += index + value
    expected = size * (sum(range(5, 261)) + sum(range(256)))
    if total != expected:
        raise AssertionError("materialized enumerate lost an index or value")
elif workload == "dict-iter-full":
    values = {value: value for value in range(size)}
    total = 0
    for value in values:
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("dict iterator lost a key")
elif workload == "dict-iter-repeat":
    values = {value: value for value in range(size)}
    expected = size * (size - 1) // 2
    for _ in range(20):
        total = 0
        for value in values:
            total += value
        if size and total != expected:
            raise AssertionError("dict iterator lost a key")
elif workload == "instance-dict-iter-repeat":
    class Plain:
        pass

    value = Plain()
    for index in range(size):
        setattr(value, "field_" + str(index), index)
    mapping = vars(value)
    observed = 0
    for _ in range(20):
        for _key in mapping:
            observed += 1
    if observed != size * 20:
        raise AssertionError("instance dict iterator lost a key")
elif workload == "slotted-instance-dict-iter-repeat":
    class Slotted:
        __slots__ = ("slot", "__dict__")

    value = Slotted()
    value.slot = -1
    for index in range(size):
        setattr(value, "field_" + str(index), index)
    mapping = vars(value)
    observed = 0
    for _ in range(20):
        for _key in mapping:
            observed += 1
    if observed != size * 20 or "slot" in mapping:
        raise AssertionError("slotted instance dict iterator exposed the wrong keys")
elif workload == "dict-build-only":
    values = {value: value for value in range(size)}
    if len(values) != size:
        raise AssertionError("dict construction lost a key")
elif workload == "set-iter-full":
    values = set(range(size))
    total = 0
    for value in values:
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("set iterator lost a key")
elif workload == "set-iter-repeat":
    values = set(range(size))
    expected = size * (size - 1) // 2
    for _ in range(20):
        total = 0
        for value in values:
            total += value
        if size and total != expected:
            raise AssertionError("set iterator lost a key")
elif workload == "nested-set-union":
    value = 0
    for _ in range(32):
        value = (value,)
    for _ in range(size):
        result = {value} | {value}
    if size and result != {value}:
        raise AssertionError("nested set union lost its key")
elif workload == "set-build-only":
    values = set(range(size))
    if len(values) != size:
        raise AssertionError("set construction lost a key")
elif workload == "list-iter-full":
    values = list(range(size))
    total = 0
    for value in values:
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("list iterator lost a value")
elif workload == "list-extended-delete":
    values = list(range(size))
    del values[::2]
    if len(values) != size // 2:
        raise AssertionError("extended slice deletion kept the wrong length")
elif workload == "tuple-slice-tiny":
    values = tuple(range(1_000))
    total = 0
    for _ in range(size):
        total += len(values[500:505])
    if total != size * 5:
        raise AssertionError("tuple slicing returned the wrong length")
elif workload == "unicode-slice-tiny":
    values = "é" * 1_000
    total = 0
    for _ in range(size):
        total += len(values[500:505])
    if total != size * 5:
        raise AssertionError("unicode slicing returned the wrong length")
elif workload == "bytearray-append":
    values = bytearray()
    for value in range(size):
        values.append(value & 255)
    if len(values) != size:
        raise AssertionError("bytearray append lost a value")
elif workload == "bytearray-extended-delete":
    values = bytearray(size)
    del values[::2]
    if len(values) != size // 2:
        raise AssertionError("bytearray extended slice deletion kept the wrong length")
elif workload == "bytearray-slice-generator":
    values = bytearray(b"x")
    values[:] = (value & 255 for value in range(size))
    if len(values) != size:
        raise AssertionError("bytearray generator assignment lost a value")
elif workload == "dict-iter-first":
    values = {value: value for value in range(size)}
    total = 0
    for _ in range(2_000):
        total += next(iter(values))
    if total != 0:
        raise AssertionError("dict iterator returned the wrong first key")
elif workload == "set-iter-first":
    values = set(range(size))
    observed = None
    for _ in range(2_000):
        observed = next(iter(values))
    if size and observed not in values:
        raise AssertionError("set iterator returned a missing key")
elif workload == "dict-mutate-no-iterator":
    values = {0: 0}
    for value in range(size):
        values[0] = value
    if size and values[0] != size - 1:
        raise AssertionError("dict overwrite lost the final value")
elif workload == "dict-mutate-no-iterator-isolated":
    def mutate_dict_without_iterator(count):
        values = {0: 0}
        for value in range(count):
            values[0] = value
        return values[0]


    final = mutate_dict_without_iterator(size)
    if size and final != size - 1:
        raise AssertionError("isolated dict overwrite lost the final value")
elif workload == "dict-mutate-active-iterator":
    def mutate_dict_with_iterator(count):
        values = {0: 0, 1: 1}
        iterator = iter(values)
        first = next(iterator)
        for value in range(count):
            values[0] = value
        second = next(iterator)
        return first, second


    first, second = mutate_dict_with_iterator(size)
    if (first, second) != (0, 1):
        raise AssertionError("active dict iterator lost its stable key order")
elif workload == "dict-method-mutate-no-alias":
    def update_ordinary_dict(count):
        target = {"key": 0}
        source = {"key": 1}
        for _ in range(count):
            target.update(source)
        return target


    target = update_ordinary_dict(size)
    if target != {"key": 1}:
        raise AssertionError("ordinary dict update lost its value")
elif workload == "globals-alias-update":
    # Keep the explicit namespace small. Running this loop in this benchmark's
    # own module would make the synchronization cost scale with every name in
    # this intentionally broad workload dispatcher.
    alias_program = """
alias_target = 0
alias = globals()

def mutate_alias(count):
    target = alias
    for value in range(count):
        target.update(alias_target=value)
    return target["alias_target"]

observed = mutate_alias(size)
expected = size - 1 if size else 0
if observed != expected:
    raise AssertionError("globals alias update lost the final value")
"""
    namespace = {"size": size}
    exec(alias_program, namespace)
elif workload == "set-mutate-no-iterator":
    values = {0}
    for _ in range(size):
        values.add(0)
    if values != {0}:
        raise AssertionError("set add changed an existing member")
elif workload == "set-toggle-no-iterator":
    def mutate_set_without_iterator(count):
        values = set()
        for _ in range(count):
            values.add(0)
            values.discard(0)
        return values


    values = mutate_set_without_iterator(size)
    if values:
        raise AssertionError("isolated set mutation retained a removed member")
elif workload == "default-construct":
    class Plain:
        pass

    values = [Plain() for _ in range(size)]
    if len(values) != size:
        raise AssertionError("class construction lost values")
elif workload == "primitive-subclass-construct":
    class Values(list):
        pass

    values = [Values() for _ in range(size)]
    if len(values) != size or (values and values[-1] != []):
        raise AssertionError("primitive subclass construction lost backing state")
elif workload == "object-repr":
    class Plain:
        pass

    value = Plain()
    for _ in range(size):
        rendered = repr(value)
    if size and not rendered:
        raise AssertionError("object repr returned an empty string")
elif workload == "list-subclass-iter":
    class Values(list):
        pass

    values = Values(range(size))
    total = 0
    for value in values:
        total += value
    if size and total != size * (size - 1) // 2:
        raise AssertionError("list subclass iterator lost a value")
else:
    raise ValueError("unknown workload: " + workload)
