# Issue #2291: keyword-argument rejection wording for builtin descriptors.
#
# Slot wrappers (wrapper_descriptor) report `wrapper <name>() takes no keyword
# arguments`; the named container slots that CPython implements as
# method_descriptors report `<type>.<name>() takes no keyword arguments`; and
# ordinary method_descriptors (list.append, list.count, ...) report
# `<type>.<method>() takes no keyword arguments`.  Verified against python3.12.


def show(label, fn):
    try:
        fn()
    except TypeError as e:
        print(label, "->", e)
    else:
        print(label, "-> NO ERROR")


# --- anonymous slot wrappers: `wrapper <name>()` --------------------------
show("int.__add__ bound", lambda: (1).__add__(2, foo=3))
show("int.__add__ unbound", lambda: int.__add__(1, 2, foo=3))
show("int.__sub__ bound", lambda: (1).__sub__(2, foo=3))
show("list.__add__ bound", lambda: [].__add__([1], foo=3))
show("str.__hash__ bound", lambda: "x".__hash__(k=1))
show("str.__hash__ unbound", lambda: str.__hash__("x", k=1))
show("list.__len__ bound", lambda: [].__len__(k=1))

# --- named container method_descriptors: `<type>.<name>()` ----------------
show("list.__getitem__ bound", lambda: [1].__getitem__(0, foo=3))
show("list.__getitem__ unbound", lambda: list.__getitem__([1], 0, foo=3))
show("dict.__contains__ bound", lambda: {}.__contains__(1, foo=3))

# --- ordinary list method_descriptors: `list.<method>()` ------------------
show("list.append bound", lambda: [].append(1, foo=3))
show("list.append unbound", lambda: list.append([], 1, foo=3))
show("list.extend", lambda: [].extend([1], foo=3))
show("list.insert", lambda: [].insert(0, 1, foo=3))
show("list.remove", lambda: [].remove(1, foo=3))
show("list.pop", lambda: [].pop(foo=3))
show("list.clear", lambda: [].clear(foo=3))
show("list.copy", lambda: [].copy(foo=3))
show("list.reverse", lambda: [].reverse(foo=3))
show("list.index", lambda: [].index(1, foo=3))
show("list.count", lambda: [].count(1, foo=3))


# list subclass routes through the same descriptor wording.
class L(list):
    pass


show("L().append", lambda: L().append(1, foo=3))
show("L().count", lambda: L([1]).count(1, foo=3))

# --- happy paths still work -----------------------------------------------
xs = [3, 1, 2]
xs.append(4)
print(xs)
xs.sort()
print(xs)
xs.sort(reverse=True)
print(xs)
print(xs.count(1), xs.index(2))
