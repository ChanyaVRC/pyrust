# A slice bound's __index__ can run arbitrary Python code, including
# reassigning the variable that holds the source sequence (#2114 review).
# GetSlice must not borrow the source register across such a dunder call: doing
# so can invalidate the borrow when the callback reassigns that register.
# Retaining an owned tuple Value is now an O(1) Rc bump, so every bound follows
# the same safe path while preserving the old no-deep-copy performance property.

class Clob:
    def __index__(self):
        global v
        v = "clobbered_after_index_xyz"
        return 5

v = tuple(range(1000))
print(v[Clob():900])
print(repr(v))

# stop bound clobbers
class ClobStop:
    def __index__(self):
        global w
        w = tuple(range(3))
        return 7

w = tuple(range(100))
print(w[1:ClobStop()])
print(repr(w))

# step bound clobbers, negative-step result
class ClobStep:
    def __index__(self):
        global z
        z = (1,)
        return -2

z = tuple(range(20))
print(z[18:0:ClobStep()])
print(repr(z))

# non-clobbering __index__ bounds still produce the right slice
class Idx:
    def __init__(self, n):
        self.n = n
    def __index__(self):
        return self.n

t = tuple(range(20))
print(t[Idx(3):Idx(15):Idx(2)])
print(t[Idx(-5):])
print(t[:Idx(4)])
