# vars() / dir() builtins

# --- vars(instance) returns __dict__ contents ---
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

p = Point(1, 2)
v = vars(p)
assert v == {"x": 1, "y": 2}, v

# vars on a class returns class attrs
class C:
    cls_attr = 42
    def m(self):
        return 1

cv = vars(C)
assert "cls_attr" in cv
assert "m" in cv

# --- dir(instance) includes instance attrs and class methods ---
d = dir(p)
assert "x" in d
assert "y" in d
assert "__init__" in d
# sorted
assert d == sorted(d)

# --- dir on built-in types ---
dl = dir([1, 2, 3])
assert "append" in dl
assert "pop" in dl
assert "sort" in dl

ds = dir("hello")
assert "upper" in ds
assert "split" in ds
assert "format" in ds

dd = dir({})
assert "keys" in dd
assert "values" in dd
assert "items" in dd

dt = dir(())
assert "count" in dt
assert "index" in dt

dset = dir(set())
assert "add" in dset
assert "union" in dset

# --- dir is sorted and deduped ---
result = dir([])
assert result == sorted(result)
assert len(result) == len(set(result))

# --- TypeError on bad vars() argument ---
try:
    _ = vars(42)
    print("FAIL: expected TypeError")
except TypeError:
    pass

print("vars/dir OK")
