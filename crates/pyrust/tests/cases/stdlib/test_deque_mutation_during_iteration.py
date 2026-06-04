# collections.deque mutation during iteration raises RuntimeError (#1994).
# CPython 3.12: "deque mutated during iteration".  Detection mirrors CPython's
# deque->state counter, so even net-zero-size mutations (rotate, a maxlen-bound
# append) raise, while value-only edits (__setitem__, reverse) do not.

import collections


def guard(label, fn):
    try:
        fn()
        print(label, "no-error")
    except RuntimeError as e:
        print(label, "RuntimeError:", e)


# ── size-changing mutations raise ────────────────────────────────────────────
def dq_append():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.append(9)


guard("append", dq_append)


def dq_pop():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.pop()


guard("pop", dq_pop)


def dq_popleft():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.popleft()


guard("popleft", dq_popleft)


def dq_clear():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.clear()


guard("clear", dq_clear)


def dq_extendleft():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.extendleft([9])


guard("extendleft", dq_extendleft)


def dq_insert():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.insert(0, 9)


guard("insert", dq_insert)


def dq_delitem():
    d = collections.deque([1, 2, 3])
    for x in d:
        del d[0]


guard("delitem", dq_delitem)


def dq_remove():
    d = collections.deque([1, 2, 3, 4])
    for x in d:
        d.remove(4)


guard("remove", dq_remove)


# ── net-zero-size mutations still raise (state counter, not just length) ─────
def dq_maxlen_append():
    d = collections.deque([1, 2, 3], maxlen=3)
    for x in d:
        d.append(9)


guard("maxlen-append", dq_maxlen_append)


def dq_rotate():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.rotate(1)


guard("rotate", dq_rotate)


def dq_rotate_fullcycle():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.rotate(3)  # normalises to a no-op move, still bumps state


guard("rotate-fullcycle", dq_rotate_fullcycle)


# ── value-only edits do NOT raise (match CPython) ────────────────────────────
def dq_setitem():
    d = collections.deque([1, 2, 3])
    for x in d:
        d[0] = 99


guard("setitem", dq_setitem)


def dq_reverse():
    d = collections.deque([1, 2, 3])
    for x in d:
        d.reverse()


guard("reverse", dq_reverse)


# ── manual iter() / next() form is guarded too ───────────────────────────────
def dq_manual():
    d = collections.deque([1, 2, 3])
    it = iter(d)
    next(it)
    d.append(9)
    next(it)


guard("manual", dq_manual)


# ── normal iteration is unchanged ────────────────────────────────────────────
d = collections.deque([1, 2, 3])
print("normal", [x for x in d], list(d))

# break before mutation is fine
d = collections.deque([1, 2, 3])
for x in d:
    break
d.append(9)
print("break-then-mutate", list(d))
