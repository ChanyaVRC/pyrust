# Parity fixture for issue #2222.
#
# dict.update() must accept any keys()-bearing mapping (ChainMap, OrderedDict,
# Counter, defaultdict, UserDict, custom), not just a plain dict, matching
# CPython. Existing dict / iterable-of-pairs / kwargs updates stay unregressed.

from collections import ChainMap, OrderedDict, Counter, defaultdict, UserDict


def show(d):
    print(sorted(d.items()))


# ── Non-dict mappings ──────────────────────────────────────────────────
d = {}
d.update(ChainMap({'a': 1}, {'b': 2}))
show(d)                                   # [('a', 1), ('b', 2)]

d = {}
d.update(OrderedDict(x=1, y=2))
show(d)                                   # [('x', 1), ('y', 2)]

d = {'a': 1}
d.update(Counter('aab'))
show(d)                                   # [('a', 2), ('b', 1)]

dd = defaultdict(int)
dd['m'] = 5
d = {}
d.update(dd)
show(d)                                   # [('m', 5)]


# ── Custom mapping with keys() + __getitem__ ───────────────────────────
class MyMap:
    def __init__(self, data):
        self._d = data

    def keys(self):
        return self._d.keys()

    def __getitem__(self, k):
        return self._d[k]


d = {}
d.update(MyMap({'p': 10, 'q': 20}))
show(d)                                   # [('p', 10), ('q', 20)]

d = {}
d.update(UserDict({'u': 1}))
show(d)                                   # [('u', 1)]


# ── Mapping plus keyword arguments (mapping applied first) ─────────────
d = {}
d.update(OrderedDict(x=1), y=2)
show(d)                                   # [('x', 1), ('y', 2)]


# ── Existing paths unregressed ─────────────────────────────────────────
d = {}
d.update([('p', 1), ('q', 2)])           # iterable of pairs
show(d)                                   # [('p', 1), ('q', 2)]

d = {'a': 1}
d.update({'b': 2}, c=3)                   # dict + kwargs
show(d)                                   # [('a', 1), ('b', 2), ('c', 3)]

d = {'a': 1}
d.update(a=10, b=20)                      # kwargs only
show(d)                                   # [('a', 10), ('b', 20)]

d = {'a': 1, 'b': 2}
d.update(d)                              # self-update (no change)
show(d)                                   # [('a', 1), ('b', 2)]
