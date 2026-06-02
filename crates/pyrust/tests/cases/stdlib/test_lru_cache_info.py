# functools.lru_cache / functools.cache — cache_info() and __wrapped__.
#
# Issues #2014 / #2062: the lru_cache wrapper must expose `cache_info()`
# returning a `CacheInfo(hits, misses, maxsize, currsize)` named tuple
# (tuple subclass: indexable, field access, repr) and `__wrapped__`,
# track hit/miss/currsize counts, and reset the counters on
# `cache_clear()`.

import functools


@functools.lru_cache(maxsize=2)
def g(n):
    return n * 2


g(1)
g(2)
g(1)
info = g.cache_info()
print("repr", repr(info))
print("fields", info.hits, info.misses, info.maxsize, info.currsize)
print("index", info[0], info[1], info[2], info[3])
print("is-tuple", isinstance(info, tuple))
print("as-tuple", tuple(info))
print("eq-tuple", info == (1, 2, 2, 2))
print("wrapped-is-g", g.__wrapped__ is g.__wrapped__)
print("wrapped-callable", g.__wrapped__(10))


# Eviction reduces neither currsize beyond maxsize nor breaks counting.
g(3)  # miss → evicts LRU (key 2)
g(1)  # hit (1 still resident)
print("after-evict", repr(g.cache_info()))


# cache_clear resets the counters and currsize.
g.cache_clear()
print("after-clear", repr(g.cache_info()))


# functools.cache → maxsize=None.
@functools.cache
def h(n):
    return n + 1


h(5)
h(5)
h(6)
print("cache", repr(h.cache_info()))


# Bare @lru_cache → maxsize=128.
@functools.lru_cache
def b(n):
    return n


b(1)
b(1)
print("bare", repr(b.cache_info()))


# maxsize=0 → nothing cached; every call is a miss.
@functools.lru_cache(maxsize=0)
def z(n):
    return n


z(1)
z(1)
print("zero", repr(z.cache_info()))


# typed=True keeps int/float in separate slots.
@functools.lru_cache(typed=True)
def t(x):
    return x


t(1)
t(1.0)
t(1)
print("typed", repr(t.cache_info()))


# CacheInfo is a full named tuple: _fields / _asdict / _replace / _make.
info = h.cache_info()
print("fields", info._fields)
print("asdict", info._asdict())
print("replace", repr(info._replace(hits=99)))
print("make", repr(type(info)._make([1, 2, 3, 4])))
