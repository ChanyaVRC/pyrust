# Parity fixture for #2053: map()/filter() per-element machinery.
# Locks in exact lazy/short-circuit/re-entrant/exception semantics after the
# step_map_iter / step_filter_iter hot-loop de-clone/de-downcast rework.

# --- laziness: side effects fire only as consumed, in source order ---
log = []


def f(x):
    log.append(x)
    return x * 10


it = map(f, [1, 2, 3])
print(log)  # [] — nothing consumed yet
print(next(it), log)  # 10 [1]
print(list(it), log)  # [20, 30] [1, 2, 3]

# --- multi-source map stops at the shortest source ---
print(list(map(lambda a, b, c: a + b + c, [1, 2, 3], [10, 20, 30, 40], [100, 200])))

# --- filter(None, ...) uses truthiness over a mixed bag ---
print(list(filter(None, [0, 1, 2, 0, 3, "", {}, "x", [], (1,)])))

# --- nested map over filter over map ---
print(list(map(lambda x: x + 1, filter(lambda x: x > 2, map(lambda x: x * x, [1, 2, 3, 4])))))

# --- aggregate consumers exercise the full lazy chain ---
print(sum(map(lambda x: x * x, range(6))))
print(sum(map(lambda x: x * 2, filter(lambda x: x % 2 == 0, range(10)))))

# --- exception from the mapped func propagates out of list()/sum() ---
try:
    list(map(lambda x: 1 // x, [4, 2, 0, 1]))
except ZeroDivisionError as e:
    print("map-zde", e)

try:
    sum(map(lambda x: 1 // x, [4, 2, 0, 1]))
except ZeroDivisionError as e:
    print("sum-map-zde", e)

# --- exception from the filter predicate propagates ---
try:
    list(filter(lambda x: 1 // x, [4, 2, 0, 1]))
except ZeroDivisionError as e:
    print("filter-zde", e)

# --- re-entrancy: mapped func pulls from the same map iterator ---
shared = None


def pull_next(x):
    if x == 1:
        return next(shared)
    return x


shared = map(pull_next, [1, 2, 3, 4])
print(list(shared))

# --- re-entrancy: filter predicate advances the same filter iterator ---
shared_f = None


def skip_at_two(x):
    if x == 2:
        next(shared_f)
    return True


shared_f = filter(skip_at_two, [1, 2, 3, 4, 5])
print(list(shared_f))

# --- map over empty / single element ---
print(list(map(lambda x: x, [])))
print(list(map(lambda x: x + 1, [41])))

# --- filter exhaustion via for-loop ---
for v in filter(lambda x: x % 3 == 0, range(12)):
    print(v)
