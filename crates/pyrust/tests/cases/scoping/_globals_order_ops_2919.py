"""Imported module exercising del+rebind and global-only writes (issue #2919)."""

a = 1
b = 2
c = 3
del a
a = 10


def set_global_only():
    global g_only
    g_only = 42


def bounce_b():
    global b
    del b
    b = 20


d = 4
set_global_only()
bounce_b()
e = 5

inner_keys = [k for k in globals() if not k.startswith("__")]
