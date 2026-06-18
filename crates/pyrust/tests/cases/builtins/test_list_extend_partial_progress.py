# list.extend appends incrementally: when the iterator raises mid-iteration,
# the elements produced before the exception are kept (issue #2531).
# bytearray.extend, by contrast, is atomic in CPython — it discards partial
# progress — so the two behaviours are pinned side by side here.


# --- generator raising mid-iteration: list keeps partial progress ---
def boom():
    yield 1
    yield 2
    raise ValueError("mid-way")


r = [0]
try:
    r.extend(boom())
except ValueError:
    pass
print(r)  # [0, 1, 2]


# --- user iterator raising mid-iteration: list keeps partial progress ---
class Mid:
    def __init__(self):
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.i == 2:
            raise RuntimeError("mid")
        v = self.i
        self.i += 1
        return v


r = [0]
try:
    r.extend(Mid())
except RuntimeError:
    pass
print(r)  # [0, 0, 1]


# --- bytearray.extend stays atomic on a mid-iteration exception ---
ba = bytearray(b"\x09")
try:
    ba.extend(boom())
except ValueError:
    pass
print(ba)  # bytearray(b'\t')


# --- self-extend still terminates and snapshots the original length ---
a = [1, 2, 3]
a.extend(a)
print(a)  # [1, 2, 3, 1, 2, 3]

ba = bytearray(b"abc")
ba.extend(ba)
print(ba)  # bytearray(b'abcabc')


# --- list subclass keeps partial progress through its backing buffer ---
class MyList(list):
    pass


m = MyList([7])
try:
    m.extend(boom())
except ValueError:
    pass
print(type(m).__name__, m)  # MyList [7, 1, 2]
