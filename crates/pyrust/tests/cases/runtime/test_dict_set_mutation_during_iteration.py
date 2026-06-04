# dict / set size mutation during iteration raises RuntimeError (#1988).
# CPython 3.12: "dictionary changed size during iteration" /
# "Set changed size during iteration".  Value-only mutation (no size change)
# is allowed; list mutation stays unguarded.


def guard(label, fn):
    try:
        fn()
        print(label, "no-error")
    except RuntimeError as e:
        print(label, "RuntimeError:", e)


# ── dict: add / delete a key during iteration ────────────────────────────────
def dict_add():
    d = {1: 1, 2: 2}
    for k in d:
        d[99] = 0


guard("dict-add", dict_add)


def dict_del():
    d = {1: 1, 2: 2}
    for k in d:
        del d[k]


guard("dict-del", dict_del)


def dict_del_single():
    d = {1: 1}
    for k in d:
        del d[k]


guard("dict-del-single", dict_del_single)


# ── dict views: keys / values / items ────────────────────────────────────────
def keys_view():
    d = {1: 1, 2: 2}
    for k in d.keys():
        d[99] = 0


guard("keys-view", keys_view)


def values_view():
    d = {1: 1, 2: 2}
    for v in d.values():
        d[99] = 0


guard("values-view", values_view)


def items_view():
    d = {1: 1, 2: 2}
    for it in d.items():
        d[99] = 0


guard("items-view", items_view)


# ── set: add / discard during iteration ──────────────────────────────────────
def set_add():
    s = {1, 2, 3}
    for x in s:
        s.add(99)


guard("set-add", set_add)


def set_discard():
    s = {1, 2, 3}
    for x in s:
        s.discard(x)


guard("set-discard", set_discard)


# ── manual iter() form is guarded too ────────────────────────────────────────
def manual_iter():
    d = {1: 1, 2: 2}
    it = iter(d)
    next(it)
    d[99] = 0
    next(it)


guard("manual-iter", manual_iter)


# ── value-only mutation is allowed (no size change) ──────────────────────────
d = {1: 1, 2: 2}
for k in d:
    d[k] = d[k] * 10
print("value-only", sorted(d.items()))

# add then delete in the same step nets zero size change → allowed
d = {1: 1, 2: 2}
for k in d:
    d[99] = 0
    del d[99]
print("net-zero", sorted(d.keys()))

# ── lists remain unguarded (CPython does not guard them) ─────────────────────
lst = [1, 2, 3]
for x in lst:
    if x == 1:
        lst.append(9)
print("list-unguarded", lst)

# ── normal iteration is unchanged ────────────────────────────────────────────
d = {1: 1, 2: 2, 3: 3}
print("normal-dict", sorted(d), sorted(d.values()), sorted(d.items()))
s = {1, 2, 3}
print("normal-set", sorted(s))

# break before mutation is fine
d = {1: 1, 2: 2, 3: 3}
for k in d:
    break
d[99] = 0
print("break-then-mutate", len(d))
